//! Writer C ABI — wraps [`crate::writer::Creator`] so C callers can build
//! ZIM archives. This is the surface the libzim-shim's `zim::writer::*`
//! wrappers forward into, which in turn lets `zim-tools`' writer-side
//! binaries (`zimrecreate`, `zimwriterfs`, `zimdiff`, `zimpatch`) link
//! and run against the shim with no source changes.
//!
//! Lifecycle expected by callers:
//!
//! ```text
//! zimru_creator_new()
//!   → zimru_creator_set_*       (configuration; before any add_*)
//!   → zimru_creator_add_*       (item / metadata / illustration / redirect)
//!   → zimru_creator_write_to    (consumes the buffered work to disk)
//!   → zimru_creator_free        (the only legal call after write_to)
//! ```
//!
//! After `zimru_creator_write_to` returns successfully the inner
//! [`Creator`] has been consumed; subsequent `set_*` / `add_*` /
//! `write_to` calls fail loudly rather than silently producing a
//! malformed second archive.
//!
//! Error protocol matches the rest of the C ABI: every fallible function
//! takes a `zimru_error_t **err` out-parameter, returns `false` (or NULL)
//! on failure, and the caller frees the error with `zimru_error_free`.

use std::ffi::{c_char, CStr};

use crate::cffi::error::{set_err, zimru_error_t};
use crate::writer::{Creator, Item};
use crate::Compression;

/// Opaque writer handle. Owns a [`Creator`] until `zimru_creator_write_to`
/// consumes it. May also hold a single in-flight chunked item between
/// `begin_item` and `end_item` calls.
pub struct zimru_creator_t {
    inner: Option<Creator>,
    /// Set between `zimru_creator_begin_item` and `zimru_creator_end_item`.
    /// Holds the metadata + accumulated chunks. We don't use the borrowed
    /// `ItemBuilder` here because lifetime-tying to `inner` would require
    /// self-referential storage; instead we replicate its state and call
    /// `Creator::begin_item().write_chunk(...).finish()` synthetically at
    /// `end_item` time.
    in_flight: Option<InFlightItem>,
}

struct InFlightItem {
    namespace: Option<u8>,
    path: String,
    title: String,
    mimetype: String,
    content: Vec<u8>,
}

/// Resolve `c` to a `&mut Creator` or set `*err` and return `null_mut()`.
/// Centralises the NULL-check + already-finalized check that every
/// `set_*` / `add_*` entry point performs.
unsafe fn inner_mut(
    c: *mut zimru_creator_t,
    err: *mut *mut zimru_error_t,
) -> *mut Creator {
    if c.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return std::ptr::null_mut();
    }
    match (*c).inner.as_mut() {
        Some(inner) => inner as *mut Creator,
        None => {
            set_err(
                err,
                crate::Error::Io(std::io::Error::other(
                    "zimru_creator: already finalized — call free, not set/add",
                )),
            );
            std::ptr::null_mut()
        }
    }
}

/// Convert a NUL-terminated C string to an owned `String`. Sets `*err`
/// on bad UTF-8 and returns `None`.
unsafe fn cstr_to_string(
    s: *const c_char,
    err: *mut *mut zimru_error_t,
) -> Option<String> {
    if s.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return None;
    }
    match CStr::from_ptr(s).to_str() {
        Ok(s) => Some(s.to_string()),
        Err(_) => {
            set_err(err, crate::Error::BadUtf8(0));
            None
        }
    }
}

/// Copy `len` bytes from `data` into an owned `Vec<u8>`. Treats NULL +
/// zero-length as an empty vec; NULL + non-zero length is an error.
unsafe fn ptr_to_vec(
    data: *const u8,
    len: usize,
    err: *mut *mut zimru_error_t,
) -> Option<Vec<u8>> {
    if len == 0 {
        return Some(Vec::new());
    }
    if data.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return None;
    }
    Some(std::slice::from_raw_parts(data, len).to_vec())
}

/// Allocate a new creator with default settings (Zstd level 3, 2 MiB
/// cluster target, time-and-pid-seeded UUID). Always succeeds.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_new() -> *mut zimru_creator_t {
    Box::into_raw(Box::new(zimru_creator_t {
        inner: Some(Creator::new()),
        in_flight: None,
    }))
}

/// Free a creator. Safe to call on NULL. Discards any still-buffered
/// work if the creator was never finalized.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_free(c: *mut zimru_creator_t) {
    if !c.is_null() {
        drop(Box::from_raw(c));
    }
}

