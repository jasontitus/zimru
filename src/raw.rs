//! Tiny zero-copy binary reader helpers used by the format parsers.
//!
//! All multi-byte ZIM fields are little-endian.

use crate::error::{Error, Result};

#[inline]
pub fn u8_at(buf: &[u8], off: usize) -> Result<u8> {
    buf.get(off).copied().ok_or(Error::Truncated(off as u64))
}

#[inline]
pub fn u16_at(buf: &[u8], off: usize) -> Result<u16> {
    let end = off + 2;
    let s = buf.get(off..end).ok_or(Error::Truncated(off as u64))?;
    Ok(u16::from_le_bytes(s.try_into().unwrap()))
}

#[inline]
pub fn u32_at(buf: &[u8], off: usize) -> Result<u32> {
    let end = off + 4;
    let s = buf.get(off..end).ok_or(Error::Truncated(off as u64))?;
    Ok(u32::from_le_bytes(s.try_into().unwrap()))
}

#[inline]
pub fn u64_at(buf: &[u8], off: usize) -> Result<u64> {
    let end = off + 8;
    let s = buf.get(off..end).ok_or(Error::Truncated(off as u64))?;
    Ok(u64::from_le_bytes(s.try_into().unwrap()))
}

/// Read a NUL-terminated string starting at `off`. Returns the string slice
/// (excluding the NUL) and the offset of the byte AFTER the NUL. Strings
/// in ZIM are UTF-8.
pub fn cstr_at(buf: &[u8], off: usize) -> Result<(&str, usize)> {
    let start = off;
    let tail = buf.get(start..).ok_or(Error::Truncated(off as u64))?;
    let nul = tail
        .iter()
        .position(|&b| b == 0)
        .ok_or(Error::Truncated(off as u64))?;
    let bytes = &tail[..nul];
    let s = std::str::from_utf8(bytes).map_err(|_| Error::BadUtf8(off as u64))?;
    Ok((s, start + nul + 1))
}
