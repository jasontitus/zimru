use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;

use crate::cffi::entry::{zimru_entry_t, ENTRY_VTABLE};
use crate::cffi::error::{set_err, zimru_error_t};
use crate::Archive;

/// Opaque handle wrapping a [`crate::Archive`] plus the caches used to
/// give C callers stable pointers.
pub struct zimru_archive_t {
    pub(crate) inner: Archive,
    /// Stable per-key storage for metadata values returned by
    /// `zimru_archive_metadata`: the raw bytes plus a trailing NUL,
    /// keyed by metadata name. One slot per distinct key — repeat calls
    /// return the cached pointer instead of appending, so a long-lived
    /// handle serving many metadata reads doesn't grow without bound.
    /// `Box<[u8]>` keeps the data pointer stable while the map grows.
    pub(crate) metadata_values: std::sync::Mutex<std::collections::HashMap<String, Box<[u8]>>>,
    /// Metadata key names (NUL-terminated), computed once on first use
    /// by `zimru_archive_metadata_key` — enumerating k keys is then k
    /// pointer reads instead of k full-archive scans.
    pub(crate) metadata_keys: std::sync::OnceLock<Vec<CString>>,
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
            metadata_values: std::sync::Mutex::new(std::collections::HashMap::new()),
            metadata_keys: std::sync::OnceLock::new(),
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

/// True iff the archive was opened from a split `.zimaa`/`.zimab`/...
/// part-set rather than a single `.zim` file. zimru does not yet
/// implement multipart open, so this currently always returns `false`;
/// the accessor exists so downstream tooling that branches on it
/// (single- vs multi-URL download flows) can compile and run against
/// zimru without conditionally compiling the call out.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_is_multipart(arc: *const zimru_archive_t) -> bool {
    if arc.is_null() {
        return false;
    }
    false
}

/// Reconfigure the cluster-cache byte budget. The cache holds
/// decompressed clusters; `max_bytes` caps the summed payload sizes
/// resident at once. The default is conservative (~64 MB) so memory-
/// constrained hosts (phones, embedded) don't pin hundreds of MB of
/// decompressed cluster data; bulk-iteration tools can size up.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_set_cluster_cache_max_bytes(
    arc: *const zimru_archive_t,
    max_bytes: usize,
) {
    if arc.is_null() {
        return;
    }
    (*arc).inner.set_cluster_cache_max_bytes(max_bytes);
}

/// Current cluster-cache byte budget. Returns `0` on a NULL archive.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_cluster_cache_max_bytes(
    arc: *const zimru_archive_t,
) -> usize {
    if arc.is_null() {
        return 0;
    }
    (*arc).inner.cluster_cache_max_bytes()
}

/// Snapshot of cluster-cache occupancy and lifetime counters. Mirrors
/// [`crate::ClusterCacheStats`]. Counters are monotonic over the
/// archive's lifetime — useful for verifying hot-article hit ratios,
/// tuning the byte budget against eviction pressure, or surfacing
/// cache health to long-running-server telemetry.
#[repr(C)]
pub struct zimru_cluster_cache_stats_t {
    pub max_bytes: u64,
    pub current_bytes: u64,
    pub entries: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

/// Populate `out` with the cluster cache's current state. Safe on a
/// NULL `arc` or `out` (no-op).
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_cluster_cache_stats(
    arc: *const zimru_archive_t,
    out: *mut zimru_cluster_cache_stats_t,
) {
    if arc.is_null() || out.is_null() {
        return;
    }
    let s = (*arc).inner.cluster_cache_stats();
    (*out).max_bytes = s.max_bytes;
    (*out).current_bytes = s.current_bytes;
    (*out).entries = s.entries;
    (*out).hits = s.hits;
    (*out).misses = s.misses;
    (*out).evictions = s.evictions;
}

/// Cover / thumbnail descriptor returned by
/// [`zimru_archive_illustrations`]. Mirrors libzim's
/// `IllustrationInfo`: width and height in pixels, and a `scale`
/// factor (typically 1 or 2 for HiDPI variants).
#[repr(C)]
pub struct zimru_illustration_t {
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

/// Enumerate every illustration descriptor recorded in the archive's
/// metadata. ZIM stores covers/thumbnails as
/// `M/Illustration_<W>x<H>@<scale>` entries; this returns a heap-
/// allocated array sorted ascending. Sets `*out_count` and returns the
/// pointer; the caller frees with [`zimru_illustrations_free`]. On
/// archives with no illustrations sets `*out_count = 0` and returns
/// NULL (still safe to pass to free).
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_illustrations(
    arc: *const zimru_archive_t,
    out_count: *mut usize,
) -> *mut zimru_illustration_t {
    if arc.is_null() || out_count.is_null() {
        return std::ptr::null_mut();
    }
    let triples = (*arc).inner.illustrations();
    *out_count = triples.len();
    if triples.is_empty() {
        return std::ptr::null_mut();
    }
    let boxed: Box<[zimru_illustration_t]> = triples
        .into_iter()
        .map(|(width, height, scale)| zimru_illustration_t {
            width,
            height,
            scale,
        })
        .collect();
    // `into_boxed_slice` (which `collect::<Box<[_]>>` goes through) is
    // *guaranteed* to hand back an allocation whose size is exactly the
    // element count, reallocating if it has to. `shrink_to_fit`, which this
    // used to call, is explicitly best-effort: it may leave capacity > len,
    // and the matching free below then reconstructs the allocation with the
    // wrong size, which is undefined behaviour.
    Box::into_raw(boxed) as *mut zimru_illustration_t
}

/// Free an illustration array previously returned by
/// [`zimru_archive_illustrations`]. `count` must match the value
/// written into `*out_count`. Safe on a NULL pointer (no-op).
#[no_mangle]
pub unsafe extern "C" fn zimru_illustrations_free(ptr: *mut zimru_illustration_t, count: usize) {
    if !ptr.is_null() && count > 0 {
        // Mirror of the `Box<[_]>` the allocation was created as.
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, count)));
    }
}

