# DS4 sweep review (perf focus) — zimru

Exhaustive per-file pass: 57 code files across 6 batches.

## Findings

# Batch 1 — performance findings (zimru)

- [medium] src/archive.rs:637 — `entry_by_ns_title` linear fallback scans `for i in 0..n` parsing **every** dirent (n = `header.entry_count`, e.g. 700 K on a big Wikipedia) to satisfy a title lookup in an M/W/X namespace of a modern archive — O(n) dirent parses + 2 `String` allocations (`url.to_string()`, `title.to_string()`) per dirent, per lookup — a single metadata/index title lookup becomes a full-archive scan (~700 K parses, tens of ms). Smallest safe fix: restrict the fallback to the namespace's URL-pointer range via `namespace_range(ns)` (URL pointers are sorted by `(ns, url)`, so M/W/X entries are contiguous `start..end`), scanning only those dirents.

- [medium] src/archive.rs:734 — `get_metadata_keys` loops `for i in 0..n` parsing every dirent (2 `String` allocs each) just to collect M-namespace keys, even though the M namespace is tiny — O(n) over the whole archive per call; `illustrations()` calls it and pays the same full scan. Smallest safe fix: iterate `namespace_range(NS_METADATA)` (contiguous via the sorted URL-pointer list) instead of `0..n`.

- [medium] src/archive.rs:1042 — `blob_direct_access` calls `self.cluster(cluster_idx)` which runs a full `Cluster::parse`: for `Compression::None` that copies the **entire** cluster body into a fresh heap `Arc<[u8]>` (`Arc::from(body.to_vec())` in `src/cluster.rs`) and inserts it into the 64 MB byte-budget LRU cache — for a multi-GB uncompressed Xapian index cluster this is a full heap copy + memcpy, and it evicts hot article clusters on a path whose whole purpose is to hand an fd/lseek to libxapian *without* copying bytes. Smallest safe fix: for `Compression::None` read the blob offset table directly out of the mmap (`cluster_range.start + 1`) and compute `r.start`/`r.end` without materializing/caching the payload.

- [medium] src/bin/_index_helper.rs:184 — `feed_title` and (line 201) `feed_fulltext` allocate a fresh `Vec<u8>` (`encode_title_doc`/`encode_fulltext_doc`, each `Vec::with_capacity`) and call `write_raw` → `stdin.write_all(buf)` — one heap allocation + one write syscall to the `xapianbuilder` child **per entry** for every content/HTML entry of the archive (hundreds of thousands on a real Wikipedia) — per-item I/O + allocation churn on the writer hot path; the child is also a single consumer of many small writes. Smallest safe fix: keep a reusable write buffer in `Job` and batch/`flush` once per N bytes or on `finish()`, reusing the buffer across entries.

## Coverage

.github/workflows/ci.yml — clean
bench/cache_aware_compare.sh — clean
bench/parity.sh — clean
bench/recreate-bench.sh — clean
bench/recreate-suite.sh — clean
bench/run.sh — clean
benches/cluster_decode.rs — clean
benches/get_entry_by_path.rs — clean
benches/warmup.rs — clean
build.rs — clean
Cargo.toml — clean
cbindgen.toml — clean
include/zimru.h — clean
src/archive.rs — findings: 3
src/bin/_index_helper.rs — findings: 1
src/bin/zimbench.rs — clean

# Perf review — batch-2 (zimru)

Files reviewed for performance only (Rust). Findings below.

- [medium] src/bin/zimwriterfs.rs:734 — `mime_for_path` rebuilds a `HashMap<&str,&str>` from the static `TABLE` on **every call**: `let map: HashMap<&str, &str> = TABLE.iter().copied().collect();` — called once per file from the per-file parallel-read closure (line 466) and the streaming path (line 386). With hundreds of thousands of files (Wikipedia-scale `zimwriterfs`) this is ~40 hash inserts + an allocation per file, i.e. allocation churn in a loop over file count. — Fix: hoist to a `static`/`OnceCell` map (or a plain `match` on the extension), built once; `mime_for_path` then just does the lookup + `to_string`.

