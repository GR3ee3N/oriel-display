//! Serve producer and control clients through Linux abstract SOCK_SEQPACKET sockets.

use crate::kodi::{error, info};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Handshake deadline before releasing the socket for another client.
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);

/// Per-client handshake, service and disconnection callbacks.
pub struct Protocol {
    /// Client label used in logs, such as "producer".
    pub name: &'static str,
    pub socket: &'static [u8],
    pub thread: &'static str,
    /// Validate and answer the client hello; return false to refuse it.
    pub hello: fn(RawFd) -> bool,
    /// Process messages until client disconnection.
    pub serve: fn(RawFd),
    /// Clean up after the client connection closes.
    pub gone: fn(),
}

/// Dedicated socket thread serving one client at a time.
pub struct Server {
    protocol: Protocol,
    listener: Mutex<Option<OwnedFd>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Shared ownership keeps the client descriptor open during sends from other threads.
    client: Mutex<Option<Arc<OwnedFd>>>,
    /// Handshaking descriptor that stop must wake during shutdown.
    greeting: Mutex<Option<RawFd>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Server {
    pub const fn new(protocol: Protocol) -> Server {
        Server {
            protocol,
            listener: Mutex::new(None),
            thread: Mutex::new(None),
            client: Mutex::new(None),
            greeting: Mutex::new(None),
        }
    }

    pub fn start(&'static self) {
        let name = self.protocol.name;
        match listen(self.protocol.socket) {
            Ok(listener) => {
                let fd = listener.as_raw_fd();
                *lock(&self.listener) = Some(listener);
                match std::thread::Builder::new().name(self.protocol.thread.into()).spawn(move || self.accept_loop(fd))
                {
                    Ok(thread) => *lock(&self.thread) = Some(thread),
                    Err(e) => error!("oriel-display: {name} thread: {e}"),
                }
            }
            Err(e) => error!("oriel-display: {name} socket: {e}"),
        }
    }

    /// Wake socket operations and wait for the thread to release the abstract
    /// socket name before another add-on load.
    pub fn stop(&self) {
        let listener = lock(&self.listener).take();
        if let Some(listener) = &listener {
            unsafe { libc::shutdown(listener.as_raw_fd(), libc::SHUT_RDWR) };
        }
        if let Some(client) = lock(&self.client).as_ref() {
            unsafe { libc::shutdown(client.as_raw_fd(), libc::SHUT_RDWR) };
        }
        if let Some(fd) = *lock(&self.greeting) {
            unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };
        }
        // Keep the listener descriptor open until the thread has stopped using it.
        if !join(lock(&self.thread).take()) {
            error!("oriel-display: the {} thread did not end", self.protocol.name);
        }
        drop(listener);
    }

    pub fn connected(&self) -> bool {
        lock(&self.client).is_some()
    }

    /// Retain the client descriptor until the returned shared handle is dropped.
    pub fn client(&self) -> Option<Arc<OwnedFd>> {
        lock(&self.client).clone()
    }

    fn accept_loop(&self, listener: RawFd) {
        loop {
            let fd = unsafe { libc::accept4(listener, std::ptr::null_mut(), std::ptr::null_mut(), libc::SOCK_CLOEXEC) };
            if fd < 0 {
                match std::io::Error::last_os_error().raw_os_error() {
                    Some(libc::EINTR | libc::ECONNABORTED) => continue,
                    _ => return,
                }
            }
            let client = unsafe { OwnedFd::from_raw_fd(fd) };
            if !self.greet(fd) {
                continue;
            }
            info!("oriel-display: {} connected", self.protocol.name);
            *lock(&self.client) = Some(Arc::new(client));
            (self.protocol.serve)(fd);
            // Make sends still queued for this connection fail at once, even if the peer stopped reading.
            unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };
            *lock(&self.client) = None;
            (self.protocol.gone)();
            info!("oriel-display: {} gone", self.protocol.name);
        }
    }

    /// Bound handshake reads so a silent client cannot block all later clients.
    fn greet(&self, fd: RawFd) -> bool {
        {
            let mut greeting = lock(&self.greeting);
            // Match shutdown ordering: stop removes the listener before checking the greeting.
            if lock(&self.listener).is_none() {
                return false;
            }
            *greeting = Some(fd);
        }
        let greeted =
            receive_timeout(fd, HELLO_TIMEOUT) && (self.protocol.hello)(fd) && receive_timeout(fd, Duration::ZERO);
        // Clear the greeting before closing its fd so stop cannot shut down a reused descriptor.
        *lock(&self.greeting) = None;
        greeted
    }
}

/// Set the receive timeout; zero permits an indefinite wait.
fn receive_timeout(fd: RawFd, timeout: Duration) -> bool {
    let tv = libc::timeval {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_usec: timeout.subsec_micros() as libc::suseconds_t,
    };
    let len = size_of_val(&tv) as libc::socklen_t;
    unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, (&raw const tv).cast(), len) == 0 }
}

/// Wait up to 0.5 s for shutdown to release the socket thread. A thread
/// inside accept retains the abstract socket name and can block an immediate reload.
fn join(thread: Option<JoinHandle<()>>) -> bool {
    let Some(thread) = thread else { return true };
    let deadline = Instant::now() + Duration::from_millis(500);
    while !thread.is_finished() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = thread.join();
    true
}

/// Bind a Linux abstract socket named by the bytes after the implicit leading NUL.
fn listen(name: &[u8]) -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    if name.len() + 1 > addr.sun_path.len() {
        return Err(std::io::Error::other("socket name too long"));
    }
    for (i, b) in name.iter().enumerate() {
        addr.sun_path[i + 1] = *b as libc::c_char;
    }
    let len = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + 1 + name.len()) as libc::socklen_t;
    if unsafe { libc::bind(fd, (&raw const addr).cast(), len) } != 0 || unsafe { libc::listen(fd, 1) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(socket)
}

/// Receive a packet and owned ancillary descriptors; return None on disconnect or error.
pub fn receive(fd: RawFd) -> Option<(Vec<u8>, Vec<OwnedFd>)> {
    let mut bytes = vec![0u8; 4096];
    let mut control = [0u64; 8]; // Aligned storage for CMSG_SPACE(4 fds).
    let mut iov = libc::iovec { iov_base: bytes.as_mut_ptr().cast(), iov_len: bytes.len() };
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = size_of_val(&control) as _;
    let n = loop {
        let n = unsafe { libc::recvmsg(fd, &mut header, libc::MSG_CMSG_CLOEXEC) };
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        break n;
    };
    if n <= 0 {
        return None;
    }
    let mut fds = Vec::new();
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&header);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / size_of::<RawFd>();
                let data = libc::CMSG_DATA(c).cast::<RawFd>();
                for i in 0..count {
                    fds.push(OwnedFd::from_raw_fd(data.add(i).read_unaligned()));
                }
            }
            c = libc::CMSG_NXTHDR(&header, c);
        }
    }
    bytes.truncate(n as usize);
    Some((bytes, fds))
}

/// Send one packet, waiting while the socket is full; return false on disconnect or error.
pub fn send(fd: RawFd, bytes: &[u8]) -> bool {
    loop {
        let n = unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL) };
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return n == bytes.len() as isize;
    }
}
