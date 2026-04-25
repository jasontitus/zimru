//! High-level [`Archive`] reader.
//!
//! The API mirrors libzim's user-facing surface (Archive / Entry / Item / Blob)
//! using snake_case as is idiomatic in Rust. Methods are named to match the
//! C++ class methods one-for-one so swapping out a Rust libzim binding for
//! `zimru` should mostly require only a `use` change.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use memmap2::Mmap;
use rayon::prelude::*;

use crate::cluster::Cluster;
use crate::dirent::{ArticleEntry, Dirent};
use crate::error::{Error, Result};
use crate::header::{Header, NO_MAIN_PAGE};
use crate::mime::MimeList;
use crate::raw;

/// Maximum redirect chain depth before bailing out.
const MAX_REDIRECTS: usize = 16;

/// Default content namespace for "new namespace" archives.
pub const NS_CONTENT_NEW: u8 = b'C';
/// Metadata namespace (new-style archives).
pub const NS_METADATA: u8 = b'M';
/// Well-known namespace (new-style archives).
pub const NS_WELLKNOWN: u8 = b'W';
/// Index namespace (search indexes etc.).
pub const NS_INDEX: u8 = b'X';
/// Articles namespace (legacy archives).
pub const NS_ARTICLES_LEGACY: u8 = b'A';

#[derive(Debug)]
struct ArchiveCore {
    mmap: Mmap,
    header: Header,
    mimes: MimeList,
    /// File length, used as the upper bound when sizing the trailing cluster.
    file_len: u64,
    cluster_cache: Mutex<HashMap<u32, Arc<Cluster>>>,
    /// Cached title-order listing: dirent indices sorted by (namespace, title).
    /// In legacy archives this comes from `header.title_ptr_pos`; in modern
    /// archives (v6.2+) it's stored at `X/listing/titleOrdered/v1` and only
    /// covers C-namespace entries.
    title_listing: OnceLock<Arc<[u32]>>,
    /// Cached `(article_count, media_count)` for the content namespace,
    /// computed on first request via [`Archive::article_and_media_counts`].
    /// "article" is any item whose mimetype starts with `text/html`;
    /// "media" is any non-redirect, non-article entry. Redirects don't
    /// count toward either total.
    content_counts: OnceLock<(u64, u64)>,
}

/// A read-only ZIM archive.
#[derive(Clone, Debug)]
pub struct Archive {
    core: Arc<ArchiveCore>,
}

