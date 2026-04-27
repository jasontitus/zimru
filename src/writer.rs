//! ZIM file writer.
//!
//! Mirrors the shape of libzim's `Creator`:
//!
//! ```no_run
//! use zimru::writer::{Creator, Item};
//! let mut c = Creator::new();
//! c.set_main_path("home");
//! c.add_item(Item::html("home", "Home", "<h1>Hi</h1>"));
//! c.add_metadata("Title", "Demo");
//! c.add_metadata("Language", "eng");
//! c.write_to("demo.zim")?;
//! # Ok::<(), zimru::Error>(())
//! ```
//!
//! Output: version `5.1` (new namespaces: `C` for content, `M` metadata,
//! `W` well-known, `X` indexes), in-header URL + title pointer lists, MD5
//! trailer. Readable by libzim 8+ and by `zimru` itself. Cross-validated
//! with upstream `zimcheck -A`.
//!
//! Bin-packing strategy: items are grouped greedily until the running
//! payload reaches `cluster_size_target` bytes (default 2 MiB, matching
//! upstream `zimwriterfs`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read as _, Seek, SeekFrom, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use md5::{Digest, Md5};
use rayon::prelude::*;

use crate::cluster::Compression;
use crate::error::{Error, Result};
use crate::header::{HEADER_SIZE, MAGIC};

/// Default target size for one cluster's worth of decompressed payload
/// (2 MiB — same as libzim's zimwriterfs default).
pub const DEFAULT_CLUSTER_SIZE_TARGET: usize = 2 * 1024 * 1024;

/// Default mimetype recorded for [`Creator::add_metadata`] entries when
/// the caller does not specify one. Most ZIM metadata keys are short
/// UTF-8 strings (Title, Language, Description, …); the explicit-mime
/// variant [`Creator::add_metadata_with_mimetype`] is for the handful
/// that aren't (e.g. the per-favicon PNG metadata used by old archives).
pub const DEFAULT_METADATA_MIMETYPE: &str = "text/plain;charset=utf-8";

/// A content item to add to the archive.
#[derive(Debug, Clone)]
pub struct Item {
    pub path: String,
    pub title: String,
    pub mimetype: String,
    pub content: Vec<u8>,
    /// Override the on-disk namespace. `None` = content namespace `'C'`,
    /// which is what every existing call site uses. `Some(ns)` routes the
    /// item to a non-default namespace — the current consumer is the
    /// libzim-shim's fulltext-index emission, which writes
    /// `{ns:'X', url:"fulltext/xapian"}`.
    pub namespace: Option<u8>,
}

impl Item {
    pub fn new(
        path: impl Into<String>,
        title: impl Into<String>,
        mimetype: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            path: path.into(),
            title: title.into(),
            mimetype: mimetype.into(),
            content: content.into(),
            namespace: None,
        }
    }
    /// Construct an item that lands in `namespace` rather than `'C'`.
    /// `path` is used as the dirent URL (no prefix peeling — pass the
    /// raw URL the reader will look up). Same content semantics as
    /// [`Item::new`] otherwise.
    pub fn in_namespace(
        namespace: u8,
        path: impl Into<String>,
        title: impl Into<String>,
        mimetype: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            path: path.into(),
            title: title.into(),
            mimetype: mimetype.into(),
            content: content.into(),
            namespace: Some(namespace),
        }
    }
    pub fn html(
        path: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self::new(path, title, "text/html", content)
    }
    pub fn text(
        path: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self::new(path, title, "text/plain", content)
    }
    pub fn png(
        path: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self::new(path, title, "image/png", content)
    }
}

/// A redirect alias. Resolved to a URL-pointer index at finalize time.
#[derive(Debug, Clone)]
pub struct Redirection {
    pub path: String,
    pub title: String,
    pub target_path: String,
}

/// One metadata entry buffered by [`Creator`]. Carries a per-entry mimetype
/// so non-text metadata (PNG favicons on legacy archives, etc.) round-trips
/// faithfully — the historical default is [`DEFAULT_METADATA_MIMETYPE`].
#[derive(Debug, Clone)]
pub struct MetadataEntry {
    pub name: String,
    pub mimetype: String,
    pub value: Vec<u8>,
}

/// Per-build statistics emitted by the streaming writer at finalize
/// time. Held inside [`Streamer`] and printed to stderr if the
/// `ZIMRU_STATS=1` env var is set, or always when the binary
/// passed `--verbose`-style instrumentation is wired in. The user-
/// visible breakdown lets us pin where wall-time and bytes-out are
/// going on a multi-GB build:
///
/// * how many items took each routing path
///   (`add_item` buffered, chunked-buffered, chunked-streamed),
/// * how many clusters of each flavour ended up on disk
///   (parallel-batch encoded buckets, streamed huge items),
/// * total time spent in each phase
///   (bucket flush + parallel encode, streaming-encode, table
///   write, MD5 trailer, post-write verify),
/// * raw input bytes vs compressed output bytes (compression
///   ratio).
#[derive(Default, Debug)]
pub struct BuildStats {
    pub items_buffered: u64,
    pub items_streamed: u64,
    pub raw_bytes_total: u64,
    pub clusters_buffered: u64,
    pub clusters_streamed: u64,
    pub bytes_clusters_written: u64,
    pub parallel_encode: Duration,
    pub streaming_encode: Duration,
    pub table_write: Duration,
    pub md5_pass: Duration,
    pub verify_pass: Duration,
}

impl BuildStats {
    fn print_summary(&self, started: Instant, output_size: u64) {
        let total = started.elapsed();
        let pct = |d: Duration| -> f64 {
            if total.is_zero() {
                0.0
            } else {
                100.0 * d.as_secs_f64() / total.as_secs_f64()
            }
        };
        let mb = |b: u64| -> f64 { b as f64 / (1024.0 * 1024.0) };
        let ratio = if self.raw_bytes_total > 0 {
            mb(self.bytes_clusters_written) / mb(self.raw_bytes_total)
        } else {
            0.0
        };
        eprintln!("--- zimru BuildStats ---");
        eprintln!(
            "  total wall:           {:>8.2}s",
            total.as_secs_f64()
        );
        eprintln!(
            "  parallel encode:      {:>8.2}s ({:.1}%)  buffered clusters: {}",
            self.parallel_encode.as_secs_f64(),
            pct(self.parallel_encode),
            self.clusters_buffered
        );
        eprintln!(
            "  streaming encode:     {:>8.2}s ({:.1}%)  streamed clusters: {}",
            self.streaming_encode.as_secs_f64(),
            pct(self.streaming_encode),
            self.clusters_streamed
        );
        eprintln!(
            "  table+dirent write:   {:>8.2}s ({:.1}%)",
            self.table_write.as_secs_f64(),
            pct(self.table_write)
        );
        eprintln!(
            "  MD5 trailer:          {:>8.2}s ({:.1}%)",
            self.md5_pass.as_secs_f64(),
            pct(self.md5_pass)
        );
        eprintln!(
            "  post-write verify:    {:>8.2}s ({:.1}%)",
            self.verify_pass.as_secs_f64(),
            pct(self.verify_pass)
        );
        eprintln!(
            "  items: buffered={}  streamed={}  raw_input={:.1} MB",
            self.items_buffered,
            self.items_streamed,
            mb(self.raw_bytes_total)
        );
        eprintln!(
            "  output: file={:.1} MB  cluster_bytes={:.1} MB  ratio={:.3}",
            mb(output_size),
            mb(self.bytes_clusters_written),
            ratio
        );
    }
}

/// Threshold for switching from buffered-chunked-item mode (which
/// accumulates chunks into a single `Vec<u8>`, then runs the normal
/// bin-packer) to streaming-encode mode (which dedicates a fresh
/// cluster to the item and streams its bytes through a zstd encoder
/// straight to disk).
///
/// Trade-off (measured on `texas-unpacked` at zstd 19):
///
/// * **Buffered path**: each item gets its own cluster (since it
///   busts the 2 MiB bin-pack target), enters the
///   `pending_encode` queue, and is encoded in a parallel batch
///   of `rayon::current_num_threads()` clusters. Peak memory is
///   `threads × max_item_size + encoded_output`. Wall-time is
///   excellent — 8 cores compress 8 clusters simultaneously at
///   full speed.
/// * **Streaming-encode path**: zstd encoder owns the file across
///   chunked feeds; uses zstdmt internally to parallelise within
///   one frame. Memory is bounded at ~zstd-encoder-state (~50 MB)
///   regardless of item size. But streaming-encodes are
///   **serial across items** — only one in flight at a time —
///   so 534 medium-large items end up encoded sequentially, which
///   dominated 76% of total wall on the texas run.
///
/// Conclusion: streaming-encode is only worth its serialising
/// cost on items so big that buffering them would blow the memory
/// budget. We pick 256 MiB as the cutoff: items below that go
/// through the parallel-batch path (memory peak ~`8 × 256 MiB =
/// 2 GiB` during encode bursts, totally fine), only truly huge
/// items (e.g. texas's 1.74 GB `addr.json` and 516 MB `poi.json`)
/// take the slow-but-low-memory streaming path.
pub(crate) const STREAMING_ENCODE_THRESHOLD: usize = 256 * 1024 * 1024;

