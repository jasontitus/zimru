//! MIME type list.
//!
//! Stored as a sequence of NUL-terminated UTF-8 strings; the list itself is
//! terminated by an empty string (i.e. a lone NUL byte).

use crate::error::Result;
use crate::raw;

#[derive(Debug, Clone)]
pub struct MimeList(Vec<String>);

impl MimeList {
    pub fn parse(buf: &[u8], start: usize) -> Result<(Self, usize)> {
        let mut off = start;
        let mut list = Vec::new();
        loop {
            let (s, next) = raw::cstr_at(buf, off)?;
            if s.is_empty() {
                off = next;
                break;
            }
            list.push(s.to_string());
            off = next;
        }
        Ok((MimeList(list), off))
    }

    pub fn get(&self, idx: u16) -> Option<&str> {
        self.0.get(idx as usize).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_typical_mime_list() {
        let blob = b"text/html\0image/png\0application/javascript\0\0";
        let (m, end) = MimeList::parse(blob, 0).unwrap();
        assert_eq!(end, blob.len());
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(0), Some("text/html"));
        assert_eq!(m.get(1), Some("image/png"));
        assert_eq!(m.get(2), Some("application/javascript"));
        assert_eq!(m.get(3), None);
    }

    #[test]
    fn handles_empty_mime_list() {
        let blob = b"\0";
        let (m, end) = MimeList::parse(blob, 0).unwrap();
        assert_eq!(end, 1);
        assert!(m.is_empty());
    }
}
