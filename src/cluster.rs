//! Cluster reader.
//!
//! On-disk layout of a cluster:
//!
//! ```text
//! u8  info_byte
//!     bits 0..3 : compression type
//!                 0 = none (legacy default)
//!                 1 = none
//!                 2 = zlib   (deprecated, not supported)
//!                 3 = bzip2  (deprecated, not supported)
//!                 4 = xz / lzma2
//!                 5 = zstd
//!     bit  4    : extended flag
//!                 if set, blob offsets in the payload are u64 (8 bytes)
//!                 otherwise they are u32 (4 bytes).
//!
//! ── decompressed payload ──
//! offsets: [P; N+1]   where P = u32 or u64 depending on extended flag
//! blob[0] data
//! blob[1] data
//! ...
//! blob[N-1] data
//! ```
//!
//! `offsets[0]` always equals `(N+1) * sizeof(P)` so the blob count `N`
//! can be derived from the first offset alone.

use std::io::Read;
use std::ops::Range;
use std::sync::Arc;

use crate::error::{Error, Result};

const COMPRESSION_NONE_LEGACY: u8 = 0;
const COMPRESSION_NONE: u8 = 1;
const COMPRESSION_ZLIB: u8 = 2;
const COMPRESSION_BZIP2: u8 = 3;
const COMPRESSION_XZ: u8 = 4;
const COMPRESSION_ZSTD: u8 = 5;

const EXTENDED_FLAG: u8 = 0x10;

/// Decompressed-size ceiling for standard (u32 blob offsets) clusters.
/// The format cannot address payload bytes past `u32::MAX` with 4-byte
/// offsets, so anything larger is corrupt input or a decompression bomb.
pub const MAX_STANDARD_CLUSTER_BYTES: u64 = u32::MAX as u64;

/// Decompressed-size ceiling for extended (u64 blob offsets) clusters —
/// a defensive bound so a crafted cluster in an untrusted archive cannot
/// grow the decode buffer without limit and OOM the process. 16 GiB,
/// spelled as a decimal literal so the cbindgen-exported C macro stays a
/// well-defined `long long` constant (a `16 << 30` would overflow `int`).
pub const MAX_EXTENDED_CLUSTER_BYTES: u64 = 17_179_869_184;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Xz,
    Zstd,
}

#[derive(Debug)]
pub struct Cluster {
    /// Decompressed (or pass-through) payload, NOT including the info byte.
    payload: Arc<[u8]>,
    extended: bool,
    blob_count: u32,
    compression: Compression,
}

impl Cluster {
    /// Decode a cluster from its on-disk bytes (including the info byte).
    pub fn parse(raw: &[u8]) -> Result<Self> {
        if raw.is_empty() {
            return Err(Error::Truncated(0));
        }
        let info = raw[0];
        let compression_id = info & 0x0F;
        let extended = info & EXTENDED_FLAG != 0;
        let body = &raw[1..];

        let limit = if extended {
            MAX_EXTENDED_CLUSTER_BYTES
        } else {
            MAX_STANDARD_CLUSTER_BYTES
        };
        let (payload, compression): (Arc<[u8]>, Compression) = match compression_id {
            COMPRESSION_NONE_LEGACY | COMPRESSION_NONE => {
                (Arc::from(body.to_vec()), Compression::None)
            }
            COMPRESSION_XZ => (Arc::from(decode_xz(body, limit)?), Compression::Xz),
            COMPRESSION_ZSTD => (Arc::from(decode_zstd(body, limit)?), Compression::Zstd),
            COMPRESSION_ZLIB | COMPRESSION_BZIP2 => {
                return Err(Error::UnsupportedCompression(compression_id));
            }
            other => return Err(Error::UnsupportedCompression(other)),
        };

        let ptr_size = if extended { 8 } else { 4 };
        if payload.len() < ptr_size {
            return Err(Error::Truncated(0));
        }
        let first = read_off(&payload, 0, extended)?;
        if first == 0 || first as usize > payload.len() {
            return Err(Error::Truncated(first));
        }
        // first = (N + 1) * ptr_size  =>  N = first/ptr_size - 1
        let total_ptrs = first as usize / ptr_size;
        if total_ptrs == 0 {
            return Err(Error::Truncated(0));
        }
        let blob_count = (total_ptrs - 1) as u32;

        Ok(Cluster {
            payload,
            extended,
            blob_count,
            compression,
        })
    }