/// Builder for a chunked-input item — see [`Creator::begin_item`].
/// Holds the partially-accumulated body until `finish()` is called,
/// at which point the assembled item is pushed through the same
/// streaming bin-packer as `add_item`. The borrow on the parent
/// `Creator` keeps misuse compile-checked: only one chunked item
/// is in flight at a time.
pub struct ItemBuilder<'a> {
    creator: &'a mut Creator,
    path: String,
    title: String,
    mimetype: String,
    namespace: Option<u8>,
    content: Vec<u8>,
}

impl ItemBuilder<'_> {
    /// Append a chunk of body bytes to the in-flight item.
    pub fn write_chunk(&mut self, chunk: &[u8]) {
        self.content.extend_from_slice(chunk);
    }

    /// Finalise the item — pushes it through the streaming
    /// bin-packer. After this call the builder is consumed; the
    /// caller can call `begin_item` again for the next item.
    pub fn finish(self) -> Result<()> {
        let item = Item {
            path: self.path,
            title: self.title,
            mimetype: self.mimetype,
            content: self.content,
            namespace: self.namespace,
        };
        // Safe to unwrap because `begin_item` checked stream.is_some().
        let s = self
            .creator
            .stream
            .as_mut()
            .expect("ItemBuilder requires streaming mode");
        s.push_item(item)
    }
}

/// Metadata for a chunked item before its body bytes accumulate.
pub(crate) struct ChunkedMeta {
    pub namespace: Option<u8>,
    pub path: String,
    pub title: String,
    pub mimetype: String,
}

/// In-flight state for a chunked item between `begin_item` and
/// `end_item`. Either accumulating into a `Vec<u8>` (small / unknown
/// size) or streaming straight through a zstd encoder into a
/// dedicated cluster on disk (huge known size).
pub(crate) enum ChunkedInFlight {
    /// Buffered: chunks append to `content`. At `finish` time the
    /// assembled body goes through `Streamer::push_item` like a
    /// regular `add_item` call.
    Buffered {
        meta: ChunkedMeta,
        content: Vec<u8>,
    },
    /// Streaming-encode: chunks feed a zstd encoder that writes to
    /// a per-item *temp file* (not the main output). At
    /// `end_chunked_item` we hand the encoder + temp-file path off
    /// to a background thread that calls `finish()` while the
    /// producer continues with subsequent items / parallel-batch
    /// encode of buckets. At `finalize` we join the background
    /// threads and concat each completed temp file into the main
    /// output in cluster_idx order, then delete the temps.
    StreamingZstd {
        meta: ChunkedMeta,
        encoder: zstd::stream::Encoder<'static, File>,
        temp_path: std::path::PathBuf,
        cluster_idx: u32,
        bytes_written: u64,
        expected_size: u64,
        mime_idx: u16,
    },
}

/// Handle to a streaming-encode task that's draining its zstd
/// encoder in the background. Joined at finalize time; produces
/// `(cluster_idx, temp_path, encoded_byte_count)` so the finalize
/// stage can splice the temp file into the main output at the
/// correct offset.
pub(crate) struct StreamingTask {
    pub(crate) cluster_idx: u32,
    pub(crate) handle: std::thread::JoinHandle<Result<(std::path::PathBuf, u64)>>,
}

/// Reserved bytes after the 80-byte header for the mime-type list.
/// Real libzim asserts `mimelistPos == 80`; the streaming writer pins
/// it there and writes the actual mime list at finalize time, padded
/// to this size. Empirically, real-world mime lists are <1 KB; 64 KB
/// gives generous headroom and is irrelevant on big ZIMs (0.0016 % of
/// a 4 GB output).
pub(crate) const MIME_LIST_RESERVE: usize = 64 * 1024;

/// Choice of how the streaming writer decides which "in-flight
/// cluster" each item joins. Different choices group similar
/// content into the same cluster so zstd's match-finder can reuse
/// dictionary entries across adjacent items.
///
/// Trade-off: more buckets → more in-flight clusters → more peak
/// RAM (`buckets × cluster_size_target`) and potentially smaller
/// per-cluster averages (less dictionary warmup → worse
/// compression on under-filled clusters). Empirically, gains are
/// largest on ZIMs with non-trivial mime / extension diversity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterStrategy {
    /// One in-flight cluster, items packed in `add_item` order
    /// (URL-sort order if the caller is the buffered API). Default.
    Single,
    /// One in-flight cluster per distinct mime type. HTML items go
    /// together, JSON items go together, PNG items go together, and
    /// so on.
    ByMime,
    /// One in-flight cluster per file-extension (the bytes after
    /// the last `.` in the path, or `""` for paths without an
    /// extension).
    ByExtension,
    /// One in-flight cluster per first path segment (the bytes
    /// before the first `/`, or the whole path if it has none).
    /// Useful for ZIMs whose directory layout reflects content
    /// types (`wiki/...`, `maps/...`, `images/...`).
    ByFirstPathSegment,
}

impl Default for ClusterStrategy {
    fn default() -> Self {
        ClusterStrategy::Single
    }
}

/// High-level ZIM file builder. Two usage modes:
///
/// * **Buffered** — call `add_*` then `write_to(path)`. Every body
///   stays in RAM until finalize. Convenient for tests.
/// * **Streaming** — call `start_writing(path)`, then `add_*`, then
///   `finish_writing()`. Bodies are bin-packed into clusters and
///   stream-encoded-and-written as items arrive; only dirent metadata
///   (small, ~50 B/item) and the in-flight cluster's bytes
///   (≤ `cluster_size_target × thread_count`) stay resident. Use this
///   for production builds where peak RSS matters — typical reduction
///   is ~600× on a multi-GB workload.
pub struct Creator {
    items: Vec<Item>,
    redirections: Vec<Redirection>,
    metadata: Vec<MetadataEntry>,
    illustrations: Vec<(u32, Vec<u8>)>,
    main_path: Option<String>,
    compression: Compression,
    compression_level: Option<i32>,
    cluster_size_target: usize,
    /// Soft cap on bytes queued for parallel-batch encode at any
    /// time. `0` means "unlimited" (drain only when the queue hits
    /// `rayon::current_num_threads()` clusters, the default).
    /// Setting a non-zero value forces an early drain whenever the
    /// queue's running total exceeds the cap — trades wall-time
    /// parallelism for a tighter peak-RSS bound.
    max_in_flight_bytes: usize,
    uuid: [u8; 16],
    cluster_strategy: ClusterStrategy,

    /// Set when `start_writing` is called. Once set, all `add_*` and
    /// `set_main_path` calls route directly into the streamer; any
    /// previously-buffered work was already drained into it at
    /// `start_writing` time.
    stream: Option<Streamer>,
}

impl Default for Creator {
    fn default() -> Self {
        Self::new()
    }
}

impl Creator {
    pub fn new() -> Self {
        Creator {
            items: Vec::new(),
            redirections: Vec::new(),
            metadata: Vec::new(),
            illustrations: Vec::new(),
            main_path: None,
            compression: Compression::Zstd,
            compression_level: None,
            cluster_size_target: DEFAULT_CLUSTER_SIZE_TARGET,
            max_in_flight_bytes: 0,
            uuid: default_uuid(),
            cluster_strategy: ClusterStrategy::Single,
            stream: None,
        }
    }

    /// Pick how items are grouped into clusters during streaming.
    /// See [`ClusterStrategy`] for the choices. Must be called
    /// before [`Creator::start_writing`] (or [`Creator::write_to`]);
    /// changing strategy mid-write is unsupported and will panic.
    pub fn set_cluster_strategy(&mut self, s: ClusterStrategy) -> &mut Self {
        assert!(
            self.stream.is_none(),
            "set_cluster_strategy called after start_writing",
        );
        self.cluster_strategy = s;
        self
    }

    pub fn add_item(&mut self, item: Item) -> &mut Self {
        if let Some(s) = self.stream.as_mut() {
            // Streaming mode — bin-pack body into the current cluster,
            // encode-write-free if it overflows. Errors here panic
            // because the &mut Self return shape doesn't propagate; if
            // a caller wants error propagation it should use the
            // explicit `add_item_streaming` helper (TODO if needed).
            if let Err(e) = s.push_item(item) {
                panic!("streaming add_item: {e}");
            }
        } else {
            self.items.push(item);
        }
        self
    }

    /// True iff `start_writing` has been called and the creator is
    /// ready for streaming-mode `add_*` calls. Used by the C ABI to
    /// validate `begin_item` preconditions without taking out a
    /// borrow on the inner streamer.
    #[doc(hidden)]
    pub fn peek_streaming(&self) -> Option<()> {
        if self.stream.is_some() {
            Some(())
        } else {
            None
        }
    }

    /// Streaming-mode push that propagates errors (unlike
    /// `add_item`'s panic-on-error). Used by the chunked C ABI
    /// after assembling an item from chunks. Errors if streaming
    /// mode hasn't been started.
    #[doc(hidden)]
    pub fn push_streaming_item(&mut self, item: Item) -> Result<()> {
        match self.stream.as_mut() {
            Some(s) => s.push_item(item),
            None => Err(Error::Io(std::io::Error::other(
                "push_streaming_item before start_writing",
            ))),
        }
    }

