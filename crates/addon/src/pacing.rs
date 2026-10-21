//! Holds Kodi's GUI thread until the display's next refresh before a GUI
//! frame that follows one with the page in the browse window.
//!
//! CoreELEC's Kodi submits each GUI frame's page flip without waiting for
//! it to land, and gives the buffer still on screen back for rendering. It
//! waits for the flip only when it presents the next frame, after rendering
//! it, so that frame can draw into the buffer being scanned out, which
//! tears. So the browse window's dirty callback, which runs before Kodi
//! renders, waits here for the next vblank of the active CRTC unless one has
//! passed since the flip. The vblanks come from a DRM file of our own (no
//! master, nothing submitted).

use crate::kodi::{error, info};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Pace GUI frames (`orieldisplay://pacing-1/go`), or not (`-0`).
pub static ENABLED: AtomicBool = AtomicBool::new(true);

/// After a failed wait, the next try at finding the display's refresh.
const RETRY: Duration = Duration::from_secs(5);
/// Longer than one refresh at 23.976 Hz.
const TIMEOUT_MS: i32 = 50;

#[derive(Default)]
struct Pacing {
    vblank: Option<Vblank>,
    retry_at: Option<Instant>,
    /// A GUI frame drew the page in the browse window since the last wait.
    pending: bool,
    /// The vblank count when it did. Its flip goes in after the draw, so it
    /// has landed once two more vblanks have passed.
    drawn_at: Option<u64>,
    /// Failed since the last wait that worked, as while the display is off.
    failing: bool,
}

static PACING: Mutex<Option<Pacing>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Pacing) -> R) -> R {
    let mut pacing = PACING.lock().unwrap_or_else(|e| e.into_inner());
    f(pacing.get_or_insert_with(Pacing::default))
}

/// A GUI frame drew the page in the browse window.
pub fn drew() {
    with(|p| {
        p.pending = true;
        p.drawn_at = p.vblank.as_ref().and_then(|v| v.sequence().ok());
    });
}

/// In the browse window's dirty callback, before Kodi renders the frame.
pub fn wait() {
    with(|p| {
        if !std::mem::take(&mut p.pending) || !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        if p.vblank.is_none() {
            if p.retry_at.is_some_and(|at| Instant::now() < at) {
                return;
            }
            match Vblank::open() {
                Ok(vblank) => p.vblank = Some(vblank),
                Err(e) => return p.failed(e),
            }
        }
        let Some(vblank) = p.vblank.as_mut() else { return };
        let landed = match (p.drawn_at, vblank.sequence()) {
            (Some(drawn), Ok(now)) => now >= drawn + 2,
            (_, Err(e)) => return p.failed(e),
            _ => false,
        };
        let waited = if landed { Ok(()) } else { vblank.wait() };
        match waited {
            Ok(()) => p.failing = false,
            Err(e) => p.failed(e),
        }
    });
}

/// The add-on is going.
pub fn stop() {
    with(|p| *p = Pacing::default());
}

impl Pacing {
    /// Logged once, until a wait works again.
    fn failed(&mut self, e: io::Error) {
        if !std::mem::replace(&mut self.failing, true) {
            error!("oriel-display: waiting for the display's refresh failed: {e}");
        }
        self.vblank = None;
        self.retry_at = Some(Instant::now() + RETRY);
    }
}

// The kernel's DRM structures and ioctls (drm.h, drm_mode.h).

#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Default)]
struct CrtcGetSequence {
    crtc_id: u32,
    active: u32,
    sequence: u64,
    sequence_ns: i64,
}

#[repr(C)]
#[derive(Default)]
struct CrtcQueueSequence {
    crtc_id: u32,
    flags: u32,
    sequence: u64,
    user_data: u64,
}

const DRM_IOCTL_MODE_GETRESOURCES: libc::c_ulong = 0xc040_64a0;
const DRM_IOCTL_CRTC_GET_SEQUENCE: libc::c_ulong = 0xc018_643b;
const DRM_IOCTL_CRTC_QUEUE_SEQUENCE: libc::c_ulong = 0xc018_643c;
const DRM_CRTC_SEQUENCE_RELATIVE: u32 = 1;
const DRM_CRTC_SEQUENCE_NEXT_ON_MISS: u32 = 2;
const DRM_EVENT_CRTC_SEQUENCE: u32 = 3;
/// drm_event (type, length), user_data, time_ns, sequence.
const EVENT_SIZE: usize = 32;

