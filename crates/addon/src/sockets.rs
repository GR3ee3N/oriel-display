//! `SOCK_SEQPACKET` sockets in the abstract namespace, for both clients.

use crate::kodi::{error, info};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a client has for its hello before the next one may connect.
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);

/// What a server does with each client it accepts.
pub struct Protocol {
    /// The client, for logs: "producer".
    pub name: &'static str,
    pub socket: &'static [u8],
    pub thread: &'static str,
    /// Reads the client's hello and answers it; false refuses the client.
    pub hello: fn(RawFd) -> bool,
    /// Serves the client until it disconnects.
    pub serve: fn(RawFd),
    /// After the client's connection closed.
    pub gone: fn(),
}

/// A socket served by a thread of its own, one client at a time.
pub struct Server {
    protocol: Protocol,
    listener: Mutex<Option<OwnedFd>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Senders on other threads hold it while they send.
    client: Mutex<Option<Arc<OwnedFd>>>,
    /// A client not past its hello yet, for `stop` to wake.
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

    /// Wakes the thread and waits for it to end, which frees the socket's
    /// name for the next load.
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
        // The thread may still use the listener's fd.
        if !join(lock(&self.thread).take()) {
            error!("oriel-display: the {} thread did not end", self.protocol.name);
        }
        drop(listener);
    }

    pub fn connected(&self) -> bool {
        lock(&self.client).is_some()
    }

    /// The connected client's fd, open until the returned handle drops.
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
            *lock(&self.client) = None;
            (self.protocol.gone)();
            info!("oriel-display: {} gone", self.protocol.name);
        }
    }

    /// The client's hello, with a deadline: one that stays silent would keep
    /// every other client out.
    fn greet(&self, fd: RawFd) -> bool {
        {
            let mut greeting = lock(&self.greeting);
            // `stop` takes the listener before it looks for a greeting.
            if lock(&self.listener).is_none() {
                return false;
            }
            *greeting = Some(fd);
        }
        let greeted =
            receive_timeout(fd, HELLO_TIMEOUT) && (self.protocol.hello)(fd) && receive_timeout(fd, Duration::ZERO);
        // Before the fd closes, so `stop` never shuts down a reused one.
        *lock(&self.greeting) = None;
        greeted
    }
}

/// Bounds how long a receive on `fd` waits; zero waits for ever.
fn receive_timeout(fd: RawFd, timeout: Duration) -> bool {
    let tv = libc::timeval {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_usec: timeout.subsec_micros() as libc::suseconds_t,
    };
    let len = size_of_val(&tv) as libc::socklen_t;
    unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, (&raw const tv).cast(), len) == 0 }
}

/// Waits up to half a second for a socket's thread, woken by a shutdown, to
/// end. A thread still inside `accept` keeps the socket, and so its name,
/// alive: Kodi can load the add-on again right after unloading it.
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

/// A listening socket at `name` (after the leading NUL: no file to clean up).
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

/// The next packet and the fds that came with it; None once the peer is gone.
pub fn receive(fd: RawFd) -> Option<(Vec<u8>, Vec<OwnedFd>)> {
    let mut bytes = vec![0u8; 4096];
    let mut control = [0u64; 8]; // room for CMSG_SPACE(4 fds), aligned
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

pub fn send(fd: RawFd, bytes: &[u8]) -> bool {
    unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL) == bytes.len() as isize }
}
