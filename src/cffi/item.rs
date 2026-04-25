use std::ffi::{c_char, CString};

use crate::cffi::blob::{zimru_blob_t, BLOB_VTABLE};
use crate::cffi::error::{set_err, zimru_error_t};
use crate::Item;

/// Opaque item handle. Holds a Rust `Item` plus owned `CString`s for
/// `path` and `mimetype` so the C-side `const char*` returned by the
/// accessors stays valid for the item's lifetime.
pub struct zimru_item_t {
    pub(crate) inner: Item,
    path: CString,
    mimetype: CString,
}

pub(crate) struct ItemVtable;
pub(crate) const ITEM_VTABLE: ItemVtable = ItemVtable;

impl ItemVtable {
    pub(crate) fn box_item(&self, it: Item) -> *mut zimru_item_t {
        let path = CString::new(it.path()).unwrap_or_else(|_| CString::new("").unwrap());
        let mimetype = CString::new(it.mimetype()).unwrap_or_else(|_| CString::new("").unwrap());
        Box::into_raw(Box::new(zimru_item_t {
            inner: it,
            path,
            mimetype,
        }))
    }
}

/// Free a `zimru_item_t`.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_free(it: *mut zimru_item_t) {
    if !it.is_null() {
        drop(Box::from_raw(it));
    }
}

/// NUL-terminated path. Lifetime tied to the item.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_path(it: *const zimru_item_t) -> *const c_char {
    if it.is_null() {
        return std::ptr::null();
    }
    (*it).path.as_ptr()
}

/// NUL-terminated MIME type. Lifetime tied to the item.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_mimetype(it: *const zimru_item_t) -> *const c_char {
    if it.is_null() {
        return std::ptr::null();
    }
    (*it).mimetype.as_ptr()
}

/// Decompressed size of the item's data. Returns 0 with `*err` set if
/// the cluster cannot be decoded.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_size(
    it: *const zimru_item_t,
    err: *mut *mut zimru_error_t,
) -> u64 {
    if it.is_null() {
        return 0;
    }
    match (*it).inner.size() {
        Ok(n) => n,
        Err(e) => {
            set_err(err, e);
            0
        }
    }
}

/// Read the item's data as a heap-allocated blob handle. Caller frees
/// with `zimru_blob_free`.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_get_data(
    it: *const zimru_item_t,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_blob_t {
    if it.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    match (*it).inner.get_data() {
        Ok(blob) => BLOB_VTABLE.box_blob(blob),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}