    /// C-ABI-shape chunked-item begin. Internally dispatches between
    /// buffered (small / unknown size) and streaming-encode (huge,
    /// zstd-compressed) paths based on `expected_size` and the
    /// configured compression. Streaming-encode bounds peak memory
    /// at ~zstd encoder state regardless of how big the body is.
    #[doc(hidden)]
    pub fn begin_chunked_item(
        &mut self,
        namespace: Option<u8>,
        path: String,
        title: String,
        mimetype: String,
        expected_size: Option<u64>,
    ) -> Result<()> {
        let s = self.stream.as_mut().ok_or_else(|| {
            Error::Io(std::io::Error::other(
                "begin_chunked_item before start_writing",
            ))
        })?;
        s.begin_chunked_item(
            ChunkedMeta { namespace, path, title, mimetype },
            expected_size,
        )
    }

    /// Append a chunk to the in-flight chunked item. See
    /// [`Creator::begin_chunked_item`].
    #[doc(hidden)]
    pub fn chunked_item_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        let s = self.stream.as_mut().ok_or_else(|| {
            Error::Io(std::io::Error::other(
                "chunked_item_chunk before start_writing",
            ))
        })?;
        s.chunked_item_chunk(chunk)
    }

    /// Finalise the in-flight chunked item.
    #[doc(hidden)]
    pub fn end_chunked_item(&mut self) -> Result<()> {
        let s = self.stream.as_mut().ok_or_else(|| {
            Error::Io(std::io::Error::other(
                "end_chunked_item before start_writing",
            ))
        })?;
        s.end_chunked_item()
    }

    /// Begin a chunked item — for callers that have a streaming
    /// content source (e.g. `ContentProvider::feed()` chunks from
    /// libzim-shim) and don't want to slurp the body into one
    /// `Vec<u8>` before handing it off. Memory peak on the *caller*
    /// side becomes one chunk; zimru still accumulates the item's
    /// bytes internally until cluster flush.
    ///
    /// Returns a builder that owns the in-flight item state. Caller
    /// pushes chunks via [`ItemBuilder::write_chunk`] and finishes
    /// with [`ItemBuilder::finish`].
    ///
    /// Requires streaming mode (`start_writing` must have been
    /// called); buffered-mode chunked items aren't supported.
    pub fn begin_item(
        &mut self,
        path: impl Into<String>,
        title: impl Into<String>,
        mimetype: impl Into<String>,
        namespace: Option<u8>,
        size_hint: Option<usize>,
    ) -> Result<ItemBuilder<'_>> {
        if self.stream.is_none() {
            return Err(Error::Io(std::io::Error::other(
                "begin_item requires start_writing first",
            )));
        }
        let mut content = Vec::new();
        if let Some(n) = size_hint {
            content.reserve(n);
        }
        Ok(ItemBuilder {
            creator: self,
            path: path.into(),
            title: title.into(),
            mimetype: mimetype.into(),
            namespace,
            content,
        })
    }

    pub fn add_redirection(
        &mut self,
        path: impl Into<String>,
        title: impl Into<String>,
        target: impl Into<String>,
    ) -> &mut Self {
        let r = Redirection {
            path: path.into(),
            title: title.into(),
            target_path: target.into(),
        };
        if let Some(s) = self.stream.as_mut() {
            s.redirections.push(r);
        } else {
            self.redirections.push(r);
        }
        self
    }

    /// Metadata entry, stored in the `M` namespace. The recorded mimetype
    /// is [`DEFAULT_METADATA_MIMETYPE`] (`text/plain;charset=utf-8`) — for
    /// non-text metadata (PNG favicons on legacy archives, etc.) call
    /// [`Creator::add_metadata_with_mimetype`] instead.
    pub fn add_metadata(
        &mut self,
        name: impl Into<String>,
        value: impl Into<Vec<u8>>,
    ) -> &mut Self {
        self.add_metadata_with_mimetype(name, DEFAULT_METADATA_MIMETYPE, value)
    }

    /// Metadata entry with an explicit mimetype. The mimetype is stored
    /// verbatim in the dirent's mimetype index — callers are responsible
    /// for choosing a value that downstream readers will recognise.
    pub fn add_metadata_with_mimetype(
        &mut self,
        name: impl Into<String>,
        mimetype: impl Into<String>,
        value: impl Into<Vec<u8>>,
    ) -> &mut Self {
        let m = MetadataEntry {
            name: name.into(),
            mimetype: mimetype.into(),
            value: value.into(),
        };
        if let Some(s) = self.stream.as_mut() {
            s.metadata.push(m);
        } else {
            self.metadata.push(m);
        }
        self
    }

    /// ZIM illustration (PNG) of the given side length. Stored at
    /// `M/Illustration_NxN@1`.
    pub fn add_illustration(&mut self, side: u32, png: impl Into<Vec<u8>>) -> &mut Self {
        let il = (side, png.into());
        if let Some(s) = self.stream.as_mut() {
            s.illustrations.push(il);
        } else {
            self.illustrations.push(il);
        }
        self
    }

    /// Declare the archive's main page. A `W/mainPage` redirect to this
    /// path is written at finalize time, matching how upstream tools
    /// expect to find the main page.
    pub fn set_main_path(&mut self, path: impl Into<String>) -> &mut Self {
        let p: String = path.into();
        if let Some(s) = self.stream.as_mut() {
            s.main_path = Some(p);
        } else {
            self.main_path = Some(p);
        }
        self
    }

    pub fn set_compression(&mut self, c: Compression) -> &mut Self {
        self.compression = c;
        self
    }

    /// Set the compression level for the active algorithm.
    ///
    /// Range and meaning depend on the algorithm:
    /// * `Compression::Zstd` — accepts `1..=22` (libzstd's normal range, plus
    ///   negative "fast" levels). The default is `3`.
    /// * `Compression::Xz`   — accepts `0..=9`. The default is `3`.
    /// * `Compression::None` — ignored.
    ///
    /// Pass `None` (or never call this) to use the default for the algorithm.
    pub fn set_compression_level(&mut self, level: i32) -> &mut Self {
        self.compression_level = Some(level);
        self
    }

    pub fn set_cluster_size_target(&mut self, bytes: usize) -> &mut Self {
        self.cluster_size_target = bytes.max(1);
        self
    }

    /// Soft cap on the number of raw bytes queued in the
    /// parallel-batch encode pipeline at any time. Zero (the
    /// default) means "drain only when the queue hits
    /// `rayon::current_num_threads()` clusters" — i.e. saturate
    /// CPU at the cost of a wider memory footprint.
    ///
    /// A non-zero value forces an early drain whenever the
    /// queue's accumulated bytes cross the cap, trading
    /// parallel-batch CPU saturation for a tighter peak RSS.
    /// Useful in memory-constrained environments (Kiwix
    /// zimfarm worker, embedded builds): for example pass
    /// `512 * 1024 * 1024` to keep the parallel-batch buffer
    /// under ~512 MiB on top of zimru's other in-process state
    /// (~few hundred MB).
    pub fn set_max_in_flight_bytes(&mut self, bytes: usize) -> &mut Self {
        self.max_in_flight_bytes = bytes;
        self
    }

    pub fn set_uuid(&mut self, uuid: impl Into<crate::Uuid>) -> &mut Self {
        self.uuid = uuid.into().into_bytes();
        self
    }

    /// Materialize the archive to `path`. Consumes the builder.
    ///
    /// Two routes through this:
    ///
    /// * If `start_writing` was already called, this is equivalent to
    ///   [`Creator::finish_writing`] — `path` is ignored (the file is
    ///   already open).
    /// * If `start_writing` was not called, all `add_*` work is in
    ///   in-RAM buffers; this opens the file at `path`, drains the
    ///   buffers through the streaming pipeline, and finalizes. Same
    ///   bytes on disk, but peak RSS = sum-of-bodies + cluster encode
    ///   overhead. Use [`Creator::start_writing`] +
    ///   [`Creator::finish_writing`] for production builds where
    ///   memory matters.
    pub fn write_to(mut self, path: impl AsRef<Path>) -> Result<()> {
        if self.stream.is_some() {
            return self.finish_writing();
        }
        self.start_writing(path)?;
        self.finish_writing()
    }

    /// Open the output file and switch to streaming mode. Subsequent
    /// `add_*` calls bin-pack into the in-flight cluster and stream-
    /// encode-write to disk as the cluster fills, dropping body
    /// references immediately. Buffered work added before this call
    /// is drained into the streamer here.
    pub fn start_writing(&mut self, path: impl AsRef<Path>) -> Result<()> {
        if self.stream.is_some() {
            return Err(Error::Io(std::io::Error::other(
                "Creator::start_writing called twice",
            )));
        }
        let mut s = Streamer::open(
            path.as_ref(),
            self.compression,
            self.compression_level,
            self.cluster_size_target,
            self.max_in_flight_bytes,
            self.uuid,
            self.main_path.take(),
            self.cluster_strategy,
        )?;
        // Drain anything the caller buffered through add_* before
        // they decided to stream.
        for item in self.items.drain(..) {
            s.push_item(item)?;
        }
        for r in self.redirections.drain(..) {
            s.redirections.push(r);
        }
        for m in self.metadata.drain(..) {
            s.metadata.push(m);
        }
        for il in self.illustrations.drain(..) {
            s.illustrations.push(il);
        }
        self.stream = Some(s);
        Ok(())
    }

    /// Drain buffered metadata / illustrations / redirections into
    /// clusters, write the URL/title/cluster pointer tables and
    /// dirents, write the mime list at offset 80, write the final
    /// header, compute the MD5 trailer. Consumes the Creator.
    pub fn finish_writing(mut self) -> Result<()> {
        let s = self.stream.take().ok_or_else(|| {
            Error::Io(std::io::Error::other(
                "Creator::finish_writing called before start_writing",
            ))
        })?;
        s.finalize()
    }
}

