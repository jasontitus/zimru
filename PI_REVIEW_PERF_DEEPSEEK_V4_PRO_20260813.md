# Pi sweep review (perf focus) — zimru-f516830a

Exhaustive per-file pass: 57 code files across 6 batches.

## Findings

- [high] src/archive.rs:1043 — `blob_direct_access` calls `self.cluster(cluster_idx)` to answer an offset/size query. For an uncompressed cluster that path is `Cluster::parse` → `Arc::from(body.to_vec())`, i.e. a full heap copy of the entire decompressed payload, which is then inserted (and, as the sole entry over the 64 MB budget, pinned) into the cluster cache. Xapian fulltext/title indexes are conventionally stored uncompressed and can be hundreds of MB to multiple GB, and `blob_direct_access` is exactly the primitive the C ABI (`zimru_item_direct_access`) and `Item::warmup` use to hand libxapian an `fd + lseek` — so a single direct-access/warmup query materializes and pins the whole multi-GB cluster in RAM, defeating the zero-copy design and risking OOM at large archive scale. — concrete consequence: opening/warming the fulltext index of a big ZIM copies the entire index cluster into the heap (multi-GB allocation, latency + memory blow-up). — smallest safe fix: for `Compression::None`, read the info byte and the two offset words for `blob_idx`/`blob_idx+1` directly from the mmap at `cluster_range.start + 1 + blob_idx * ptr_size` (never call `self.cluster`); only fall back to `self.cluster` for compressed clusters to learn `is_direct == false`.
- [medium] src/bin/_index_helper.rs:241 — `finish()` uses `std::fs::read(&out_path)` to slurp the entire produced Xapian index into one `Vec<u8>`, and both callers (`zimrecreate`, `zimwriterfs`) then feed it through the buffered `creator.add_item` path, so peak RSS scales with the fulltext/title DB size (roughly a full extra in-RAM copy of the index while the cluster is assembled). The fulltext DB is multi-GB for large Wikipedias. — concrete consequence: writer OOM / swap on large archives despite the streaming chunked-item API existing precisely to avoid this. — smallest safe fix: stream the temp file into `creator.begin_item`/`item_chunk`/`end_item` (or otherwise read in bounded chunks) instead of `std::fs::read` + whole-blob `add_item`.
- [low] src/archive.rs:1236 — `check_dirent_order` allocates a heap `String` per dirent (`prev = Some((ns, url.to_string()))`) inside its O(N) sweep over every entry; `Dirent::key_at` already returns a borrowed `&str` from the mmap, so the allocation is avoidable. Same pattern at src/archive.rs:1259 (`check_title_index`). N is the archive entry count (millions on real ZIMs). — concrete consequence: N avoidable heap alloc/free pairs slow `zimcheck -A`'s order checks on large archives. — smallest safe fix: store `prev: Option<(u8, &str)>` borrowing from `&self.core.mmap` (the borrow outlives the loop) instead of `String`.

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
src/archive.rs — findings: 2
src/bin/_index_helper.rs — findings: 1
src/bin/zimbench.rs — clean
- [high] src/bin/zimdump.rs:379 — `zimdump analyze --by-item` re-decompresses every cluster once per blob — pass 1 decodes each cluster via `arc.cluster_uncached(idx)` (bypassing the shared cache) and discards the `Cluster`, then the `--by-item` loop calls `item.size()` → `get_data()` → `load_cluster` for every entry in URL-pointer order. URL order visits clusters near-randomly, so the 64 MB cluster LRU thrashes and the same clusters are re-decoded up to once per blob (the exact thrash `cmd_dump`'s own comment documents and fixes by grouping by cluster). Concrete consequence: on a multi-GB ZIM the whole decompressed content is re-decoded many times over, turning a single-pass analysis into an hours-long job. Smallest safe fix: group entries by `cluster_idx` (as `cmd_dump`/`zimrecreate` already do) and decode each cluster once, or retain the per-blob sizes computed in pass 1.
- [low] src/bin/zimdump.rs:255 — `entry_by_filtered_index` (and `filtered_index_of`) linearly scan every entry to locate the idx-th entry of a namespace; `Archive::namespace_range(ns)` returns the namespace range in O(log n) and the idx-th entry is then `entry_by_url_index(range.start + idx)`. Concrete consequence: `zimdump list --idx=N` / `show --idx=N` parse N dirents (two String allocations each) — seconds on a tens-of-millions-entry archive. Smallest safe fix: use `namespace_range` + direct index instead of the scan.
- [low] src/bin/zimcheck.rs:638 — allocation churn on the content-scan path: `let Dirent::Article(a) = e.dirent().clone()` clones the full dirent (url + title Strings) for every C-namespace entry and immediately drops the cloned title, and `mime_list.get(...).to_string()` allocates a fresh mimetype String per entry. Concrete consequence: tens of millions of redundant String allocations on `-A` over a large archive. Smallest safe fix: match on `e.dirent()` by reference, clone only `a.url`, and store an `is_html: bool` (computed once here from `mime_list`) instead of the mimetype String.
- [low] src/bin/zimcheck.rs:541 — `check_integrity`'s cluster-validation loop calls `touch_cluster(arc, c)`, which ignores `c` and runs `arc.entry_by_url_index(0)` (full dirent parse + two String allocations) on every iteration. Concrete consequence: `cluster_count` redundant parses of the same dirent with no per-cluster work — wasted CPU over a loop sized by the (large) cluster count. Smallest safe fix: make `touch_cluster` actually parse `arc.cluster_uncached(c)`, or drop the loop.
- [medium] src/bin/zimru.rs:220 — `cmd_readall` sets the cluster cache budget to `usize::MAX`, pinning the entire decompressed archive in RAM. Concrete consequence: `zimru readall` on a large ZIM (decompressed content can be many× the compressed file size) grows memory without bound and can OOM. Smallest safe fix: cap the cache and group reads by cluster (as `zimdump`/`zimrecreate` do) so each cluster is decoded exactly once at bounded memory.
- [low] src/bin/zimwriterfs.rs:734 — `mime_for_path` rebuilds a `HashMap<&str,&str>` from the 38-entry static `TABLE` on every call, i.e. once per file in the walk. Concrete consequence: a HashMap allocation + ~38 hashes per file — millions of allocations on a large directory tree. Smallest safe fix: a `static OnceLock<HashMap>` or a `match` on the lowercased extension string.
- [medium] src/bin/zimwriterfs.rs:774 — `derive_title_from_bytes` calls `text.to_ascii_lowercase()`, allocating a full lowercase copy of the entire HTML body just to locate `<title>` case-insensitively; it runs once per HTML file. Concrete consequence: transient allocation proportional to every article body's size (tens of GB of churn on a 1 M-article build). Smallest safe fix: case-insensitive byte search for `<title>` (e.g. `[u8]::windows` / memchr-style compare) without materializing a lowercase copy.
- [low] src/cffi/archive.rs:816 — `zimru_archive_metadata_keys_count` returns `get_metadata_keys().len()`, which scans every dirent (O(n)) and allocates a String per metadata key just to produce a count; `Archive::entry_count_in_namespace(NS_METADATA)` is O(log n). Concrete consequence: a full-archive scan plus allocation churn for a count operation. Smallest safe fix: return `entry_count_in_namespace(NS_METADATA)`.
- [medium] src/cffi/archive.rs:829 — `zimru_archive_metadata_key` re-runs `get_metadata_keys()` (a full O(n) dirent scan + String allocation per metadata entry) on every call, so a caller enumerating k metadata keys does O(k·n) dirent parses and allocations. Concrete consequence: tens of seconds per enumeration on a large archive (e.g. 20 keys × 10 M entries). Smallest safe fix: compute the key list once and cache it in `zimru_archive_t`, indexing into the cached list.
- [low] src/cffi/archive.rs:727 — the `interned: Mutex<Vec<CString>>` store only grows: every `zimru_archive_metadata` / `zimru_archive_metadata_key` call appends a CString that is freed only at archive close. Concrete consequence: on a long-lived archive handle (server) reading metadata per request, memory grows monotonically with the call count. Smallest safe fix: cache one entry per distinct key/value instead of appending on every call (or reuse a bounded store).
- [medium] src/cffi/item.rs:155 — `zimru_item_direct_access` (and `zimru_item_warmup`) call `Archive::blob_direct_access`, which loads the full cluster via `self.cluster(idx)` — copying uncompressed payloads (`body.to_vec()`) and fully zstd/xz-decompressing compressed ones — just to answer "is this blob directly mmap-able", then caches the cluster. Concrete consequence: the zero-copy direct-access hint materializes multi-MB clusters and pollutes the 64 MB LRU on the per-request server hot path, defeating the API's purpose. Smallest safe fix: read the 1-byte compression id from the on-disk info byte and, for uncompressed clusters, read blob offsets straight from the mmap slice without `Cluster::parse`.
## Coverage
src/bin/zimcheck.rs — findings: 2
src/bin/zimdump.rs — findings: 2
src/bin/zimrecreate.rs — clean
src/bin/zimru.rs — findings: 1
src/bin/zimsplit.rs — clean
src/bin/zimwriterfs.rs — findings: 2
src/cffi/archive.rs — findings: 3
src/cffi/blob.rs — clean
src/cffi/creator.rs — clean
src/cffi/entry.rs — clean
src/cffi/error.rs — clean
src/cffi/item.rs — findings: 1
src/cffi/mod.rs — clean
src/cffi/uuid.rs — clean
src/cluster.rs — clean
src/dirent.rs — clean
src/error.rs — clean
src/header.rs — clean
src/io_hints.rs — clean
src/lib.rs — clean
- [medium] src/writer.rs:1745 — `bucket_key` allocates a fresh `String` via `Cow::Owned(format!("{tag}|{strat}"))` on every `push_item` call for the non-`Single` cluster strategies (`ByMime`, `ByExtension`, `ByFirstPathSegment`) — this is the per-item streaming hot path, so a multi-million-item archive incurs one heap alloc+free per item, plus a second `key.clone().into_owned()` clone (line 1777) whenever a new bucket is created; the `Single` fast path (lines 1727–1736) exists precisely to avoid this allocation, so the opt-in strategies regress it — the growing input is the item count. Smallest safe fix: for `ByMime` key on the already-interned `mime_idx` (a `u16`) instead of the mime string so the key is a cheap composite `(tag, u16)`; for the path-derived strategies use an interned key or a reusable key type rather than `format!` per item.

## Coverage
src/mime.rs — clean
src/raw.rs — clean
src/uuid.rs — clean
src/writer.rs — findings: 1
- [low] tests/real_files.rs:65 — `by_url` materializes a heap-allocated `String` path for EVERY entry (`e.path().to_string()`), but the only consumer is the binary-search spot-check at line 80 which samples ~200 entries via `step_by(stride)`. The growing input is the real downloaded ZIM's `entry_count` (millions of entries for full Wikipedia dumps), so the test's memory and allocation churn scale with total entry count even though only ~200 `(ns, url)` pairs are ever used. — Fix: compute `stride = entry_count/200` up front from `h.entry_count` and push into `by_url` only when `i % stride == 0`, keeping article/redirect counts as plain counters instead of storing all paths.
- [low] tests/real_files.rs:94-98 — the title-order walk allocates two `String`s per title: `e.title().to_string()` for `key` and a `key.clone()` stored into `prev`. It iterates the entire title set (millions on real ZIMs) while only ~50 titles are retained in `sample_titles`. The clone on every iteration doubles allocation churn for no benefit, since `key` is only conditionally moved into `sample_titles`. — Fix: move `key` into `prev` (drop the unconditional `.clone()`) and clone only on the sample branch, e.g. `if is_sample { sample_titles.push(key.clone()); } prev = Some(key);`.

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
tests/real_files.rs — findings: 2
tests/streaming_encode_smoke.rs — clean
tests/synthetic_zim.rs — clean
tests/writer_roundtrip.rs — clean
tests/xapianbuilder_helper.rs — clean
tests/zimdump_analyze.rs — clean
tests/zimwriterfs_e2e.rs — clean
# Batch 5 — performance findings

- [medium] src/bin/zimrecreate.rs:232 — `pending: Vec<PendingContent>` accumulates owned `path`/`title`/`mime` `String`s (3 heap allocations per entry) for every C-namespace article during pass 1, before pass 2 can run — peak RSS scales linearly with entry count (~150–200 B/entry): a multi-million-entry archive (full Wikipedia ≈ 10M entries) holds ~1.5–2 GB of dirent-metadata strings in RAM, undermining the streaming writer's documented bounded-RSS design (the `start_writing` doc claims peak RSS is `cluster_size_target × thread_count` plus "small" per-item metadata) — smallest safe fix: store a compact `(cluster: u32, blob: u32, url_index: u32)` per entry and re-resolve `path`/`title`/`mime` from the mmap in pass 2 (one extra cheap dirent parse per item, ~12× less metadata memory).
- [low] src/bin/zimrecreate.rs:339 — `cluster.blob(p.blob)?.to_vec()` allocates a fresh `Vec<u8>` and memcpy's the full decompressed body for every item, a redundant intermediate copy: the bytes already live in the decoded `Arc<[u8]>` cluster payload and are copied *again* into the encode buffer (`encode_cluster`'s `payload.extend_from_slice` / raw `bytes.extend_from_slice`) — every content byte is memcpy'd an extra time (and an extra heap allocation per item) at multi-GB archive scale — fix is architectural (thread `Arc<[u8]>`/`Bytes` slices through `Item`/`Bucket`/`encode_cluster`) rather than a one-liner; flag as a known hot-path copy.
- [low] src/cluster.rs:73 — the uncompressed (`COMPRESSION_NONE`/`LEGACY`) branch does `Arc::from(body.to_vec())`, copying the *entire* cluster payload out of the mmap-backed `raw` slice into a fresh heap buffer just to satisfy the uniform `Arc<[u8]>` ownership — for raw clusters (the `keep_raw` media clusters zimrecreate deliberately preserves: JPEG/WebM/fonts, typically the bulk of the bytes in map/tile archives) every byte is memcpy'd once here and again per-blob at zimrecreate.rs:339 — the mmap borrow outlives the `Cluster` (it is owned by the same `Arc<ArchiveCore>` that owns the cluster cache), so a `Cow<'a, [u8]>` payload (borrowed for uncompressed, owned for decompressed) would eliminate the copy — fix is a lifetime-threading refactor of `Cluster`/`Blob`/`load_cluster`/`par_clusters`.

## Coverage
include/zimru.h — clean
src/cffi/creator.rs — clean
src/bin/zimrecreate.rs — findings: 2
src/cluster.rs — findings: 1
- [low] src/dirent.rs:114 — `Dirent::parse` allocates and copies the URL string twice for every dirent whose `title` field is empty (`url: url.to_string()` at line 114 plus `title: … url.to_string()` at line 116; same pattern in the redirect arm at 76/78 and the linktarget/deleted arm at 93/95). Empty-title dirents are the common case for ZIM content entries (title falls back to url), and `parse` runs per-probe in `by_prefix`'s binary search and per-entry in `check_mimetypes`/`check_dirent_ptrs`/`entry_by_ns_path` lookups — so a full scan over N entries does N extra heap allocations + memcpys of the URL. — concrete consequence: doubled url-string allocation cost on the hot materialization path, ~1 extra alloc per title-less dirent parsed, scaling with entry count. — smallest safe fix: defer the empty-title fallback to the `title()` accessor (store the possibly-empty title and return `url` when empty, matching `title_key_at`'s existing behaviour) so only one String is ever allocated; or bind `let url = url.to_string();` once and reuse it.

## Coverage
src/dirent.rs — findings: 1

## Run stats

input 375440 tok (+3609216 cached), output 81870 tok, cost $0.25 — 62 files in 11m (333.1 files/h, 1.9 min/batch)
