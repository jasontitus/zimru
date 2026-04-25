use std::ffi::{c_char, CString};

use crate::cffi::error::{set_err, zimru_error_t};
use crate::cffi::item::{zimru_item_t, ITEM_VTABLE};
use crate::Entry;

/// Opaque entry handle. Holds a Rust `Entry` plus owned `CString`s for
/// `path` and `title` so the C-side `const char*` returned by
/// `zimru_entry_path` / `zimru_entry_title` are stable for the entry's
/// lifetime.
pub struct zimru_entry_t {
    pub(crate) inner: Entry,
    path: CString,
    title: CString,
}

pub(crate) struct EntryVtable;
pub(crate) const ENTRY_VTABLE: EntryVtable = EntryVtable;

impl EntryVtable {
    pub(crate) fn box_entry(&self, e: Entry) -> *mut zimru_entry_t {
        let path = CString::new(e.path()).unwrap_or_else(|_| CString::new("").unwrap());
        let title = CString::new(e.title()).unwrap_or_else(|_| CString::new("").unwrap());
        Box::into_raw(Box::new(zimru_entry_t {
            inner: e,
            path,
            title,
        }))
    }
}

/// Free a `zimru_entry_t` previously returned by an Archive lookup.
#[no_mangle]
pub unsafe extern "C" fn zimru_entry_free(e: *mut zimru_entry_t) {
    if !e.is_null() {
        drop(Box::from_raw(e));
    }
}

/// NUL-terminated path (e.g. "index.html"). Lifetime tied to the entry.
#[no_mangle]
pub unsafe extern "C" fn zimru_entry_path(e: *const zimru_entry_t) -> *const c_char {
    if e.is_null() {
        return std::ptr::null();
    }
    (*e).path.as_ptr()
}

/// NUL-terminated title. Lifetime tied to the entry.
#[no_mangle]
pub unsafe extern "C" fn zimru_entry_title(e: *const zimru_entry_t) -> *const c_char {
    if e.is_null() {
        return std::ptr::null();
    }
    (*e).title.as_ptr()
}

/// Single-byte namespace (`'C'`, `'M'`, `'W'`, `'X'`, `'A'`).
#[no_mangle]
pub unsafe extern "C" fn zimru_entry_namespace(e: *const zimru_entry_t) -> u8 {
    if e.is_null() {
        return 0;
    }
    (*e).inner.namespace()
}

/// URL-pointer index of the entry.
#[no_mangle]
pub unsafe extern "C" fn zimru_entry_index(e: *const zimru_entry_t) -> u32 {
    if e.is_null() {
        return 0;
    }
    (*e).inner.index()
}

/// True iff this entry is a redirect.
#[no_mangle]
pub unsafe extern "C" fn zimru_entry_is_redirect(e: *const zimru_entry_t) -> bool {
    if e.is_null() {
        return false;
    }
    (*e).inner.is_redirect()
}

/// Resolve an entry to an Item. If `follow` is true and the entry is a
/// redirect, the chain is followed; if false, calling on a redirect
/// returns `NotAnItem`.
#[no_mangle]
pub unsafe extern "C" fn zimru_entry_get_item(
    e: *const zimru_entry_t,
    follow: bool,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_item_t {
    if e.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    match (*e).inner.get_item(follow) {
        Ok(item) => ITEM_VTABLE.box_item(item),
        Err(err_) => {
            set_err(err, err_);
            std::ptr::null_mut()
        }
    }
}

/// For a redirect entry, return the (one-hop) target entry.
#[no_mangle]
pub unsafe extern "C" fn zimru_entry_get_redirect_entry(
    e: *const zimru_entry_t,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_entry_t {
    if e.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    match (*e).inner.get_redirect_entry() {
        Ok(t) => ENTRY_VTABLE.box_entry(t),
        Err(err_) => {
            set_err(err, err_);
            std::ptr::null_mut()
        }
    }
}
