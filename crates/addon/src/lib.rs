//! Draw Oriel pages in Kodi GUI windows. Framework Python controls windows
//! through the control socket; the framework shell sends WebKit buffers
//! through the producer socket. See docs/protocol.md for both protocols.
//!
//! Use kodi.vfs so Kodi loads the add-on at startup and keeps it available.
//! Debug URLs are accessible through Python xbmcvfs.exists or JSON-RPC
//! Files.GetFileDetails. Include a file component so xbmcvfs treats the URL
//! as a file request:
//!
//! - `orieldisplay://pattern-1/go`: draw a test pattern; `-0` selects page buffers.
//! - `orieldisplay://dirty-1/go`: redraw every GUI frame; `-0` redraws for page updates.
//! - `orieldisplay://timing-1/go`: measure GPU draw time; `-0` disables timing.
//! - `orieldisplay://pacing-0/go`: disable display pacing; `-1` enables it.
//!
//! Log frame statistics to Kodi's debug log every 10 s.

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

/// Serialize lifecycle transitions because Kodi can load and unload on different threads.
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