impl Archive {
    /// Open the archive at `path` (memory mapped).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let f = File::open(path.as_ref())?;
        let mmap = unsafe { Mmap::map(&f)? };
        Self::from_mmap(mmap)
    }

    /// Construct from an already-mapped buffer (useful for tests / multi-part).
    pub fn from_mmap(mmap: Mmap) -> Result<Self> {
        let header = Header::parse(&mmap)?;
        let (mimes, _end) = MimeList::parse(&mmap, header.mime_list_pos as usize)?;
        let file_len = mmap.len() as u64;
        Ok(Archive {
            core: Arc::new(ArchiveCore {
                mmap,
                header,
                mimes,
                file_len,
                cluster_cache: Mutex::new(HashMap::new()),
                title_listing: OnceLock::new(),
                content_counts: OnceLock::new(),
            }),
        })
    }

    pub fn header(&self) -> &Header {
        &self.core.header
    }

    pub fn uuid(&self) -> crate::Uuid {
        crate::Uuid::from_bytes(self.core.header.uuid)
    }

    /// Total number of dirents (articles + redirects + reserved).
    pub fn all_entry_count(&self) -> u32 {
        self.core.header.entry_count
    }

    /// Same as [`Archive::all_entry_count`] — included to match libzim naming.
    pub fn entry_count(&self) -> u32 {
        self.core.header.entry_count
    }

    pub fn cluster_count(&self) -> u32 {
        self.core.header.cluster_count
    }

    pub fn has_main_entry(&self) -> bool {
        self.core.header.has_main_page()
    }

    pub fn main_entry(&self) -> Result<Entry> {
        let idx = self.core.header.main_page;
        if idx == NO_MAIN_PAGE {
            return Err(Error::NoMainEntry);
        }
        self.entry_by_url_index(idx)
    }

    pub fn has_checksum(&self) -> bool {
        self.core.header.has_checksum()
    }

    /// The 16-byte trailing MD5 checksum. Returns [`Error::NoChecksum`] if the
    /// archive doesn't have one.
    pub fn checksum(&self) -> Result<[u8; 16]> {
        if !self.has_checksum() {
            return Err(Error::NoChecksum);
        }
        let pos = self.core.header.checksum_pos as usize;
        if pos + 16 > self.core.mmap.len() {
            return Err(Error::Truncated(pos as u64 + 16));
        }
        let mut out = [0u8; 16];
        out.copy_from_slice(&self.core.mmap[pos..pos + 16]);
        Ok(out)
    }

    /// Verify the archive's MD5 checksum.
    pub fn check(&self) -> Result<bool> {
        use md5::{Digest, Md5};
        let pos = self.core.header.checksum_pos as usize;
        if pos == 0 || pos + 16 > self.core.mmap.len() {
            return Err(Error::NoChecksum);
        }
        let mut h = Md5::new();
        // Hash everything before the checksum.
        h.update(&self.core.mmap[..pos]);
        let computed: [u8; 16] = h.finalize().into();
        let stored = self.checksum()?;
        Ok(computed == stored)
    }

    /// Return the full mime-type list (in declaration order).
    pub fn mime_list(&self) -> &MimeList {
        &self.core.mimes
    }

    /// Look up an entry by index in the URL-pointer list (i.e. the canonical
    /// path order).
    pub fn entry_by_url_index(&self, idx: u32) -> Result<Entry> {
        let off = self.url_pointer(idx)?;
        let dirent = Dirent::parse(&self.core.mmap, off as usize)?;
        Ok(Entry {
            archive: self.clone(),
            url_index: idx,
            dirent,
        })
    }

    /// Look up an entry by index in the title-pointer list (title order).
    pub fn entry_by_title_index(&self, idx: u32) -> Result<Entry> {
        let listing = self.title_listing()?;
        let url_index = *listing
            .get(idx as usize)
            .ok_or(Error::BadTitleIndex(idx, listing.len() as u32))?;
        self.entry_by_url_index(url_index)
    }

    /// Number of entries in the title-order listing. In modern archives this
    /// is typically *less* than [`Archive::entry_count`] because only the
    /// content (C) namespace is indexed by title.
    pub fn title_count(&self) -> Result<u32> {
        Ok(self.title_listing()?.len() as u32)
    }

    /// Returns the title-order listing as dirent indices.
    ///
    /// - Legacy archives: read from the in-header u32 array at `title_ptr_pos`.
    /// - Modern archives (v6.2+ where `title_ptr_pos == u64::MAX`): read from
    ///   the `X/listing/titleOrdered/v1` entry as a packed u32 LE array.
    pub fn title_listing(&self) -> Result<Arc<[u32]>> {
        if let Some(cached) = self.core.title_listing.get() {
            return Ok(cached.clone());
        }
        let v: Arc<[u32]> = if self.core.header.title_ptr_pos != u64::MAX {
            let n = self.core.header.entry_count as usize;
            let base = self.core.header.title_ptr_pos as usize;
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                out.push(raw::u32_at(&self.core.mmap, base + i * 4)?);
            }
            out.into()
        } else {
            // Try v1 first (current), then v0 (older modern files).
            let entry = self
                .entry_by_ns_path(NS_INDEX, "listing/titleOrdered/v1")
                .or_else(|_| self.entry_by_ns_path(NS_INDEX, "listing/titleOrdered/v0"))
                .or_else(|_| self.entry_by_ns_path(NS_INDEX, "listing/titleOrdered"))?;
            let item = entry.get_item(true)?;
            let blob = item.get_data()?;
            let bytes = blob.data();
            if !bytes.len().is_multiple_of(4) {
                return Err(Error::Truncated(bytes.len() as u64));
            }
            let mut out = Vec::with_capacity(bytes.len() / 4);
            for chunk in bytes.chunks_exact(4) {
                out.push(u32::from_le_bytes(chunk.try_into().unwrap()));
            }
            out.into()
        };
        let _ = self.core.title_listing.set(v.clone());
        Ok(v)
    }

    /// Modeled after libzim's `Archive::getEntryByPath(path)` — for "new
    /// namespace" archives this looks up `C/<path>`. For legacy archives the
    /// caller must include the namespace prefix as `A/...`.
    pub fn get_entry_by_path(&self, path: &str) -> Result<Entry> {
        if self.core.header.uses_new_namespaces() {
            self.entry_by_ns_path(NS_CONTENT_NEW, path)
        } else if let Some((ns, rest)) = split_legacy_path(path) {
            self.entry_by_ns_path(ns, rest)
        } else {
            // Legacy archive without explicit namespace — try articles ns.
            self.entry_by_ns_path(NS_ARTICLES_LEGACY, path)
        }
    }

    pub fn has_entry_by_path(&self, path: &str) -> bool {
        self.get_entry_by_path(path).is_ok()
    }

    /// Look up by title in the content namespace.
    pub fn get_entry_by_title(&self, title: &str) -> Result<Entry> {
        let ns = if self.core.header.uses_new_namespaces() {
            NS_CONTENT_NEW
        } else {
            NS_ARTICLES_LEGACY
        };
        self.entry_by_ns_title(ns, title)
    }

    pub fn has_entry_by_title(&self, title: &str) -> bool {
        self.get_entry_by_title(title).is_ok()
    }

    /// Count entries in a single namespace by binary-searching for the
    /// namespace boundary in the URL pointer list. O(log n) — much faster
    /// than iterating every dirent when the caller only wants a count.
    pub fn entry_count_in_namespace(&self, ns: u8) -> Result<u32> {
        let n = self.core.header.entry_count;
        // Find first index where namespace >= ns.
        let lo_idx = self.lower_bound_ns(ns, n)?;
        // Find first index where namespace > ns (i.e. >= ns+1).
        let hi_idx = self.lower_bound_ns(ns.saturating_add(1), n)?;
        Ok(hi_idx.saturating_sub(lo_idx))
    }

    /// On-disk byte length of the archive (the mmapped extent). For
    /// multipart archives this is currently the size of the part we've
    /// opened; multipart-aware sizing is a future TODO.
    pub fn file_len(&self) -> u64 {
        self.core.file_len
    }

    /// Walk the content namespace once, counting how many entries are
    /// "articles" (mimetype starts with `text/html`) and how many are
    /// "media" (any other non-redirect item). Redirects don't count
    /// toward either total. Result is cached after the first call.
    pub fn article_and_media_counts(&self) -> Result<(u64, u64)> {
        if let Some(&cached) = self.core.content_counts.get() {
            return Ok(cached);
        }
        let ns = if self.core.header.uses_new_namespaces() {
            NS_CONTENT_NEW
        } else {
            NS_ARTICLES_LEGACY
        };
        let range = self.namespace_range(ns)?;
        let mut articles: u64 = 0;
        let mut media: u64 = 0;
        for idx in range {
            let entry = self.entry_by_url_index(idx)?;
            if entry.is_redirect() {
                continue;
            }
            // Get the mimetype without paying the full Item construction.
            let mime_idx = match entry.dirent() {
                crate::Dirent::Article(a) => a.mimetype,
                crate::Dirent::Redirect(_) => continue,
            };
            let is_article = self
                .core
                .mimes
                .get(mime_idx)
                .map(|m| m.starts_with("text/html"))
                .unwrap_or(false);
            if is_article {
                articles += 1;
            } else {
                media += 1;
            }
        }
        let _ = self.core.content_counts.set((articles, media));
        Ok((articles, media))
    }

    /// Number of "article" entries (text/html items) in the content
    /// namespace. See [`Archive::article_and_media_counts`].
    pub fn article_count(&self) -> Result<u64> {
        Ok(self.article_and_media_counts()?.0)
    }

    /// Number of "media" entries (non-html, non-redirect items) in the
    /// content namespace. See [`Archive::article_and_media_counts`].
    pub fn media_count(&self) -> Result<u64> {
        Ok(self.article_and_media_counts()?.1)
    }

    /// Pick a uniformly-random entry from the content namespace.
    /// Pseudo-random, seeded from the system clock + process id; not
    /// suitable for cryptographic use. Returns [`Error::EntryNotFound`]
    /// if the content namespace is empty.
    pub fn random_content_entry(&self) -> Result<Entry> {
        let ns = if self.core.header.uses_new_namespaces() {
            NS_CONTENT_NEW
        } else {
            NS_ARTICLES_LEGACY
        };
        let range = self.namespace_range(ns)?;
        if range.is_empty() {
            return Err(Error::EntryNotFound);
        }
        let span = range.end - range.start;
        // Cheap PRNG seed: nanos + pid, hashed via xorshift mixer. Adequate
        // for "give me a random article" — not for security.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut x = nanos.wrapping_mul(0x9E3779B97F4A7C15)
            ^ (std::process::id() as u64).wrapping_mul(0xBF58476D1CE4E5B9);
        x ^= x >> 30;
        x = x.wrapping_mul(0xBF58476D1CE4E5B9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94D049BB133111EB);
        x ^= x >> 31;
        let pick = range.start + (x % span as u64) as u32;
        self.entry_by_url_index(pick)
    }

    fn lower_bound_ns(&self, ns: u8, n: u32) -> Result<u32> {
        let mut lo = 0u32;
        let mut hi = n;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = self.url_pointer(mid)?;
            // Read the namespace byte directly without parsing the rest of
            // the dirent — it's at a fixed offset (3) inside every dirent.
            let dirent_ns = raw::u8_at(&self.core.mmap, off as usize + 3)?;
            if dirent_ns < ns {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }

    /// Look up `(namespace, url)` directly via binary search of the URL ptr list.
    pub fn entry_by_ns_path(&self, ns: u8, url: &str) -> Result<Entry> {
        let n = self.core.header.entry_count;
        let mut lo = 0u32;
        let mut hi = n;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let off = self.url_pointer(mid)?;
            let d = Dirent::parse(&self.core.mmap, off as usize)?;
            let cmp = match ns.cmp(&d.namespace()) {
                Ordering::Equal => url.cmp(d.url()),
                other => other,
            };
            match cmp {
                Ordering::Less => hi = mid,
                Ordering::Greater => lo = mid + 1,
                Ordering::Equal => {
                    return Ok(Entry {
                        archive: self.clone(),
                        url_index: mid,
                        dirent: d,
                    });
                }
            }
        }
        Err(Error::EntryNotFound)
    }

    /// Look up `(namespace, title)` directly via binary search of the title
    /// listing. In modern archives the listing only covers the content
    /// namespace, so lookups in M/W/X namespaces fall back to a linear scan.
    pub fn entry_by_ns_title(&self, ns: u8, title: &str) -> Result<Entry> {
        let listing = self.title_listing()?;
        let listing_covers_ns = !listing.is_empty() && {
            let probe = self.entry_by_url_index(listing[0])?.dirent.namespace();
            // Modern listings are entirely C; legacy listings span all ns. We
            // can take the binary-search path only when the listing namespace
            // matches the requested one.
            probe == ns || self.core.header.title_ptr_pos != u64::MAX // legacy: full
        };
        if listing_covers_ns {
            let mut lo = 0usize;
            let mut hi = listing.len();
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let url_idx = listing[mid];
                let d = self.entry_by_url_index(url_idx)?.dirent;
                let cmp = match ns.cmp(&d.namespace()) {
                    Ordering::Equal => title.cmp(d.title()),
                    other => other,
                };
                match cmp {
                    Ordering::Less => hi = mid,
                    Ordering::Greater => lo = mid + 1,
                    Ordering::Equal => {
                        return Ok(Entry {
                            archive: self.clone(),
                            url_index: url_idx,
                            dirent: d,
                        });
                    }
                }
            }
        }
        // Linear fallback for non-content namespaces in modern archives.
        let n = self.core.header.entry_count;
        for i in 0..n {
            let off = self.url_pointer(i)?;
            let d = Dirent::parse(&self.core.mmap, off as usize)?;
            if d.namespace() == ns && d.title() == title {
                return Ok(Entry {
                    archive: self.clone(),
                    url_index: i,
                    dirent: d,
                });
            }
        }
        Err(Error::EntryNotFound)
    }

    /// Iterate every entry in URL-pointer (path) order.
    pub fn iter_by_path(&self) -> EntryIter {
        EntryIter {
            archive: self.clone(),
            kind: IterKind::ByPath,
            i: 0,
            n: self.core.header.entry_count,
        }
    }

    /// Iterate every entry in title order. In modern archives this only
    /// yields C-namespace entries (matching libzim's `iterByTitle()`).
    pub fn iter_by_title(&self) -> EntryIter {
        let n = self.title_listing().map(|v| v.len() as u32).unwrap_or(0);
        EntryIter {
            archive: self.clone(),
            kind: IterKind::ByTitle,
            i: 0,
            n,
        }
    }

    /// Get a metadata value by name (e.g. "Title", "Description", "Language").
    /// Looks the entry up in the `M` namespace and returns its content as bytes.
    pub fn get_metadata(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self.entry_by_ns_path(NS_METADATA, name)?;
        let item = entry.get_item(true)?;
        Ok(item.get_data()?.to_vec())
    }

    /// List all metadata keys (entries in the `M` namespace).
    pub fn get_metadata_keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let n = self.core.header.entry_count;
        for i in 0..n {
            let Ok(off) = self.url_pointer(i) else {
                continue;
            };
            let Ok(d) = Dirent::parse(&self.core.mmap, off as usize) else {
                continue;
            };
            if d.namespace() == NS_METADATA {
                out.push(d.url().to_string());
            }
        }
        out
    }

    // ============================================================
    // ---- Ergonomic convenience API (idiomatic Rust extras) ----
    // ============================================================
    //
    // Everything below is layered on top of the libzim-mirror methods and
    // designed around common read-only workflows: read one article as text,
    // enumerate all non-redirect content, iterate a path-prefix range,
    // process every blob in parallel, or get a one-shot snapshot of the
    // archive's header/counts. These shapes weren't possible (or were
    // much more verbose) in the original C++ libzim.

    /// One-liner: fetch the decompressed bytes of the entry at `path`,
    /// following any redirects. Equivalent to
    /// `archive.get_entry_by_path(path)?.get_item(true)?.get_data()?.to_vec()`.
    ///
    /// ```no_run
    /// # use zimru::Archive;
    /// let archive = Archive::open("wiki.zim")?;
    /// let bytes = archive.get_bytes("Albert_Einstein")?;
    /// # Ok::<(), zimru::Error>(())
    /// ```
    pub fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self.get_entry_by_path(path)?;
        let item = entry.get_item(true)?;
        Ok(item.get_data()?.to_vec())
    }

    /// One-liner: fetch the entry at `path` as a UTF-8 [`String`]. Fails
    /// with [`Error::BadUtf8`] if the content isn't valid UTF-8.
    pub fn get_text(&self, path: &str) -> Result<String> {
        let blob = self.get_bytes(path)?;
        String::from_utf8(blob).map_err(|_| Error::BadUtf8(0))
    }

    /// One-liner: fetch the [`Item`] for the entry at `path`, following
    /// any redirects.
    pub fn get_item(&self, path: &str) -> Result<Item> {
        self.get_entry_by_path(path)?.get_item(true)
    }

    /// Resolve the archive's main entry directly to an [`Item`], following
    /// the `W/mainPage` redirect if present.
    pub fn main_item(&self) -> Result<Item> {
        self.main_entry()?.get_item(true)
    }

    /// Just the path of the resolved main page (e.g. `"index"`), skipping
    /// the redirect chain. Useful for logging / info output.
    pub fn main_path(&self) -> Result<String> {
        let mut cur = self.main_entry()?;
        let mut depth = 0;
        while cur.is_redirect() {
            depth += 1;
            if depth > 16 {
                return Err(Error::RedirectLoop);
            }
            cur = cur.get_redirect_entry()?;
        }
        Ok(cur.path().to_string())
    }

    /// Read a metadata entry as a UTF-8 string. Shorthand for
    /// `String::from_utf8(archive.get_metadata(name)?)`.
    pub fn metadata_str(&self, name: &str) -> Result<String> {
        let bytes = self.get_metadata(name)?;
        String::from_utf8(bytes).map_err(|_| Error::BadUtf8(0))
    }

    /// Check whether a metadata key exists without reading its value.
    pub fn has_metadata(&self, name: &str) -> bool {
        self.entry_by_ns_path(NS_METADATA, name).is_ok()
    }

    /// The `(start, end)` URL-pointer range occupied by `namespace`. Entries
    /// `start..end` in URL-pointer order are exactly the ones in `namespace`.
    /// Runs in O(log n) via binary search on the namespace byte.
    pub fn namespace_range(&self, ns: u8) -> Result<std::ops::Range<u32>> {
        let n = self.core.header.entry_count;
        let start = self.lower_bound_ns(ns, n)?;
        let end = self.lower_bound_ns(ns.saturating_add(1), n)?;
        Ok(start..end)
    }

    /// Iterate only the non-redirect entries in path order.
    pub fn articles(&self) -> impl Iterator<Item = Result<Entry>> + '_ {
        self.iter_by_path().filter(|r| match r {
            Ok(e) => !e.is_redirect(),
            Err(_) => true, // surface errors up
        })
    }

    /// Iterate only the redirect entries in path order.
    pub fn redirects(&self) -> impl Iterator<Item = Result<Entry>> + '_ {
        self.iter_by_path().filter(|r| match r {
            Ok(e) => e.is_redirect(),
            Err(_) => true,
        })
    }

    /// Iterate only the content-namespace (`C`) entries in path order,
    /// skipping redirects. The canonical "every article" loop.
    ///
    /// ```no_run
    /// # use zimru::Archive;
    /// let archive = Archive::open("wiki.zim")?;
    /// for entry in archive.content_entries() {
    ///     let e = entry?;
    ///     println!("{}", e.path());
    /// }
    /// # Ok::<(), zimru::Error>(())
    /// ```
    pub fn content_entries(&self) -> EntryIter {
        let ns = if self.core.header.uses_new_namespaces() {
            NS_CONTENT_NEW
        } else {
            NS_ARTICLES_LEGACY
        };
        let range = self.namespace_range(ns).unwrap_or(0..0);
        EntryIter {
            archive: self.clone(),
            kind: IterKind::ByPath,
            i: range.start,
            n: range.end,
        }
    }

    /// Iterate every entry whose (namespace, url) starts with a given
    /// prefix. Binary-searches for the start, then walks forward while the
    /// prefix still matches — O(log n + k) where k is the number of matches.
    ///
    /// Common use: "all pages under `images/`" or "every X/listing/* entry".
    ///
    /// ```no_run
    /// # use zimru::Archive;
    /// let archive = Archive::open("wiki.zim")?;
    /// for entry in archive.by_prefix(b'X', "listing/") {
    ///     let e = entry?;
    ///     println!("{} → {}", e.path(), e.title());
    /// }
    /// # Ok::<(), zimru::Error>(())
    /// ```
    pub fn by_prefix<'a>(
        &'a self,
        ns: u8,
        prefix: &'a str,
    ) -> impl Iterator<Item = Result<Entry>> + 'a {
        let n = self.core.header.entry_count;
        // First index whose (ns, url) is >= (ns, prefix).
        let start = {
            let mut lo = 0u32;
            let mut hi = n;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let off = self.url_pointer(mid).unwrap_or(0);
                let d = Dirent::parse(&self.core.mmap, off as usize)
                    .ok()
                    .map(|d| (d.namespace(), d.url().to_string()));
                let cmp = match d {
                    Some((dns, durl)) => dns.cmp(&ns).then_with(|| durl.as_str().cmp(prefix)),
                    None => Ordering::Greater,
                };
                match cmp {
                    Ordering::Less => lo = mid + 1,
                    _ => hi = mid,
                }
            }
            lo
        };
        PrefixIter {
            archive: self.clone(),
            ns,
            prefix: prefix.to_string(),
            i: start,
            n,
        }
    }

    /// Rayon-aware parallel iterator over every entry in URL-pointer order.
    ///
    /// Useful for embarrassingly-parallel scans that touch dirents only
    /// (e.g. counting redirects by namespace). When the inner closure needs
    /// blob content, prefer [`Archive::par_clusters`] instead — it
    /// decompresses each cluster exactly once, which is usually much faster.
    pub fn par_iter_by_path(&self) -> impl ParallelIterator<Item = Result<Entry>> + '_ {
        let n = self.core.header.entry_count;
        (0..n)
            .into_par_iter()
            .map(move |i| self.entry_by_url_index(i))
    }

    /// Run a closure over every decompressed cluster in parallel. Workers
    /// bypass the shared cluster cache, so memory usage stays bounded at
    /// roughly `num_threads × largest_cluster_size`. Used internally by
    /// `zimcheck -A` to hit 8× upstream throughput; exposed here so library
    /// users can build their own parallel content scans (e.g. full-text
    /// indexers, dedup scanners, stats collectors).
    ///
    /// The closure receives `(cluster_index, cluster)` and returns a `T`
    /// per cluster. Results come back in cluster-index order.
    pub fn par_clusters<T, F>(&self, f: F) -> Result<Vec<T>>
    where
        T: Send,
        F: Fn(u32, &Cluster) -> T + Sync + Send,
    {
        let n = self.core.header.cluster_count;
        let results: Vec<Result<T>> = (0..n)
            .into_par_iter()
            .map(|idx| {
                let c = self.cluster_uncached(idx)?;
                Ok(f(idx, &c))
            })
            .collect();
        results.into_iter().collect()
    }

    /// Compact summary of the archive's header + aggregate counts, handy
    /// for `--info` style commands and logging. Single allocation + one
    /// optional main-entry lookup.
    pub fn summary(&self) -> Summary {
        let h = &self.core.header;
        Summary {
            uuid: crate::Uuid::from_bytes(h.uuid),
            major_version: h.major_version,
            minor_version: h.minor_version,
            uses_new_namespaces: h.uses_new_namespaces(),
            entry_count: h.entry_count,
            content_entry_count: self
                .namespace_range(if h.uses_new_namespaces() {
                    NS_CONTENT_NEW
                } else {
                    NS_ARTICLES_LEGACY
                })
                .map(|r| r.end - r.start)
                .unwrap_or(0),
            cluster_count: h.cluster_count,
            mime_type_count: self.core.mimes.len(),
            has_main_entry: h.has_main_page(),
            main_path: self.main_path().ok(),
            has_checksum: h.has_checksum(),
        }
    }

    // ---- internal helpers ----

    fn url_pointer(&self, idx: u32) -> Result<u64> {
        let n = self.core.header.entry_count;
        if idx >= n {
            return Err(Error::BadUrlIndex(idx, n));
        }
        let off = self.core.header.url_ptr_pos as usize + idx as usize * 8;
        raw::u64_at(&self.core.mmap, off)
    }

    fn cluster_pointer(&self, idx: u32) -> Result<u64> {
        let n = self.core.header.cluster_count;
        if idx >= n {
            return Err(Error::BadClusterIndex(idx, n));
        }
        let off = self.core.header.cluster_ptr_pos as usize + idx as usize * 8;
        raw::u64_at(&self.core.mmap, off)
    }

    /// End-of-cluster-region: where the last cluster ends. We use the checksum
    /// position when present, otherwise the file length.
    fn cluster_region_end(&self) -> u64 {
        if self.core.header.has_checksum() {
            self.core.header.checksum_pos
        } else {
            self.core.file_len
        }
    }

    /// Public access to a decoded cluster by its index. The cluster is cached
    /// inside the archive so repeat calls for the same index are free.
    pub fn cluster(&self, idx: u32) -> Result<Arc<Cluster>> {
        self.load_cluster(idx)
    }

    /// On-disk byte range occupied by cluster `idx`, including its info
    /// byte. The returned range's length is the compressed size the cluster
    /// takes up in the file.
    pub fn cluster_byte_range(&self, idx: u32) -> Result<std::ops::Range<u64>> {
        let start = self.cluster_pointer(idx)?;
        let end = if idx + 1 < self.core.header.cluster_count {
            self.cluster_pointer(idx + 1)?
        } else {
            self.cluster_region_end()
        };
        if end < start {
            return Err(Error::Truncated(end));
        }
        Ok(start..end)
    }

    /// Decompress a cluster *without* touching the cache. Useful for parallel
    /// scans where each cluster is read exactly once and caching would only
    /// waste memory + add Mutex contention.
    pub fn cluster_uncached(&self, idx: u32) -> Result<Cluster> {
        let start = self.cluster_pointer(idx)? as usize;
        let end = if idx + 1 < self.core.header.cluster_count {
            self.cluster_pointer(idx + 1)? as usize
        } else {
            self.cluster_region_end() as usize
        };
        if end < start || end > self.core.mmap.len() {
            return Err(Error::Truncated(end as u64));
        }
        Cluster::parse(&self.core.mmap[start..end])
    }

    fn load_cluster(&self, idx: u32) -> Result<Arc<Cluster>> {
        if let Some(c) = self.core.cluster_cache.lock().unwrap().get(&idx).cloned() {
            return Ok(c);
        }
        let start = self.cluster_pointer(idx)? as usize;
        let end = if idx + 1 < self.core.header.cluster_count {
            self.cluster_pointer(idx + 1)? as usize
        } else {
            self.cluster_region_end() as usize
        };
        if end < start || end > self.core.mmap.len() {
            return Err(Error::Truncated(end as u64));
        }
        let raw = &self.core.mmap[start..end];
        let cluster = Arc::new(Cluster::parse(raw)?);
        self.core
            .cluster_cache
            .lock()
            .unwrap()
            .insert(idx, cluster.clone());
        Ok(cluster)
    }
}