// ---------- internals ----------

#[derive(Debug, Clone)]
enum RawDirent {
    Article {
        namespace: u8,
        url: String,
        title: String,
        mime_idx: u16,
        cluster: u32,
        blob: u32,
    },
    Redirect {
        namespace: u8,
        url: String,
        title: String,
        target_ns: u8,
        target_url: String,
        resolved_index: Option<u32>,
    },
}

impl RawDirent {
    fn namespace(&self) -> u8 {
        match self {
            RawDirent::Article { namespace, .. } | RawDirent::Redirect { namespace, .. } => {
                *namespace
            }
        }
    }
    fn url(&self) -> &str {
        match self {
            RawDirent::Article { url, .. } | RawDirent::Redirect { url, .. } => url,
        }
    }
    fn title(&self) -> &str {
        match self {
            RawDirent::Article { title, .. } | RawDirent::Redirect { title, .. } => title,
        }
    }
}

fn intern_mime(m: &str, mimes: &mut Vec<String>, index: &mut BTreeMap<String, u16>) -> u16 {
    if let Some(&i) = index.get(m) {
        return i;
    }
    let i = mimes.len() as u16;
    mimes.push(m.to_string());
    index.insert(m.to_string(), i);
    i
}

/// Build the `M/Counter` body: `mime=count;mime=count;…`. Counts
/// only `Article` dirents (redirect dirents have no body / no
/// mime). Returned in the order mimes were interned, which is
/// also the order they appear in the trailing mime list — so
/// `Counter` parses one-pass alongside the mime list.
fn build_counter_string(mimes: &[String], dirents: &[RawDirent]) -> String {
    let mut counts: Vec<u64> = vec![0; mimes.len()];
    for d in dirents {
        if let RawDirent::Article { mime_idx, .. } = d {
            let i = *mime_idx as usize;
            if i < counts.len() {
                counts[i] += 1;
            }
        }
    }
    let mut out = String::new();
    let mut first = true;
    for (i, m) in mimes.iter().enumerate() {
        if counts[i] == 0 {
            continue;
        }
        if !first {
            out.push(';');
        }
        first = false;
        out.push_str(m);
        out.push('=');
        out.push_str(&counts[i].to_string());
    }
    out
}

// ---------- streaming ----------

/// Stream-write engine. Owns the output file and per-build state that
/// changes as items arrive. Created by `Creator::start_writing`,
/// finalised by `Creator::finish_writing`.
///
/// On-disk layout this writer produces:
///
/// ```text
/// [header (80 B placeholder)]
/// [mime list, pinned at offset 80, padded to MIME_LIST_RESERVE]
/// [clusters, stream-encoded as items arrive]
/// [URL pointer list]
/// [title pointer list]
/// [cluster pointer list]
/// [dirents]
/// [16-byte MD5 trailer]
/// ```
///
/// vs. the buffered writer's layout (mime / url_ptrs / title_ptrs /
/// cluster_ptrs / dirents BEFORE clusters), this puts everything
/// known-late at the tail so we can stream forward without seeking
/// back. Real libzim's only positional invariant is `mime_list_pos
/// == 80`, which we honour via the reserved region.
struct Streamer {
    /// Output file handle. `Some` normally; temporarily `None` while
    /// a streaming-encode huge item is in flight (the zstd encoder
    /// owns the file across `item_chunk` calls and gives it back at
    /// `end_item`).
    file: Option<File>,
    file_pos: u64,
    output_path: std::path::PathBuf,

    compression: Compression,
    compression_level: Option<i32>,
    cluster_size_target: usize,
    /// Soft cap on raw bytes queued in `pending_encode` (see
    /// [`Creator::set_max_in_flight_bytes`]). 0 = unlimited.
    max_in_flight_bytes: usize,
    /// Running total of raw bytes currently in `pending_encode`,
    /// kept incrementally so flush_bucket doesn't have to walk
    /// every queued cluster on every push.
    pending_bytes: usize,
    uuid: [u8; 16],
    main_path: Option<String>,
    cluster_strategy: ClusterStrategy,

    // Mime list, accumulating as items arrive.
    mimes: Vec<String>,
    mime_index: BTreeMap<String, u16>,

    // One in-flight cluster per bucket key (`""` for `Single` mode,
    // mime / extension / first path segment for the others). When a
    // bucket exceeds `cluster_size_target` it is flushed independently
    // of the others — its blobs are queued for parallel-batch encode
    // and its dirents are committed with the freshly-allocated
    // `cluster_idx`.
    buckets: BTreeMap<String, Bucket>,

    // Per-cluster offsets in the output file. Each closed cluster
    // gets a slot here; the slot is filled with the actual offset
    // when its encoded bytes are written. `cluster_offsets.len()`
    // doubles as the next cluster_idx allocator at flush time.
    cluster_offsets: Vec<u64>,

    // Bounded queue of (cluster_idx, raw_blobs) waiting to be
    // encode-and-written. When this fills to `rayon::current_num_threads()`
    // we drain it as a single `par_iter` batch and write the encoded
    // bytes in cluster_idx order. Cap is small
    // (`cluster_size_target * thread_count` raw bytes) so peak RSS
    // stays bounded.
    pending_encode: Vec<(u32, Vec<Vec<u8>>)>,

    // Dirents committed for items whose cluster has been flushed.
    // Small (~50 B/item).
    dirents: Vec<RawDirent>,

    // Buffered until finalize — small data.
    redirections: Vec<Redirection>,
    metadata: Vec<MetadataEntry>,
    illustrations: Vec<(u32, Vec<u8>)>,

    // The single in-flight chunked item between `begin_chunked_item`
    // and `end_chunked_item` (if any). Held on the streamer so the
    // streaming-encode variant can swap the file out while the
    // encoder owns it.
    in_flight: Option<ChunkedInFlight>,

    /// Background streaming-encode tasks that have been handed off
    /// at `end_chunked_item` and are draining their zstd encoders
    /// independently. Joined at finalize time; their compressed
    /// temp-file bytes are spliced into the main output in
    /// cluster_idx order.
    streaming_tasks: Vec<StreamingTask>,

    // Per-build telemetry. Updated at every routing decision and
    // every encode call. Printed to stderr at finalize time if
    // `ZIMRU_STATS` is set in the environment.
    started: Instant,
    stats: BuildStats,
}

/// One in-flight cluster's worth of un-flushed work for a single
/// bucket. Lives inside [`Streamer::buckets`].
#[derive(Default)]
struct Bucket {
    /// Raw blob bytes for items pushed to this bucket since the
    /// last flush. Encoded into one cluster on the next flush.
    blobs: Vec<Vec<u8>>,
    /// Sum of `blobs[i].len()`, the trigger for flush.
    size_bytes: usize,
    /// Per-item dirent metadata for the items currently in `blobs`.
    /// `cluster_idx` is filled at flush time (when we know which
    /// cluster index this bucket's contents will become); `blob_idx`
    /// is the position within `blobs` and is final.
    pending: Vec<PendingArticle>,
}

struct PendingArticle {
    namespace: u8,
    url: String,
    title: String,
    mime_idx: u16,
    blob_idx: u32,
}