struct Vblank {
    file: File,
    crtc: u32,
    /// Matches each event to its request, so a late one doesn't end a later wait.
    request: u64,
}

impl Vblank {
    /// The first active CRTC of the first DRM card that has one.
    fn open() -> io::Result<Vblank> {
        let mut cards: Vec<_> = std::fs::read_dir("/dev/dri")?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str()?.strip_prefix("card"))
                    .is_some_and(|n| n.parse::<u32>().is_ok())
            })
            .collect();
        cards.sort();
        for path in cards {
            let Ok(file) = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&path) else { continue };
            for crtc in crtcs(&file) {
                let mut sequence = CrtcGetSequence { crtc_id: crtc, ..Default::default() };
                let ok = unsafe { libc::ioctl(file.as_raw_fd(), DRM_IOCTL_CRTC_GET_SEQUENCE, &mut sequence) } == 0;
                if ok && sequence.active != 0 {
                    info!("oriel-display: pacing to the refresh of {} CRTC {crtc}", path.display());
                    return Ok(Vblank { file, crtc, request: 0 });
                }
            }
        }
        Err(io::Error::other("no active display"))
    }

    /// The CRTC's vblank count.
    fn sequence(&self) -> io::Result<u64> {
        let mut sequence = CrtcGetSequence { crtc_id: self.crtc, ..Default::default() };
        if unsafe { libc::ioctl(self.file.as_raw_fd(), DRM_IOCTL_CRTC_GET_SEQUENCE, &mut sequence) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if sequence.active == 0 {
            return Err(io::Error::other("display off"));
        }
        Ok(sequence.sequence)
    }

    /// Returns at the next vblank.
    fn wait(&mut self) -> io::Result<()> {
        self.request += 1;
        let mut queue = CrtcQueueSequence {
            crtc_id: self.crtc,
            flags: DRM_CRTC_SEQUENCE_RELATIVE | DRM_CRTC_SEQUENCE_NEXT_ON_MISS,
            sequence: 1,
            user_data: self.request,
        };
        if unsafe { libc::ioctl(self.file.as_raw_fd(), DRM_IOCTL_CRTC_QUEUE_SEQUENCE, &mut queue) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS as u64);
        let mut events = [0u8; EVENT_SIZE * 8];
        loop {
            let left = deadline.saturating_duration_since(Instant::now()).as_millis() as i32;
            let mut poll = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            match unsafe { libc::poll(&mut poll, 1, left) } {
                -1 => {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                0 => return Err(io::Error::new(io::ErrorKind::TimedOut, "no vblank")),
                _ => {}
            }
            let n = match self.file.read(&mut events) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            };
            let mut at = 0;
            let mut done = false;
            while at + 8 <= n {
                let field =
                    |offset: usize| u32::from_ne_bytes(events[at + offset..at + offset + 4].try_into().unwrap());
                let (kind, length) = (field(0), field(4) as usize);
                if length < 8 || at + length > n {
                    break;
                }
                if kind == DRM_EVENT_CRTC_SEQUENCE && length >= EVENT_SIZE {
                    let user_data = u64::from_ne_bytes(events[at + 8..at + 16].try_into().unwrap());
                    done |= user_data == self.request;
                }
                at += length;
            }
            if done {
                return Ok(());
            }
        }
    }
}

fn crtcs(file: &File) -> Vec<u32> {
    let mut res = CardRes::default();
    if unsafe { libc::ioctl(file.as_raw_fd(), DRM_IOCTL_MODE_GETRESOURCES, &mut res) } != 0 {
        return Vec::new();
    }
    let mut crtcs = vec![0u32; res.count_crtcs as usize];
    let mut res =
        CardRes { crtc_id_ptr: crtcs.as_mut_ptr() as u64, count_crtcs: crtcs.len() as u32, ..Default::default() };
    if unsafe { libc::ioctl(file.as_raw_fd(), DRM_IOCTL_MODE_GETRESOURCES, &mut res) } != 0 {
        return Vec::new();
    }
    crtcs.truncate(res.count_crtcs as usize);
    crtcs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_abi() {
        assert_eq!(std::mem::size_of::<CardRes>(), 64);
        assert_eq!(std::mem::offset_of!(CardRes, count_crtcs), 36);
        assert_eq!(std::mem::size_of::<CrtcGetSequence>(), 24);
        assert_eq!(std::mem::offset_of!(CrtcGetSequence, sequence), 8);
        assert_eq!(std::mem::size_of::<CrtcQueueSequence>(), 24);
    }
}