fn split_legacy_path(path: &str) -> Option<(u8, &str)> {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b'/' && bytes[0].is_ascii_alphabetic() {
        Some((bytes[0], &path[2..]))
    } else {
        None
    }
}

// ---- Entry / Item / Blob ----

#[derive(Clone, Debug)]
pub struct Entry {
    archive: Archive,
    url_index: u32,
    dirent: Dirent,
}

impl Entry {
    pub fn path(&self) -> &str {
        self.dirent.url()
    }

    pub fn title(&self) -> &str {
        self.dirent.title()
    }

    pub fn namespace(&self) -> u8 {
        self.dirent.namespace()
    }

    pub fn is_redirect(&self) -> bool {
        self.dirent.is_redirect()
    }

    pub fn index(&self) -> u32 {
        self.url_index
    }

    /// Borrow the raw [`Dirent`] backing this entry. Useful for callers that
    /// want to inspect the article's cluster/blob/mimetype without paying
    /// for the [`Item`] resolution path.
    pub fn dirent(&self) -> &Dirent {
        &self.dirent
    }

    /// Shortcut: `get_item(true)`, matching the most common workflow where
    /// users want the item and don't care about redirects.
    pub fn item(&self) -> Result<Item> {
        self.get_item(true)
    }

    /// Walk redirects to the final content entry (no-op if non-redirect).
    pub fn resolve(&self) -> Result<Entry> {
        let mut cur = self.clone();
        let mut hops = 0;
        while cur.is_redirect() {
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(Error::RedirectLoop);
            }
            cur = cur.get_redirect_entry()?;
        }
        Ok(cur)
    }

    /// libzim parity: returns the [`Item`] for this entry. If `follow` is
    /// true, redirect chains are resolved transparently; otherwise calling
    /// this on a redirect returns [`Error::NotAnItem`].
    pub fn get_item(&self, follow: bool) -> Result<Item> {
        let mut entry = self.clone();
        let mut hops = 0;
        while entry.is_redirect() {
            if !follow {
                return Err(Error::NotAnItem);
            }
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(Error::RedirectLoop);
            }
            entry = entry.get_redirect_entry()?;
        }
        let article = match entry.dirent {
            Dirent::Article(a) => a,
            Dirent::Redirect(_) => unreachable!(),
        };
        Ok(Item {
            archive: entry.archive,
            url_index: entry.url_index,
            article,
        })
    }

    /// Resolve a single redirect step. Returns Self on a non-redirect.
    pub fn get_redirect_entry(&self) -> Result<Entry> {
        if let Dirent::Redirect(r) = &self.dirent {
            self.archive.entry_by_url_index(r.redirect_index)
        } else {
            Ok(self.clone())
        }
    }
}

