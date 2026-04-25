//! Directory entry parsing.
//!
//! There are two kinds of dirents:
//!
//! ## Article entry (mimetype != 0xFFFF)
//! ```text
//! u16  mimetype index   (0xFFFE/0xFFFD reserved for old "linktarget"/"deletedentry")
//! u8   parameter_len    (always 0 in current format; reserved)
//! u8   namespace        (single ASCII char)
//! u32  revision         (always 0; reserved)
//! u32  cluster_number
//! u32  blob_number
//! cstr url
//! cstr title            (empty == reuse url as title)
//! u8[parameter_len] parameter
//! ```
//!
//! ## Redirect entry (mimetype == 0xFFFF)
//! ```text
//! u16  mimetype = 0xFFFF
//! u8   parameter_len
//! u8   namespace
//! u32  revision
//! u32  redirect_index   (index into the URL-pointer list)
//! cstr url
//! cstr title
//! u8[parameter_len] parameter
//! ```

use crate::error::Result;
use crate::raw;

pub const MIME_REDIRECT: u16 = 0xFFFF;
pub const MIME_LINKTARGET: u16 = 0xFFFE; // deprecated
pub const MIME_DELETED: u16 = 0xFFFD; // deprecated

#[derive(Debug, Clone)]
pub enum Dirent {
    Article(ArticleEntry),
    Redirect(RedirectEntry),
}

#[derive(Debug, Clone)]
pub struct ArticleEntry {
    pub mimetype: u16,
    pub namespace: u8,
    pub cluster: u32,
    pub blob: u32,
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct RedirectEntry {
    pub namespace: u8,
    pub redirect_index: u32,
    pub url: String,
    pub title: String,
}

impl Dirent {
    /// Parse a dirent starting at `off` in the underlying buffer.
    pub fn parse(buf: &[u8], off: usize) -> Result<Self> {
        let mimetype = raw::u16_at(buf, off)?;
        let parameter_len = raw::u8_at(buf, off + 2)? as usize;
        let namespace = raw::u8_at(buf, off + 3)?;
        let _revision = raw::u32_at(buf, off + 4)?;

        match mimetype {
            MIME_REDIRECT => {
                let redirect_index = raw::u32_at(buf, off + 8)?;
                let (url, after_url) = raw::cstr_at(buf, off + 12)?;
                let (title, _after_title) = raw::cstr_at(buf, after_url)?;
                Ok(Dirent::Redirect(RedirectEntry {
                    namespace,
                    redirect_index,
                    url: url.to_string(),
                    title: if title.is_empty() {
                        url.to_string()
                    } else {
                        title.to_string()
                    },
                }))
            }
            MIME_LINKTARGET | MIME_DELETED => {
                // Treat deprecated forms as articles with no content, leaving
                // the cluster/blob fields readable for diagnostic purposes.
                let cluster = raw::u32_at(buf, off + 8)?;
                let blob = raw::u32_at(buf, off + 12)?;
                let (url, after_url) = raw::cstr_at(buf, off + 16)?;
                let (title, _) = raw::cstr_at(buf, after_url)?;
                Ok(Dirent::Article(ArticleEntry {
                    mimetype,
                    namespace,
                    cluster,
                    blob,
                    url: url.to_string(),
                    title: if title.is_empty() {
                        url.to_string()
                    } else {
                        title.to_string()
                    },
                }))
            }
            _ => {
                let cluster = raw::u32_at(buf, off + 8)?;
                let blob = raw::u32_at(buf, off + 12)?;
                let (url, after_url) = raw::cstr_at(buf, off + 16)?;
                let (title, after_title) = raw::cstr_at(buf, after_url)?;
                let _param_end = after_title + parameter_len;
                Ok(Dirent::Article(ArticleEntry {
                    mimetype,
                    namespace,
                    cluster,
                    blob,
                    url: url.to_string(),
                    title: if title.is_empty() {
                        url.to_string()
                    } else {
                        title.to_string()
                    },
                }))
            }
        }
    }

