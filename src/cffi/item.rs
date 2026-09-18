use std::ffi::{c_char, c_void, CString};
use std::sync::Arc;

use crate::cffi::blob::{zimru_blob_t, BLOB_VTABLE};
use crate::cffi::error::{set_err, zimru_error_t};
use crate::{Cluster, Item};

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

/// Single-byte namespace of the resolved entry this item belongs to
/// (e.g. `'C'`, `'A'`, `'I'`). On items obtained via redirect-following
/// lookups this is the redirect *target's* namespace, which may differ
/// from the source entry's namespace on legacy cross-namespace
/// redirects. Returns `0` on a NULL item.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_namespace(it: *const zimru_item_t) -> u8 {
    if it.is_null() {
        return 0;
    }
    (*it).inner.namespace()
}

/// Cluster index this item's bytes live in. Combine with
/// [`crate::cffi::archive::zimru_archive_cluster_offset`] to find where the cluster starts
/// in the file, or with [`zimru_item_blob_index`] +
/// [`zimru_item_direct_access`] to address the blob within the
/// cluster. Returns `0` on a NULL item — callers that care must not
/// call this without a non-NULL `it`.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_cluster_index(it: *const zimru_item_t) -> u32 {
    if it.is_null() {
        return 0;
    }
    (*it).inner.cluster_index()
}

/// Blob index within the item's cluster. Together with
/// [`zimru_item_cluster_index`] this is the `(cluster, blob)` pair
/// `zimcheck` and similar tools use to talk about an item's physical
/// position. Returns `0` on a NULL item.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_blob_index(it: *const zimru_item_t) -> u32 {
    if it.is_null() {
        return 0;
    }
    (*it).inner.blob_index()
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

/// Zero-allocation blob view: a borrowed `(data, size)` slice into the
/// item's decompressed cluster, plus an opaque pin handle that keeps
/// the underlying cluster alive even if the LRU cluster cache evicts
/// the entry. Designed for hot-path consumers (servers serving content
/// per-request) that would otherwise pay one heap allocation per call
/// for the `zimru_blob_t*` wrapper.
///
/// Populated by [`zimru_item_blob_view`]; released by
/// [`zimru_blob_view_release`]. Treat `_pin` as opaque — it is not a
/// data pointer.
#[repr(C)]
pub struct zimru_blob_view_t {
    pub data: *const u8,
    pub size: u64,
    /// Opaque cluster-pin handle. Pass to
    /// [`zimru_blob_view_release`]; do not dereference. NULL means the
    /// view has already been released (or was never populated).
    pub _pin: *const c_void,
}

/// Populate `out` with a borrowed view of the item's bytes plus a pin
/// handle that keeps them alive. Returns `false` with `*err` set on
/// any failure (cluster decode error, bad blob index, NULL inputs).
/// Avoids the per-call `Box::new(zimru_blob_t)` of
/// [`zimru_item_get_data`] — costs one `Arc::clone` (refcount bump,
/// no allocation) per call. The pin must be released exactly once
/// with [`zimru_blob_view_release`].
#[no_mangle]
pub unsafe extern "C" fn zimru_item_blob_view(
    it: *const zimru_item_t,
    out: *mut zimru_blob_view_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if it.is_null() || out.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    let (cluster, range) = match (*it).inner.pinned_blob() {
        Ok(t) => t,
        Err(e) => {
            set_err(err, e);
            return false;
        }
    };
    let payload = cluster.payload();
    (*out).data = payload[range.clone()].as_ptr();
    (*out).size = range.len() as u64;
    // Hand ownership of the Arc<Cluster> to C as a thin raw pointer.
    // Arc<Cluster> is sized so `into_raw` returns a single-word
    // pointer — no Box wrapping, no allocation beyond the refcount
    // bump that already happened during `load_cluster`.
    (*out)._pin = Arc::into_raw(cluster) as *const c_void;
    true
}

/// Pre-fault the OS page cache for this item's on-disk bytes. Issues
/// `madvise(MADV_WILLNEED)` on the item's region and touches one byte
/// per 4 KB page to force synchronous fault-in. After return, every
/// page is resident; subsequent reads (including reads issued through
/// a different fd onto the same file — the OS page cache is keyed by
/// inode) hit cache instead of disk.
///
/// Designed for the libzim-shim P7 scenario: kiwix-serve hosting a
/// multi-GB Wikipedia hands Xapian a fd at the embedded Xapian-DB
/// offset, Xapian issues eager DB-validation reads scattered across
/// the DB, and on a cold page cache the first user `/search`
/// blocks for ~12 seconds. Calling this at server start (or in a
/// background thread concurrent with other init) shifts the disk-I/O
/// cost off the first user-facing search.
///
/// No-op on items in compressed clusters (the page-fault game only
/// makes sense for direct-access regions). Returns `false` with `*err`
/// set on a NULL item or unrecoverable lookup failure.
#[no_mangle]
pub unsafe extern "C" fn zimru_item_warmup(
    it: *const zimru_item_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if it.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    match (*it).inner.warmup() {
        Ok(()) => true,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Release the pin handle inside a [`zimru_blob_view_t`]. Safe to call
/// on a NULL `view` or a view whose `_pin` is already NULL (no-op).
/// After release the `data` and `size` fields are unchanged but
/// `data` must not be dereferenced.
#[no_mangle]
pub unsafe extern "C" fn zimru_blob_view_release(view: *mut zimru_blob_view_t) {
    if view.is_null() {
        return;
    }
    let pin = (*view)._pin;
    if !pin.is_null() {
        let arc = Arc::from_raw(pin as *const Cluster);
        drop(arc);
        (*view)._pin = std::ptr::null();
    }
}
