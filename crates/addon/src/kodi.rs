//! A small safe layer over Kodi's add-on C API (`kodi-sys`): the library's
//! entry points, logging, a VFS instance, windows and the render control.
//!
//! Kodi calls everything here from its own threads. Every callback catches
//! panics, which must not unwind into Kodi.
//!
//! Kodi unloads the library after `destroy`, but its windows can still call
//! back afterwards, so the library pins itself in memory (`RTLD_NODELETE`).
//! After [`detach`] nothing here calls Kodi any more.

use kodi_sys as sys;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, Ordering};

static INTERFACE: AtomicPtr<sys::AddonGlobalInterface> = AtomicPtr::new(null_mut());
static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// What the add-on gives Kodi.
pub struct Hooks {
    /// The library was loaded, or loaded again after a `destroy`.
    pub create: fn(),
    /// Kodi is about to unload the library. Call [`detach`] at the end.
    pub destroy: fn(),
    /// `Exists` on the add-on's VFS protocol, with the whole URL.
    pub vfs_exists: fn(&str) -> bool,
}

#[derive(Clone, Copy)]
pub enum Level {
    Debug = sys::ADDON_LOG_DEBUG as isize,
    Info = sys::ADDON_LOG_INFO as isize,
    Error = sys::ADDON_LOG_ERROR as isize,
}

/// Stops all calls into Kodi; its tables go with the add-on.
pub fn detach() {
    INTERFACE.store(null_mut(), Ordering::Release);
}

fn to_kodi() -> Option<&'static sys::AddonToKodiFuncTable_Addon> {
    let interface = INTERFACE.load(Ordering::Acquire);
    // Set once in ADDON_Create; Kodi keeps the tables until it unloads us.
    unsafe { interface.as_ref().and_then(|i| i.toKodi.as_ref()) }
}

/// Kodi's handle for this add-on, the first argument of most calls.
fn base() -> *mut c_void {
    to_kodi().map_or(null_mut(), |t| t.kodiBase)
}

fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', " ")).unwrap_or_default()
}

/// Where Kodi installed the add-on.
pub fn addon_path() -> Option<String> {
    let kodi = to_kodi()?;
    let get = unsafe { kodi.kodi_addon.as_ref() }?.get_addon_path?;
    let path = unsafe { get(base()) };
    if path.is_null() {
        return None;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().into_owned();
    if let Some(free) = kodi.free_string {
        unsafe { free(base(), path) };
    }
    Some(text)
}

pub fn log(level: Level, message: &str) {
    let Some(log) = to_kodi().and_then(|t| t.addon_log_msg) else { return };
    // Kodi clears it while unloading the add-on.
    if base().is_null() {
        return;
    }
    let message = c_string(message);
    unsafe { log(base(), level as c_int, message.as_ptr()) };
}

/// Shown only with Kodi's debug logging on.
macro_rules! debug {
    ($($arg:tt)*) => { $crate::kodi::log($crate::kodi::Level::Debug, &format!($($arg)*)) };
}
macro_rules! info {
    ($($arg:tt)*) => { $crate::kodi::log($crate::kodi::Level::Info, &format!($($arg)*)) };
}
macro_rules! error {
    ($($arg:tt)*) => { $crate::kodi::log($crate::kodi::Level::Error, &format!($($arg)*)) };
}
pub(crate) use {debug, error, info};

/// Runs a callback from Kodi; a panic is logged and gives `fallback`.
fn guard<R>(name: &str, fallback: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(panic) => {
            let why = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            log(Level::Error, &format!("oriel-display: panic in {name}: {why}"));
            fallback
        }
    }
}

/// Kodi's first call after loading the library.
///
/// # Safety
/// `interface` is Kodi's `AddonGlobalInterface`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ADDON_Create(interface: *mut c_void) -> sys::ADDON_STATUS {
    guard("ADDON_Create", sys::ADDON_STATUS_PERMANENT_FAILURE, || unsafe {
        let interface = interface.cast::<sys::AddonGlobalInterface>();
        let Some(global) = interface.as_mut() else { return sys::ADDON_STATUS_PERMANENT_FAILURE };
        let Some(to_addon) = global.toAddon.as_mut() else { return sys::ADDON_STATUS_PERMANENT_FAILURE };
        if global.toKodi.is_null() {
            return sys::ADDON_STATUS_PERMANENT_FAILURE;
        }
        pin_library();
        INTERFACE.store(interface, Ordering::Release);
        // Kodi hands this back to every call below; we keep no state in it.
        global.addonBase = interface.cast();
        to_addon.create = None;
        to_addon.destroy = Some(destroy);
        to_addon.create_instance = Some(create_instance);
        to_addon.destroy_instance = Some(destroy_instance);
        let hooks = HOOKS.get_or_init(crate::hooks);
        (hooks.create)();
        sys::ADDON_STATUS_OK
    })
}