- [medium] src/bin/zimwriterfs.rs:814 — `derive_title_from_bytes` materializes a full lowercase copy of every HTML body before searching for the title tag: `let lower = text.to_ascii_lowercase();` where `text` is the entire HTML document (`std::str::from_utf8(bytes)`). Called once per HTML file in the batch closure (line 474). On a Wikipedia-shaped build this is one full-document allocation per HTML article (GBs of redundant allocation/zeroing over a build). — Fix: case-insensitive scan without materializing (e.g. search the raw bytes for `b"<title>"`/`</title>` with a manual case-insensitive compare, or only lowercase the candidate window) so no O(file-size) copy is needed.

- [medium] src/bin/zimcheck.rs:728 — internal-URL check performs one `arc.entry_by_ns_path(b'C', &resolved)` **per link target** inside the per-cluster scan. `entry_by_ns_path` is an O(log N) binary search over the URL-pointer list (14 probes on a 17k-entry archive), so total cost is O(L·log N) where L is the number of internal links across all HTML articles — a full `-A` zimcheck of a large archive does one archive lookup per link. — Fix: build a `HashSet<String>` (or `HashSet<&str>` over the dirent region) of every content-namespace path once before the parallel scan, then check membership in O(1) instead of per-link binary search; the `entry_by_ns_path` call is only needed to confirm existence, so the set answers it.

- [low] src/cffi/archive.rs:708,727,838 — the archive's `interned: Mutex<Vec<CString>>` pointer-stability cache grows monotonically with **call count, not distinct keys**: `zimru_archive_metadata` appends a freshly-built CString on every call (line 727) and `zimru_archive_metadata_key` appends on every index (line 838), with no dedup or eviction. A long-lived server that periodically re-fetches the same metadata key grows the vec one CString per call for the archive's lifetime. — Fix: deduplicate by key (e.g. a `Mutex<HashMap<String, CString>>` that reuses the entry for repeated keys), or cap/clear the interning vec; repeated lookups of the same key then stop allocating.

## Coverage
- src/bin/zimcheck.rs — findings: 3
- src/bin/zimdump.rs — clean
- src/bin/zimrecreate.rs — clean
- src/bin/zimru.rs — clean
- src/bin/zimsplit.rs — clean
- src/bin/zimwriterfs.rs — findings: 2
- src/cffi/archive.rs — findings: 1
- src/cffi/blob.rs — clean
- src/cffi/creator.rs — clean
- src/cffi/entry.rs — clean
- src/cffi/error.rs — clean
- src/cffi/item.rs — clean
- src/cffi/mod.rs — clean
- src/cffi/uuid.rs — clean
- src/cluster.rs — clean
- src/dirent.rs — clean
- src/error.rs — clean
- src/header.rs — clean
- src/io_hints.rs — clean
- src/lib.rs — clean
# batch-3 performance findings

Files reviewed (PERFORMANCE ONLY, rust-performance-review lens): src/mime.rs, src/raw.rs, src/uuid.rs, src/writer.rs.

## Findings

- [medium] src/writer.rs:1745 — `bucket_key` allocates an owned `Cow::Owned(format!("{tag}|{strat}"))` per item for every non-default cluster strategy (ByMime / ByExtension / ByFirstPathSegment), i.e. one heap `String` allocation on every `push_item` call in the streaming hot path — the code comment at 1726-1733 already notes the default `Single` strategy is the allocation-free fast path. On a multi-million-item build (e.g. a Wikipedia ZIM) this is millions of small `format!` allocations + frees. Smallest safe fix: key the `buckets` map by the borrowed strategy string (`Cow::Borrowed(mimetype)`/ext/head — those `&str` already live in the item) plus the `compression_tag` encoded as a separate cheap key (e.g. a `(u8, &str)`-style composite or storing compression in the Bucket, which it already does), eliminating the per-item `format!`.

- [medium] src/writer.rs:2089-2096 — `finalize` performs a second full title sort (`title_order.sort_unstable_by(...)` over all dirents) immediately after `title_order_snapshot` was already sorted by the same comparator at 2021; the only element added between the two sorts is the single `X/listing/titleOrdered/v1` dirent. On a ~1M-dirent build this is a redundant second O(n log n) sort (~20M title-string comparisons). Smallest safe fix: reuse `title_order_snapshot` and binary-search-insert the one new listing entry's (remapped) index instead of re-sorting all n dirents.