/// Half-open title-order range `[*out_lo, *out_hi)` of entries in
/// namespace `ns` whose title starts with `prefix`. Returns `true` on
/// success (including empty matches, where `*out_lo == *out_hi`),
/// `false` with `*err` set on a parse failure. `prefix` is a
/// NUL-terminated UTF-8 string. Designed to back
/// `SuggestionSearcher` fallbacks; iterate the returned range and
/// resolve each index with `zimru_archive_entry_by_title_index`.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_title_prefix_range(
    arc: *const zimru_archive_t,
    ns: u8,
    prefix: *const c_char,
    out_lo: *mut u32,
    out_hi: *mut u32,
    err: *mut *mut zimru_error_t,
) -> bool {
    if arc.is_null() || prefix.is_null() || out_lo.is_null() || out_hi.is_null() {
        return false;
    }
    let prefix_str = match CStr::from_ptr(prefix).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_err(err, crate::Error::BadUtf8(0));
            return false;
        }
    };
    match (*arc).inner.title_prefix_range(ns, prefix_str) {
        Ok(range) => {
            *out_lo = range.start;
            *out_hi = range.end;
            true
        }
        Err(e) => {
            set_err(err, e);
            false
        }
    }
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

/// URL-pointer index of the archive's main entry, or
/// [`NO_MAIN_PAGE`] (`0xFFFFFFFF`) when no main entry is set in the
/// header. Cheaper than [`zimru_archive_main_entry`] — avoids the
/// dirent parse and the entry-handle allocation — for callers that
/// only need the index (e.g. the libzim-shim's `getMainEntryIndex()`,
/// or zim-tools' equality checks against `entry.index()` while
/// iterating).
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_main_entry_index(arc: *const zimru_archive_t) -> u32 {
    if arc.is_null() {
        return crate::header::NO_MAIN_PAGE;
    }
    (*arc)
        .inner
        .main_entry_index()
        .unwrap_or(crate::header::NO_MAIN_PAGE)
}

/// Absolute on-disk byte offset of cluster `idx` (the byte where the
/// cluster's leading info-byte begins). Used by `zimsplit` /
/// `zimdump` to walk the cluster region by file position. Returns
/// `0` with `*err` set on out-of-range indices or read errors.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_cluster_offset(
    arc: *const zimru_archive_t,
    idx: u32,
    err: *mut *mut zimru_error_t,
) -> u64 {
    if arc.is_null() {
        set_err(err, crate::Error::BadClusterIndex(idx, 0));
        return 0;
    }
    match (*arc).inner.cluster_offset(idx) {
        Ok(off) => off,
        Err(e) => {
            set_err(err, e);
            0
        }
    }
}

