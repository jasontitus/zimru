use std::io;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("not a zim file (bad magic 0x{0:08x})")]
    BadMagic(u32),

    #[error("unsupported zim version {major}.{minor}")]
    UnsupportedVersion { major: u16, minor: u16 },

    #[error("file truncated at offset {0}")]
    Truncated(u64),

    #[error("invalid utf-8 string at offset {0}")]
    BadUtf8(u64),

    #[error("entry not found")]
    EntryNotFound,

    #[error("invalid dirent (mimetype index {0} out of range)")]
    BadMimeIndex(u16),

    #[error("invalid url-pointer index {0} (count {1})")]
    BadUrlIndex(u32, u32),

    #[error("invalid title-pointer index {0} (count {1})")]
    BadTitleIndex(u32, u32),

    #[error("invalid cluster index {0} (count {1})")]
    BadClusterIndex(u32, u32),

    #[error("invalid blob index {blob} in cluster {cluster} (blob count {count})")]
    BadBlobIndex { cluster: u32, blob: u32, count: u32 },

    #[error("unsupported cluster compression type {0}")]
    UnsupportedCompression(u8),

    #[error("cluster decompression failed: {0}")]
    Decompression(String),

    #[error("redirect chain exceeded depth limit")]
    RedirectLoop,

    #[error("checksum mismatch (expected {expected}, computed {computed})")]
    ChecksumMismatch { expected: String, computed: String },

    #[error("file has no checksum")]
    NoChecksum,

    #[error("file has no main entry")]
    NoMainEntry,

    #[error("not an item (entry is a redirect)")]
    NotAnItem,
}