/// Keeps this library mapped after Kodi's `dlclose`.
fn pin_library() {
    unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(pin_library as *const c_void, &mut info) == 0 || info.dli_fname.is_null() {
            log(Level::Error, "oriel-display: cannot find the library to pin it");
            return;
        }
        // A reference never released.
        if libc::dlopen(info.dli_fname, libc::RTLD_NOW | libc::RTLD_NOLOAD | libc::RTLD_NODELETE).is_null() {
            log(Level::Error, "oriel-display: cannot pin the library");
        }
    }
}

/// The API versions the library was built against, per type it uses. Kodi
/// refuses the add-on unless they are at least Kodi's minimums.
#[unsafe(no_mangle)]
pub extern "C" fn ADDON_GetTypeVersion(kind: c_int) -> *const c_char {
    type_version(kind, false)
}

/// The oldest API versions the library works with, per type it uses.
#[unsafe(no_mangle)]
pub extern "C" fn ADDON_GetTypeMinVersion(kind: c_int) -> *const c_char {
    type_version(kind, true)
}

fn type_version(kind: c_int, min: bool) -> *const c_char {
    let (version, minimum): (&'static [u8], &'static [u8]) = match c_uint::try_from(kind) {
        Ok(sys::ADDON_GLOBAL_MAIN) => (sys::ADDON_GLOBAL_VERSION_MAIN, sys::ADDON_GLOBAL_VERSION_MAIN_MIN),
        Ok(sys::ADDON_GLOBAL_GUI) => (sys::ADDON_GLOBAL_VERSION_GUI, sys::ADDON_GLOBAL_VERSION_GUI_MIN),
        Ok(sys::ADDON_INSTANCE_VFS) => (sys::ADDON_INSTANCE_VERSION_VFS, sys::ADDON_INSTANCE_VERSION_VFS_MIN),
        // Not used: Kodi skips the check for an empty version.
        _ => (b"\0", b"\0"),
    };
    (if min { minimum } else { version }).as_ptr().cast()
}

unsafe extern "C" fn destroy(_hdl: *mut c_void) {
    guard("destroy", (), || {
        if let Some(hooks) = HOOKS.get() {
            (hooks.destroy)();
        }
    });
}

unsafe extern "C" fn create_instance(
    _hdl: *mut c_void,
    instance: *mut sys::KODI_ADDON_INSTANCE_STRUCT,
) -> sys::ADDON_STATUS {
    guard("create_instance", sys::ADDON_STATUS_UNKNOWN, || unsafe {
        let Some(instance) = instance.as_mut() else { return sys::ADDON_STATUS_UNKNOWN };
        let Some(info) = instance.info.as_ref() else { return sys::ADDON_STATUS_UNKNOWN };
        if i64::from(info.type_) != i64::from(sys::ADDON_INSTANCE_VFS) {
            return sys::ADDON_STATUS_UNKNOWN;
        }
        let vfs = instance.__bindgen_anon_1.vfs;
        let Some(to_addon) = vfs.as_mut().and_then(|v| v.toAddon.as_mut()) else {
            return sys::ADDON_STATUS_PERMANENT_FAILURE;
        };
        // Kodi checks the others for null before calling them.
        to_addon.addonInstance = vfs.cast();
        to_addon.exists = Some(vfs_exists);
        to_addon.stat = Some(vfs_stat);
        instance.hdl = vfs.cast();
        sys::ADDON_STATUS_OK
    })
}

unsafe extern "C" fn destroy_instance(_hdl: *mut c_void, _instance: *mut sys::KODI_ADDON_INSTANCE_STRUCT) {}

unsafe fn url_text(url: *const sys::VFSURL) -> String {
    unsafe { url.as_ref().filter(|u| !u.url.is_null()).map(|u| CStr::from_ptr(u.url).to_string_lossy().into_owned()) }
        .unwrap_or_default()
}

unsafe extern "C" fn vfs_exists(_instance: *const sys::AddonInstance_VFSEntry, url: *const sys::VFSURL) -> bool {
    guard("exists", false, || {
        let url = unsafe { url_text(url) };
        HOOKS.get().is_some_and(|h| (h.vfs_exists)(&url))
    })
}

