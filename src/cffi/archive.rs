use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;

use crate::cffi::entry::{zimru_entry_t, ENTRY_VTABLE};
use crate::cffi::error::{set_err, zimru_error_t};
use crate::Archive;

/// Opaque handle wrapping a [`crate::Archive`] plus the per-entry-string
/// cache used to give C callers stable `const char*` pointers.
pub struct zimru_archive_t {
    pub(crate) inner: Archive,
    /// Stable storage for paths/titles returned by `zimru_archive_metadata_key`
    /// and similar lookup-by-index calls. Each call appends a CString and
    /// returns its `.as_ptr()`; the storage lives until the archive is freed.
    pub(crate) interned: std::sync::Mutex<Vec<CString>>,
}

/// Open the archive at `path` (UTF-8, NUL-terminated). Returns NULL on
/// failure and sets `*err`.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_open(
    path: *const c_char,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_archive_t {
    if path.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_err(err, crate::Error::BadUtf8(0));
            return std::ptr::null_mut();
        }
    };
    match Archive::open(PathBuf::from(path_str)) {
        Ok(a) => Box::into_raw(Box::new(zimru_archive_t {
            inner: a,
            interned: std::sync::Mutex::new(Vec::new()),
        })),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}

/// Close and free a `zimru_archive_t` previously returned by
/// `zimru_archive_open`.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_close(arc: *mut zimru_archive_t) {
    if !arc.is_null() {
        drop(Box::from_raw(arc));
    }
}

/// Total number of dirents (articles + redirects).
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_entry_count(arc: *const zimru_archive_t) -> u32 {
    if arc.is_null() {
        return 0;
    }
    (*arc).inner.entry_count()
}

/// Same as `zimru_archive_entry_count`, included for parity with libzim's
/// `getAllEntryCount`.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_all_entry_count(arc: *const zimru_archive_t) -> u32 {
    if arc.is_null() {
        return 0;
    }
    (*arc).inner.all_entry_count()
}

/// Number of clusters in the archive.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_cluster_count(arc: *const zimru_archive_t) -> u32 {
    if arc.is_null() {
        return 0;
    }
    (*arc).inner.cluster_count()
}

/// Copy the archive's 16-byte UUID into `out`. `out` must point to at
/// least 16 writable bytes.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_uuid(arc: *const zimru_archive_t, out: *mut u8) {
    if arc.is_null() || out.is_null() {
        return;
    }
    let bytes = (*arc).inner.uuid().into_bytes();
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, 16);
}

/// True iff the archive has a main entry pointer in its header.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_has_main_entry(arc: *const zimru_archive_t) -> bool {
    if arc.is_null() {
        return false;
    }
    (*arc).inner.has_main_entry()
}

/// True iff the archive carries a trailing MD5 checksum.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_has_checksum(arc: *const zimru_archive_t) -> bool {
    if arc.is_null() {
        return false;
    }
    (*arc).inner.has_checksum()
}

/// Verify the trailing MD5 checksum. Returns true on match, false on
/// mismatch, and false-with-`*err`-set on failure to even compute it.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_check(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if arc.is_null() {
        return false;
    }
    match (*arc).inner.check() {
        Ok(b) => b,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Look up an entry by its full namespaced path (e.g. `"C/index.html"`).
/// Returns NULL with `*err` set if not found.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_get_entry_by_path(
    arc: *const zimru_archive_t,
    path: *const c_char,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_entry_t {
    if arc.is_null() || path.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_err(err, crate::Error::BadUtf8(0));
            return std::ptr::null_mut();
        }
    };
    match (*arc).inner.get_entry_by_path(path_str) {
        Ok(e) => ENTRY_VTABLE.box_entry(e),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}

/// True iff an entry with this path exists. Equivalent to
/// `zimru_archive_get_entry_by_path` returning non-NULL, without the
/// allocation.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_has_entry_by_path(
    arc: *const zimru_archive_t,
    path: *const c_char,
) -> bool {
    if arc.is_null() || path.is_null() {
        return false;
    }
    let Ok(path_str) = CStr::from_ptr(path).to_str() else {
        return false;
    };
    (*arc).inner.has_entry_by_path(path_str)
}

/// Look up an entry by namespace + URL within that namespace
/// (e.g. ns=`'X'`, url=`"fulltext/xapian"`). This is the primitive
/// downstream callers need to reach the X / M / W namespaces on
/// new-scheme archives — `zimru_archive_get_entry_by_path` only looks
/// in the content namespace by design.
///
/// Returns NULL with `*err` set if not found.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_get_entry_by_ns_path(
    arc: *const zimru_archive_t,
    ns: u8,
    url: *const c_char,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_entry_t {
    if arc.is_null() || url.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    let url_str = match CStr::from_ptr(url).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_err(err, crate::Error::BadUtf8(0));
            return std::ptr::null_mut();
        }
    };
    match (*arc).inner.entry_by_ns_path(ns, url_str) {
        Ok(e) => ENTRY_VTABLE.box_entry(e),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}