impl Streamer {
    /// Mutable access to the output file. Panics if called while a
    /// streaming-encode item is in flight (the encoder has the file
    /// in that case). All non-streaming-encode code paths can rely
    /// on this never panicking.
    #[inline]
    fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("Streamer file was taken by streaming-encode and not yet returned")
    }

    fn open(
        path: &Path,
        compression: Compression,
        compression_level: Option<i32>,
        cluster_size_target: usize,
        max_in_flight_bytes: usize,
        uuid: [u8; 16],
        main_path: Option<String>,
        cluster_strategy: ClusterStrategy,
    ) -> Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        // Reserve [0, 80) for the header placeholder + [80, 80 +
        // MIME_LIST_RESERVE) for the mime list. We'll fill both at
        // finalize time once we know the final values.
        file.write_all(&[0u8; HEADER_SIZE])?;
        let zeros = vec![0u8; MIME_LIST_RESERVE];
        file.write_all(&zeros)?;
        Ok(Streamer {
            file: Some(file),
            file_pos: HEADER_SIZE as u64 + MIME_LIST_RESERVE as u64,
            output_path: path.to_path_buf(),
            compression,
            compression_level,
            cluster_size_target,
            max_in_flight_bytes,
            pending_bytes: 0,
            uuid,
            main_path,
            cluster_strategy,
            mimes: Vec::new(),
            mime_index: BTreeMap::new(),
            buckets: BTreeMap::new(),
            cluster_offsets: Vec::new(),
            pending_encode: Vec::new(),
            dirents: Vec::new(),
            redirections: Vec::new(),
            metadata: Vec::new(),
            illustrations: Vec::new(),
            in_flight: None,
            streaming_tasks: Vec::new(),
            started: Instant::now(),
            stats: BuildStats::default(),
        })
    }

    /// Open a chunked item. If `expected_size > STREAMING_ENCODE_THRESHOLD`
    /// AND compression is zstd, the body will be stream-encoded to its
    /// own cluster on disk; otherwise chunks accumulate in a `Vec<u8>`
    /// and the assembled item runs through the normal bin-packer.
    fn begin_chunked_item(
        &mut self,
        meta: ChunkedMeta,
        expected_size: Option<u64>,
    ) -> Result<()> {
        if self.in_flight.is_some() {
            return Err(Error::Io(std::io::Error::other(
                "begin_chunked_item: another chunked item is already in flight",
            )));
        }
        let use_streaming = matches!(self.compression, Compression::Zstd)
            && expected_size.is_some_and(|s| s as usize >= STREAMING_ENCODE_THRESHOLD);

        if use_streaming {
            let expected = expected_size.unwrap();
            self.stats.items_streamed += 1;
            self.stats.raw_bytes_total += expected;

            // Allocate cluster_idx now (placeholder offset filled in
            // at finalize, when we splice the temp-file's compressed
            // bytes into the main output). No need to drain or flush
            // anything here: the streaming encode targets its OWN
            // temp file, so the in-flight bucket / parallel-batch
            // queue can keep doing whatever it was doing — they
            // write to `self.file`, the streaming encoder writes
            // somewhere else.
            let cluster_idx = self.cluster_offsets.len() as u32;
            self.cluster_offsets.push(0);

            // Intern this item's mime so we can record it in the dirent.
            let mime_idx = intern_mime(&meta.mimetype, &mut self.mimes, &mut self.mime_index);

            // Build the cluster's in-band header: ptr table with one
            // blob. ptr[0] = header_len (start of blob 0), ptr[1] =
            // header_len + expected_size (end). Choose 4-byte vs
            // 8-byte ptrs by whether the total (header+blob) overflows
            // u32.
            let extended_4 = (8u64 + expected) > u32::MAX as u64;
            let ptr_size: u64 = if extended_4 { 8 } else { 4 };
            let header_len = 2 * ptr_size;
            let mut header = Vec::with_capacity(header_len as usize);
            push_offset(&mut header, header_len, extended_4);
            push_offset(&mut header, header_len + expected, extended_4);

            // Compression info-byte: zstd id (5) | extended bit if needed.
            let info_byte: u8 = 5 | if extended_4 { 0x10 } else { 0 };

            // Per-item temp file. Each streamed item writes to its
            // own scratch file so multiple streaming-encode tasks
            // can run concurrently AND the parallel-batch path can
            // keep writing to the main output while we feed.
            let temp_path = std::env::temp_dir().join(format!(
                "zimru-stream-{}-{}-{}.tmp",
                std::process::id(),
                cluster_idx,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            let mut tmp_file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temp_path)?;
            // Cluster format prefix on disk: info_byte then zstd-
            // compressed (ptr table + blob bytes).
            tmp_file.write_all(&[info_byte])?;

            let level = self.compression_level.unwrap_or_else(|| {
                std::env::var("ZSTD_CLEVEL")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(3)
            });
            let mut encoder = zstd::stream::Encoder::new(tmp_file, level)
                .map_err(|e| Error::Decompression(format!("zstd init: {e}")))?;
            // Enable zstd's internal multi-worker parallelism for
            // the streaming-encode path. With per-item temp files
            // multiple streamed items also run concurrently with
            // each other (Idea C); each gets fewer workers but
            // they overlap across items.
            let workers = rayon::current_num_threads().max(1) as u32;
            let _ = encoder.multithread(workers);
            encoder
                .write_all(&header)
                .map_err(|e| Error::Decompression(format!("zstd header write: {e}")))?;

            self.in_flight = Some(ChunkedInFlight::StreamingZstd {
                meta,
                encoder,
                temp_path,
                cluster_idx,
                bytes_written: 0,
                expected_size: expected,
                mime_idx,
            });
            return Ok(());
        }

        // Buffered path — accumulate chunks into a Vec, push as a
        // regular item at end_chunked_item time.
        let capacity = expected_size.map(|s| s as usize).unwrap_or(0);
        let mut content = Vec::new();
        if capacity > 0 {
            content.reserve(capacity);
        }
        self.in_flight = Some(ChunkedInFlight::Buffered { meta, content });
        Ok(())
    }

    fn chunked_item_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        match self.in_flight.as_mut() {
            None => Err(Error::Io(std::io::Error::other(
                "chunked_item_chunk: no item in flight",
            ))),
            Some(ChunkedInFlight::Buffered { content, .. }) => {
                content.extend_from_slice(chunk);
                Ok(())
            }
            Some(ChunkedInFlight::StreamingZstd {
                encoder,
                bytes_written,
                expected_size,
                ..
            }) => {
                if *bytes_written + chunk.len() as u64 > *expected_size {
                    return Err(Error::Io(std::io::Error::other(format!(
                        "chunked_item_chunk: body exceeds expected size {} > {}",
                        *bytes_written + chunk.len() as u64,
                        *expected_size
                    ))));
                }
                let phase_start = Instant::now();
                let r = encoder
                    .write_all(chunk)
                    .map_err(|e| Error::Decompression(format!("zstd chunk write: {e}")));
                self.stats.streaming_encode += phase_start.elapsed();
                r?;
                if let Some(ChunkedInFlight::StreamingZstd { bytes_written, .. }) =
                    self.in_flight.as_mut()
                {
                    *bytes_written += chunk.len() as u64;
                }
                Ok(())
            }
        }
    }

    fn end_chunked_item(&mut self) -> Result<()> {
        match self.in_flight.take() {
            None => Err(Error::Io(std::io::Error::other(
                "end_chunked_item: no item in flight",
            ))),
            Some(ChunkedInFlight::Buffered { meta, content }) => {
                self.push_item(Item {
                    path: meta.path,
                    title: meta.title,
                    mimetype: meta.mimetype,
                    content,
                    namespace: meta.namespace,
                })
            }
            Some(ChunkedInFlight::StreamingZstd {
                meta,
                encoder,
                temp_path,
                cluster_idx,
                bytes_written,
                expected_size,
                mime_idx,
            }) => {
                if bytes_written != expected_size {
                    return Err(Error::Io(std::io::Error::other(format!(
                        "end_chunked_item: body size mismatch (got {}, expected {})",
                        bytes_written, expected_size
                    ))));
                }
                // Commit the dirent now — the cluster_idx is already
                // allocated. The actual cluster-offset slot stays at
                // its placeholder 0 until finalize splices the
                // temp file into the main output.
                let title = if meta.title.is_empty() {
                    meta.path.clone()
                } else {
                    meta.title
                };
                self.dirents.push(RawDirent::Article {
                    namespace: meta.namespace.unwrap_or(b'C'),
                    url: meta.path,
                    title,
                    mime_idx,
                    cluster: cluster_idx,
                    blob: 0,
                });

                // Hand the encoder + temp path off to a background
                // thread that drains it via `finish()`. The producer
                // can immediately begin the next chunked item or
                // continue feeding the parallel-batch path. The bg
                // thread also returns the final compressed-byte
                // count so finalize knows how many bytes to splice.
                let temp_path_for_thread = temp_path.clone();
                let handle = std::thread::spawn(move || -> Result<(std::path::PathBuf, u64)> {
                    let file = encoder
                        .finish()
                        .map_err(|e| Error::Decompression(format!("zstd finish: {e}")))?;
                    let bytes = file.metadata()?.len();
                    drop(file); // close the temp file
                    Ok((temp_path_for_thread, bytes))
                });
                self.streaming_tasks.push(StreamingTask {
                    cluster_idx,
                    handle,
                });
                Ok(())
            }
        }
    }

    /// Drain every background streaming-encode task: join their
    /// threads, splice each completed temp file into the main
    /// output in cluster_idx order, update cluster_offsets, and
    /// delete the temp file. Called once at finalize time.
    fn drain_streaming_tasks(&mut self) -> Result<()> {
        if self.streaming_tasks.is_empty() {
            return Ok(());
        }
        let phase_start = Instant::now();
        let tasks = std::mem::take(&mut self.streaming_tasks);
        // Collect (cluster_idx, temp_path, bytes) tuples; bail on
        // any thread panic / encode error.
        let mut completed: Vec<(u32, std::path::PathBuf, u64)> = Vec::new();
        for t in tasks {
            let cluster_idx = t.cluster_idx;
            let res = t.handle.join().map_err(|_| {
                Error::Io(std::io::Error::other(
                    "streaming-encode thread panicked",
                ))
            })?;
            let (temp_path, encoded_bytes) = res?;
            completed.push((cluster_idx, temp_path, encoded_bytes));
        }
        // Splice in cluster_idx order — readers index into
        // cluster_ptrs by cluster_idx, so the on-disk byte order
        // doesn't matter, but smaller cluster_idx values having
        // smaller offsets is nicer for sequential reads.
        completed.sort_by_key(|(idx, _, _)| *idx);
        let mut buf = vec![0u8; 1 << 20];
        for (cluster_idx, temp_path, bytes) in completed {
            self.cluster_offsets[cluster_idx as usize] = self.file_pos;
            // Stream-copy temp → main. read+write loop on a 1 MiB
            // buffer, no full-file slurp.
            let mut tf = std::fs::File::open(&temp_path)?;
            let mut copied: u64 = 0;
            loop {
                let n = tf.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                self.file_mut().write_all(&buf[..n])?;
                copied += n as u64;
            }
            drop(tf);
            // Best-effort cleanup; non-fatal if it fails.
            let _ = std::fs::remove_file(&temp_path);
            self.file_pos += copied;
            self.stats.clusters_streamed += 1;
            self.stats.bytes_clusters_written += bytes;
        }
        self.stats.streaming_encode += phase_start.elapsed();
        Ok(())
    }

    /// Pick which bucket an item joins under the active strategy.
    /// Pure function of `path` and `mimetype` — not of dynamic
    /// state — so a given (item, strategy) always lands in the
    /// same bucket regardless of arrival order.
    fn bucket_key(&self, path: &str, mimetype: &str) -> String {
        match self.cluster_strategy {
            ClusterStrategy::Single => String::new(),
            ClusterStrategy::ByMime => mimetype.to_string(),
            ClusterStrategy::ByExtension => match path.rsplit_once('.') {
                Some((_, ext)) if !ext.contains('/') => ext.to_string(),
                _ => String::new(),
            },
            ClusterStrategy::ByFirstPathSegment => {
                match path.split_once('/') {
                    Some((head, _)) => head.to_string(),
                    None => path.to_string(),
                }
            }
        }
    }

    /// Stream-process one item: bin-pack its body into the bucket
    /// it belongs to, flush that bucket if it overflows, record the
    /// pending dirent (committed at the bucket's next flush).
    fn push_item(&mut self, item: Item) -> Result<()> {
        let mime_idx = intern_mime(&item.mimetype, &mut self.mimes, &mut self.mime_index);
        let title = if item.title.is_empty() {
            item.path.clone()
        } else {
            item.title
        };
        let body = item.content;
        let body_len = body.len();
        self.stats.items_buffered += 1;
        self.stats.raw_bytes_total += body_len as u64;
        let key = self.bucket_key(&item.path, &item.mimetype);

        // Ensure the bucket exists, then check overflow against
        // *this* bucket's running size (not a global running size).
        let needs_flush = match self.buckets.get(&key) {
            Some(b) => !b.blobs.is_empty() && b.size_bytes + body_len > self.cluster_size_target,
            None => false,
        };
        if needs_flush {
            self.flush_bucket(&key)?;
        }

        let bucket = self.buckets.entry(key).or_default();
        let blob_idx = bucket.blobs.len() as u32;
        bucket.blobs.push(body);
        bucket.size_bytes += body_len;
        bucket.pending.push(PendingArticle {
            namespace: item.namespace.unwrap_or(b'C'),
            url: item.path,
            title,
            mime_idx,
            blob_idx,
        });
        Ok(())
    }

    /// Allocate a cluster_idx for this bucket's accumulated blobs,
    /// commit the bucket's pending dirents with that cluster_idx,
    /// and queue the raw blobs for parallel-batch encoding. The
    /// actual encode + write happens in `drain_pending_encode` when
    /// the queue fills (typically at `rayon::current_num_threads()`
    /// entries) or at finalize.
    fn flush_bucket(&mut self, key: &str) -> Result<()> {
        let bucket = match self.buckets.remove(key) {
            Some(b) if !b.blobs.is_empty() => b,
            // Empty bucket — nothing to do; reinsert default so the
            // map shape is stable across call patterns.
            Some(_) => {
                self.buckets.insert(key.to_string(), Bucket::default());
                return Ok(());
            }
            None => return Ok(()),
        };
        let cluster_idx = self.cluster_offsets.len() as u32;
        // Reserve the slot now (offset filled in when we write).
        self.cluster_offsets.push(0);
        // Commit dirents — this cluster_idx is final regardless of
        // when the encode completes.
        for pa in bucket.pending {
            self.dirents.push(RawDirent::Article {
                namespace: pa.namespace,
                url: pa.url,
                title: pa.title,
                mime_idx: pa.mime_idx,
                cluster: cluster_idx,
                blob: pa.blob_idx,
            });
        }
        // Track bytes incrementally; the cap-driven drain check
        // doesn't have to walk every queued cluster.
        let added_bytes: usize = bucket.blobs.iter().map(|b| b.len()).sum();
        self.pending_bytes += added_bytes;
        self.pending_encode.push((cluster_idx, bucket.blobs));
        // Reinsert empty bucket for reuse without map churn.
        self.buckets.insert(key.to_string(), Bucket::default());

        // Drain trigger: either we hit thread-pool size (CPU-
        // saturation default) OR the byte-cap is non-zero and the
        // queue's running total exceeded it. Cap takes priority —
        // a memory-conscious user wants a tight RSS bound even if
        // it means smaller (fewer-thread) parallel batches.
        let threads = rayon::current_num_threads().max(1);
        let drain = if self.max_in_flight_bytes > 0 {
            self.pending_bytes >= self.max_in_flight_bytes
                || self.pending_encode.len() >= threads
        } else {
            self.pending_encode.len() >= threads
        };
        if drain {
            self.drain_pending_encode()?;
        }
        Ok(())
    }

    /// Encode all queued clusters in parallel, then write them
    /// sequentially in cluster_idx order. Updates
    /// `cluster_offsets[i]` with the absolute file position where
    /// each cluster lands. Frees source blobs after encoding.
    fn drain_pending_encode(&mut self) -> Result<()> {
        if self.pending_encode.is_empty() {
            return Ok(());
        }
        let phase_start = Instant::now();
        let chunk = std::mem::take(&mut self.pending_encode);
        // Queue is empty after the take — reset the running byte
        // total so subsequent flushes start from zero.
        self.pending_bytes = 0;
        let chunk_len = chunk.len() as u64;
        let comp = self.compression;
        let level = self.compression_level;
        let mut encoded: Vec<(u32, Vec<u8>)> = chunk
            .into_par_iter()
            .map(|(idx, blobs)| {
                encode_cluster(&blobs, comp, level).map(|bytes| (idx, bytes))
            })
            .collect::<Result<Vec<_>>>()?;
        encoded.sort_by_key(|(idx, _)| *idx);
        let mut bytes_written = 0u64;
        for (idx, bytes) in encoded {
            self.cluster_offsets[idx as usize] = self.file_pos;
            bytes_written += bytes.len() as u64;
            self.file_mut().write_all(&bytes)?;
            self.file_pos += bytes.len() as u64;
        }
        self.stats.parallel_encode += phase_start.elapsed();
        self.stats.clusters_buffered += chunk_len;
        self.stats.bytes_clusters_written += bytes_written;
        Ok(())
    }

    /// Drain every non-empty bucket, in deterministic key order,
    /// then drain the encode queue so all clusters are on disk.
    /// Called once at finalize after all add_* work is done.
    fn flush_all_buckets(&mut self) -> Result<()> {
        let keys: Vec<String> = self
            .buckets
            .iter()
            .filter(|(_, b)| !b.blobs.is_empty())
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            self.flush_bucket(&k)?;
        }
        self.drain_pending_encode()?;
        Ok(())
    }

    /// Finish the build: drain metadata/illustrations/redirections
    /// into clusters and dirents, sort, render tables, fill mime
    /// list, write final header, MD5.
    fn finalize(mut self) -> Result<()> {
        // 1. Funnel buffered metadata + illustrations through the
        //    same bin-packing path as items so they share clusters
        //    with the tail of the user-content stream.
        let metadata = std::mem::take(&mut self.metadata);
        for m in metadata {
            self.push_item(Item {
                path: m.name.clone(),
                title: m.name,
                mimetype: m.mimetype,
                content: m.value,
                namespace: Some(b'M'),
            })?;
        }

        let illustrations = std::mem::take(&mut self.illustrations);
        for (side, png) in illustrations {
            let url = format!("Illustration_{side}x{side}@1");
            self.push_item(Item {
                path: url.clone(),
                title: url,
                mimetype: "image/png".to_string(),
                content: png,
                namespace: Some(b'M'),
            })?;
        }

        // 2a. Auto-emit `M/Counter` (mime histogram) before the
        //     final flush so it bin-packs into the same trailing
        //     cluster as other small metadata. Format matches
        //     real libzim: `mime=count;mime=count;…` (semicolon-
        //     separated, no trailing semicolon, mimes in mime-list
        //     index order). zimcheck consumes this for its mime
        //     report and some kiwix tooling reads it for stats.
        // Skip if a caller already added an `M/Counter` entry of
        // their own (e.g. zimrecreate forwarding the source ZIM's
        // value, or a test that wants exact bytes). We always
        // trust caller-supplied metadata over our auto-generated
        // version.
        let counter_already_set = self
            .dirents
            .iter()
            .any(|d| d.namespace() == b'M' && d.url() == "Counter");
        if !counter_already_set {
            let counter_str = build_counter_string(&self.mimes, &self.dirents);
            self.push_item(Item {
                path: "Counter".to_string(),
                title: "Counter".to_string(),
                mimetype: "text/plain;charset=utf-8".to_string(),
                content: counter_str.into_bytes(),
                namespace: Some(b'M'),
            })?;
        }

        // 2b. Close every still-open bucket — one cluster per
        //     non-empty bucket, in deterministic key order.
        self.flush_all_buckets()?;

        // 3. Build redirect dirents (no content). Includes the
        //    optional W/mainPage redirect.
        let mut pending_redirects: Vec<RawDirent> = Vec::new();
        let redirections = std::mem::take(&mut self.redirections);
        for r in redirections {
            let title = if r.title.is_empty() { r.path.clone() } else { r.title };
            pending_redirects.push(RawDirent::Redirect {
                namespace: b'C',
                url: r.path,
                title,
                target_ns: b'C',
                target_url: r.target_path,
                resolved_index: None,
            });
        }
        if let Some(m) = &self.main_path {
            pending_redirects.push(RawDirent::Redirect {
                namespace: b'W',
                url: "mainPage".to_string(),
                title: "mainPage".to_string(),
                target_ns: b'C',
                target_url: m.clone(),
                resolved_index: None,
            });
        }
        self.dirents.extend(pending_redirects);

        // 4. Sort dirents by (ns, url) → URL-pointer order.
        self.dirents.sort_by(|a, b| {
            a.namespace()
                .cmp(&b.namespace())
                .then_with(|| a.url().cmp(b.url()))
        });

        // 5. Resolve redirect targets to URL-pointer-order indices.
        for i in 0..self.dirents.len() {
            if let RawDirent::Redirect {
                target_ns,
                target_url,
                ..
            } = &self.dirents[i]
            {
                let ns = *target_ns;
                let url = target_url.clone();
                let resolved = self
                    .dirents
                    .binary_search_by(|d| d.namespace().cmp(&ns).then_with(|| d.url().cmp(&url)))
                    .ok()
                    .map(|j| j as u32);
                match &mut self.dirents[i] {
                    RawDirent::Redirect { resolved_index, .. } => *resolved_index = resolved,
                    _ => unreachable!(),
                }
            }
        }
        for d in &self.dirents {
            if let RawDirent::Redirect {
                resolved_index: None,
                url,
                target_ns,
                target_url,
                ..
            } = d
            {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "redirect {} -> {}/{} has no target",
                        url,
                        char::from(*target_ns),
                        target_url
                    ),
                )));
            }
        }

        // 6a. Compute the title-pointer order for the dirents we
        //     have so far. This becomes the body of the
        //     `X/listing/titleOrdered/v1` listing entry we're
        //     about to emit. The listing entry itself isn't in
        //     this snapshot — its content lists every other
        //     entry's URL-pointer index in title order, but not
        //     its own. Real libzim's reader doesn't need the
        //     listing entry to refer to itself, so this is fine.
        let title_order_snapshot: Vec<u32> = {
            let mut t: Vec<u32> = (0..self.dirents.len() as u32).collect();
            t.sort_by(|&a, &b| {
                let da = &self.dirents[a as usize];
                let db = &self.dirents[b as usize];
                da.namespace()
                    .cmp(&db.namespace())
                    .then_with(|| da.title().cmp(db.title()))
            });
            t
        };

        // 6b. Emit `X/listing/titleOrdered/v1`. Its body is the
        //     `title_order_snapshot` serialised as little-endian
        //     u32s (4 bytes per entry). Real libzim writes this
        //     entry uncompressed; ours goes through the same zstd
        //     path as everything else, which is fine — the
        //     reader-side just zstd-decodes and indexes into the
        //     resulting bytes. Mimetype matches real libzim's so
        //     `zimcheck -A` is happy.
        //
        //     Skip if a caller already added an
        //     `X/listing/titleOrdered/v1` (e.g. zimrecreate
        //     forwarding from the source ZIM).
        let listing_already_set = self
            .dirents
            .iter()
            .any(|d| d.namespace() == b'X' && d.url() == "listing/titleOrdered/v1");
        if !listing_already_set {
            let mut listing_bytes: Vec<u8> = Vec::with_capacity(title_order_snapshot.len() * 4);
            for idx in &title_order_snapshot {
                listing_bytes.extend_from_slice(&idx.to_le_bytes());
            }
            self.push_item(Item {
                path: "listing/titleOrdered/v1".to_string(),
                title: "listing/titleOrdered/v1".to_string(),
                mimetype: "application/octet-stream+zimlisting".to_string(),
                content: listing_bytes,
                namespace: Some(b'X'),
            })?;
            // Flush so the listing's cluster commits and its
            // dirent lands in `self.dirents` for the final sort
            // below.
            self.flush_all_buckets()?;
        }

        // 6b.5. Drain background streaming-encode tasks now that
        //       the producer is done. Each completed temp file's
        //       compressed bytes are spliced into the main output
        //       in cluster_idx order; cluster_offsets slots that
        //       were placeholders get their final positions.
        self.drain_streaming_tasks()?;

        // 6c. Re-sort dirents with the new listing entry in place.
        //     Adding an X-namespace entry doesn't shift C-, M-, or
        //     W-namespace URL-pointer indices, so resolved redirect
        //     targets (always C in our pipeline) stay valid — no
        //     re-resolution needed.
        self.dirents.sort_by(|a, b| {
            a.namespace()
                .cmp(&b.namespace())
                .then_with(|| a.url().cmp(b.url()))
        });

        // 6d. Final title-pointer order — over all dirents
        //     including the listing entry. This is what gets
        //     written to the title pointer table.
        let mut title_order: Vec<u32> = (0..self.dirents.len() as u32).collect();
        title_order.sort_by(|&a, &b| {
            let da = &self.dirents[a as usize];
            let db = &self.dirents[b as usize];
            da.namespace()
                .cmp(&db.namespace())
                .then_with(|| da.title().cmp(db.title()))
        });

        // 7. Compute layout positions for the trailing tables.
        let entry_count = self.dirents.len() as u32;
        let cluster_count = self.cluster_offsets.len() as u32;

        let url_ptr_pos = self.file_pos;
        let title_ptr_pos = url_ptr_pos + (entry_count as u64) * 8;
        let cluster_ptr_pos = title_ptr_pos + (entry_count as u64) * 4;
        let dirents_pos = cluster_ptr_pos + (cluster_count as u64) * 8;

        // 8. Render dirents; compute their absolute offsets.
        let mut dirent_blobs: Vec<Vec<u8>> = Vec::with_capacity(self.dirents.len());
        let mut dirent_offsets: Vec<u64> = Vec::with_capacity(self.dirents.len());
        let mut cursor = dirents_pos;
        for d in &self.dirents {
            let bytes = encode_dirent(d);
            dirent_offsets.push(cursor);
            cursor += bytes.len() as u64;
            dirent_blobs.push(bytes);
        }
        let checksum_pos = cursor;

        // 9. Find main page index (in URL order).
        let main_page_idx = if self.main_path.is_some() {
            self.dirents
                .binary_search_by(|d| {
                    d.namespace()
                        .cmp(&b'W')
                        .then_with(|| d.url().cmp("mainPage"))
                })
                .ok()
                .map(|i| i as u32)
                .unwrap_or(u32::MAX)
        } else {
            u32::MAX
        };

        // 10. Write the trailing tables in one forward pass.
        let table_phase = Instant::now();
        for off in &dirent_offsets {
            self.file_mut().write_all(&off.to_le_bytes())?;
        }
        for idx in &title_order {
            self.file_mut().write_all(&idx.to_le_bytes())?;
        }
        let cluster_offsets = std::mem::take(&mut self.cluster_offsets);
        for off in &cluster_offsets {
            self.file_mut().write_all(&off.to_le_bytes())?;
        }
        for blob in &dirent_blobs {
            self.file_mut().write_all(blob)?;
        }
        drop(dirent_blobs);
        drop(dirent_offsets);
        self.stats.table_write += table_phase.elapsed();

        // 11. Encode the mime list and write at offset 80 (the rest
        //     of the reserved region stays zeros, which is harmless —
        //     readers stop at the double-NUL terminator).
        let mime_list_bytes = encode_mime_list(&self.mimes);
        if mime_list_bytes.len() > MIME_LIST_RESERVE {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "mime list ({} bytes) exceeds reserved region ({} bytes)",
                    mime_list_bytes.len(),
                    MIME_LIST_RESERVE
                ),
            )));
        }
        self.file_mut().seek(SeekFrom::Start(HEADER_SIZE as u64))?;
        self.file_mut().write_all(&mime_list_bytes)?;

        // 12. Write final header at offset 0.
        let final_header = encode_header(&HeaderFields {
            major_version: 6,
            minor_version: 3,
            uuid: self.uuid,
            entry_count,
            cluster_count,
            url_ptr_pos,
            title_ptr_pos,
            cluster_ptr_pos,
            mime_list_pos: HEADER_SIZE as u64,
            main_page: main_page_idx,
            checksum_pos,
        });
        self.file_mut().seek(SeekFrom::Start(0))?;
        self.file_mut().write_all(&final_header)?;
        self.file_mut().flush()?;

        // 13. MD5 over [0, checksum_pos) by re-reading the now-final
        //     file. SSDs do this at ~1 GB/s; small relative to the
        //     cluster-encode time we just saved by streaming.
        let md5_phase = Instant::now();
        self.file_mut().seek(SeekFrom::Start(0))?;
        let mut hasher = Md5::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut hashed: u64 = 0;
        while hashed < checksum_pos {
            let want = ((checksum_pos - hashed) as usize).min(buf.len());
            let n = self.file_mut().read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            hashed += n as u64;
        }
        let digest: [u8; 16] = hasher.finalize().into();
        self.file_mut().seek(SeekFrom::Start(checksum_pos))?;
        self.file_mut().write_all(&digest)?;
        self.file_mut().flush()?;
        self.stats.md5_pass += md5_phase.elapsed();

        // 14. Verify the just-written archive opens cleanly and
        //     passes every structural check we have. This is the
        //     reliability gate the user asked for: every successful
        //     `finish_writing` returns only after the produced ZIM
        //     reads back with sorted dirents, valid pointer tables,
        //     resolvable mimetypes, and a matching MD5. Failure
        //     here means our writer produced something we can't
        //     read — surface that loudly rather than ship it.
        //
        //     The verify step is a single forward pass over the
        //     file (mmap + zimru's checks); cost is sub-second on
        //     small ZIMs, ~1-2 s per GB on big ZIMs.
        let path = self.output_path.clone();
        let started = self.started;
        let mut stats = std::mem::take(&mut self.stats);
        // Drop our File handle before the Archive opens it — keeps
        // ownership clean and avoids the verify path racing on a
        // still-open writer fd.
        drop(self);

        let verify_phase = Instant::now();
        verify_archive(&path)?;
        stats.verify_pass += verify_phase.elapsed();

        // Print only when the user explicitly asks via env var, so
        // routine cargo-test runs stay quiet.
        if std::env::var_os("ZIMRU_STATS").is_some() {
            let output_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            stats.print_summary(started, output_size);
        }
        Ok(())
    }
}

