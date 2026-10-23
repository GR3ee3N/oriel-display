//! Aggregate rendering statistics for Kodi debug logs in 10 s intervals.

use crate::gl::GLint;
use crate::kodi::debug;
use crate::producer;
use crate::surface::PATTERN_MODE;
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct Stats {
    since: f64,
    refresh: f64,
    last_render: f64,
    frame_start: f64,
    frames: u32,
    /// Count passes within 3 ms of the previous render as part of the same GUI frame.
    passes: u32,
    /// Count skipped opaque front-to-back passes.
    opaque_passes: u32,
    interval_sum: f64,
    interval_max: f64,
    /// Count frame intervals longer than 1.5 display refresh periods.
    late: u32,
    cpu_sum: f64,
    cpu_max: f64,
    gpu_sum: u64,
    gpu_max: u64,
    gpu_n: u32,
    drawn_new: u32,
    last_new: f64,
    /// Count gaps between new page frames longer than 1.5 display refresh periods.
    new_gaps: u32,
    imports: u32,
    /// Framebuffer bound during drawing; HDR GUI composition uses a nonzero id.
    fbo: GLint,
    viewport: [GLint; 4],
}

/// Read output refresh rate from a CoreELEC mode name such as "1080p23hz".
fn refresh() -> f64 {
    let mode = std::fs::read_to_string("/sys/class/display/mode").unwrap_or_default();
    let hz: u32 = mode
        .split_once('p')
        .map(|(_, rest)| rest.chars().take_while(char::is_ascii_digit).collect::<String>())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(60);
    match hz {
        23 => 24000.0 / 1001.0,
        29 => 30000.0 / 1001.0,
        59 => 60000.0 / 1001.0,
        0 => 60.0,
        hz => f64::from(hz),
    }
}

impl Stats {
    /// Start collecting after surface GL initialization at timestamp now.
    pub fn start(&mut self, now: f64) {
        self.since = now;
        self.refresh = refresh();
    }

    pub fn opaque_pass(&mut self) {
        self.opaque_passes += 1;
    }

    pub fn import(&mut self) {
        self.imports += 1;
    }

    /// Record GPU draw duration in nanoseconds.
    pub fn gpu(&mut self, ns: u64) {
        self.gpu_sum += ns;
        self.gpu_max = self.gpu_max.max(ns);
        self.gpu_n += 1;
    }

    /// Record the framebuffer and viewport supplied by Kodi for drawing.
    pub fn target(&mut self, fbo: GLint, viewport: [GLint; 4]) {
        self.fbo = fbo;
        self.viewport = viewport;
    }

    /// Record the first draw of a new page frame at timestamp t.
    pub fn new_frame(&mut self, t: f64) {
        self.drawn_new += 1;
        if self.last_new > 0.0 && t - self.last_new > 1.5 / self.refresh {
            self.new_gaps += 1;
        }
        self.last_new = t;
    }

    /// Record a render interval and log/reset counters after 10 s.
    pub fn render(&mut self, t0: f64, t1: f64) {
        let s = self;
        if s.last_render > 0.0 {
            if t0 - s.last_render < 0.003 {
                s.passes += 1;
                s.last_render = t0;
                s.cpu_sum += t1 - t0;
                s.cpu_max = s.cpu_max.max(t1 - t0);
                return;
            }
            let interval = t0 - s.frame_start;
            s.interval_sum += interval;
            s.interval_max = s.interval_max.max(interval);
            if interval > 1.5 / s.refresh {
                s.late += 1;
            }
        }
        s.last_render = t0;
        s.frame_start = t0;
        s.frames += 1;
        s.cpu_sum += t1 - t0;
        s.cpu_max = s.cpu_max.max(t1 - t0);
        if t1 - s.since < 10.0 {
            return;
        }
        let (frames_in, replaced) = {
            let queue = producer::queue();
            (queue.frames_in, queue.frames_replaced)
        };
        let n = f64::from(s.frames);
        let ms = |seconds: f64| seconds * 1e3;
        debug!(
            "oriel-display: {} {:.3}Hz frames={} ({:.1}/s) extra passes={} skipped opaque passes={} interval mean={:.2}ms max={:.2}ms late(>1.5 vblank)={} cpu mean={:.3}ms max={:.3}ms gpu mean={:.3}ms max={:.3}ms (n {}) new frames drawn={} (gaps>1.5 vblank {}) imports={} fbo={} viewport={}x{} | producer frames={} replaced before drawn={}",
            if PATTERN_MODE.load(Ordering::Relaxed) { "pattern" } else { "socket" },
            s.refresh,
            s.frames,
            n / (t1 - s.since),
            s.passes,
            s.opaque_passes,
            if s.frames > 1 { ms(s.interval_sum / (n - 1.0)) } else { 0.0 },
            ms(s.interval_max),
            s.late,
            ms(s.cpu_sum / (n + f64::from(s.passes))),
            ms(s.cpu_max),
            if s.gpu_n > 0 { s.gpu_sum as f64 / 1e6 / f64::from(s.gpu_n) } else { 0.0 },
            s.gpu_max as f64 / 1e6,
            s.gpu_n,
            s.drawn_new,
            s.new_gaps,
            s.imports,
            s.fbo,
            s.viewport[2],
            s.viewport[3],
            frames_in,
            replaced
        );
        *s = Stats {
            since: t1,
            refresh: refresh(),
            last_render: t0,
            frame_start: t0,
            last_new: s.last_new,
            ..Stats::default()
        };
    }
}