    /// Cheap key extraction for binary-search probes: read just the
    /// `(namespace, url)` of the dirent at `off` without allocating. The
    /// returned `&str` borrows from `buf`. Hot-path equivalent of
    /// [`Dirent::parse`] — that fully materializes the dirent and pays
    /// two `String` allocations per call (url + title), which is the
    /// bulk of the `~1.2 µs/op` `entry_by_ns_path` cost on a 17 K-entry
    /// archive (14-probe binary search → 28 String allocs per lookup).
    pub fn key_at(buf: &[u8], off: usize) -> Result<(u8, &str)> {
        let mimetype = raw::u16_at(buf, off)?;
        let namespace = raw::u8_at(buf, off + 3)?;
        let url_off = if mimetype == MIME_REDIRECT {
            off + 12
        } else {
            off + 16
        };
        let (url, _) = raw::cstr_at(buf, url_off)?;
        Ok((namespace, url))
    }

    /// Cheap title extraction for binary-search probes — returns
    /// `(namespace, title)` borrowed from `buf`. Title falls back to
    /// `url` when the dirent's title field is empty (matching
    /// [`Dirent::parse`]'s behaviour).
    pub fn title_key_at(buf: &[u8], off: usize) -> Result<(u8, &str)> {
        let mimetype = raw::u16_at(buf, off)?;
        let namespace = raw::u8_at(buf, off + 3)?;
        let url_off = if mimetype == MIME_REDIRECT {
            off + 12
        } else {
            off + 16
        };
        let (url, after_url) = raw::cstr_at(buf, url_off)?;
        let (title, _) = raw::cstr_at(buf, after_url)?;
        Ok((namespace, if title.is_empty() { url } else { title }))
    }

    pub fn url(&self) -> &str {
        match self {
            Dirent::Article(a) => &a.url,
            Dirent::Redirect(r) => &r.url,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Dirent::Article(a) => &a.title,
            Dirent::Redirect(r) => &r.title,
        }
    }

    pub fn namespace(&self) -> u8 {
        match self {
            Dirent::Article(a) => a.namespace,
            Dirent::Redirect(r) => r.namespace,
        }
    }

    pub fn is_redirect(&self) -> bool {
        matches!(self, Dirent::Redirect(_))
    }
}

/// Compare a (namespace, url) tuple — that's the canonical sort key for the
/// URL pointer list.
pub fn cmp_path(a_ns: u8, a_url: &str, b_ns: u8, b_url: &str) -> std::cmp::Ordering {
    a_ns.cmp(&b_ns).then_with(|| a_url.cmp(b_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_article(
        mime: u16,
        ns: u8,
        cluster: u32,
        blob: u32,
        url: &str,
        title: &str,
    ) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&mime.to_le_bytes());
        v.push(0); // parameter_len
        v.push(ns);
        v.extend_from_slice(&0u32.to_le_bytes()); // revision
        v.extend_from_slice(&cluster.to_le_bytes());
        v.extend_from_slice(&blob.to_le_bytes());
        v.extend_from_slice(url.as_bytes());
        v.push(0);
        v.extend_from_slice(title.as_bytes());
        v.push(0);
        v
    }

    fn build_redirect(ns: u8, idx: u32, url: &str, title: &str) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&MIME_REDIRECT.to_le_bytes());
        v.push(0);
        v.push(ns);
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&idx.to_le_bytes());
        v.extend_from_slice(url.as_bytes());
        v.push(0);
        v.extend_from_slice(title.as_bytes());
        v.push(0);
        v
    }

    #[test]
    fn round_trips_an_article_dirent() {
        let buf = build_article(0, b'C', 5, 3, "home", "Home");
        let d = Dirent::parse(&buf, 0).unwrap();
        match d {
            Dirent::Article(a) => {
                assert_eq!(a.mimetype, 0);
                assert_eq!(a.namespace, b'C');
                assert_eq!(a.cluster, 5);
                assert_eq!(a.blob, 3);
                assert_eq!(a.url, "home");
                assert_eq!(a.title, "Home");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn empty_title_falls_back_to_url() {
        let buf = build_article(2, b'C', 0, 0, "kiwix.png", "");
        let d = Dirent::parse(&buf, 0).unwrap();
        assert_eq!(d.title(), "kiwix.png");
    }

    #[test]
    fn parses_redirects() {
        let buf = build_redirect(b'C', 17, "old", "Old");
        let d = Dirent::parse(&buf, 0).unwrap();
        assert!(d.is_redirect());
        if let Dirent::Redirect(r) = d {
            assert_eq!(r.redirect_index, 17);
            assert_eq!(r.namespace, b'C');
            assert_eq!(r.url, "old");
            assert_eq!(r.title, "Old");
        }
    }
}
