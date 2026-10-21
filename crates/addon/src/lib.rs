//! vfs.oriel.display: Kodi windows that draw an Oriel app's page in Kodi's
//! own GUI frame. The framework's Python drives the windows over the control
//! socket, and its shell sends WebKit's buffers over the producer socket
//! (docs/protocol.md).
//!
//! A `kodi.vfs` add-on, because Kodi loads those at start and keeps them. A
//! path under `orieldisplay://` reaches it from Python (`xbmcvfs.exists`) or
//! JSON-RPC (`Files.GetFileDetails`), for testing; it needs a file part, or
//! xbmcvfs takes it for a directory:
//!
//! - `orieldisplay://pattern-1/go` draws a test pattern instead of the page (`-0`: the page)
//! - `orieldisplay://dirty-1/go` redraws every GUI frame (`-0`: only for new frames)
//! - `orieldisplay://timing-1/go` times the draw on the GPU (`-0`: stops)
//! - `orieldisplay://pacing-0/go` stops pacing GUI frames to the display (`-1`: paces them)
//!
//! The add-on's frame statistics go to Kodi's debug log every 10 s.

mod book;
mod control;
mod gl;
mod kodi;
mod pacing;
mod producer;
mod sockets;
mod stats;
mod surface;
mod windows;

use kodi::info;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

/// Kodi can unload the add-on on one thread while loading it on another.
static LIFECYCLE: Mutex<()> = Mutex::new(());

fn hooks() -> kodi::Hooks {
    kodi::Hooks {
        create: || {
            let _lifecycle = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
            info!("oriel-display: loaded");
            windows::reset();
            producer::start();
            control::start();
        },
        destroy: || {
            let _lifecycle = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
            info!("oriel-display: unloading");
            windows::unload();
            kodi::detach();
        },
        vfs_exists: |url| {
            let what = url.split_once("://").map_or(url, |(_, rest)| rest);
            info!("oriel-display: command {what}");
            if let Some(n) = what.strip_prefix("pattern-") {
                surface::PATTERN_MODE.store(n.starts_with('1'), Ordering::Relaxed);
                surface::redraw();
            } else if let Some(n) = what.strip_prefix("timing-") {
                surface::TIMING.store(n.starts_with('1'), Ordering::Relaxed);
            } else if let Some(n) = what.strip_prefix("dirty-") {
                surface::ALWAYS_DIRTY.store(n.starts_with('1'), Ordering::Relaxed);
            } else if let Some(n) = what.strip_prefix("pacing-") {
                pacing::ENABLED.store(n.starts_with('1'), Ordering::Relaxed);
            }
            true
        },
    }
}