/// Verify every URL-pointer-list entry resolves to a parseable dirent
/// within file bounds. Returns `true` on a clean archive, `false` on a
/// structural defect, `false` with `*err` set if the pointer list
/// itself can't be read. Designed as one of the per-check primitives
/// the libzim-shim's `IntegrityCheckList` / `validate()` aggregator
/// composes; callers that want a single yes/no can `&&` the set.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_check_dirent_ptrs(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if arc.is_null() {
        return false;
    }
    match (*arc).inner.check_dirent_ptrs() {
        Ok(b) => b,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Verify every dirent appears in `(namespace, url)` order in the
/// URL-pointer list. A `false` here would silently break every path
/// lookup on the archive (the binary search returns the wrong leaf or
/// `EntryNotFound`).
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_check_dirent_order(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if arc.is_null() {
        return false;
    }
    match (*arc).inner.check_dirent_order() {
        Ok(b) => b,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Verify the title-pointer list (legacy archives) or the modern
/// `X/listing/titleOrdered/v1` stream yields dirents in
/// `(namespace, title)` order. `entry_by_ns_title` is a binary search
/// over this ordering.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_check_title_index(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if arc.is_null() {
        return false;
    }
    match (*arc).inner.check_title_index() {
        Ok(b) => b,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Verify every cluster-pointer-list entry is in-bounds for the file.
/// Catches truncation between the end of the dirent region and the
/// trailing checksum.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_check_cluster_ptrs(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if arc.is_null() {
        return false;
    }
    match (*arc).inner.check_cluster_ptrs() {
        Ok(b) => b,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Verify every article-dirent's mimetype index resolves inside the
/// on-file mime-type list. Reserved values `0xFFFE` (linktarget) and
/// `0xFFFD` (deletedentry) are accepted as legal even though they sit
/// outside the list.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_check_mimetypes(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if arc.is_null() {
        return false;
    }
    match (*arc).inner.check_mimetypes() {
        Ok(b) => b,
        Err(e) => {
            set_err(err, e);
            false
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
    // Serve repeat reads of the same key from the per-key cache.
    let mut store = (*arc).metadata_values.lock().unwrap();
    if let Some(slot) = store.get(name_str) {
        if !out_len.is_null() {
            *out_len = slot.len() - 1;
        }
        return slot.as_ptr();
    }
    match (*arc).inner.get_metadata(name_str) {
        Ok(mut bytes) => {
            // Store the raw bytes verbatim (interior NULs included —
            // binary metadata like `M/Illustration_*` PNGs must round-
            // trip exactly) with a trailing NUL appended for C string
            // convenience; `*out_len` reports the original length.
            let len = bytes.len();
            bytes.push(0);
            let slot = store
                .entry(name_str.to_string())
                .or_insert_with(|| bytes.into_boxed_slice());
            if !out_len.is_null() {
                *out_len = len;
            }
            slot.as_ptr()
        }
        Err(e) => {
            set_err(err, e);
            std::ptr::null()
        }
    }
}

/// On-disk byte length of the archive (the mmapped extent).
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_filesize(arc: *const zimru_archive_t) -> u64 {
    if arc.is_null() {
        return 0;
    }
    (*arc).inner.file_len()
}

/// Number of "article" entries — non-redirect content-namespace items
/// whose mimetype starts with `text/html`. Result is cached after the
/// first call (one O(N) walk over the content namespace). Returns 0
/// with `*err` set on read error.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_article_count(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> u64 {
    if arc.is_null() {
        return 0;
    }
    match (*arc).inner.article_count() {
        Ok(n) => n,
        Err(e) => {
            set_err(err, e);
            0
        }
    }
}

/// Number of "media" entries — non-redirect content-namespace items
/// that are NOT articles. Result shares the cache with
/// [`zimru_archive_article_count`].
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_media_count(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> u64 {
    if arc.is_null() {
        return 0;
    }
    match (*arc).inner.media_count() {
        Ok(n) => n,
        Err(e) => {
            set_err(err, e);
            0
        }
    }
}

/// Pick a pseudo-random entry from the content namespace. Suitable for
/// "random article" UI links; not for cryptographic use.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_random_entry(
    arc: *const zimru_archive_t,
    err: *mut *mut zimru_error_t,
) -> *mut zimru_entry_t {
    if arc.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    match (*arc).inner.random_content_entry() {
        Ok(e) => ENTRY_VTABLE.box_entry(e),
        Err(e) => {
            set_err(err, e);
            std::ptr::null_mut()
        }
    }
}

/// Number of metadata keys in the archive. Derived from the same
/// cached key list `zimru_archive_metadata_key` indexes into, so the
/// documented `for (i = 0; i < count; i++)` enumeration can never see
/// an index the key lookup answers with NULL (a raw namespace-range
/// count could exceed the list when a corrupt dirent fails to parse).
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_metadata_keys_count(arc: *const zimru_archive_t) -> usize {
    if arc.is_null() {
        return 0;
    }
    cached_metadata_keys(&*arc).len()
}

/// Borrowed pointer to the i'th metadata key as a NUL-terminated string.
/// Lifetime tied to `arc`. Returns NULL if `idx` is out of range.
///
/// The key list is computed once on first call and cached on the
/// archive handle, so enumerating k keys costs one metadata-range walk
/// total instead of one per call.
#[no_mangle]
pub unsafe extern "C" fn zimru_archive_metadata_key(
    arc: *const zimru_archive_t,
    idx: usize,
) -> *const c_char {
    if arc.is_null() {
        return std::ptr::null();
    }
    match cached_metadata_keys(&*arc).get(idx) {
        Some(cs) => cs.as_ptr(),
        None => std::ptr::null(),
    }
}

/// The archive's metadata key names, computed once per handle. Shared
/// by `zimru_archive_metadata_keys_count` and
/// `zimru_archive_metadata_key` so count and list always agree.
fn cached_metadata_keys(arc: &zimru_archive_t) -> &Vec<CString> {
    arc.metadata_keys.get_or_init(|| {
        arc.inner
            .get_metadata_keys()
            .into_iter()
            .filter_map(|k| CString::new(k).ok())
            .collect()
    })
}
