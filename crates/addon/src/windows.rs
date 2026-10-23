//! Manage the standalone browse window and the player dialog over Kodi video.
//! The control client requests visibility changes; Kodi's GUI thread runs
//! the window callbacks.
//!
//! Never call Kodi while holding STATE. Window callbacks acquire STATE while
//! already holding Kodi's GUI lock, so reversing the lock order can deadlock.

use crate::control;
use crate::kodi::{self, RenderControl, Window, WindowEvents, WindowRef, info};
use crate::producer;
use crate::surface::{self, Page};
use kodi_sys as sys;
use serde_json::json;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Shared renderaddon control id in both skin XML files.
const CONTROL: i32 = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Browse,
    Player,
}

impl Which {
    pub fn name(self) -> &'static str {
        match self {
            Which::Browse => "browse",
            Which::Player => "player",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    fn xml(self) -> &'static str {
        match self {
            Which::Browse => "oriel-display-browse.xml",
            Which::Player => "oriel-display-player.xml",
        }
    }

    /// Player uses a dialog over fullscreen video; browse uses a standalone
    /// window that Kodi leaves when playback starts.
    fn dialog(self) -> bool {
        self == Which::Player
    }
}

struct State {
    windows: [Option<Window>; 2],
    open: [bool; 2],
    /// Kodi window ids reported during the most recent opening.
    ids: [i32; 2],
}