- [medium] src/writer.rs:1767-1789 — `push_item` performs up to 3-6 BTreeMap probes per item on the streaming hot path: `buckets.get(key.as_ref())` for the flush check, then `contains_key` + `get_mut` (and, on flush, `remove` + `insert`) — the `get`/`contains_key`/`get_mut` are three probes of the same key. On multi-million-item builds with ByMime/ByExtension (many buckets) this is repeated log-n map probing per item. Smallest safe fix: restructure to a single `entry`/`get_mut` probe (compute `needs_flush` from the borrowed bucket, then flush via `get_mut`/`remove` on the same key) to collapse redundant lookups.

- [low] src/writer.rs:1503-1507 — `chunked_item_chunk` (StreamingZstd arm) calls `Instant::now()` twice per chunk just to accumulate `self.stats.streaming_encode`; the StreamingRaw arm at 1529-1533 does the same. A huge item streamed in many small chunks (e.g. a multi-GB body fed in 64 KiB chunks) generates millions of `clock_gettime` syscalls that do no work for the output. Smallest safe fix: accumulate bytes per chunk and sample `Instant` at coarser checkpoints (e.g. every N MiB) or drop per-chunk timing on the streaming path.

- [low] src/writer.rs:1845-1850 — `flush_all_buckets` clones every bucket key into a `Vec<String>` (one alloc per bucket), then `flush_bucket` re-inserts the empty bucket with `key.to_string()` (another alloc per bucket) at 1806/1835 and does a `remove`+`insert` node churn per flush. Bounded by distinct bucket keys (mime/ext diversity) rather than item count, but on ByMime builds with thousands of distinct mimes this doubles key allocations per flush. Smallest safe fix: iterate bucket keys by reference (`for k in self.buckets.keys()...`) and use `entry`/`get_mut` to avoid the remove+reinsert and the second `to_string()`.

## Coverage
- src/mime.rs — clean
- src/raw.rs — clean
- src/uuid.rs — clean
- src/writer.rs — findings: 5
# Batch 4 — performance findings (zimru)

No findings. Every file in this batch is a Rust test, a benchmark example, or a C/C++ FFI smoke program that runs over tiny, fixed, bounded fixtures (a handful of articles, a 6 MiB synthetic payload, a 5-entry temp archive). None is an application hot path or request handler; per the performance-review skill, test fixtures are out of scope for findings except to establish data sizes. The only files that touch scaled data are `examples/writer_bench.rs` (a benchmark whose whole purpose is to stream 20 000 synthetic items) and `tests/real_files.rs` (an integration test that intentionally walks every entry / decompresses every cluster of downloaded real ZIMs); that work is the tests' stated purpose, not a production cost.

Dismissed false positives (with input-size reasoning):
- `tests/real_files.rs:92-102` — title-order iteration calls `e.title().to_string()` and `prev = Some(key.clone())` per entry over the whole archive, i.e. allocation per entry. This is test code whose purpose is to walk every entry; the cost is bounded by the test workload and is not on any app request path.
- `tests/ergonomic_api.rs:194-197` — `title_order.sort_by_key` calls `a.title.to_string()` per key inside the comparator. The fixture is 9 entries (bounded), so the allocation cost is negligible and not O(n²)-driven by growing data.
- `examples/writer_bench.rs:91-108` — `format!("article{i}")` / `format!("Article {i}")` per item and `make_body` word-by-word `extend_from_slice` are inherent to a synthetic benchmark; `make_body` preallocates (`Vec::with_capacity(target_len + 16)`), so there is no realloc churn. The measured cost is the benchmark's purpose.
- `tests/streaming_encode_smoke.rs:35-37` — `flat_map`+`collect` builds a 6 MiB payload with no `size_hint` (Vec grows by doubling), but the size is a fixed 6 MiB test constant, so the reallocs are bounded and trivial.

## Coverage

