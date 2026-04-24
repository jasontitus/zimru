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
            }),
        })
    }

    pub fn header(&self) -> &Header {
        &self.core.header
    }

    pub fn uuid(&self) -> [u8; 16] {
        self.core.header.uuid
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
            probe == ns
                || self.core.header.title_ptr_pos != u64::MAX // legacy: full
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
            let Ok(off) = self.url_pointer(i) else { continue };
            let Ok(d) = Dirent::parse(&self.core.mmap, off as usize) else {
                continue;
            };
            if d.namespace() == NS_METADATA {
                out.push(d.url().to_string());
            }
        }
        out
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

    pub fn to_vec(&self) -> Vec<u8> {
        self.data().to_vec()
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
