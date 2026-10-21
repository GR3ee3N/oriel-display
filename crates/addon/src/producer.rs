//! The producer's socket (docs/protocol.md). A thread accepts one producer at
//! a time and queues what it sends for the render callback; replies go out
//! from whichever thread has them.

use crate::book::{Book, Reply};
use crate::kodi::{error, info};
use crate::sockets::{self, Protocol, Server};
use oriel_display_protocol::{self as protocol, kind};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{Sender, channel};
use std::sync::{LazyLock, Mutex, MutexGuard};

/// A buffer the producer sent, not imported yet. Its fds close with it.
pub struct Incoming {
    pub desc: protocol::Msg,
    pub fds: Vec<OwnedFd>,
}

#[derive(Default)]
pub struct Queue {
    pub buffers: Vec<Incoming>,
    pub forget: Vec<u32>,
    /// The producer is gone: drop everything.
    pub reset: bool,
    pub book: Book,
    pub frames_in: u64,
    pub frames_replaced: u64,
}

static QUEUE: LazyLock<Mutex<Queue>> = LazyLock::new(Mutex::default);
static SERVER: Server =
    Server::new(Protocol { name: "producer", socket: protocol::SOCKET, thread: "oriel-display", hello, serve, gone });
/// Replies go out from a thread of their own: the send wakes the producer,
/// which can preempt the sending thread, and that must not be Kodi's GUI thread.
static OUTBOX: Mutex<Outbox> = Mutex::new(Outbox { sender: None, shown: false });

struct Outbox {
    sender: Option<Sender<protocol::Msg>>,
    /// Whether the page is drawn. Every change is queued under the same lock,
    /// so the producer hears them in order and its last word is the current one.
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

/// Sends the book's replies, after the queue's lock is let go.
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
    // Ends the sender thread.
    lock(&OUTBOX).sender.take();
    SERVER.stop();
}

/// The producer's first message must be a hello with our version.
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
    // Through the outbox, behind any change already queued for this producer.
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