examples/writer_bench.rs — clean
tests/cffi_smoke.c — clean
tests/cffi_smoke.cpp — clean
tests/cffi_smoke.rs — clean
tests/cffi_writer_smoke.c — clean
tests/cffi_writer_smoke.rs — clean
tests/direct_access.rs — clean
tests/ergonomic_api.rs — clean
tests/integrity_checks.rs — clean
tests/per_item_compression.rs — clean
tests/real_files.rs — clean
tests/streaming_encode_smoke.rs — clean
tests/synthetic_zim.rs — clean
tests/writer_roundtrip.rs — clean
tests/xapianbuilder_helper.rs — clean
tests/zimdump_analyze.rs — clean
tests/zimwriterfs_e2e.rs — clean
# Batch 5 — performance review (zimru)

Findings for: include/zimru.h, src/bin/zimdump.rs, src/cffi/creator.rs, src/bin/zimrecreate.rs
Checklist: performance-review + rust-performance-review lens. This is a Rust CLI/FFI codebase; no MLX/Metal/audio/SQL/Flutter/JS stacks apply. No external perf linters installed (no ruff/staticcheck/clippy run in this env).

- [high] src/bin/zimdump.rs:368-393 — `cmd_analyze --by-item` re-decompresses every cluster once per blob, in URL order, thrashing the 64 MB LRU cluster cache — `let item = match e.get_item(false) { Ok(it) => it, Err(_) => continue }; let blob_size = item.size().unwrap_or(0);` per entry. `item.size()` (archive.rs:1528) calls `get_data()` -> `pinned_blob()` -> `load_cluster()` which decompresses and caches the cluster, while the first pass (line 350 `arc.cluster_uncached(idx)` for the same clusters) bypasses that cache entirely. URL order visits clusters near-randomly, so with a ~30-cluster budget the LRU evicts and each cluster is decoded ~once per blob in it: O(total blobs) cluster decodes instead of O(clusters). — On a Wikipedia-scale archive (millions of blobs) `zimdump analyze --by-item` does tens-to-hundreds of redundant zstd decodes per cluster; the codebase's own dump Pass-2 comment (lines 552-558) documents this exact thrash as ">5 minutes of redundant zstd work" on a 1.1 GB archive. — Smallest safe fix: in by-item mode group entries by cluster and decode each cluster exactly once via `cluster_uncached`, sizing each blob from that single cluster's `blob(b)` (same shape as cmd_dump Pass 2, lines 564-583); don't call `item.size()` per entry.
- [low] src/bin/zimdump.rs:283-306 — `print_list_entry` with `details` calls `e.get_item(false)` then `item.size()` per listed entry (`let item = e.get_item(false)...; if let Ok(sz) = item.size()`), which decompresses the item's cluster through the shared LRU cache. A whole-namespace `zimdump list --details` walks entries in URL order and re-decodes the same clusters many times as the 64 MB cache evicts. — `list --details` on a large archive spends most of its time re-decompressing clusters rather than listing; latency scales with total blobs instead of total clusters. — Fix: group by cluster and decode once per cluster (reuse the cluster to read each blob size), or drop to per-cluster decode like dump.
- [low] src/bin/zimdump.rs:635-640 — `write_entry` calls `fs::create_dir_all(parent)` for every dumped entry (`if let Some(parent) = dest.parent() { fs::create_dir_all(parent)?; } fs::write(dest, data)`), re-stat/re-creating the full ancestor chain once per file. A multi-million-entry dump issues O(depth) stat/mkdir syscalls per entry. — Millions of redundant directory syscalls dominate dump wall-time on big archives (the per-entry file write itself is inherent). — Fix: memoize already-created parent dirs (per rayon thread) and skip `create_dir_all` when the parent was already ensured, creating dirs once per directory instead of once per file.
- [medium] src/bin/zimrecreate.rs:355 — `let mut item = Item::new(p.path.clone(), p.title.clone(), p.mime.clone(), data);` clones the three Strings that `pending[i]` already owns, then `Item::new` re-copies them via `Into<String>`; the `PendingContent` strings are never moved out and are dropped at line 363. Every C-namespace article pays 3 redundant String allocations on top of the ones already allocated when building `pending` (lines 285-291). — On a millions-of-articles archive this is ~3 × total-articles extra heap allocations (plus their copies) in the encode hot path, measurable peak-RSS and allocation churn. — Fix: take the strings out of `pending[i]` instead of cloning, e.g. `Item::new(std::mem::take(&mut p.path), std::mem::take(&mut p.title), std::mem::take(&mut p.mime), data)` (iterate `pending` by mutable index / by-value).
- [low] src/bin/zimrecreate.rs:348-353 — `indexer.feed_title(...)` / `indexer.feed_fulltext(...)` are called once per front article; each `feed_*` does one unbuffered `stdin.write_all(buf)` pipe write to the xapianbuilder child (write_raw in _index_helper.rs:284-296). On a millions-of-articles archive that is millions of single write() syscalls to the index subprocess. — Recreate's index-feed phase pays ~2 syscalls per article; wall-time grows linearly with entry count at syscall rate. — Fix: wrap the held `ChildStdin` in a `BufWriter` (flush once at `finish()`/`write_raw` boundaries) so many entries batch into one pipe write.