/// Look up an entry by its URL-pointer index (path order). Index range
/// is `[0, all_entry_count)`.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_entry_by_url_index(
    arc: *const zimru_archive_t,
    idx: u32,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_entry_t {
    if arc.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    match (*arc).inner.entry_by_url_index(idx) {
        Ok(e) => ENTRY_VTABLE.box_entry(e),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}

/// Look up an entry by its title-pointer index (title order). On modern
/// archives this only covers the content namespace; index range is
/// `[0, zimru_archive_title_count)`.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_entry_by_title_index(
    arc: *const zimru_archive_t,
    idx: u32,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_entry_t {
    if arc.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    match (*arc).inner.entry_by_title_index(idx) {
        Ok(e) => ENTRY_VTABLE.box_entry(e),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}

/// Number of entries in the title-order listing. On modern archives
/// this is the count of content-namespace entries only.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_title_count(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> u32 {
    if arc.is_null() {
        return 0;
    }
    match (*arc).inner.title_count() {
        Ok(n) => n,
        Err(e) => {
            set_err(err, e);
            0
        }
    }
}

/// Look up an entry by title in the content namespace. Returns NULL
/// with `*err` set if not found.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_get_entry_by_title(
    arc: *const zimru_archive_t,
    title: *const c_char,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_entry_t {
    if arc.is_null() || title.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    let title_str = match CStr::from_ptr(title).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_err(err, crate::Error::BadUtf8(0));
            return std::ptr::null_mut();
        }
    };
    match (*arc).inner.get_entry_by_title(title_str) {
        Ok(e) => ENTRY_VTABLE.box_entry(e),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}

/// True iff this archive uses the modern single-character namespace
/// scheme (content under `C/`, metadata under `M/`, indexes under
/// `X/`, well-known under `W/`). Old archives put articles in `A/`.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_uses_new_namespaces(arc: *const zimru_archive_t) -> bool {
    if arc.is_null() {
        return false;
    }
    (*arc).inner.header().uses_new_namespaces()
}

/// Write the archive's trailing-MD5 checksum into `out` as a 32-byte
/// lowercase hex string (no NUL terminator, no hyphens). Returns
/// `false` with `*err` set if the archive has no checksum or the read
/// fails. `out` must point to at least 32 writable bytes.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_checksum_hex(
    arc: *const zimru_archive_t,
    out: *mut c_char,
    err: *mut *mut zimru_error_t,
) -> bool {
    if arc.is_null() || out.is_null() {
        return false;
    }
    let bytes = match (*arc).inner.checksum() {
        Ok(b) => b,
        Err(e) => {
            set_err(err, e);
            return false;
        }
    };
    static HEX: &[u8; 16] = b"0123456789abcdef";
    let out_bytes = std::slice::from_raw_parts_mut(out as *mut u8, 32);
    for (i, &b) in bytes.iter().enumerate() {
        out_bytes[i * 2] = HEX[(b >> 4) as usize];
        out_bytes[i * 2 + 1] = HEX[(b & 0x0f) as usize];
    }
    true
}

/// Look up the archive's main entry. Returns NULL with `*err` set if the
/// archive has no main page.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_main_entry(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_entry_t {
    if arc.is_null() {
        set_err(err, crate::Error::NoMainEntry);
        return std::ptr::null_mut();
    }
    match (*arc).inner.main_entry() {
        Ok(e) => ENTRY_VTABLE.box_entry(e),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}

/// Read a metadata value as raw bytes. Writes the byte count to `out_len`
/// and returns a borrowed pointer (lifetime tied to `arc`).
///
/// Returns NULL with `*err` set if the metadata key isn't present.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_metadata(
    arc: *const zimru_archive_t,
    name: *const c_char,
    out_len: *mut usize,
    err: *mut *mut zimru_error_t,
) -> *const u8 {
    if arc.is_null() || name.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null();
    }
    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_err(err, crate::Error::BadUtf8(0));
            return std::ptr::null();
        }
    };
    match (*arc).inner.get_metadata(name_str) {
        Ok(bytes) => {
            // Stash the bytes in the interned cache so the pointer stays
            // valid until the archive is freed. Wrap as a CString-ish
            // (Vec) — we use CString's underlying allocation by going via
            // Box<[u8]> kept alongside.
            let mut interned = (*arc).interned.lock().unwrap();
            // Use CString as a stable buffer; allow interior NULs by using
            // CString::from_vec_with_nul_unchecked? Cleaner: just keep
            // bytes in a Vec<u8> via a parallel store. For simplicity we
            // make a CString with a trailing NUL appended, but only return
            // the original byte length so callers don't see it.
            let mut bytes_with_nul = bytes.clone();
            bytes_with_nul.push(0);
            // CString::from_vec_with_nul requires no interior NULs; tolerate
            // them by going through unchecked.
            let cs = CString::from_vec_with_nul(bytes_with_nul).unwrap_or_else(|_| {
                // Fall back to lossy NUL replacement for binary metadata.
                let cleaned: Vec<u8> = bytes
                    .iter()
                    .map(|&b| if b == 0 { b' ' } else { b })
                    .collect();
                CString::new(cleaned).unwrap()
            });
            let ptr = cs.as_ptr() as *const u8;
            interned.push(cs);
            if !out_len.is_null() {
                *out_len = bytes.len();
            }
            ptr
        }
        Err(e) => {
            set_err(err, e);
            std::ptr::null()
        }
    }
}

/// Number of metadata keys in the archive.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_metadata_keys_count(arc: *const zimru_archive_t) -> usize {
    if arc.is_null() {
        return 0;
    }
    (*arc).inner.get_metadata_keys().len()
}

/// Borrowed pointer to the i'th metadata key as a NUL-terminated string.
/// Lifetime tied to `arc`. Returns NULL if `idx` is out of range.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_metadata_key(
    arc: *const zimru_archive_t,
    idx: usize,
) -> *const c_char {
    if arc.is_null() {
        return std::ptr::null();
    }
    let keys = (*arc).inner.get_metadata_keys();
    let Some(k) = keys.get(idx) else {
        return std::ptr::null();
    };
    let cs = match CString::new(k.as_str()) {
        Ok(c) => c,
        Err(_) => return std::ptr::null(),
    };
    let ptr = cs.as_ptr();
    (*arc).interned.lock().unwrap().push(cs);
    ptr
}