    pub fn compression(&self) -> Compression {
        self.compression
    }

    pub fn blob_count(&self) -> u32 {
        self.blob_count
    }

    pub fn payload(&self) -> &Arc<[u8]> {
        &self.payload
    }

    pub fn blob(&self, idx: u32) -> Result<&[u8]> {
        let r = self.blob_range(idx)?;
        Ok(&self.payload[r])
    }

    pub fn blob_range(&self, idx: u32) -> Result<Range<usize>> {
        if idx >= self.blob_count {
            return Err(Error::BadBlobIndex {
                cluster: u32::MAX,
                blob: idx,
                count: self.blob_count,
            });
        }
        let ptr_size = if self.extended { 8 } else { 4 };
        let off_pos = idx as usize * ptr_size;
        let start = read_off(&self.payload, off_pos, self.extended)? as usize;
        let end = read_off(&self.payload, off_pos + ptr_size, self.extended)? as usize;
        if end < start || end > self.payload.len() {
            return Err(Error::Truncated(end as u64));
        }
        Ok(start..end)
    }
}

/// Resolve a blob's byte range within the payload of the on-disk cluster
/// bytes `raw` (info byte included) **without copying or caching the
/// payload** — only valid when the cluster is stored uncompressed, where
/// the offset table can be read in place.
///
/// Returns `Ok(None)` for xz/zstd clusters (the payload is not directly
/// addressable on disk) and `Err(UnsupportedCompression)` for
/// zlib/bzip2/unknown compression ids, mirroring [`Cluster::parse`].
pub(crate) fn raw_blob_range(raw: &[u8], blob_idx: u32) -> Result<Option<Range<usize>>> {
    if raw.is_empty() {
        return Err(Error::Truncated(0));
    }
    let info = raw[0];
    match info & 0x0F {
        COMPRESSION_NONE_LEGACY | COMPRESSION_NONE => {}
        COMPRESSION_XZ | COMPRESSION_ZSTD => return Ok(None),
        other => return Err(Error::UnsupportedCompression(other)),
    }
    let extended = info & EXTENDED_FLAG != 0;
    let body = &raw[1..];
    let ptr_size = if extended { 8 } else { 4 };
    let first = read_off(body, 0, extended)?;
    if first == 0 || first as usize > body.len() {
        return Err(Error::Truncated(first));
    }
    let total_ptrs = first as usize / ptr_size;
    if total_ptrs == 0 {
        return Err(Error::Truncated(0));
    }
    let blob_count = (total_ptrs - 1) as u32;
    if blob_idx >= blob_count {
        return Err(Error::BadBlobIndex {
            cluster: u32::MAX,
            blob: blob_idx,
            count: blob_count,
        });
    }
    let off_pos = blob_idx as usize * ptr_size;
    let start = read_off(body, off_pos, extended)? as usize;
    let end = read_off(body, off_pos + ptr_size, extended)? as usize;
    if end < start || end > body.len() {
        return Err(Error::Truncated(end as u64));
    }
    Ok(Some(start..end))
}