/// Set the cluster compression algorithm by ZIM info-byte ID:
///
/// * `1` — uncompressed (`Compression::None`)
/// * `4` — xz / lzma2 (`Compression::Xz`)
/// * `5` — zstd, the default (`Compression::Zstd`)
///
/// Other IDs return `false` with `*err` set to
/// [`zimru_error_code::UnsupportedCompression`].
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_set_compression(
    c: *mut zimru_creator_t,
    compression_id: u8,
    err: *mut *mut zimru_error_t,
) -> bool {
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    let comp = match compression_id {
        1 => Compression::None,
        4 => Compression::Xz,
        5 => Compression::Zstd,
        other => {
            set_err(err, crate::Error::UnsupportedCompression(other));
            return false;
        }
    };
    (*inner).set_compression(comp);
    true
}

/// Set the compression level for the active algorithm. Range and meaning
/// are algorithm-specific:
///
/// * Zstd — `1..=22` (negative "fast" levels also accepted by libzstd).
///   Default `3`.
/// * Xz — `0..=9`. Default `3`.
/// * None — ignored.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_set_compression_level(
    c: *mut zimru_creator_t,
    level: i32,
    err: *mut *mut zimru_error_t,
) -> bool {
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    (*inner).set_compression_level(level);
    true
}

/// Set the per-cluster decompressed-payload byte target. The bin-packer
/// closes the current cluster once its accumulated payload would exceed
/// this. Default 2 MiB matches `zimwriterfs` / `zimrecreate`.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_set_cluster_size_target(
    c: *mut zimru_creator_t,
    bytes: usize,
    err: *mut *mut zimru_error_t,
) -> bool {
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    (*inner).set_cluster_size_target(bytes);
    true
}

/// Set the archive UUID. `uuid` must point to 16 readable bytes; the
/// bytes are copied (caller retains ownership).
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_set_uuid(
    c: *mut zimru_creator_t,
    uuid: *const u8,
    err: *mut *mut zimru_error_t,
) -> bool {
    if uuid.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    let mut bytes = [0u8; 16];
    std::ptr::copy_nonoverlapping(uuid, bytes.as_mut_ptr(), 16);
    (*inner).set_uuid(bytes);
    true
}

