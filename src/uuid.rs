//! 128-bit ZIM archive identifier with canonical-string round-tripping.
//!
//! ```
//! use zimru::Uuid;
//! let u: Uuid = "529b7e6e-3e90-9b9b-3d24-eac14f2f1f00".parse().unwrap();
//! assert_eq!(u.to_string(), "529b7e6e-3e90-9b9b-3d24-eac14f2f1f00");
//! assert_eq!(*u.as_bytes(), [
//!     0x52, 0x9b, 0x7e, 0x6e, 0x3e, 0x90, 0x9b, 0x9b,
//!     0x3d, 0x24, 0xea, 0xc1, 0x4f, 0x2f, 0x1f, 0x00,
//! ]);
//! ```

use std::fmt;
use std::str::FromStr;

/// Sixteen-byte archive identifier, displayed in canonical 8-4-4-4-12 hex.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Uuid([u8; 16]);

impl Uuid {
    pub const fn from_bytes(b: [u8; 16]) -> Self {
        Uuid(b)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; 16] {
        self.0
    }
}

impl From<[u8; 16]> for Uuid {
    fn from(b: [u8; 16]) -> Self {
        Uuid(b)
    }
}

impl From<Uuid> for [u8; 16] {
    fn from(u: Uuid) -> Self {
        u.0
    }
}

impl AsRef<[u8; 16]> for Uuid {
    fn as_ref(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let u = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            u[0], u[1], u[2], u[3],
            u[4], u[5],
            u[6], u[7],
            u[8], u[9],
            u[10], u[11], u[12], u[13], u[14], u[15],
        )
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Uuid({self})")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UuidParseError {
    pub message: &'static str,
}

impl fmt::Display for UuidParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for UuidParseError {}

impl FromStr for Uuid {
    type Err = UuidParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        let mut out = [0u8; 16];
        match bytes.len() {
            // Canonical hyphenated: 8-4-4-4-12.
            36 => {
                if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
                    return Err(UuidParseError {
                        message: "uuid: hyphens not at positions 8/13/18/23",
                    });
                }
                let groups = [
                    (0, 8),   // bytes 0..4
                    (9, 13),  // bytes 4..6
                    (14, 18), // bytes 6..8
                    (19, 23), // bytes 8..10
                    (24, 36), // bytes 10..16
                ];
                let mut bi = 0usize;
                for (start, end) in groups {
                    let mut p = start;
                    while p < end {
                        let hi = hex_nibble(bytes[p])?;
                        let lo = hex_nibble(bytes[p + 1])?;
                        out[bi] = (hi << 4) | lo;
                        bi += 1;
                        p += 2;
                    }
                }
                Ok(Uuid(out))
            }
            // Unhyphenated 32-hex.
            32 => {
                for i in 0..16 {
                    let hi = hex_nibble(bytes[i * 2])?;
                    let lo = hex_nibble(bytes[i * 2 + 1])?;
                    out[i] = (hi << 4) | lo;
                }
                Ok(Uuid(out))
            }
            _ => Err(UuidParseError {
                message: "uuid: expected 32 or 36 characters",
            }),
        }
    }
}

fn hex_nibble(b: u8) -> Result<u8, UuidParseError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(10 + b - b'a'),
        b'A'..=b'F' => Ok(10 + b - b'A'),
        _ => Err(UuidParseError {
            message: "uuid: non-hex character",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_canonical_form() {
        let bytes: [u8; 16] = [
            0x52, 0x9b, 0x7e, 0x6e, 0x3e, 0x90, 0x9b, 0x9b, 0x3d, 0x24, 0xea, 0xc1, 0x4f, 0x2f,
            0x1f, 0x00,
        ];
        let u = Uuid::from_bytes(bytes);
        let s = u.to_string();
        assert_eq!(s, "529b7e6e-3e90-9b9b-3d24-eac14f2f1f00");
        let parsed: Uuid = s.parse().unwrap();
        assert_eq!(parsed, u);
        assert_eq!(*parsed.as_bytes(), bytes);
    }

    #[test]
    fn accepts_unhyphenated_form() {
        let s = "529b7e6e3e909b9b3d24eac14f2f1f00";
        let u: Uuid = s.parse().unwrap();
        assert_eq!(u.to_string(), "529b7e6e-3e90-9b9b-3d24-eac14f2f1f00");
    }

    #[test]
    fn accepts_uppercase() {
        let s = "529B7E6E-3E90-9B9B-3D24-EAC14F2F1F00";
        let u: Uuid = s.parse().unwrap();
        // Display always lowercases.
        assert_eq!(u.to_string(), s.to_ascii_lowercase());
    }

    #[test]
    fn rejects_wrong_length() {
        for bad in ["", "abc", "x".repeat(35).as_str(), "x".repeat(37).as_str()] {
            assert!(bad.parse::<Uuid>().is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_misplaced_hyphens() {
        // 36 chars, hyphens at wrong positions.
        let bad = "529b7e6e3-e90-9b9b-3d24-eac14f2f1f000";
        assert!(bad.parse::<Uuid>().is_err());
    }

    #[test]
    fn rejects_non_hex_chars() {
        let bad = "529b7e6e-3e90-9b9b-3d24-eac14f2f1zzz";
        assert!(bad.parse::<Uuid>().is_err());
    }

    #[test]
    fn debug_uses_hyphenated_form() {
        let u = Uuid::from_bytes([0; 16]);
        assert_eq!(
            format!("{u:?}"),
            "Uuid(00000000-0000-0000-0000-000000000000)"
        );
    }

    #[test]
    fn from_into_bytes_round_trip() {
        let bytes = [0xab; 16];
        let u: Uuid = bytes.into();
        let back: [u8; 16] = u.into();
        assert_eq!(back, bytes);
    }
}
