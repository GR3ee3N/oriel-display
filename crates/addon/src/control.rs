//! Serve framework Python control requests as one JSON object per packet.
//! The client controls windows and receives remote actions and window events
//! through the protocol in docs/protocol.md.

use crate::kodi::error;
use crate::sockets::{self, Protocol, Server};
use crate::windows::{self, Which};
use oriel_display_protocol as protocol;
use serde_json::{Value, json};
use std::os::fd::{AsRawFd, RawFd};

static SERVER: Server = Server::new(Protocol {
    name: "control client",
    socket: protocol::CONTROL_SOCKET,
    thread: "oriel-display-ctl",
    hello,
    serve,
    gone,
});

pub fn connected() -> bool {
    SERVER.connected()
}

pub fn send(message: Value) {
    if let Some(client) = SERVER.client() {
        sockets::send(client.as_raw_fd(), message.to_string().as_bytes());
    }
}

pub fn start() {
    SERVER.start();
}

pub fn stop() {
    SERVER.stop();
}

fn serve(fd: RawFd) {
    while let Some((bytes, _)) = sockets::receive(fd) {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(message) => {
                if let Err(why) = handle(&message) {
                    error!("oriel-display: control: {why}: {message}");
                    send(json!({"error": why, "message": message}));
                }
            }
            Err(e) => error!("oriel-display: control: not JSON: {e}"),
        }
    }
}

fn gone() {
    // Close disconnected client windows so they cannot retain Kodi input.
    if !windows::unloading() {
        windows::request_close();
    }
}

fn hello(fd: RawFd) -> bool {
    let version = sockets::receive(fd)
        .and_then(|(bytes, _)| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|m| m.get("hello").and_then(Value::as_u64));
    if version == Some(u64::from(protocol::CONTROL_VERSION)) {
        // Each client starts drawing; an earlier client's draw setting ends with it.
        windows::set_draw(true);
        return sockets::send(fd, json!({"hello": protocol::CONTROL_VERSION}).to_string().as_bytes());
    }
    error!("oriel-display: control client speaks version {version:?}, not {}", protocol::CONTROL_VERSION);
    let refusal = json!({"error": "unsupported version", "hello": protocol::CONTROL_VERSION});
    sockets::send(fd, refusal.to_string().as_bytes());
    false
}

fn which(name: Option<&Value>) -> Result<Which, String> {
    match name.and_then(Value::as_str) {
        Some("browse") => Ok(Which::Browse),
        Some("player") => Ok(Which::Player),
        other => Err(format!("unknown window {other:?}")),
    }
}

fn handle(message: &Value) -> Result<(), String> {
    if let Some(name) = message.get("open") {
        windows::open(which(Some(name))?, message.get("mark").and_then(Value::as_str));
    } else if let Some(name) = message.get("close") {
        windows::close(which(Some(name))?);
    } else if message.get("started").is_some() {
        windows::started();
    } else if let Some(draw) = message.get("draw") {
        windows::set_draw(draw.as_bool().ok_or("draw must be true or false")?);
    } else {
        return Err("unknown message".into());
    }
    Ok(())
}