## Coverage
include/zimru.h — clean
src/bin/zimdump.rs — findings: 3
src/cffi/creator.rs — clean
src/bin/zimrecreate.rs — findings: 2
# Batch 6 — performance review (zimru)

Files: src/cffi/item.rs, src/bin/zimru.rs, src/dirent.rs.
Checklist: performance-review + rust-performance-review lens. Rust FFI/CLI/library code; no MLX/Metal/audio/SQL/Flutter/JS stacks apply. No external perf linters installed in this env.

- [high] src/cffi/item.rs:152 — `zimru_item_direct_access` calls `(*it).inner.archive().blob_direct_access(cluster, blob)` which (archive.rs:1041-1043) invokes `self.cluster(cluster_idx)` → `load_cluster` → `Cluster::parse`; for `Compression::None` clusters that runs `Arc::from(body.to_vec())` (cluster.rs:73), a full heap copy + memcpy of the **entire on-disk cluster**, which is then inserted into the LRU cluster cache. This contradicts the function's own contract (item.rs:118-125 "no decompression, no copy ... instead of materialising the database in memory"). On a direct-access item that lives in a multi-GB uncompressed Xapian index cluster, a single call copies the whole cluster into RAM, pins it in the cache (the byte-budget evictor keeps an oversized entry pinned), and evicts hot article clusters — exactly the path the FFI exists to avoid. — Consequence: multi-GB allocation + memcpy + permanent cache residency per call on the zero-copy direct-access API. — Smallest safe fix: in `blob_direct_access`, for `Compression::None`, read the blob offset table straight out of the mmap region (`cluster_range.start + 1`) to compute `r.start`/`r.end` and the file offset, without `load_cluster`/materializing the payload (mirrors how `cluster_raw` already bounds the region).

- [medium] src/cffi/item.rs:107 — `zimru_item_size` calls `(*it).inner.size()` (archive.rs:1528-1531) which runs `get_data()` → `pinned_blob()` → `load_cluster()`, decompressing/materializing and caching the whole cluster just to report one blob's decompressed size. For a direct-access (uncompressed) item the size is already available from `blob_direct_access` with no copy, but `size()` ignores it; on a large index cluster `zimru_item_size` copies the entire cluster into RAM and pins it in the cache. — Consequence: multi-GB copy + cache residency to answer a u64 size query on the direct-access hot path. — Smallest safe fix: in `Item::size()`, when the item is direct (`blob_direct_access(...).is_direct`), return `direct.size` without calling `get_data`; keep the decompress path only for compressed clusters.

- [low] src/dirent.rs:78 — `Dirent::parse` on an empty-title dirent calls `url.to_string()` twice (line 78 for the `url` field, line 80 for the title fallback `if title.is_empty() { url.to_string() }`), and the same double-allocation pattern repeats at lines 98-100 and 117-121. `parse` runs on every `entry_by_url_index`/`entry_by_title_index` and on the matched leaf of every `entry_by_ns_path`/`entry_by_ns_title` binary search, so each hot lookup/iteration on a dirent with no title pays one redundant String allocation. On a 700 K-entry Wikipedia archive that is ~700 K wasted allocs across listing/iteration/lookup paths. — Smallest safe fix: compute `let url_s = url.to_string();` once per branch and reuse it for both `url` and the empty-title fallback.

## Coverage
src/cffi/item.rs — findings: 2
src/bin/zimru.rs — clean
src/dirent.rs — findings: 1

## Run stats

Engine throughput (weighted across batches): prefill 73594 tok @ 2284 t/s, generated 19902 tok @ 24.0 t/s (6 batches)
