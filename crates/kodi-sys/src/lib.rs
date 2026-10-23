//! Bindings for the Kodi C API used by this add-on. build/bindgen.sh generates
//! bindings.rs from the dev-kit pinned by KODI_DEVKIT_REF; build/compile.sh
//! verifies that the committed bindings match those headers.

#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::all)]

include!("bindings.rs");