/// Declare the archive's main page. A `W/mainPage` redirect to
/// `C/<main_path>` is written at finalize time, matching how libzim
/// readers locate the main page.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_set_main_path(
    c: *mut zimru_creator_t,
    main_path: *const c_char,
    err: *mut *mut zimru_error_t,
) -> bool {
    let Some(path) = cstr_to_string(main_path, err) else {
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    (*inner).set_main_path(path);
    true
}

/// Add a content-namespace item (stored at `C/<path>`). `content` /
/// `content_len` is copied — the caller retains ownership of the input
/// buffer. Empty payloads (`content_len == 0`) are permitted.
///
/// Duplicate paths are not detected here; the duplicate surfaces as a
/// dirent-order failure at finalize time.
///
/// **Namespace-prefix shortcut.** A `path` of the form `"X/<rest>"`
/// (with non-empty `<rest>`) is routed to the `X` namespace with the
/// dirent URL set to `<rest>`. This is the surface the libzim-shim's
/// fulltext-index emission uses — `"X/fulltext/xapian"` becomes
/// `{ns:'X', url:"fulltext/xapian"}` so reader-side
/// `xapian_loader.cpp` finds it. Other namespaces (M, W, Z, …) are
/// **not** peeled here; use [`zimru_creator_add_item_in_namespace`]
/// for those.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_add_item(
    c: *mut zimru_creator_t,
    path: *const c_char,
    title: *const c_char,
    mimetype: *const c_char,
    content: *const u8,
    content_len: usize,
    err: *mut *mut zimru_error_t,
) -> bool {
    let Some(path_str) = cstr_to_string(path, err) else {
        return false;
    };
    let Some(title_str) = cstr_to_string(title, err) else {
        return false;
    };
    let Some(mime_str) = cstr_to_string(mimetype, err) else {
        return false;
    };
    let Some(bytes) = ptr_to_vec(content, content_len, err) else {
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    let item = if let Some(rest) = path_str.strip_prefix("X/") {
        if rest.is_empty() {
            // "X/" alone is degenerate — fall through to default 'C'.
            Item::new(path_str, title_str, mime_str, bytes)
        } else {
            Item::in_namespace(b'X', rest.to_string(), title_str, mime_str, bytes)
        }
    } else {
        Item::new(path_str, title_str, mime_str, bytes)
    };
    (*inner).add_item(item);
    true
}

/// Add an item under an explicit namespace. `namespace` is the
/// single-byte ZIM namespace identifier (`'C'`, `'M'`, `'W'`, `'X'`,
/// `'Z'`, …); `url` is used verbatim as the dirent URL — no prefix
/// peeling. Use this when the caller already knows the target
/// namespace and wants to bypass [`zimru_creator_add_item`]'s
/// `"X/<rest>"` shortcut (or needs to write into a namespace other
/// than `C` or `X`).
///
/// Routing semantics:
///
/// * `'C'` (`0x43`) — same as [`zimru_creator_add_item`] without a
///   prefix. The default content namespace.
/// * `'M'` (`0x4D`) — metadata. [`zimru_creator_add_metadata`] is the
///   normal entry point but explicit-namespace works for callers that
///   need a non-default mimetype outside the metadata pipeline.
/// * `'X'` (`0x58`) — fulltext / title indexes (`X/fulltext/xapian`,
///   `X/title/xapian`). The libzim-shim writer uses this on
///   `finishZimCreation` after compacting its in-memory Glass DB.
/// * Other namespaces — accepted; the writer emits the dirent under
///   the requested namespace without further validation.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_add_item_in_namespace(
    c: *mut zimru_creator_t,
    namespace: u8,
    url: *const c_char,
    title: *const c_char,
    mimetype: *const c_char,
    content: *const u8,
    content_len: usize,
    err: *mut *mut zimru_error_t,
) -> bool {
    let Some(url_str) = cstr_to_string(url, err) else {
        return false;
    };
    let Some(title_str) = cstr_to_string(title, err) else {
        return false;
    };
    let Some(mime_str) = cstr_to_string(mimetype, err) else {
        return false;
    };
    let Some(bytes) = ptr_to_vec(content, content_len, err) else {
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    (*inner).add_item(Item::in_namespace(
        namespace, url_str, title_str, mime_str, bytes,
    ));
    true
}

/// Add a metadata entry under `M/<name>`. `mimetype` is recorded
/// verbatim in the dirent — typical values are
/// `text/plain;charset=utf-8` (Title, Language, Description, …) and
/// `image/png` (favicons on legacy archives).
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_add_metadata(
    c: *mut zimru_creator_t,
    name: *const c_char,
    mimetype: *const c_char,
    content: *const u8,
    content_len: usize,
    err: *mut *mut zimru_error_t,
) -> bool {
    let Some(name_str) = cstr_to_string(name, err) else {
        return false;
    };
    let Some(mime_str) = cstr_to_string(mimetype, err) else {
        return false;
    };
    let Some(bytes) = ptr_to_vec(content, content_len, err) else {
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    (*inner).add_metadata_with_mimetype(name_str, mime_str, bytes);
    true
}

/// Add a square illustration of `side` pixels, stored at
/// `M/Illustration_<side>x<side>@1`. `png` / `png_len` is copied — the
/// caller retains ownership.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_add_illustration(
    c: *mut zimru_creator_t,
    side: u32,
    png: *const u8,
    png_len: usize,
    err: *mut *mut zimru_error_t,
) -> bool {
    let Some(bytes) = ptr_to_vec(png, png_len, err) else {
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    (*inner).add_illustration(side, bytes);
    true
}

/// Add a content-namespace redirect: `C/<path>` → `C/<target_path>`.
/// The target is resolved at finalize time; an unresolved target is a
/// hard failure of `zimru_creator_write_to`.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_add_redirection(
    c: *mut zimru_creator_t,
    path: *const c_char,
    title: *const c_char,
    target_path: *const c_char,
    err: *mut *mut zimru_error_t,
) -> bool {
    let Some(path_str) = cstr_to_string(path, err) else {
        return false;
    };
    let Some(title_str) = cstr_to_string(title, err) else {
        return false;
    };
    let Some(target_str) = cstr_to_string(target_path, err) else {
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    (*inner).add_redirection(path_str, title_str, target_str);
    true
}

/// Add an alias entry. zimru does not yet implement true aliases (which
/// would be a type-preserving copy of the target dirent under a new
/// path/title); the implementation currently produces a redirect, which
/// downstream readers handle gracefully. Callers that *require* alias
/// semantics should test for a follow-up zimru release before relying
/// on this.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_add_alias(
    c: *mut zimru_creator_t,
    path: *const c_char,
    title: *const c_char,
    target_path: *const c_char,
    err: *mut *mut zimru_error_t,
) -> bool {
    zimru_creator_add_redirection(c, path, title, target_path, err)
}

/// Materialize the archive at `path`. Consumes the buffered work — on
/// success the only legal next call is `zimru_creator_free`. On failure
/// `*err` is set; the creator's inner state has still been consumed
/// (the partially-written file at `path` should be deleted by the
/// caller if it exists).
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_write_to(
    c: *mut zimru_creator_t,
    path: *const c_char,
    err: *mut *mut zimru_error_t,
) -> bool {
    if c.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    let Some(path_str) = cstr_to_string(path, err) else {
        return false;
    };
    let Some(creator) = (*c).inner.take() else {
        set_err(
            err,
            crate::Error::Io(std::io::Error::other(
                "zimru_creator: already finalized — call free, not write_to",
            )),
        );
        return false;
    };
    match creator.write_to(std::path::PathBuf::from(path_str)) {
        Ok(()) => true,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Open `path` for streaming output and switch the creator to
/// streaming mode. After this call returns true, every
/// `add_item` / `add_metadata` / `add_redirection` /
/// `add_illustration` call bin-packs the content into the in-flight
/// cluster and stream-encodes-and-writes to disk as the cluster
/// fills, dropping the source bytes immediately. Peak RSS is
/// bounded by `cluster_size_target × thread_count` plus the small
/// per-item dirent metadata, regardless of the total archive size.
///
/// Any work already added via the buffered path before this call
/// is drained into the stream here.
///
/// After `start_writing` succeeds, call `add_*` and friends as
/// usual, then call [`zimru_creator_finish_writing`] (NOT
/// `write_to`) to produce the final archive.
///
/// Errors:
///
/// * `path` is unreadable as UTF-8 → `*err` set, returns `false`.
/// * Output file can't be created → `*err` set, returns `false`.
/// * Already in streaming mode → `*err` set, returns `false`.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_start_writing(
    c: *mut zimru_creator_t,
    path: *const c_char,
    err: *mut *mut zimru_error_t,
) -> bool {
    let Some(path_str) = cstr_to_string(path, err) else {
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    match (*inner).start_writing(std::path::PathBuf::from(path_str)) {
        Ok(()) => true,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Finalize a streaming-mode creator: encode the last cluster,
/// write the URL / title / cluster pointer tables and the dirent
/// region, fill the mime list at offset 80, write the final
/// header, append the MD5 trailer. Consumes the creator on
/// success.
///
/// Errors:
///
/// * Creator was never started via `start_writing` → `*err` set,
///   returns `false`. The creator handle remains valid; caller can
///   either `start_writing` it now or `free` it.
/// * Mid-finalize I/O failure or unresolved redirect → `*err` set,
///   returns `false`. Inner creator is consumed; only legal next
///   call is `free`.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_finish_writing(
    c: *mut zimru_creator_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if c.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    let Some(creator) = (*c).inner.take() else {
        set_err(
            err,
            crate::Error::Io(std::io::Error::other(
                "zimru_creator: already finalized — call free, not finish_writing",
            )),
        );
        return false;
    };
    match creator.finish_writing() {
        Ok(()) => true,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}

/// Begin a chunked item — the caller will follow up with one or
/// more [`zimru_creator_item_chunk`] calls and a single
/// [`zimru_creator_end_item`] call to finalise. Useful for callers
/// with streaming content sources (e.g. libzim's
/// `ContentProvider::feed()`) that want to avoid slurping the
/// entire body into one buffer before handing it to zimru.
///
/// `path` honours the same `X/<rest>` namespace-prefix shortcut as
/// [`zimru_creator_add_item`]; pass `'X'` (or any single-byte
/// namespace) explicitly via `namespace` to bypass the prefix logic.
/// Pass `0` for `namespace` to use the default routing
/// (`X/...` peel, otherwise `'C'`).
///
/// `size_hint` is a non-binding capacity hint used to pre-allocate
/// the in-flight buffer. Pass `0` to skip.
///
/// Errors:
///
/// * Creator not in streaming mode → `*err` set, returns `false`.
///   Call `start_writing` first.
/// * Another chunked item is already in flight → `*err` set,
///   returns `false`. Call `end_item` (or `cancel_item` — TODO if
///   needed) before starting a new one.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_begin_item(
    c: *mut zimru_creator_t,
    namespace: u8,
    path: *const c_char,
    title: *const c_char,
    mimetype: *const c_char,
    size_hint: usize,
    err: *mut *mut zimru_error_t,
) -> bool {
    if c.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    if (*c).in_flight.is_some() {
        set_err(
            err,
            crate::Error::Io(std::io::Error::other(
                "zimru_creator: another chunked item is already in flight",
            )),
        );
        return false;
    }
    let Some(path_str) = cstr_to_string(path, err) else {
        return false;
    };
    let Some(title_str) = cstr_to_string(title, err) else {
        return false;
    };
    let Some(mime_str) = cstr_to_string(mimetype, err) else {
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    let creator: &mut Creator = &mut *inner;
    // Resolve namespace: explicit `namespace` wins; if 0, fall back
    // to the X/ prefix shortcut to match zimru_creator_add_item.
    let (resolved_ns, resolved_path) = if namespace == 0 {
        if let Some(rest) = path_str.strip_prefix("X/") {
            if rest.is_empty() {
                (None, path_str)
            } else {
                (Some(b'X'), rest.to_string())
            }
        } else {
            (None, path_str)
        }
    } else {
        (Some(namespace), path_str)
    };

    // Validate that the creator is in streaming mode; reuse the
    // exact same check Creator::begin_item performs.
    if creator.peek_streaming().is_none() {
        set_err(
            err,
            crate::Error::Io(std::io::Error::other(
                "zimru_creator_begin_item requires start_writing first",
            )),
        );
        return false;
    }

    let mut content = Vec::new();
    if size_hint > 0 {
        content.reserve(size_hint);
    }
    (*c).in_flight = Some(InFlightItem {
        namespace: resolved_ns,
        path: resolved_path,
        title: title_str,
        mimetype: mime_str,
        content,
    });
    true
}

/// Append a chunk of body bytes to the in-flight chunked item.
/// Cheap — copies `len` bytes into the in-flight buffer. Empty
/// chunks (`len == 0`, `chunk` may be NULL) are no-ops.
///
/// Errors:
///
/// * No item in flight → `*err` set, returns `false`. Call
///   `begin_item` first.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_item_chunk(
    c: *mut zimru_creator_t,
    chunk: *const u8,
    len: usize,
    err: *mut *mut zimru_error_t,
) -> bool {
    if c.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    let Some(in_flight) = (*c).in_flight.as_mut() else {
        set_err(
            err,
            crate::Error::Io(std::io::Error::other(
                "zimru_creator_item_chunk: no item in flight (call begin_item first)",
            )),
        );
        return false;
    };
    if len == 0 {
        return true;
    }
    if chunk.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    let slice = std::slice::from_raw_parts(chunk, len);
    in_flight.content.extend_from_slice(slice);
    true
}

/// Finalise the in-flight chunked item — pushes it through the
/// streaming bin-packer (same path as `add_item`). After this
/// returns successfully, no item is in flight; the caller can
/// `begin_item` again or proceed to `finish_writing`.
///
/// Errors:
///
/// * No item in flight → `*err` set, returns `false`.
/// * Internal write/encode error during the (potentially
///   triggered) cluster flush → `*err` set, returns `false`.
#[no_mangle]
pub unsafe extern "C" fn zimru_creator_end_item(
    c: *mut zimru_creator_t,
    err: *mut *mut zimru_error_t,
) -> bool {
    if c.is_null() {
        set_err(err, crate::Error::EntryNotFound);
        return false;
    }
    let Some(in_flight) = (*c).in_flight.take() else {
        set_err(
            err,
            crate::Error::Io(std::io::Error::other(
                "zimru_creator_end_item: no item in flight",
            )),
        );
        return false;
    };
    let inner = inner_mut(c, err);
    if inner.is_null() {
        return false;
    }
    let creator: &mut Creator = &mut *inner;
    let item = Item {
        path: in_flight.path,
        title: in_flight.title,
        mimetype: in_flight.mimetype,
        content: in_flight.content,
        namespace: in_flight.namespace,
    };
    match creator.push_streaming_item(item) {
        Ok(()) => true,
        Err(e) => {
            set_err(err, e);
            false
        }
    }
}
