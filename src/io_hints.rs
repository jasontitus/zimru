//! Portable I/O readahead / access-pattern hints.
//!
//! These wrap `posix_fadvise(2)` (Linux) and `fcntl(2)`'s
//! `F_RDAHEAD` / `F_NOCACHE` (macOS) so callers can express access
//! patterns the kernel can prefetch for, without depending on
//! io_uring or any platform-specific async I/O surface.
//!
//! On platforms without an obvious analogue (e.g. Windows) the
//! functions compile to no-ops — every call site must remain
//! correct without the hint.

use std::fs::File;

/// Hint that `file` is about to be read sequentially from start
/// to finish. On Linux: `posix_fadvise(POSIX_FADV_SEQUENTIAL)`
/// (doubles readahead, drops pages after they're touched). On
/// macOS: `fcntl(F_RDAHEAD, 1)` (enables aggressive readahead).
/// Best-effort — failures are silently ignored.
pub fn hint_sequential(_file: &File) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::posix_fadvise(_file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::fcntl(_file.as_raw_fd(), libc::F_RDAHEAD, 1);
        }
    }
}

/// Hint that `file`'s entire contents will be read soon.
/// On Linux: `posix_fadvise(POSIX_FADV_WILLNEED)` — kernel kicks
/// off async readahead now so the bytes are warm by the time the
/// caller reads them. On macOS: same `F_RDAHEAD` path as
/// [`hint_sequential`] (no separate "will need" flag, but
/// `F_RDAHEAD` covers the prefetch case for sequential access).
/// Best-effort.
pub fn hint_will_need(_file: &File) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::posix_fadvise(_file.as_raw_fd(), 0, 0, libc::POSIX_FADV_WILLNEED);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::fcntl(_file.as_raw_fd(), libc::F_RDAHEAD, 1);
        }
    }
}