unsafe extern "C" fn vfs_stat(
    _instance: *const sys::AddonInstance_VFSEntry,
    _url: *const sys::VFSURL,
    buffer: *mut sys::STAT_STRUCTURE,
) -> c_int {
    if let Some(buffer) = unsafe { buffer.as_mut() } {
        *buffer = unsafe { std::mem::zeroed() };
    }
    0
}

fn gui() -> Option<&'static sys::AddonToKodiFuncTable_kodi_gui> {
    to_kodi().and_then(|t| unsafe { t.kodi_gui.as_ref() })
}

fn window_api() -> Option<&'static sys::AddonToKodiFuncTable_kodi_gui_window> {
    gui().and_then(|g| unsafe { g.window.as_ref() })
}

fn rendering_api() -> Option<&'static sys::AddonToKodiFuncTable_kodi_gui_control_rendering> {
    gui().and_then(|g| unsafe { g.control_rendering.as_ref() })
}

fn general_api() -> Option<&'static sys::AddonToKodiFuncTable_kodi_gui_general> {
    gui().and_then(|g| unsafe { g.general.as_ref() })
}

/// Kodi's active window.
pub fn current_window_id() -> i32 {
    general_api().and_then(|g| g.get_current_window_id).map_or(-1, |f| unsafe { f(base()) })
}

/// Kodi's topmost modal dialog, or `WINDOW_INVALID` (9999).
pub fn current_dialog_id() -> i32 {
    general_api().and_then(|g| g.get_current_window_dialog_id).map_or(-1, |f| unsafe { f(base()) })
}

/// Kodi's topmost modal dialog. Kodi refuses to activate a window while one
/// is open.
pub fn modal_dialog() -> Option<i32> {
    Some(current_dialog_id()).filter(|id| !matches!(id, -1 | 9999))
}

/// What a window hears from Kodi, on Kodi's GUI thread.
pub trait WindowEvents: Send + Sync {
    /// The window is being shown (also again after a close).
    fn init(&self, window: WindowRef);
    /// True if the window handled the action; false lets Kodi handle it.
    fn action(&self, window: WindowRef, action: u32) -> bool;
}

/// A window from the add-on's skin XML (resources/skins/Default/1080i).
pub struct Window {
    client: *mut Client,
}

/// What Kodi passes back to window callbacks.
struct Client {
    handle: *mut c_void,
    events: Box<dyn WindowEvents>,
}

// Kodi serialises window calls with its GUI lock.
unsafe impl Send for Window {}
unsafe impl Sync for Window {}

/// A window as a callback sees it, or a window to call without holding it.
#[derive(Clone, Copy)]
pub struct WindowRef(*mut c_void);

// Kodi keeps the window until Kodi exits; its calls take Kodi's GUI lock.
unsafe impl Send for WindowRef {}
unsafe impl Sync for WindowRef {}

impl Window {
    pub fn new(xml: &str, dialog: bool, events: Box<dyn WindowEvents>) -> Option<Window> {
        let api = window_api()?;
        let xml = c_string(xml);
        let handle = unsafe { api.create?(base(), xml.as_ptr(), c"Default".as_ptr(), dialog, false) };
        if handle.is_null() {
            return None;
        }
        let client = Box::into_raw(Box::new(Client { handle, events }));
        unsafe {
            api.set_callbacks?(
                base(),
                handle,
                client.cast(),
                Some(on_init),
                Some(on_focus),
                Some(on_click),
                Some(on_action),
                Some(on_context_buttons),
                Some(on_context_button),
            )
        };
        Some(Window { client })
    }

    fn handle(&self) -> *mut c_void {
        unsafe { (*self.client).handle }
    }

    /// For calls without holding on to the window.
    pub fn window_ref(&self) -> WindowRef {
        WindowRef(self.handle())
    }

    /// Lets go without asking Kodi to destroy the window: after a reload Kodi
    /// no longer knows our old handle for it.
    pub fn abandon(self) {
        std::mem::forget(self);
    }
}

impl WindowRef {
    pub fn show(self) -> bool {
        window_api().and_then(|a| a.show).is_some_and(|show| unsafe { show(base(), self.0) })
    }

    pub fn close(self) -> bool {
        window_api().and_then(|a| a.close).is_some_and(|close| unsafe { close(base(), self.0) })
    }