/// Validate the whole blob-offset table of the on-disk cluster bytes
/// `raw` in place, without copying the payload. Returns `Ok(true)` when
/// the cluster is uncompressed and its table checked out; `Ok(false)`
/// when the cluster is xz/zstd (the caller must decode to validate);
/// `Err` for a corrupt table or unsupported compression id.
pub(crate) fn validate_raw_offsets(raw: &[u8]) -> Result<bool> {
    if raw.is_empty() {
        return Err(Error::Truncated(0));
    }
    let info = raw[0];
    match info & 0x0F {
        COMPRESSION_NONE_LEGACY | COMPRESSION_NONE => {}
        COMPRESSION_XZ | COMPRESSION_ZSTD => return Ok(false),
        other => return Err(Error::UnsupportedCompression(other)),
    }
    let extended = info & EXTENDED_FLAG != 0;
    let body = &raw[1..];
    let ptr_size = if extended { 8 } else { 4 };
    let first = read_off(body, 0, extended)?;
    if first == 0 || first as usize > body.len() {
        return Err(Error::Truncated(first));
    }
    let total_ptrs = first as usize / ptr_size;
    if total_ptrs == 0 {
        return Err(Error::Truncated(0));
    }
    let mut prev = first;
    for i in 1..total_ptrs {
        let off = read_off(body, i * ptr_size, extended)?;
        if off < prev || off as usize > body.len() {
            return Err(Error::Truncated(off));
        }
        prev = off;
    }
    Ok(true)
}

fn read_off(buf: &[u8], off: usize, extended: bool) -> Result<u64> {
    if extended {
        let s = buf.get(off..off + 8).ok_or(Error::Truncated(off as u64))?;
        Ok(u64::from_le_bytes(s.try_into().unwrap()))
    } else {
        let s = buf.get(off..off + 4).ok_or(Error::Truncated(off as u64))?;
        Ok(u32::from_le_bytes(s.try_into().unwrap()) as u64)
    }
}

fn decode_xz(body: &[u8], limit: u64) -> Result<Vec<u8>> {
    let cap = (body.len().saturating_mul(4) as u64).min(limit) as usize;
    let mut out = Vec::with_capacity(cap);
    let dec = xz2::read::XzDecoder::new(body);
    // `limit + 1` so an over-limit stream is detected rather than
    // silently truncated at exactly `limit` bytes.
    dec.take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::Decompression(format!("xz: {e}")))?;
    if out.len() as u64 > limit {
        return Err(Error::Decompression(format!(
            "xz: decompressed cluster exceeds {limit}-byte limit"
        )));
    }
    Ok(out)
}