impl State {
    const fn new() -> State {
        State { windows: [None, None], open: [false, false], ids: [-1, -1] }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());
static DRAW: AtomicBool = AtomicBool::new(true);
static UNLOADING: AtomicBool = AtomicBool::new(false);
/// Request repeated GUI-thread closure attempts; CLOSED signals completion.
static CLOSE_REQUEST: AtomicBool = AtomicBool::new(false);
static CLOSED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
/// Timestamp of the latest GUI-thread closure attempt.
static CLOSE_TRIED: Mutex<Option<Instant>> = Mutex::new(None);
/// Minimum interval between closure attempts.
const CLOSE_RETRY: Duration = Duration::from_millis(200);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Actions delivered as page keys; leave volume, screenshots and keymap
/// commands to Kodi.
const PAGE_KEYS: &[RangeInclusive<u32>] = &[
    sys::ADDON_ACTION_MOVE_LEFT..=sys::ADDON_ACTION_SELECT_ITEM,
    sys::ADDON_ACTION_PREVIOUS_MENU..=sys::ADDON_ACTION_PREV_ITEM,
    sys::ADDON_ACTION_PLAYER_FORWARD..=sys::ADDON_ACTION_PLAYER_PLAY,
    sys::ADDON_ACTION_NAV_BACK..=sys::ADDON_ACTION_NAV_BACK,
    sys::ADDON_ACTION_CONTEXT_MENU..=sys::ADDON_ACTION_CONTEXT_MENU,
    sys::ADDON_ACTION_REMOTE_0..=sys::ADDON_ACTION_REMOTE_9,
    sys::ADDON_ACTION_JUMP_SMS2..=sys::ADDON_ACTION_JUMP_SMS9,
    sys::ADDON_ACTION_PLAYER_PLAYPAUSE..=sys::ADDON_ACTION_PLAYER_PLAYPAUSE,
];

/// Playback actions also handled by Kodi while the player dialog is open.
const PLAYER_KEYS: &[u32] = &[
    sys::ADDON_ACTION_PAUSE,
    sys::ADDON_ACTION_PLAYER_PLAY,
    sys::ADDON_ACTION_PLAYER_PLAYPAUSE,
    sys::ADDON_ACTION_STOP,
    sys::ADDON_ACTION_PLAYER_FORWARD,
    sys::ADDON_ACTION_PLAYER_REWIND,
    sys::ADDON_ACTION_PREV_ITEM,
];

/// Return true to consume the Kodi action; report every action to the client regardless.
fn consumed(which: Which, action: u32) -> bool {
    if which == Which::Player && PLAYER_KEYS.contains(&action) {
        return false;
    }
    PAGE_KEYS.iter().any(|keys| keys.contains(&action))
}

fn now() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

/// Discard stale handles on load without calling Kodi; the prior add-on
/// instance that owned them has already been destroyed.
pub fn reset() {
    let old = std::mem::replace(&mut *state(), State::new());
    for window in old.windows.into_iter().flatten() {
        window.abandon();
    }
    DRAW.store(true, Ordering::Release);
    UNLOADING.store(false, Ordering::Release);
    CLOSE_REQUEST.store(false, Ordering::Release);
}

pub fn unloading() -> bool {
    UNLOADING.load(Ordering::Acquire)
}

/// Apply the control client's draw setting to page visibility.
pub fn drawing() -> bool {
    DRAW.load(Ordering::Acquire) && !unloading()
}

fn update_shown() {
    let open = state().open.iter().any(|&o| o);
    producer::set_shown(open && drawing());
}

/// Create the window on first use and request that Kodi show it.
pub fn open(which: Which, mark: Option<&str>) {
    let exists = state().windows[which.index()].is_some();
    if !exists {
        let Some(window) = Window::new(which.xml(), which.dialog(), Box::new(Events::new(which))) else {
            kodi::error!("oriel-display: cannot create the {} window", which.name());
            return;
        };
        // Use absolute media paths because Kodi window lookup does not resolve add-on assets.
        if let Some(path) = kodi::addon_path() {
            window.window_ref().set_property("white", &format!("{path}/resources/skins/Default/media/white.png"));
        }
        state().windows[which.index()] = Some(window);
    }
    let Some(window) = state().windows[which.index()].as_ref().map(Window::window_ref) else { return };
    if which == Which::Browse {
        window.set_property("mark", mark.unwrap_or(""));
        window.set_property("started", "");
    }
    info!("oriel-display: opening the {} window", which.name());
    window.show();
}

pub fn close(which: Which) {
    let window = state().windows[which.index()].as_ref().map(Window::window_ref);
    if let Some(window) = window {
        info!("oriel-display: closing the {} window", which.name());
        window.close();
    }
}

/// Schedule closure from the next GUI frame and retry until complete.
/// Calls from other threads cannot close windows during Kodi sleep transitions.
pub fn request_close() {
    *CLOSE_TRIED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    CLOSE_REQUEST.store(true, Ordering::Release);
}

/// Hide the browse startup mark after the page reports its first frame.
pub fn started() {
    let window = state().windows[Which::Browse.index()].as_ref().map(Window::window_ref);
    if let Some(window) = window {
        window.set_property("started", "1");
    }
}

pub fn set_draw(draw: bool) {
    DRAW.store(draw, Ordering::Release);
    if draw {
        surface::redraw();
    }
    update_shown();
}

/// Handle render-control destruction as window closure on Kodi's GUI thread.
pub fn closed(which: Which) {
    let any_open = {
        let mut state = state();
        state.open[which.index()] = false;
        state.open.iter().any(|&o| o)
    };
    update_shown();
    if !any_open {
        surface::drop_buffers();
        close_done();
    }
    control::send(json!({"window": which.name(), "state": "closed"}));
}

/// Process closure requests during the GUI-thread dirty check, where window
/// closure cannot block waiting for the GUI thread itself.
pub fn on_gui_thread() {
    if !CLOSE_REQUEST.swap(false, Ordering::AcqRel) {
        return;
    }
    // Cancel stale closure requests if a new control client has connected.
    let (windows, player): (Vec<(Which, WindowRef)>, Option<i32>) = if !unloading() && control::connected() {
        (Vec::new(), None)
    } else {
        let state = state();
        let windows = [Which::Player, Which::Browse]
            .into_iter()
            .filter(|w| state.open[w.index()])
            .filter_map(|w| state.windows[w.index()].as_ref().map(|window| (w, window.window_ref())))
            .collect();
        let player = Which::Player.index();
        (windows, state.open[player].then_some(state.ids[player]))
    };
    if windows.is_empty() {
        signal_closed();
        return;
    }
    // Keep closure pending until closed reports the last window gone.
    CLOSE_REQUEST.store(true, Ordering::Release);
    // Browse closure restores the previous window, which Kodi cannot activate
    // while a modal dialog is open. Wait for that dialog to close. Close the
    // player first; Kodi excludes a closing dialog from the modal check.
    let modal = kodi::modal_dialog().is_some_and(|id| Some(id) != player);
    let windows: Vec<WindowRef> =
        windows.into_iter().filter(|&(which, _)| which.dialog() || !modal).map(|(_, window)| window).collect();
    if windows.is_empty() {
        return;
    }
    {
        let mut tried = CLOSE_TRIED.lock().unwrap_or_else(|e| e.into_inner());
        if tried.is_some_and(|at| at.elapsed() < CLOSE_RETRY) {
            return;
        }
        *tried = Some(Instant::now());
    }
    for window in windows {
        window.close();
    }
}

/// Complete the pending request after the last window closes.
fn close_done() {
    CLOSE_REQUEST.store(false, Ordering::Release);
    signal_closed();
}

/// Let unload proceed when windows are closed or owned by a new client.
fn signal_closed() {
    let (done, signal) = &CLOSED;
    *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
    signal.notify_all();
}

/// Request window closure, notify the client and stop socket threads before
/// unload. Bound the closure wait because Kodi may hold locks needed by its GUI thread.
pub fn unload() {
    UNLOADING.store(true, Ordering::Release);
    let open = state().open.iter().any(|&o| o);
    if open {
        let (done, signal) = &CLOSED;
        *done.lock().unwrap_or_else(|e| e.into_inner()) = false;
        request_close();
        let guard = done.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, timeout) = signal
            .wait_timeout_while(guard, Duration::from_millis(500), |done| !*done)
            .unwrap_or_else(|e| e.into_inner());
        if timeout.timed_out() && !*guard {
            kodi::error!("oriel-display: the windows did not close before unloading");
        }
    }
    control::send(json!({"gone": true}));
    control::stop();
    producer::stop();
}

