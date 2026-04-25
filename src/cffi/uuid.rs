//! Stable seed → 16-byte UUID generator for C consumers.
//!
//! Bit-for-bit compatibility with libzim's `zim::Uuid::generate(seed)`
//! is **not** required — consumers (e.g. libzim-shim) only need
//! intra-run consistency: the same seed must yield the same UUID for
//! every call within the same process / shim version. We satisfy that
//! by hashing the seed with MD5 (already a dependency for ZIM
//! checksums) and applying RFC 4122 v4-style version/variant nibbles.

use md5::{Digest, Md5};

/// Derive a deterministic 16-byte UUID from an arbitrary byte seed.
///
/// `seed` may be NULL only if `seed_len` is 0. `out` must point to at
/// least 16 writable bytes. The output has the high nibble of byte 6
/// set to `4` and the high two bits of byte 8 set to `10` (RFC 4122
/// v4 layout), so consumers that validate the variant won't reject it.
#[no_mangle]
pub unsafe extern "C" fn zimru_uuid_generate(seed: *const u8, seed_len: usize, out: *mut u8) {
    if out.is_null() {
        return;
    }
    let bytes: &[u8] = if seed_len == 0 {
        &[]
    } else if seed.is_null() {
        return;
    } else {
        std::slice::from_raw_parts(seed, seed_len)
    };
    let mut h = Md5::new();
    h.update(bytes);
    let mut digest: [u8; 16] = h.finalize().into();
    // RFC 4122-style version/variant nibbles so downstream UUID
    // validators don't reject the value.
    digest[6] = (digest[6] & 0x0f) | 0x40; // version 4
    digest[8] = (digest[8] & 0x3f) | 0x80; // variant 10xx
    std::ptr::copy_nonoverlapping(digest.as_ptr(), out, 16);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_seed() {
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        let seed = b"hello world";
        unsafe {
            zimru_uuid_generate(seed.as_ptr(), seed.len(), a.as_mut_ptr());
            zimru_uuid_generate(seed.as_ptr(), seed.len(), b.as_mut_ptr());
        }
        assert_eq!(a, b, "same seed must yield same UUID");
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        unsafe {
            zimru_uuid_generate(b"x".as_ptr(), 1, a.as_mut_ptr());
            zimru_uuid_generate(b"y".as_ptr(), 1, b.as_mut_ptr());
        }
        assert_ne!(a, b);
    }

    #[test]
    fn version_and_variant_nibbles_set() {
        let mut out = [0u8; 16];
        unsafe {
            zimru_uuid_generate(b"any".as_ptr(), 3, out.as_mut_ptr());
        }
        assert_eq!(out[6] & 0xf0, 0x40, "version nibble must be 4");
        assert_eq!(out[8] & 0xc0, 0x80, "variant must be 10xx");
    }

    #[test]
    fn empty_seed_is_handled() {
        let mut out = [0u8; 16];
        unsafe {
            zimru_uuid_generate(std::ptr::null(), 0, out.as_mut_ptr());
        }
        // MD5 of empty input is well-known: d41d8cd98f00b204e9800998ecf8427e
        // After v4/variant nibble fixup byte 6 has its top nibble forced to 4
        // and byte 8's top two bits forced to 10.
        assert_eq!(out[6] & 0xf0, 0x40);
        assert_eq!(out[8] & 0xc0, 0x80);
    }

    #[test]
    fn null_out_is_safe() {
        unsafe { zimru_uuid_generate(b"x".as_ptr(), 1, std::ptr::null_mut()) };
        // No panic, no crash.
    }
}
