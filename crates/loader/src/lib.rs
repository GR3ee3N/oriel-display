//! Load lib/oriel-display-<build>.so beside vfs.oriel.display.so and forward
//! Kodi entry-point calls to it.
//!
//! Window callbacks can outlive add-on unload, so the implementation library
//! stays mapped. Give each build a unique filename so updates load their own
//! code rather than reusing the mapped library at the same path. Kodi unloads
//! this loader independently of the retained implementation.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type Create = unsafe extern "C" fn(*mut c_void) -> c_uint;
type TypeVersion = extern "C" fn(c_int) -> *const c_char;

struct Addon {
    create: Create,
    type_version: TypeVersion,
    type_min_version: TypeVersion,
}

static ADDON: OnceLock<Option<Addon>> = OnceLock::new();

/// Kodi status code for permanent add-on startup failure.
const FAILURE: c_uint = 5;

fn here() -> Option<PathBuf> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(here as *const c_void, &mut info) } == 0 || info.dli_fname.is_null() {
        return None;
    }
    let path = unsafe { CStr::from_ptr(info.dli_fname) };
    Some(Path::new(std::ffi::OsStr::from_bytes(path.to_bytes())).parent()?.to_path_buf())
}

/// Require exactly one lib/oriel-display-*.so implementation in the installation.
fn find(dir: &Path) -> Option<PathBuf> {
    let lib = dir.join("lib");
    let mut builds: Vec<PathBuf> = match std::fs::read_dir(&lib) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("oriel-display-") && n.ends_with(".so"))
            })
            .collect(),
        Err(e) => {
            eprintln!("vfs.oriel.display: {}: {e}", lib.display());
            return None;
        }
    };
    if builds.len() != 1 {
        eprintln!("vfs.oriel.display: {} add-on builds in {}, expected one", builds.len(), lib.display());
        return None;
    }
    builds.pop()
}

unsafe fn symbol<T>(handle: *mut c_void, name: &CStr) -> Option<T> {
    let p = unsafe { libc::dlsym(handle, name.as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
}

fn load() -> Option<Addon> {
    let path = find(&here()?)?;
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL | libc::RTLD_NODELETE) };
    if handle.is_null() {
        let why = unsafe { libc::dlerror() };
        if !why.is_null() {
            eprintln!("vfs.oriel.display: {}", unsafe { CStr::from_ptr(why) }.to_string_lossy());
        }
        return None;
    }
    unsafe {
        Some(Addon {
            create: symbol(handle, c"ADDON_Create")?,
            type_version: symbol(handle, c"ADDON_GetTypeVersion")?,
            type_min_version: symbol(handle, c"ADDON_GetTypeMinVersion")?,
        })
    }
}

fn addon() -> Option<&'static Addon> {
    ADDON.get_or_init(load).as_ref()
}

/// # Safety
/// interface must point to a valid Kodi AddonGlobalInterface.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ADDON_Create(interface: *mut c_void) -> c_uint {
    match addon() {
        Some(addon) => unsafe { (addon.create)(interface) },
        None => FAILURE,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ADDON_GetTypeVersion(kind: c_int) -> *const c_char {
    addon().map_or(c"".as_ptr(), |a| (a.type_version)(kind))
}

#[unsafe(no_mangle)]
pub extern "C" fn ADDON_GetTypeMinVersion(kind: c_int) -> *const c_char {
    addon().map_or(c"".as_ptr(), |a| (a.type_min_version)(kind))
}