/// Re-open `path` and run every integrity check zimru exposes.
/// Returns `Err` with a descriptive message on the first failure.
fn verify_archive(path: &Path) -> Result<()> {
    let arc = crate::archive::Archive::open(path)?;
    let checks: &[(&str, &dyn Fn(&crate::archive::Archive) -> Result<bool>)] = &[
        ("dirent_ptrs", &|a| a.check_dirent_ptrs()),
        ("dirent_order", &|a| a.check_dirent_order()),
        ("title_index", &|a| a.check_title_index()),
        ("cluster_ptrs", &|a| a.check_cluster_ptrs()),
        ("mimetypes", &|a| a.check_mimetypes()),
        ("md5_checksum", &|a| a.check()),
    ];
    for (name, check) in checks {
        let ok = check(&arc).map_err(|e| {
            Error::Io(std::io::Error::other(format!(
                "post-write verify: {name} failed during check: {e}"
            )))
        })?;
        if !ok {
            return Err(Error::Io(std::io::Error::other(format!(
                "post-write verify: {name} reported the archive is malformed"
            ))));
        }
    }
    Ok(())
}


struct HeaderFields {
    major_version: u16,
    minor_version: u16,
    uuid: [u8; 16],
    entry_count: u32,
    cluster_count: u32,
    url_ptr_pos: u64,
    title_ptr_pos: u64,
    cluster_ptr_pos: u64,
    mime_list_pos: u64,
    main_page: u32,
    checksum_pos: u64,
}