    /// For `$INFO[Window.Property(key)]` in the skin XML.
    pub fn set_property(self, key: &str, value: &str) {
        let Some(set) = window_api().and_then(|a| a.set_property) else { return };
        let (key, value) = (c_string(key), c_string(value));
        unsafe { set(base(), self.0, key.as_ptr(), value.as_ptr()) };
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if let Some(destroy) = window_api().and_then(|a| a.destroy) {
            unsafe { destroy(base(), self.handle()) };
        }
        drop(unsafe { Box::from_raw(self.client) });
    }
}

unsafe fn client<'a>(client: *mut c_void) -> &'a Client {
    unsafe { &*client.cast::<Client>() }
}

unsafe extern "C" fn on_init(client: *mut c_void) -> bool {
    guard("window init", false, || {
        let client = unsafe { self::client(client) };
        client.events.init(WindowRef(client.handle));
        true
    })
}

unsafe extern "C" fn on_action(client: *mut c_void, action: sys::ADDON_ACTION) -> bool {
    guard("window action", false, || {
        let client = unsafe { self::client(client) };
        client.events.action(WindowRef(client.handle), action)
    })
}

unsafe extern "C" fn on_focus(_client: *mut c_void, _control: c_int) -> bool {
    false
}

unsafe extern "C" fn on_click(_client: *mut c_void, _control: c_int) -> bool {
    false
}

unsafe extern "C" fn on_context_buttons(
    _client: *mut c_void,
    _item: c_int,
    _buttons: *mut sys::gui_context_menu_pair,
    size: *mut c_uint,
) {
    if let Some(size) = unsafe { size.as_mut() } {
        *size = 0;
    }
}

unsafe extern "C" fn on_context_button(_client: *mut c_void, _item: c_int, _button: c_uint) -> bool {
    false
}

/// Draws a `renderaddon` control, on Kodi's GUI thread with its GL context
/// current. Kodi calls it on no other thread.
pub trait Renderer {
    /// The control's rectangle in screen pixels. False keeps it from drawing.
    fn create(&mut self, x: i32, y: i32, width: i32, height: i32) -> bool;
    fn render(&mut self);
    /// Kodi freed the control (the window closed).
    fn stop(&mut self);
    /// Polled every GUI frame; true has Kodi draw the frame.
    fn dirty(&mut self) -> bool;
}

/// A renderer attached to a window's `renderaddon` control.
pub struct RenderControl {
    handle: *mut c_void,
    renderer: *mut Box<dyn Renderer>,
}

// Its renderer is only called and dropped on Kodi's GUI thread.
unsafe impl Send for RenderControl {}

impl RenderControl {
    /// Kodi calls `create` from inside this, so call it on the GUI thread
    /// (from [`WindowEvents::init`]).
    pub fn attach(window: WindowRef, control: i32, renderer: Box<dyn Renderer>) -> Option<RenderControl> {
        let get = window_api()?.get_control_render_addon?;
        let set_callbacks = rendering_api()?.set_callbacks?;
        let handle = unsafe { get(base(), window.0, control) };
        if handle.is_null() {
            return None;
        }
        let renderer = Box::into_raw(Box::new(renderer));
        unsafe {
            set_callbacks(
                base(),
                handle,
                renderer.cast(),
                Some(render_create),
                Some(render_render),
                Some(render_stop),
                Some(render_dirty),
            )
        };
        Some(RenderControl { handle, renderer })
    }
}

impl Drop for RenderControl {
    fn drop(&mut self) {
        if let Some(destroy) = rendering_api().and_then(|a| a.destroy) {
            unsafe { destroy(base(), self.handle) };
        }
        drop(unsafe { Box::from_raw(self.renderer) });
    }
}

unsafe fn renderer<'a>(client: *mut c_void) -> &'a mut dyn Renderer {
    unsafe { &mut **client.cast::<Box<dyn Renderer>>() }
}

unsafe extern "C" fn render_create(
    client: *mut c_void,
    x: c_int,
    y: c_int,
    w: c_int,
    h: c_int,
    _device: *mut c_void,
) -> bool {
    guard("render create", false, || unsafe { renderer(client) }.create(x, y, w, h))
}

unsafe extern "C" fn render_render(client: *mut c_void) {
    guard("render", (), || unsafe { renderer(client) }.render());
}

unsafe extern "C" fn render_stop(client: *mut c_void) {
    guard("render stop", (), || unsafe { renderer(client) }.stop());
}

unsafe extern "C" fn render_dirty(client: *mut c_void) -> bool {
    guard("render dirty", false, || unsafe { renderer(client) }.dirty())
}
