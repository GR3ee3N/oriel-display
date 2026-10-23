//! Receive producer packets and queue work for the GUI render callback.
//! Serve one producer at a time (docs/protocol.md). Queue frame replies and
//! visibility updates for a separate sender thread.

use crate::book::{Book, Reply};
use crate::kodi::{error, info};
use crate::sockets::{self, Protocol, Server};
use oriel_display_protocol::{self as protocol, kind};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{Sender, channel};
use std::sync::{LazyLock, Mutex, MutexGuard};

/// Pending buffer import; dropping it closes the received plane descriptors.
pub struct Incoming {
    pub desc: protocol::Msg,
    pub fds: Vec<OwnedFd>,
}

#[derive(Default)]
pub struct Queue {
    pub buffers: Vec<Incoming>,
    pub forget: Vec<u32>,
    /// Clear imports after producer disconnection.
    pub reset: bool,
    pub book: Book,
    pub frames_in: u64,
    pub frames_replaced: u64,
}

static QUEUE: LazyLock<Mutex<Queue>> = LazyLock::new(Mutex::default);
static SERVER: Server =
    Server::new(Protocol { name: "producer", socket: protocol::SOCKET, thread: "oriel-display", hello, serve, gone });
/// Send replies off the GUI thread because waking the producer can preempt
/// the sending thread and delay Kodi rendering.
static OUTBOX: Mutex<Outbox> = Mutex::new(Outbox { sender: None, shown: false });

struct Outbox {
    sender: Option<Sender<protocol::Msg>>,
    /// Serialize visibility changes under this lock so the last queued update
    /// always represents current page visibility.
    shown: bool,
}

impl Outbox {
    fn send(&self, msg: protocol::Msg) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(msg);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn queue() -> MutexGuard<'static, Queue> {
    lock(&QUEUE)
}

fn send_to(fd: RawFd, msg: protocol::Msg) {
    let bytes = msg.to_bytes();
    unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT) };
}

pub fn send(kind: u32, id: u32) {
    lock(&OUTBOX).send(protocol::Msg::new(kind, id));
}

/// Queue bookkeeping replies after releasing the producer queue lock.
pub fn reply(replies: Vec<Reply>) {
    for reply in replies {
        match reply {
            Reply::Drawn(id) => send(kind::DRAWN, id),
            Reply::Released(id) => send(kind::RELEASED, id),
        }
    }
}

fn send_loop(messages: std::sync::mpsc::Receiver<protocol::Msg>) {
    for msg in messages {
        if let Some(client) = SERVER.client() {
            send_to(client.as_raw_fd(), msg);
        }
    }
}

fn visibility(shown: bool) -> protocol::Msg {
    protocol::Msg::new(if shown { kind::SHOWN } else { kind::HIDDEN }, 0)
}

pub fn set_shown(shown: bool) {
    let changed = {
        let mut outbox = lock(&OUTBOX);
        let changed = std::mem::replace(&mut outbox.shown, shown) != shown;
        if changed {
            outbox.send(visibility(shown));
        }
        changed
    };
    if changed {
        info!("oriel-display: page {}", if shown { "shown" } else { "hidden" });
    }
}

pub fn start() {
    let (outbox, messages) = channel();
    lock(&OUTBOX).sender = Some(outbox);
    if let Err(e) = std::thread::Builder::new().name("oriel-display-tx".into()).spawn(move || send_loop(messages)) {
        error!("oriel-display: sender thread: {e}");
    }
    SERVER.start();
}

pub fn stop() {
    // Dropping the channel sender lets the sender thread exit.
    lock(&OUTBOX).sender.take();
    SERVER.stop();
}

/// Accept only an initial hello with the supported producer protocol version.
fn hello(fd: RawFd) -> bool {
    match sockets::receive(fd) {
        Some((bytes, _)) if bytes.len() == protocol::SIZE => {
            let msg = protocol::Msg::from_bytes(bytes.as_slice().try_into().unwrap());
            if msg.kind == kind::HELLO && msg.id == protocol::VERSION {
                send_to(fd, protocol::Msg::new(kind::HELLO, protocol::VERSION));
                return true;
            }
            error!("oriel-display: producer speaks {} {}, not version {}", msg.kind, msg.id, protocol::VERSION);
        }
        _ => error!("oriel-display: producer sent no hello"),
    }
    send_to(fd, protocol::Msg::new(kind::REFUSED, protocol::VERSION));
    false
}

fn serve(fd: RawFd) {
    // Queue initial visibility after any updates already pending for this producer.
    {
        let outbox = lock(&OUTBOX);
        outbox.send(visibility(outbox.shown));
    }
    while let Some((bytes, fds)) = sockets::receive(fd) {
        let Ok(bytes) = <&[u8; protocol::SIZE]>::try_from(bytes.as_slice()) else { continue };
        let msg = protocol::Msg::from_bytes(bytes);
        let mut replies = Vec::new();
        {
            let mut queue = queue();
            match msg.kind {
                kind::BUFFER => queue.buffers.push(Incoming { desc: msg, fds }),
                kind::FRAME => {
                    replies = queue.book.frame(msg.id);
                    if !replies.is_empty() {
                        queue.frames_replaced += 1;
                    }
                    queue.frames_in += 1;
                }
                kind::FORGET => queue.forget.push(msg.id),
                _ => {}
            }
        }
        reply(replies);
    }
}

fn gone() {
    let mut queue = queue();
    queue.reset = true;
    queue.book.disconnected();
}