fn encode_header(h: &HeaderFields) -> Vec<u8> {
    let mut buf = vec![0u8; HEADER_SIZE];
    buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    buf[4..6].copy_from_slice(&h.major_version.to_le_bytes());
    buf[6..8].copy_from_slice(&h.minor_version.to_le_bytes());
    buf[8..24].copy_from_slice(&h.uuid);
    buf[24..28].copy_from_slice(&h.entry_count.to_le_bytes());
    buf[28..32].copy_from_slice(&h.cluster_count.to_le_bytes());
    buf[32..40].copy_from_slice(&h.url_ptr_pos.to_le_bytes());
    buf[40..48].copy_from_slice(&h.title_ptr_pos.to_le_bytes());
    buf[48..56].copy_from_slice(&h.cluster_ptr_pos.to_le_bytes());
    buf[56..64].copy_from_slice(&h.mime_list_pos.to_le_bytes());
    buf[64..68].copy_from_slice(&h.main_page.to_le_bytes());
    buf[68..72].copy_from_slice(&u32::MAX.to_le_bytes()); // layout_page (deprecated)
    buf[72..80].copy_from_slice(&h.checksum_pos.to_le_bytes());
    buf
}

fn encode_mime_list(mimes: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in mimes {
        out.extend_from_slice(m.as_bytes());
        out.push(0);
    }
    out.push(0); // terminator
    out
}

