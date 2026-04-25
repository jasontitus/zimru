use crate::Blob;

/// Opaque blob handle wrapping a [`crate::Blob`]. Holds the underlying
/// `Arc<[u8]>` cluster slice alive so the borrowed pointer returned by
/// `zimru_blob_data` stays valid until the blob is freed.
pub struct zimru_blob_t {
    pub(crate) inner: Blob,
}

pub(crate) struct BlobVtable;
pub(crate) const BLOB_VTABLE: BlobVtable = BlobVtable;

impl BlobVtable {
    pub(crate) fn box_blob(&self, b: Blob) -> *mut zimru_blob_t {
        Box::into_raw(Box::new(zimru_blob_t { inner: b }))
    }
}

/// Free a `zimru_blob_t` previously returned by `zimru_item_get_data`.
#[no_mangle]
pub unsafe extern "C" fn zimru_blob_free(b: *mut zimru_blob_t) {
    if !b.is_null() {
        drop(Box::from_raw(b));
    }
}

/// Pointer to the blob's bytes. Not NUL-terminated; lifetime tied to
/// the blob handle.
#[no_mangle]
pub unsafe extern "C" fn zimru_blob_data(b: *const zimru_blob_t) -> *const u8 {
    if b.is_null() {
        return std::ptr::null();
    }
    (*b).inner.data().as_ptr()
}

/// Number of bytes in the blob.
#[no_mangle]
pub unsafe extern "C" fn zimru_blob_size(b: *const zimru_blob_t) -> usize {
    if b.is_null() {
        return 0;
    }
    (*b).inner.data().len()
}
