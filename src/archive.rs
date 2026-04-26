//! High-level [`Archive`] reader.
//!
//! The API mirrors libzim's user-facing surface (Archive / Entry / Item / Blob)
//! using snake_case as is idiomatic in Rust. Methods are named to match the
//! C++ class methods one-for-one so swapping out a Rust libzim binding for
//! `zimru` should mostly require only a `use` change.

use std::cmp::Ordering;
use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use lru::LruCache;
use memmap2::{Advice, Mmap};
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

/// Default cluster-cache byte budget (sum of decompressed payload bytes
/// held resident at most). Sized for phone-class hosts: 64 MB is enough
/// to keep ~30 typical 2 MB clusters cached for hot-article reuse, but
/// won't dominate a 4 GB-RAM device. Override per-archive with
/// [`Archive::set_cluster_cache_max_bytes`] — bulk-iteration tools that
/// pass over the whole archive once can size up; memory-constrained
/// embedded callers can size down.
pub const DEFAULT_CLUSTER_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Byte-budget LRU cache for decompressed clusters. Wraps `LruCache`
/// (count-based) and adds eviction-by-bytes on every insert: clusters
/// are tracked by their decompressed payload length, and the LRU tail
/// is evicted until the resident-byte total is under budget. Keeps the
/// most-recently-used cluster pinned even if it exceeds the budget on
/// its own (otherwise a single oversized cluster would lock the cache
/// into a permanent miss).
#[derive(Debug)]
struct ClusterByteCache {
    inner: LruCache<u32, Arc<Cluster>>,
    max_bytes: usize,
    current_bytes: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl ClusterByteCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            inner: LruCache::unbounded(),
            max_bytes,
            current_bytes: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    fn get(&mut self, idx: u32) -> Option<Arc<Cluster>> {
        match self.inner.get(&idx) {
            Some(c) => {
                self.hits += 1;
                Some(c.clone())
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    fn put(&mut self, idx: u32, cluster: Arc<Cluster>) {
        let size = cluster.payload().len();
        if let Some(prev) = self.inner.put(idx, cluster) {
            self.current_bytes = self.current_bytes.saturating_sub(prev.payload().len());
        }
        self.current_bytes = self.current_bytes.saturating_add(size);
        self.evict_to_budget();
    }

    fn resize(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
        self.evict_to_budget();
    }

    fn evict_to_budget(&mut self) {
        while self.current_bytes > self.max_bytes && self.inner.len() > 1 {
            if let Some((_, evicted)) = self.inner.pop_lru() {
                self.current_bytes = self
                    .current_bytes
                    .saturating_sub(evicted.payload().len());
                self.evictions += 1;
            } else {
                break;
            }
        }
    }

    fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    fn snapshot(&self) -> ClusterCacheStats {
        ClusterCacheStats {
            max_bytes: self.max_bytes as u64,
            current_bytes: self.current_bytes as u64,
            entries: self.inner.len() as u64,
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
        }
    }
}

/// Snapshot of the cluster cache's resident-byte budget, current
/// occupancy, and lifetime hit/miss/eviction counters. Returned by
/// [`Archive::cluster_cache_stats`]. Counters are monotonic; resetting
/// happens only when the archive is closed.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClusterCacheStats {
    pub max_bytes: u64,
    pub current_bytes: u64,
    pub entries: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

#[derive(Debug)]
struct ArchiveCore {
    mmap: Mmap,
    header: Header,
    mimes: MimeList,
    /// File length, used as the upper bound when sizing the trailing cluster.
    file_len: u64,
    cluster_cache: Mutex<ClusterByteCache>,
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
                cluster_cache: Mutex::new(ClusterByteCache::new(
                    DEFAULT_CLUSTER_CACHE_MAX_BYTES,
                )),
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

    /// Count "articles" (mimetype starts with `text/html`) and "media"
    /// (any other non-redirect item) for catalog purposes. Redirects
    /// don't count toward either total. Result is cached.
    ///
    /// On modern (new-namespace) archives all user content lives under
    /// `C/`, so this walks just that range. On legacy archives the user
    /// content is spread across `A` (articles), `I` (images/files),
    /// `J` (image text), `-` (layout: CSS/JS/fonts), and a few rarer
    /// per-article meta namespaces (`B`, `H`, `U`, `V`, `W`); the only
    /// namespaces explicitly *not* counted are `M` (metadata) and `X`
    /// (search indexes). The walk skips both regardless of namespace
    /// scheme so it stays robust to unusual legacy shapes.
    pub fn article_and_media_counts(&self) -> Result<(u64, u64)> {
        if let Some(&cached) = self.core.content_counts.get() {
            return Ok(cached);
        }
        let new_scheme = self.core.header.uses_new_namespaces();
        let range: Box<dyn Iterator<Item = u32>> = if new_scheme {
            Box::new(self.namespace_range(NS_CONTENT_NEW)?)
        } else {
            Box::new(0..self.all_entry_count())
        };
        let mut articles: u64 = 0;
        let mut media: u64 = 0;
        for idx in range {
            let entry = self.entry_by_url_index(idx)?;
            if !new_scheme {
                let ns = entry.namespace();
                if ns == NS_METADATA || ns == NS_INDEX {
                    continue;
                }
            }
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

    /// Pick a uniformly-random entry suitable for serving as a user URL.
    /// Pseudo-random, seeded from the system clock + process id; not
    /// suitable for cryptographic use.
    ///
    /// On modern (new-namespace) archives all user content lives under
    /// `C/`, so this picks uniformly from that range. On legacy archives
    /// user content is spread across many namespaces (`A/I/J/-/B/H/U/V/W`
    /// — Kiwix OSM builds put vector tile data under `I/`); this picks
    /// uniformly from the whole entry space then rejects the metadata
    /// (`M/`) and search-index (`X/`) namespaces, retrying until a
    /// user-namespace entry is found. Probabilistic; converges in O(1)
    /// expected probes because `M/X` are typically << 1 % of entries on
    /// real archives.
    ///
    /// Returns [`Error::EntryNotFound`] when the archive is empty or
    /// every entry is in a non-user namespace.
    pub fn random_content_entry(&self) -> Result<Entry> {
        let mut x = pseudo_random_seed();
        if self.core.header.uses_new_namespaces() {
            let range = self.namespace_range(NS_CONTENT_NEW)?;
            if range.is_empty() {
                return Err(Error::EntryNotFound);
            }
            let span = range.end - range.start;
            let pick = range.start + (x % span as u64) as u32;
            return self.entry_by_url_index(pick);
        }
        // Legacy: walk the full entry space, skip metadata and index
        // namespaces. The mixer's avalanche means each retry probes a
        // different bucket, so even on adversarial archives where M/X
        // occupy a high fraction we converge in O(N) worst case.
        let n = self.core.header.entry_count;
        if n == 0 {
            return Err(Error::EntryNotFound);
        }
        for _ in 0..n {
            let pick = (x % n as u64) as u32;
            let off = self.url_pointer(pick)? as usize;
            let ns_byte = raw::u8_at(&self.core.mmap, off + 3)?;
            if ns_byte != NS_METADATA && ns_byte != NS_INDEX {
                return self.entry_by_url_index(pick);
            }
            // Advance the PRNG state for the next attempt.
            x ^= x >> 30;
            x = x.wrapping_mul(0xBF58476D1CE4E5B9);
            x ^= x >> 27;
        }
        Err(Error::EntryNotFound)
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
            // Probe with `key_at` (no allocation) — only materialize the
            // full dirent on the matched leaf.
            let (d_ns, d_url) = Dirent::key_at(&self.core.mmap, off as usize)?;
            let cmp = match ns.cmp(&d_ns) {
                Ordering::Equal => url.cmp(d_url),
                other => other,
            };
            match cmp {
                Ordering::Less => hi = mid,
                Ordering::Greater => lo = mid + 1,
                Ordering::Equal => {
                    let d = Dirent::parse(&self.core.mmap, off as usize)?;
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
                let off = self.url_pointer(url_idx)?;
                let (d_ns, d_title) =
                    Dirent::title_key_at(&self.core.mmap, off as usize)?;
                let cmp = match ns.cmp(&d_ns) {
                    Ordering::Equal => title.cmp(d_title),
                    other => other,
                };
                match cmp {
                    Ordering::Less => hi = mid,
                    Ordering::Greater => lo = mid + 1,
                    Ordering::Equal => {
                        let d = Dirent::parse(&self.core.mmap, off as usize)?;
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

    /// Half-open `[lo, hi)` range of *title-order* indices whose entries are
    /// in namespace `ns` and whose title begins with `prefix`. Returns
    /// `lo == hi` (an empty range) when nothing matches.
    ///
    /// Designed to back `SuggestionSearcher` fallbacks for ZIMs that lack a
    /// Xapian title index: callers iterate `lo..hi` and call
    /// [`Archive::entry_by_title_index`] on each to materialize entries.
    /// Two binary searches over [`Archive::title_listing`] using
    /// [`Dirent::title_key_at`] (no allocation per probe) — `O(log N)`
    /// regardless of prefix length or match count.
    ///
    /// On modern (new-namespace) archives the title listing only covers
    /// the `C/` namespace, so passing any other namespace yields an
    /// empty range. Legacy archives' listings span every namespace.
    pub fn title_prefix_range(&self, ns: u8, prefix: &str) -> Result<std::ops::Range<u32>> {
        let listing = self.title_listing()?;
        let lo = self.lower_bound_in_title_listing(&listing, ns, prefix.as_bytes())?;
        // The first key strictly greater than every UTF-8 string starting
        // with `prefix` is `prefix` followed by `0xFF` — `0xFF` never appears
        // in valid UTF-8 so this is a safe sentinel.
        let mut succ = Vec::with_capacity(prefix.len() + 1);
        succ.extend_from_slice(prefix.as_bytes());
        succ.push(0xFF);
        let hi = self.lower_bound_in_title_listing(&listing, ns, &succ)?;
        Ok(lo as u32..hi as u32)
    }

    /// First index in `listing` where the dirent's `(namespace, title)` is
    /// `>= (ns, key)`. Helper for [`Archive::title_prefix_range`].
    fn lower_bound_in_title_listing(
        &self,
        listing: &[u32],
        ns: u8,
        key: &[u8],
    ) -> Result<usize> {
        let mut lo = 0usize;
        let mut hi = listing.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let url_idx = listing[mid];
            let off = self.url_pointer(url_idx)?;
            let (d_ns, d_title) = Dirent::title_key_at(&self.core.mmap, off as usize)?;
            let cmp = match d_ns.cmp(&ns) {
                Ordering::Equal => d_title.as_bytes().cmp(key),
                other => other,
            };
            if cmp.is_lt() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
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

    /// Enumerate every cover/thumbnail illustration recorded in the
    /// archive's metadata. ZIM stores these as `M/Illustration_<W>x<H>@<scale>`
    /// entries; this helper parses every such key and returns the
    /// `(width, height, scale)` tuples sorted ascending. Keys that don't
    /// match the pattern are silently skipped — callers get only valid
    /// illustration descriptors.
    pub fn illustrations(&self) -> Vec<(u32, u32, u32)> {
        let mut out: Vec<(u32, u32, u32)> = self
            .get_metadata_keys()
            .into_iter()
            .filter_map(|k| parse_illustration_key(&k))
            .collect();
        out.sort();
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

    /// Direct-access info for a single blob: where its bytes physically
    /// live in the on-disk ZIM file, when the surrounding cluster is
    /// stored uncompressed (compression-id 0 or 1). For compressed
    /// clusters, returns `is_direct = false` and zeroes for offset/size.
    ///
    /// Useful for handing fulltext/Xapian indexes (always stored
    /// uncompressed by convention) to libxapian via `Database(int fd)`
    /// + `lseek` without copying any bytes.
    pub fn blob_direct_access(&self, cluster_idx: u32, blob_idx: u32) -> Result<DirectAccess> {
        let cluster_range = self.cluster_byte_range(cluster_idx)?;
        let cluster = self.cluster(cluster_idx)?;
        match cluster.compression() {
            crate::Compression::None => {
                let r = cluster.blob_range(blob_idx)?;
                let len = r.end - r.start;
                // The cluster's payload starts immediately after the
                // 1-byte info byte at the start of the on-disk cluster.
                Ok(DirectAccess {
                    is_direct: true,
                    file_offset: cluster_range.start + 1 + r.start as u64,
                    size: len as u64,
                })
            }
            _ => Ok(DirectAccess {
                is_direct: false,
                file_offset: 0,
                size: 0,
            }),
        }
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
        if let Some(c) = self.core.cluster_cache.lock().unwrap().get(idx) {
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
            .put(idx, cluster.clone());
        Ok(cluster)
    }

    /// Reconfigure the cluster cache's byte budget. The cache holds
    /// decompressed clusters; `max_bytes` is the upper bound on the
    /// summed payload sizes resident at once. Default is
    /// [`DEFAULT_CLUSTER_CACHE_MAX_BYTES`]. Existing entries that push
    /// the cache over the new budget are evicted in LRU order; a
    /// single most-recently-used cluster larger than the budget is
    /// kept pinned to avoid permanent misses.
    pub fn set_cluster_cache_max_bytes(&self, max_bytes: usize) {
        self.core
            .cluster_cache
            .lock()
            .unwrap()
            .resize(max_bytes);
    }

    /// Current cluster-cache byte budget. See
    /// [`Archive::set_cluster_cache_max_bytes`].
    pub fn cluster_cache_max_bytes(&self) -> usize {
        self.core.cluster_cache.lock().unwrap().max_bytes()
    }

    /// Snapshot of cluster-cache occupancy + lifetime counters. Useful
    /// for verifying that the cache is doing its job (hit:miss ratio
    /// for hot-article workloads), tuning [`Archive::set_cluster_cache_max_bytes`]
    /// against actual eviction pressure, or surfacing cache health to
    /// telemetry from a long-running server.
    pub fn cluster_cache_stats(&self) -> ClusterCacheStats {
        self.core.cluster_cache.lock().unwrap().snapshot()
    }
}

/// Cheap PRNG seed: nanoseconds + pid run through SplitMix64's mixer.
/// Adequate for "give me a random article" — not for security.
fn pseudo_random_seed() -> u64 {
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
    x
}

/// Parse a `Illustration_<W>x<H>@<scale>` metadata key into its
/// `(width, height, scale)` components. Returns `None` if the key
/// doesn't match the pattern or any of the dimensions fail to parse
/// as a `u32`.
fn parse_illustration_key(name: &str) -> Option<(u32, u32, u32)> {
    let rest = name.strip_prefix("Illustration_")?;
    let (dims, scale) = rest.split_once('@')?;
    let (w, h) = dims.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?, scale.parse().ok()?))
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

/// Where an item's bytes physically live in the on-disk ZIM file,
/// returned by [`Archive::blob_direct_access`]. When `is_direct` is
/// `false` the item is stored in a compressed cluster and must be
/// fetched via the normal `get_data` path; offset/size are zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectAccess {
    /// `true` iff the item is in an uncompressed cluster and its
    /// bytes can be `pread`/`mmap`'d straight from the ZIM file.
    pub is_direct: bool,
    /// Absolute byte offset in the ZIM file at which the item's bytes
    /// begin. Only meaningful if `is_direct` is `true`.
    pub file_offset: u64,
    /// Length of the item's bytes on disk. Equals the decompressed
    /// size for uncompressed clusters. Only meaningful if `is_direct`
    /// is `true`.
    pub size: u64,
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

    /// Single-byte namespace this item belongs to (e.g. `b'C'`, `b'A'`).
    /// On items reached via redirect-following lookups this is the
    /// resolved entry's namespace, which may differ from the source
    /// entry's namespace on legacy cross-namespace redirects.
    pub fn namespace(&self) -> u8 {
        self.article.namespace
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

    /// Borrow the archive this item came from. Useful for callers that
    /// want to call archive-level helpers (e.g. `blob_direct_access`)
    /// without having to thread the archive through separately.
    pub fn archive(&self) -> &Archive {
        &self.archive
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
        let (cluster, range) = self.pinned_blob()?;
        Ok(Blob {
            payload: cluster.payload().clone(),
            range,
        })
    }

    /// Pre-fault the OS page cache for this item's bytes. Designed for
    /// the libzim-shim P7 scenario: the shim hands Xapian a fresh fd
    /// at the offset of the embedded Xapian DB, Xapian then issues
    /// eager DB-validation reads scattered across the multi-GB DB. On
    /// a cold page cache every one of those reads is a disk seek —
    /// the 12-second hang on first `/search` after kiwix-serve restart
    /// on the 48 GB Wikipedia ZIM. The shim's `F_RDAHEAD=0` /
    /// `POSIX_FADV_RANDOM` mitigation moves the needle by only ~10 %
    /// because madvise/fadvise hints don't influence Xapian's own
    /// eager reads.
    ///
    /// This function:
    /// 1. Calls `madvise(MADV_WILLNEED)` on the item's region to ask
    ///    the kernel to start async read-ahead, then
    /// 2. Touches one byte per 4 KB page to force synchronous
    ///    fault-in.
    ///
    /// On return, every page in the region is resident — subsequent
    /// reads (whether through zimru's existing mmap or through any
    /// other fd onto the same file the OS page cache is keyed by
    /// inode, not mmap region) hit cache. Cost is O(region size) of
    /// disk I/O paid once.
    ///
    /// Direct-access only: silently no-ops on items in compressed
    /// clusters (the page-fault game only makes sense for
    /// uncompressed regions where the on-disk bytes are the same
    /// bytes the consumer will read).
    ///
    /// Suitable for kiwix-serve to call at startup (synchronous,
    /// blocks server boot but eliminates the first-search hang) or
    /// in a background thread (concurrent with other init).
    pub fn warmup(&self) -> Result<()> {
        let direct = self
            .archive
            .blob_direct_access(self.cluster_index(), self.blob_index())?;
        if !direct.is_direct {
            return Ok(());
        }
        let offset = direct.file_offset as usize;
        let size = direct.size as usize;
        let mmap = &self.archive.core.mmap;
        if offset.saturating_add(size) > mmap.len() {
            return Err(Error::Truncated((offset + size) as u64));
        }
        // Best-effort kernel hint — async read-ahead. Errors are
        // non-fatal: the page-touch below still forces fault-in.
        let _ = mmap.advise_range(Advice::WillNeed, offset, size);
        // Synchronous fault-in: one byte per page is enough to
        // trigger the kernel to read the whole page.
        const PAGE: usize = 4096;
        let region = &mmap[offset..offset + size];
        let mut acc: u64 = 0;
        let mut i = 0;
        while i < region.len() {
            acc = acc.wrapping_add(region[i] as u64);
            i += PAGE;
        }
        std::hint::black_box(acc);
        Ok(())
    }

    /// Resolve the underlying cluster (kept alive by an `Arc`) and the
    /// byte range of this item within the decompressed payload, with
    /// no per-call wrapper allocation. Designed for zero-allocation
    /// FFI consumers — the C ABI's `zimru_item_blob_view` calls this
    /// to skip the per-call `Box::new(zimru_blob_t)` that the
    /// `get_data → zimru_blob_t` path pays.
    ///
    /// Pinning the cluster `Arc` keeps the decompressed payload alive
    /// even if the cluster cache evicts the entry mid-use, so callers
    /// can hold the returned slice across LRU pressure.
    pub fn pinned_blob(&self) -> Result<(Arc<Cluster>, std::ops::Range<usize>)> {
        let cluster = self.archive.load_cluster(self.article.cluster)?;
        let range = cluster.blob_range(self.article.blob).map_err(|e| match e {
            Error::BadBlobIndex { blob, count, .. } => Error::BadBlobIndex {
                cluster: self.article.cluster,
                blob,
                count,
            },
            other => other,
        })?;
        Ok((cluster, range))
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