fn encode_dirent(d: &RawDirent) -> Vec<u8> {
    let mut out = Vec::new();
    match d {
        RawDirent::Article {
            namespace,
            url,
            title,
            mime_idx,
            cluster,
            blob,
        } => {
            out.extend_from_slice(&mime_idx.to_le_bytes());
            out.push(0); // parameter_len
            out.push(*namespace);
            out.extend_from_slice(&0u32.to_le_bytes()); // revision
            out.extend_from_slice(&cluster.to_le_bytes());
            out.extend_from_slice(&blob.to_le_bytes());
            out.extend_from_slice(url.as_bytes());
            out.push(0);
            if title != url {
                out.extend_from_slice(title.as_bytes());
            }
            out.push(0);
        }
        RawDirent::Redirect {
            namespace,
            url,
            title,
            resolved_index,
            ..
        } => {
            let idx = resolved_index.unwrap_or(0);
            out.extend_from_slice(&0xFFFFu16.to_le_bytes());
            out.push(0);
            out.push(*namespace);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&idx.to_le_bytes());
            out.extend_from_slice(url.as_bytes());
            out.push(0);
            if title != url {
                out.extend_from_slice(title.as_bytes());
            }
            out.push(0);
        }
    }
    out
}

fn encode_cluster(
    blobs: &[Vec<u8>],
    compression: Compression,
    level: Option<i32>,
) -> Result<Vec<u8>> {
    let n = blobs.len();
    let payload_bytes_sum: usize = blobs.iter().map(|b| b.len()).sum();
    // 4-byte offsets unless the total would overflow u32.
    let ptr_size_4 = (n + 1) * 4;
    let total_4 = ptr_size_4 + payload_bytes_sum;
    let extended = total_4 > u32::MAX as usize;
    let ptr_size = if extended { 8 } else { 4 };
    let header_len = (n + 1) * ptr_size;

    let mut payload = Vec::with_capacity(header_len + payload_bytes_sum);
    let mut cursor: u64 = header_len as u64;
    for b in blobs {
        push_offset(&mut payload, cursor, extended);
        cursor += b.len() as u64;
    }
    push_offset(&mut payload, cursor, extended);
    for b in blobs {
        payload.extend_from_slice(b);
    }

    // When the caller didn't pin a compression level, honour
    // `ZSTD_CLEVEL` / `XZ_DEFAULTS` env vars so cross-stack tooling
    // can configure both real libzim (which respects libzstd's env)
    // and zimru with the same one-liner. Falls back to a fast level
    // 3 default for zstd / 3 for xz when no env var is set.
    fn env_zstd_level() -> Option<i32> {
        std::env::var("ZSTD_CLEVEL").ok().and_then(|v| v.parse().ok())
    }
    let (compression_id, body) = match compression {
        Compression::None => (1u8, payload),
        Compression::Zstd => {
            let lvl = level.or_else(env_zstd_level).unwrap_or(3);
            (
                5u8,
                zstd::stream::encode_all(&payload[..], lvl)
                    .map_err(|e| Error::Decompression(format!("zstd encode: {e}")))?,
            )
        }
        Compression::Xz => {
            let lvl = level.unwrap_or(3).clamp(0, 9) as u32;
            (4u8, {
                let mut enc = xz2::write::XzEncoder::new(Vec::new(), lvl);
                enc.write_all(&payload)
                    .map_err(|e| Error::Decompression(format!("xz encode: {e}")))?;
                enc.finish()
                    .map_err(|e| Error::Decompression(format!("xz finish: {e}")))?
            })
        }
    };

    let info_byte = compression_id | (if extended { 0x10 } else { 0 });
    let mut bytes = Vec::with_capacity(1 + body.len());
    bytes.push(info_byte);
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

fn push_offset(v: &mut Vec<u8>, x: u64, extended: bool) {
    if extended {
        v.extend_from_slice(&x.to_le_bytes());
    } else {
        v.extend_from_slice(&(x as u32).to_le_bytes());
    }
}

fn default_uuid() -> [u8; 16] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let pid = std::process::id() as u64;
    let mut u = [0u8; 16];
    u[..8].copy_from_slice(&nanos.to_le_bytes());
    u[8..].copy_from_slice(&pid.wrapping_mul(0x9E3779B97F4A7C15).to_le_bytes());
    u
}
