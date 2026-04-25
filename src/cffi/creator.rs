//! Writer C ABI — placeholder. The full Creator surface (set_main_path,
//! add_item, add_metadata, add_redirection, add_illustration, write_to,
//! …) lands in a follow-up issue; the reader surface in the sibling
//! modules is what unblocks the libzim-shim work.
//!
//! Kept as a small stub here so the cbindgen header has a `zimru_creator_t`
//! forward-declared and downstream code can start sketching against it.

use crate::cffi::error::{set_err, zimru_error_t};

/// Opaque writer handle. Empty placeholder for now.
pub struct zimru_creator_t {
    _placeholder: (),
}

/// Allocate a new creator. Currently always succeeds; the writer C ABI
/// is otherwise unimplemented (see follow-up to issue #7).
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_new() -> *mut zimru_creator_t {
    Box::into_raw(Box::new(zimru_creator_t { _placeholder: () }))
}

/// Free a creator. Safe to call on NULL.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_free(c: *mut zimru_creator_t) {
    if !c.is_null() {
        drop(Box::from_raw(c));
    }
}

/// Placeholder for `Creator::write_to` — currently always errors out.
/// Real implementation lands with the rest of the writer C ABI.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_write_to(
    _c: *mut zimru_creator_t,
    _path: *const std::ffi::c_char,
    err: *mut *mut zimru_error_t,
) -> bool {
    set_err(
        err,
        crate::Error::Io(std::io::Error::other(
            "zimru_creator_write_to: writer C ABI not yet implemented",
        )),
    );
    false
}