fn decode_zstd(body: &[u8], limit: u64) -> Result<Vec<u8>> {
    // Fast path: when the frame header carries pledgedSrcSize (libzim and
    // zimru's writer both set it on every cluster), `bulk::decompress`
    // pre-allocates the exact output size and runs the whole
    // decompression in libzstd's tight inner loop with no streaming-
    // reader hop. This is the dominant case for real ZIMs and is what
    // makes the difference on zstd22 clusters.
    //
    // The declared size is untrusted input — reject it before allocating
    // when it exceeds the cluster-size limit.
    //
    // Slow path: streams without an FCS header (some hand-built test
    // fixtures, or ZIMs from older writers) fall back to streaming with
    // an 8× capacity guess, which grows transparently up to the limit.
    if let Ok(Some(size)) = zstd::zstd_safe::get_frame_content_size(body) {
        if size > limit {
            return Err(Error::Decompression(format!(
                "zstd: declared cluster size {size} exceeds {limit}-byte limit"
            )));
        }
        return zstd::bulk::decompress(body, size as usize)
            .map_err(|e| Error::Decompression(format!("zstd: {e}")));
    }
    let cap = (body.len().saturating_mul(8) as u64).min(limit) as usize;
    let mut out = Vec::with_capacity(cap);
    let dec = zstd::stream::read::Decoder::new(body)
        .map_err(|e| Error::Decompression(format!("zstd init: {e}")))?;
    dec.take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::Decompression(format!("zstd: {e}")))?;
    if out.len() as u64 > limit {
        return Err(Error::Decompression(format!(
            "zstd: decompressed cluster exceeds {limit}-byte limit"
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_uncompressed_cluster(blobs: &[&[u8]], extended: bool) -> Vec<u8> {
        let n = blobs.len();
        let ptr_size = if extended { 8 } else { 4 };
        let header_len = (n + 1) * ptr_size;
        let mut out = Vec::new();
        out.push(if extended { 0x11 } else { 0x01 });
        let mut cursor = header_len as u64;
        for blob in blobs {
            push_off(&mut out, cursor, extended);
            cursor += blob.len() as u64;
        }
        push_off(&mut out, cursor, extended);
        for blob in blobs {
            out.extend_from_slice(blob);
        }
        out
    }

    fn push_off(v: &mut Vec<u8>, x: u64, extended: bool) {
        if extended {
            v.extend_from_slice(&x.to_le_bytes());
        } else {
            v.extend_from_slice(&(x as u32).to_le_bytes());
        }
    }

    #[test]
    fn reads_uncompressed_cluster_4byte_offsets() {
        let raw = build_uncompressed_cluster(&[b"hello", b"world", b"!!"], false);
        let c = Cluster::parse(&raw).unwrap();
        assert_eq!(c.compression(), Compression::None);
        assert_eq!(c.blob_count(), 3);
        assert_eq!(c.blob(0).unwrap(), b"hello");
        assert_eq!(c.blob(1).unwrap(), b"world");
        assert_eq!(c.blob(2).unwrap(), b"!!");
        assert!(c.blob(3).is_err());
    }

    #[test]
    fn reads_uncompressed_cluster_8byte_offsets() {
        let raw = build_uncompressed_cluster(&[b"a", b"bb", b"ccc", b"dddd"], true);
        let c = Cluster::parse(&raw).unwrap();
        assert_eq!(c.blob_count(), 4);
        assert_eq!(c.blob(0).unwrap(), b"a");
        assert_eq!(c.blob(3).unwrap(), b"dddd");
    }

    #[test]
    fn reads_zstd_cluster() {
        let inner = build_uncompressed_cluster(&[b"foo", b"barbaz"], false);
        let payload = &inner[1..];
        let compressed = zstd::stream::encode_all(payload, 3).unwrap();
        let mut raw = Vec::with_capacity(1 + compressed.len());
        raw.push(0x05);
        raw.extend_from_slice(&compressed);
        let c = Cluster::parse(&raw).unwrap();
        assert_eq!(c.compression(), Compression::Zstd);
        assert_eq!(c.blob_count(), 2);
        assert_eq!(c.blob(0).unwrap(), b"foo");
        assert_eq!(c.blob(1).unwrap(), b"barbaz");
    }

    #[test]
    fn reads_xz_cluster() {
        let inner = build_uncompressed_cluster(&[b"alpha", b"beta", b"gamma"], false);
        let payload = &inner[1..];
        let compressed = {
            use std::io::Write;
            let mut enc = xz2::write::XzEncoder::new(Vec::new(), 3);
            enc.write_all(payload).unwrap();
            enc.finish().unwrap()
        };
        let mut raw = Vec::with_capacity(1 + compressed.len());
        raw.push(0x04);
        raw.extend_from_slice(&compressed);
        let c = Cluster::parse(&raw).unwrap();
        assert_eq!(c.compression(), Compression::Xz);
        assert_eq!(c.blob_count(), 3);
        assert_eq!(c.blob(0).unwrap(), b"alpha");
        assert_eq!(c.blob(2).unwrap(), b"gamma");
    }

    #[test]
    fn rejects_zstd_declared_size_over_limit() {
        let payload = vec![0u8; 1024];
        let compressed = zstd::stream::encode_all(&payload[..], 3).unwrap();
        match decode_zstd(&compressed, 100) {
            Err(Error::Decompression(msg)) => assert!(msg.contains("limit"), "{msg}"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_xz_stream_over_limit() {
        let payload = vec![0u8; 1024];
        let compressed = {
            use std::io::Write;
            let mut enc = xz2::write::XzEncoder::new(Vec::new(), 3);
            enc.write_all(&payload).unwrap();
            enc.finish().unwrap()
        };
        match decode_xz(&compressed, 100) {
            Err(Error::Decompression(msg)) => assert!(msg.contains("limit"), "{msg}"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_unsupported_compression() {
        let raw = vec![0x02u8, 0, 0, 0, 0];
        match Cluster::parse(&raw) {
            Err(Error::UnsupportedCompression(2)) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}
