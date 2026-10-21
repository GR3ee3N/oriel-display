//! Kodi's add-on C API (`kodi/c-api` in its dev-kit), the parts the add-on
//! uses. `build/bindgen.sh` generates `bindings.rs` from the dev-kit at
//! `KODI_DEVKIT_REF`; `build/compile.sh` fails if it is out of date.

#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::all)]

include!("bindings.rs");
