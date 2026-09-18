//! ZIM file header.
//!
//! The header is exactly 80 bytes laid out at the start of the file.
//! Field layout (little-endian):
//!
//! | offset | size | field            |
//! |--------|------|------------------|
//! |  0     | 4    | magic = 0x44D495A ("ZIM\x04") |
//! |  4     | 2    | major_version    |
//! |  6     | 2    | minor_version    |
//! |  8     | 16   | uuid             |
//! | 24     | 4    | entry_count (a.k.a. articleCount in older docs) |
//! | 28     | 4    | cluster_count    |
//! | 32     | 8    | url_ptr_pos      |
//! | 40     | 8    | title_ptr_pos    |
//! | 48     | 8    | cluster_ptr_pos  |
//! | 56     | 8    | mime_list_pos    |
//! | 64     | 4    | main_page (0xFFFFFFFF if none) |
//! | 68     | 4    | layout_page (deprecated, often 0xFFFFFFFF) |
//! | 72     | 8    | checksum_pos (offset of trailing 16-byte MD5) |

use crate::error::{Error, Result};
use crate::raw;

pub const MAGIC: u32 = 0x44D495A;
pub const HEADER_SIZE: usize = 80;
pub const NO_MAIN_PAGE: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct Header {
    pub major_version: u16,
    pub minor_version: u16,
    pub uuid: [u8; 16],
    pub entry_count: u32,
    pub cluster_count: u32,
    pub url_ptr_pos: u64,
    pub title_ptr_pos: u64,
    pub cluster_ptr_pos: u64,
    pub mime_list_pos: u64,
    pub main_page: u32,
    pub layout_page: u32,
    pub checksum_pos: u64,
}

impl Header {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < HEADER_SIZE {
            return Err(Error::Truncated(buf.len() as u64));
        }
        let magic = raw::u32_at(buf, 0)?;
        if magic != MAGIC {
            return Err(Error::BadMagic(magic));
        }
        let major_version = raw::u16_at(buf, 4)?;
        let minor_version = raw::u16_at(buf, 6)?;
        // Major 5 (legacy) and 6 (with full-text & extended types) are the only
        // versions the format spec defines. Anything else is unsupported.
        if major_version != 5 && major_version != 6 {
            return Err(Error::UnsupportedVersion {
                major: major_version,
                minor: minor_version,
            });
        }
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&buf[8..24]);

        Ok(Header {
            major_version,
            minor_version,
            uuid,
            entry_count: raw::u32_at(buf, 24)?,
            cluster_count: raw::u32_at(buf, 28)?,
            url_ptr_pos: raw::u64_at(buf, 32)?,
            title_ptr_pos: raw::u64_at(buf, 40)?,
            cluster_ptr_pos: raw::u64_at(buf, 48)?,
            mime_list_pos: raw::u64_at(buf, 56)?,
            main_page: raw::u32_at(buf, 64)?,
            layout_page: raw::u32_at(buf, 68)?,
            checksum_pos: raw::u64_at(buf, 72)?,
        })
    }

    /// True for ZIM files that use the "new" namespace scheme (`C` for content,
    /// `M` for metadata, `W` for well-known, `X` for indexes).
    /// New namespaces were introduced in version 6.1; version 6.0 is legacy.
    pub fn uses_new_namespaces(&self) -> bool {
        self.major_version > 6 || (self.major_version == 6 && self.minor_version >= 1)
    }

    pub fn has_main_page(&self) -> bool {
        self.main_page != NO_MAIN_PAGE
    }

    pub fn has_checksum(&self) -> bool {
        self.checksum_pos != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_synthetic_header() {
        // Build an 80-byte header with known values.
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        buf[4..6].copy_from_slice(&6u16.to_le_bytes());
        buf[6..8].copy_from_slice(&1u16.to_le_bytes());
        for (i, b) in (0..16u8).enumerate() {
            buf[8 + i] = b ^ 0xA5;
        }
        buf[24..28].copy_from_slice(&42u32.to_le_bytes());
        buf[28..32].copy_from_slice(&7u32.to_le_bytes());
        buf[32..40].copy_from_slice(&1024u64.to_le_bytes());
        buf[40..48].copy_from_slice(&2048u64.to_le_bytes());
        buf[48..56].copy_from_slice(&4096u64.to_le_bytes());
        buf[56..64].copy_from_slice(&80u64.to_le_bytes());
        buf[64..68].copy_from_slice(&3u32.to_le_bytes());
        buf[68..72].copy_from_slice(&u32::MAX.to_le_bytes());
        buf[72..80].copy_from_slice(&999_999u64.to_le_bytes());

        let h = Header::parse(&buf).unwrap();
        assert_eq!(h.major_version, 6);
        assert_eq!(h.minor_version, 1);
        assert_eq!(h.entry_count, 42);
        assert_eq!(h.cluster_count, 7);
        assert_eq!(h.url_ptr_pos, 1024);
        assert_eq!(h.title_ptr_pos, 2048);
        assert_eq!(h.cluster_ptr_pos, 4096);
        assert_eq!(h.mime_list_pos, 80);
        assert_eq!(h.main_page, 3);
        assert_eq!(h.checksum_pos, 999_999);
        assert!(h.uses_new_namespaces());
        assert!(h.has_main_page());
        assert!(h.has_checksum());
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        match Header::parse(&buf) {
            Err(Error::BadMagic(0xDEADBEEF)) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}