impl std::fmt::Display for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}/{} \"{}\"",
            if self.is_redirect() { "↪ " } else { "" },
            char::from(self.namespace()),
            self.path(),
            self.title()
        )
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    archive: Archive,
    url_index: u32,
    article: ArticleEntry,
}

impl Item {
    pub fn path(&self) -> &str {
        &self.article.url
    }

    pub fn title(&self) -> &str {
        &self.article.title
    }

    pub fn mimetype(&self) -> &str {
        self.archive
            .core
            .mimes
            .get(self.article.mimetype)
            .unwrap_or("application/octet-stream")
    }

    pub fn index(&self) -> u32 {
        self.url_index
    }

    pub fn cluster_index(&self) -> u32 {
        self.article.cluster
    }

    pub fn blob_index(&self) -> u32 {
        self.article.blob
    }

    /// Total decompressed size of the underlying blob in bytes.
    pub fn size(&self) -> Result<u64> {
        let blob = self.get_data()?;
        Ok(blob.size() as u64)
    }

    // ---- ergonomic content accessors ----

    /// Fetch the item's decompressed bytes. Shortcut for
    /// `item.get_data()?.to_vec()`.
    pub fn bytes(&self) -> Result<Vec<u8>> {
        Ok(self.get_data()?.to_vec())
    }