struct Events {
    which: Which,
    control: Mutex<Option<RenderControl>>,
}

impl Events {
    fn new(which: Which) -> Events {
        Events { which, control: Mutex::new(None) }
    }
}

impl WindowEvents for Events {
    fn init(&self, window: WindowRef) {
        let id = if self.which.dialog() { kodi::current_dialog_id() } else { kodi::current_window_id() };
        info!("oriel-display: {} window open, Kodi window {id}", self.which.name());
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        // Clear the render control after Kodi removes its callbacks on closure.
        *control = None;
        *control = RenderControl::attach(window, CONTROL, Box::new(Page::new(self.which)));
        drop(control);
        {
            let mut state = state();
            state.open[self.which.index()] = true;
            state.ids[self.which.index()] = id;
        }
        surface::redraw();
        update_shown();
        // Close windows restored by Kodi history when their app client has disconnected.
        if !control::connected() {
            info!("oriel-display: no app for the {} window, closing it", self.which.name());
            request_close();
            return;
        }
        control::send(json!({"window": self.which.name(), "id": id, "state": "opened"}));
    }

    fn action(&self, _window: WindowRef, action: u32) -> bool {
        if unloading() {
            return false;
        }
        let kept = consumed(self.which, action);
        control::send(json!({"key": action, "t": now(), "kodi": !kept}));
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_keys_stay_from_kodi_except_player_keys_in_playback() {
        assert!(consumed(Which::Browse, sys::ADDON_ACTION_MOVE_UP));
        assert!(consumed(Which::Browse, sys::ADDON_ACTION_NAV_BACK));
        assert!(consumed(Which::Browse, sys::ADDON_ACTION_PLAYER_PLAYPAUSE));
        assert!(!consumed(Which::Player, sys::ADDON_ACTION_PLAYER_PLAYPAUSE));
        assert!(!consumed(Which::Player, sys::ADDON_ACTION_STOP));
        assert!(consumed(Which::Player, sys::ADDON_ACTION_NEXT_ITEM));
        assert!(consumed(Which::Player, sys::ADDON_ACTION_SHOW_INFO));
        assert!(consumed(Which::Browse, sys::ADDON_ACTION_REMOTE_0));
        assert!(consumed(Which::Browse, sys::ADDON_ACTION_REMOTE_9));
        assert!(consumed(Which::Player, sys::ADDON_ACTION_REMOTE_0));
        assert!(!consumed(Which::Browse, sys::ADDON_ACTION_BUILT_IN_FUNCTION));
        assert!(!consumed(Which::Browse, sys::ADDON_ACTION_VOLUME_UP));
    }
}
