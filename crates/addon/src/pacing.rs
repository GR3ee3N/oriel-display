//! Pace browse GUI frames against the active display's vblank.
//!
//! CoreELEC Kodi submits page flips asynchronously and returns the buffer
//! still on screen for rendering. It waits for a flip only when presenting
//! the next frame, after drawing it, allowing writes into a buffer still
//! being scanned out. The browse dirty callback waits before Kodi renders
//! unless the preceding page flip has already completed. Read vblank events
//! through a separate DRM descriptor without becoming DRM master or submitting frames.

use crate::kodi::{error, info};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Enable display pacing with orieldisplay://pacing-1/go; -0 disables it.
pub static ENABLED: AtomicBool = AtomicBool::new(true);

/// Delay before reopening the display after a failed vblank wait.
const RETRY: Duration = Duration::from_secs(5);
/// Allow more than one refresh period at 23.976 Hz before timing out.
const TIMEOUT_MS: i32 = 50;

#[derive(Default)]
struct Pacing {
    vblank: Option<Vblank>,
    retry_at: Option<Instant>,
    /// A browse page draw has occurred since the last pacing wait.
    pending: bool,
    /// Vblank sequence observed during drawing. The flip is submitted after
    /// drawing, so two later vblanks confirm it has completed.
    drawn_at: Option<u64>,
    /// A failure has occurred since the last successful wait, such as display power-off.
    failing: bool,
}

static PACING: Mutex<Option<Pacing>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Pacing) -> R) -> R {
    let mut pacing = PACING.lock().unwrap_or_else(|e| e.into_inner());
    f(pacing.get_or_insert_with(Pacing::default))
}

/// Mark a browse draw and capture its current vblank sequence.
pub fn drew() {
    with(|p| {
        p.pending = true;
        p.drawn_at = p.vblank.as_ref().and_then(|v| v.sequence().ok());
    });
}

/// Wait from the browse dirty callback before Kodi begins rendering.
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

/// Reset pacing resources during add-on shutdown.
pub fn stop() {
    with(|p| *p = Pacing::default());
}

impl Pacing {
    /// Log the first consecutive failure and suppress repeats until a wait succeeds.
    fn failed(&mut self, e: io::Error) {
        if !std::mem::replace(&mut self.failing, true) {
            error!("oriel-display: waiting for the display's refresh failed: {e}");
        }
        self.vblank = None;
        self.retry_at = Some(Instant::now() + RETRY);
    }
}

// Match the kernel DRM structures and ioctl numbers in drm.h and drm_mode.h.

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
/// Sequence event layout: drm_event (type, length), user_data, time_ns, sequence.
const EVENT_SIZE: usize = 32;

struct Vblank {
    file: File,
    crtc: u32,
    /// Match events to requests so delayed events cannot complete a later wait.
    request: u64,
}

impl Vblank {
    /// Select the first active CRTC found while scanning DRM cards.
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

    /// Read the active CRTC vblank sequence number.
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

    /// Wait for the requested next-vblank event within TIMEOUT_MS.
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