    /// Fetch the item's content as a UTF-8 [`String`]. Returns
    /// [`Error::BadUtf8`] if the content is not valid UTF-8.
    pub fn text(&self) -> Result<String> {
        let bytes = self.get_data()?;
        std::str::from_utf8(bytes.data())
            .map(str::to_owned)
            .map_err(|_| Error::BadUtf8(0))
    }

    /// True if the mimetype is `text/html` (with any charset parameter).
    pub fn is_html(&self) -> bool {
        let m = self.mimetype();
        m.starts_with("text/html") || m.starts_with("application/xhtml")
    }

    /// True if the mimetype starts with `text/`.
    pub fn is_text(&self) -> bool {
        self.mimetype().starts_with("text/")
    }

    /// True if the mimetype starts with `image/`.
    pub fn is_image(&self) -> bool {
        self.mimetype().starts_with("image/")
    }

    /// Fetch the blob bytes (decompressing the cluster if needed).
    pub fn get_data(&self) -> Result<Blob> {
        let cluster = self.archive.load_cluster(self.article.cluster)?;
        let range = cluster.blob_range(self.article.blob).map_err(|e| match e {
            Error::BadBlobIndex { blob, count, .. } => Error::BadBlobIndex {
                cluster: self.article.cluster,
                blob,
                count,
            },
            other => other,
        })?;
        Ok(Blob {
            payload: cluster.payload().clone(),
            range,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Blob {
    payload: Arc<[u8]>,
    range: std::ops::Range<usize>,
}

impl Blob {
    pub fn data(&self) -> &[u8] {
        &self.payload[self.range.clone()]
    }

    pub fn size(&self) -> usize {
        self.range.len()
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.data().to_vec()
    }

    /// Borrow the blob's bytes as `&str` if they're valid UTF-8. Zero-copy.
    pub fn as_str(&self) -> Result<&str> {
        std::str::from_utf8(self.data()).map_err(|_| Error::BadUtf8(0))
    }

    /// A `std::io::Read` cursor over the blob that starts at byte 0.
    /// Useful for passing blob content to streaming parsers (e.g. gzip
    /// inflaters, serde_json readers) without materializing a `Vec<u8>`.
    pub fn reader(&self) -> std::io::Cursor<&[u8]> {
        std::io::Cursor::new(self.data())
    }
}

impl std::ops::Deref for Blob {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.data()
    }
}

impl AsRef<[u8]> for Blob {
    fn as_ref(&self) -> &[u8] {
        self.data()
    }
}

#[derive(Clone, Copy, Debug)]
enum IterKind {
    ByPath,
    ByTitle,
}

pub struct EntryIter {
    archive: Archive,
    kind: IterKind,
    i: u32,
    n: u32,
}

impl Iterator for EntryIter {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.i >= self.n {
            return None;
        }
        let idx = self.i;
        self.i += 1;
        let res = match self.kind {
            IterKind::ByPath => self.archive.entry_by_url_index(idx),
            IterKind::ByTitle => self.archive.entry_by_title_index(idx),
        };
        Some(res)
    }
}

impl ExactSizeIterator for EntryIter {
    fn len(&self) -> usize {
        (self.n - self.i) as usize
    }
}

/// Iterator returned by [`Archive::by_prefix`] — yields every dirent whose
/// `(namespace, url)` starts with the given prefix, in URL-pointer order.
pub struct PrefixIter {
    archive: Archive,
    ns: u8,
    prefix: String,
    i: u32,
    n: u32,
}

impl Iterator for PrefixIter {
    type Item = Result<Entry>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.i >= self.n {
            return None;
        }
        let idx = self.i;
        self.i += 1;
        let entry = match self.archive.entry_by_url_index(idx) {
            Ok(e) => e,
            Err(e) => return Some(Err(e)),
        };
        // First (namespace, url) outside the prefix range ends iteration.
        if entry.namespace() != self.ns || !entry.path().starts_with(&self.prefix) {
            self.i = self.n;
            return None;
        }
        Some(Ok(entry))
    }
}

/// Snapshot of an archive's header + aggregate counts, returned by
/// [`Archive::summary`].
#[derive(Debug, Clone)]
pub struct Summary {
    pub uuid: crate::Uuid,
    pub major_version: u16,
    pub minor_version: u16,
    pub uses_new_namespaces: bool,
    pub entry_count: u32,
    pub content_entry_count: u32,
    pub cluster_count: u32,
    pub mime_type_count: usize,
    pub has_main_entry: bool,
    pub main_path: Option<String>,
    pub has_checksum: bool,
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "uuid            {}", self.uuid)?;
        writeln!(
            f,
            "version         {}.{}{}",
            self.major_version,
            self.minor_version,
            if self.uses_new_namespaces {
                " (new namespaces)"
            } else {
                " (legacy)"
            }
        )?;
        writeln!(
            f,
            "entries         {} total ({} content)",
            self.entry_count, self.content_entry_count
        )?;
        writeln!(f, "clusters        {}", self.cluster_count)?;
        writeln!(f, "mime types      {}", self.mime_type_count)?;
        if let Some(p) = &self.main_path {
            writeln!(f, "main page       {}", p)?;
        }
        writeln!(
            f,
            "checksum        {}",
            if self.has_checksum { "yes" } else { "no" }
        )?;
        Ok(())
    }
}
