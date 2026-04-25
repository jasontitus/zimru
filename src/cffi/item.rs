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

/// Direct-access info for an item. POD struct populated by
/// [`zimru_item_direct_access`]; mirrors [`crate::DirectAccess`].
///
/// When `is_direct` is `true`, callers can `pread()` or `mmap()` the
/// item's bytes directly from the on-disk ZIM file at `file_offset`
/// for `size` bytes — no decompression, no copy. The standard ZIM
/// convention is to store fulltext / suggestion / Xapian indexes in
/// uncompressed clusters precisely so consumers can hand `libxapian`
/// an `int fd` + `lseek` instead of materialising the database in
/// memory or in a temp file.
///
/// When `is_direct` is `false`, the item lives in a compressed
/// cluster; fall back to [`zimru_item_get_data`] for normal access.
#[repr(C)]
pub struct zimru_direct_access_t {
    pub is_direct: bool,
    pub file_offset: u64,
    pub size: u64,
}

/// Populate `out` with direct-access info for the item. Safe on a
/// NULL `it` or `out` (no-op).
#[no_mangle]
pub unsafe extern "C" fn zimru_item_direct_access(
    it: *const zimru_item_t,
    out: *mut zimru_direct_access_t,
) {
    if it.is_null() || out.is_null() {
        return;
    }
    let cluster = (*it).inner.cluster_index();
    let blob = (*it).inner.blob_index();
    // Errors here mean the cluster index is bogus — treat as "not
    // direct" rather than surfacing through an out-pointer, since this
    // function's contract is "best-effort hint" and the caller has the
    // full get_data fallback anyway.
    let info = (*it)
        .inner
        .archive()
        .blob_direct_access(cluster, blob)
        .unwrap_or(crate::DirectAccess {
            is_direct: false,
            file_offset: 0,
            size: 0,
        });
    (*out).is_direct = info.is_direct;
    (*out).file_offset = info.file_offset;
    (*out).size = info.size;
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
