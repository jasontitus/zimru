use std::ffi::{c_char, CString};

use crate::error::Error;

/// Opaque error type returned by every fallible C API entry point.
/// Intentionally NOT `#[repr(C)]` so cbindgen forward-declares it as an
/// opaque struct in the generated header (callers can only hold a
/// `zimru_error_t*`, never inspect the layout).
pub struct zimru_error_t {
    code: i32,
    message: CString,
}

/// Numeric error codes corresponding to [`crate::Error`] variants. Stable
/// across releases — new variants are added at the end.
#[repr(i32)]
pub enum zimru_error_code {
    Unknown = 0,
    Io = 1,
    BadMagic = 2,
    Truncated = 3,
    UnsupportedCompression = 4,
    UnsupportedVersion = 5,
    EntryNotFound = 6,
    NoMainEntry = 7,
    NoChecksum = 8,
    BadIndex = 9,
    Decompression = 10,
    RedirectLoop = 11,
    BadUtf8 = 12,
    BadMimeIndex = 13,
    ChecksumMismatch = 14,
    NotAnItem = 15,
}

impl From<&Error> for zimru_error_code {
    fn from(e: &Error) -> Self {
        match e {
            Error::Io(_) => zimru_error_code::Io,
            Error::BadMagic(_) => zimru_error_code::BadMagic,
            Error::Truncated(_) => zimru_error_code::Truncated,
            Error::UnsupportedCompression(_) => zimru_error_code::UnsupportedCompression,
            Error::UnsupportedVersion { .. } => zimru_error_code::UnsupportedVersion,
            Error::EntryNotFound => zimru_error_code::EntryNotFound,
            Error::NoMainEntry => zimru_error_code::NoMainEntry,
            Error::NoChecksum => zimru_error_code::NoChecksum,
            Error::BadUrlIndex(_, _)
            | Error::BadTitleIndex(_, _)
            | Error::BadClusterIndex(_, _)
            | Error::BadBlobIndex { .. } => zimru_error_code::BadIndex,
            Error::Decompression(_) => zimru_error_code::Decompression,
            Error::RedirectLoop => zimru_error_code::RedirectLoop,
            Error::BadUtf8(_) => zimru_error_code::BadUtf8,
            Error::BadMimeIndex(_) => zimru_error_code::BadMimeIndex,
            Error::ChecksumMismatch { .. } => zimru_error_code::ChecksumMismatch,
            Error::NotAnItem => zimru_error_code::NotAnItem,
        }
    }
}

/// Wrap a Rust error in a heap-allocated `zimru_error_t` and write it
/// through `out`. No-op if `out` is null. Internal helper used by every
/// fallible cffi entry point.
pub(crate) fn set_err(out: *mut *mut zimru_error_t, err: Error) {
    if out.is_null() {
        return;
    }
    let code = zimru_error_code::from(&err) as i32;
    let msg = CString::new(err.to_string()).unwrap_or_else(|_| CString::new("error").unwrap());
    let boxed = Box::new(zimru_error_t { code, message: msg });
    unsafe {
        *out = Box::into_raw(boxed);
    }
}

/// Get the human-readable message of a `zimru_error_t`. Pointer remains
/// valid until the error is freed.
#[no_mangle]
pub unsafe extern "C" fn zimru_error_message(err: *const zimru_error_t) -> *const c_char {
    if err.is_null() {
        return std::ptr::null();
    }
    (*err).message.as_ptr()
}

/// Numeric error code (see `zimru_error_code` enum).
#[no_mangle]
pub unsafe extern "C" fn zimru_error_code(err: *const zimru_error_t) -> i32 {
    if err.is_null() {
        return zimru_error_code::Unknown as i32;
    }
    (*err).code
}

/// Free a `zimru_error_t` allocated by a previous fallible call.
#[no_mangle]
pub unsafe extern "C" fn zimru_error_free(err: *mut zimru_error_t) {
    if !err.is_null() {
        drop(Box::from_raw(err));
    }
}
