//! The two Kodi windows the page lives in: the browse window, active while
//! the app browses, and the player dialog, over Kodi's fullscreen video.
//! The control client opens and closes them; Kodi's GUI thread runs their
//! callbacks.
//!
//! Never call Kodi while holding `STATE`: Kodi's GUI thread takes it in our
//! callbacks while it holds Kodi's GUI lock.

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

/// The render control in both skin files.
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

    /// The player is a dialog over Kodi's fullscreen video; browsing is a
    /// window of its own, which Kodi leaves when playback starts.
    fn dialog(self) -> bool {
        self == Which::Player
    }
}

struct State {
    windows: [Option<Window>; 2],
    open: [bool; 2],
    /// Kodi's id for each window, from when it last opened.
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
/// Asks Kodi's GUI thread to close the windows. It tries until they are
/// closed, and `CLOSED` signals that.
static CLOSE_REQUEST: AtomicBool = AtomicBool::new(false);
static CLOSED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
/// When the GUI thread last asked the windows to close.
static CLOSE_TRIED: Mutex<Option<Instant>> = Mutex::new(None);
/// Before it asks again.
const CLOSE_RETRY: Duration = Duration::from_millis(200);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keys the page gets; any other action stays with Kodi (volume, screenshots,
/// keymap commands).
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

/// During playback Kodi also acts on these.
const PLAYER_KEYS: &[u32] = &[
    sys::ADDON_ACTION_PAUSE,
    sys::ADDON_ACTION_PLAYER_PLAY,
    sys::ADDON_ACTION_PLAYER_PLAYPAUSE,
    sys::ADDON_ACTION_STOP,
    sys::ADDON_ACTION_PLAYER_FORWARD,
    sys::ADDON_ACTION_PLAYER_REWIND,
    sys::ADDON_ACTION_PREV_ITEM,
];

/// True keeps the action from Kodi. The client hears every action either way.
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

/// After a load: windows from an earlier load belong to an add-on Kodi has
/// destroyed, so they are dropped without telling Kodi.
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

/// Whether the page is drawn at all (`draw` from the client).
pub fn drawing() -> bool {
    DRAW.load(Ordering::Acquire) && !unloading()
}

fn update_shown() {
    let open = state().open.iter().any(|&o| o);
    producer::set_shown(open && drawing());
}

/// Opens (shows) a window, creating it the first time.
pub fn open(which: Which, mark: Option<&str>) {
    let exists = state().windows[which.index()].is_some();
    if !exists {
        let Some(window) = Window::new(which.xml(), which.dialog(), Box::new(Events::new(which))) else {
            kodi::error!("oriel-display: cannot create the {} window", which.name());
            return;
        };
        // Kodi doesn't find the add-on's own media by name in its windows.
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

/// Has Kodi's GUI thread close the open windows from its next frame on, until
/// they are closed: a close from another thread does nothing while Kodi goes
/// to sleep.
pub fn request_close() {
    *CLOSE_TRIED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    CLOSE_REQUEST.store(true, Ordering::Release);
}

/// The page drew its first frame: the browse window drops the app's mark.
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

/// A window's control was freed: the window closed. On Kodi's GUI thread.
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

/// Called from each GUI frame (the controls' dirty check), on Kodi's GUI
/// thread, where closing a window doesn't wait on that thread.
pub fn on_gui_thread() {
    if !CLOSE_REQUEST.swap(false, Ordering::AcqRel) {
        return;
    }
    // Another app may have connected since the request.
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
    // Until `closed` hears the last one go.
    CLOSE_REQUEST.store(true, Ordering::Release);
    // Closing the browse window activates the window before it. Kodi refuses
    // that while a modal dialog is open, as while it goes to sleep, and the
    // window then has none to go back to; so it waits for the dialog. The
    // player closes first in the same pass, and Kodi skips a closing dialog.
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

/// The last window closed: a close request is done with.
fn close_done() {
    CLOSE_REQUEST.store(false, Ordering::Release);
    signal_closed();
}

/// For `unload`: no window is left to close, or an app has them again.
fn signal_closed() {
    let (done, signal) = &CLOSED;
    *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
    signal.notify_all();
}

/// Before Kodi unloads the add-on: the windows close, the client hears it,
/// and the threads end. Kodi may hold locks its GUI thread needs, so the
/// wait for the windows is bounded.
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
        // Kodi drops the control's callbacks when the window closes.
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
        // Kodi went back to a window of an app that has gone.
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
