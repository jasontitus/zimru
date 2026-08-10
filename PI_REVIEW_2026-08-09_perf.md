# Pi sweep review (perf focus) — zimru-c9bc4e8a

Exhaustive per-file pass: 57 code files across 7 batches.

## Findings

# Pi perf sweep — batch-1 (zimru)

## Findings

- [medium] src/archive.rs:734 — `get_metadata_keys()` scans the *entire* URL-pointer list (`for i in 0..n { Dirent::parse(...) }`) to collect the handful of `M`-namespace keys, although dirents are sorted by `(namespace, url)` and the contiguous `M` run is reachable in O(log n) via `namespace_range(NS_METADATA)`. Consequence: on a 700 K-entry archive every metadata listing parses 700 K dirents (each a bounds-checked parse) even though metadata is typically <100 entries; cost grows linearly with `entry_count`, and `illustrations()` (line 757) re-triggers the scan on each illustration request. Fix: replace the `0..n` loop with the `start..end` range from `self.namespace_range(NS_METADATA)` and iterate only that (constant per namespace, O(log n) to locate).

- [low] src/archive.rs:919 — `by_prefix()`'s binary-search probe does `Dirent::parse(...)` + `d.url().to_string()` per iteration (a heap String per probe), whereas the equivalent `entry_by_ns_path` search uses the no-allocation `Dirent::key_at`. Consequence: each prefix lookup pays `~log2(entry_count)` temporary String allocations on the hot path used for "all pages under images/" style scans. Fix: use `Dirent::key_at` (returns `(u8, &str)` borrowed from the mmap) for the comparison and materialize a full dirent only on the matched leaf, exactly as `entry_by_ns_path` does.

- [low] src/archive.rs:636 — `entry_by_ns_title()`'s linear fallback for namespaces not covered by the title listing (M/W/X in modern archives) parses every one of `n` dirents per call and is uncached. Consequence: a title lookup on a non-content namespace is O(entry_count) — full linear scan (700 K dirent parses) on each invocation, with no memoization. Fix: bound the scan by the namespace range (`namespace_range(ns)`, O(log n)) instead of the whole entry space, since dirents are sorted by namespace.

## Coverage
.github/workflows/ci.yml — clean
Cargo.toml — clean
bench/cache_aware_compare.sh — clean
bench/parity.sh — clean
bench/recreate-bench.sh — clean
bench/recreate-suite.sh — clean
bench/run.sh — clean
benches/cluster_decode.rs — clean
benches/get_entry_by_path.rs — clean
benches/warmup.rs — clean
build.rs — clean
cbindgen.toml — clean
include/zimru.h — clean
src/archive.rs — findings: 3
src/bin/_index_helper.rs — clean
src/bin/zimbench.rs — clean
# Pi sweep — performance review — batch-2

Performance-only findings for the listed files (anchored to files in this batch).
No security/style findings. Fallback checklist greps (clone/alloc, collect, HashMap, loop-dense)
ran against the batch; the meaningful hits are below.

## Findings

- [medium] src/bin/zimwriterfs.rs:734 — `mime_for_path` rebuilds a fresh `HashMap<&str,&str>` from the static `TABLE` (`let map: HashMap<&str, &str> = TABLE.iter().copied().collect();`) on **every** call — and it is called once per ingested file (both the streaming path at ~line 521 and every item in the parallel-read batch at ~line 634). Growing input = number of files under HTML_DIR (hundreds of thousands on real wikis). Consequence: ~43 heap allocations + SipHash insert/query per file just to resolve an extension into a mimetype — measurable constant-factor CPU on the ingest loop of a large build. Fix: hoist to a `static`/`OnceLock<HashMap>` (or a sorted `&[(&str,&str)]` + `binary_search`, or a `match`), so the lookup table is built once per process.

- [low] src/bin/zimwriterfs.rs:814 — `derive_title_from_bytes` calls `text.to_ascii_lowercase()` on the **entire** HTML body (a full-size `String` allocation + full scan) just to locate `<title>`/`</title>` case-insensitively. Growing input = number × size of HTML files (Wikipedia-scale builds). Consequence: a duplicate allocation and O(body) lowercase pass per HTML file on top of the reads already done. Fix: search for the literal `<title>/</title>` byte patterns (or walk `is_ascii`-case-insensitive comparisons via `eq_ignore_ascii_case` over windows) without materializing a full lowercase copy.

- [low] src/bin/zimcheck.rs:632,686-747 — `scan_content` materializes a `PerEntryFinding` (owned `path: String` clone, `mimetype: String` clone, plus dangling/external Vecs) for **every** C-namespace article, gathers them across all clusters in parallel, then sorts the whole list — so peak RAM grows linearly with archive entry count (a 10M-article archive holds ~10M owned path strings + structs resident at once). Consequence: multi-hundred-MB peak RSS on very large archives for a one-shot CLI, and the per-article path/mimetype clones double memory over borrowing from dirents. Fix: aggregate counts/reports streaming by cluster (emit empty/redundant results in URL order as they stream rather than buffering every entry), and borrow names during aggregation instead of cloning into the per-entry struct.

## Coverage
- src/bin/zimcheck.rs — findings: 1
- src/bin/zimdump.rs — clean
- src/bin/zimrecreate.rs — clean
- src/bin/zimru.rs — clean
- src/bin/zimsplit.rs — clean
- src/bin/zimwriterfs.rs — findings: 2
- src/cffi/archive.rs — clean
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
# Pi sweep perf — batch 3

Performance review of src/mime.rs, src/raw.rs, src/uuid.rs, src/writer.rs (Rust, ZIM archive reader/writer). The three parser/format files are clean. writer.rs is heavily optimized (syscall batching, mmap MD5, bounded pipeline); two defensible findings about CPU and I/O on the build path.

## Findings

- [medium] src/writer.rs:1434-1435 — streaming-encode path gives each ≥256 MiB item's zstd encoder the ENTIRE machine (`rayon::current_num_threads()` passed to `encoder.multithread`) while the continuous encode pipeline's N workers (line 1158) keep running on the same cores, so a huge item streams with N zstdmt threads *plus* N pipeline workers hammering N cores (~2× oversubscription). The doc comment at the multithread call claims "each gets fewer workers but they overlap across items", but the code hands the full thread count, so when one huge item streams (or several finish draining in background threads via `encoder.finish()`), every core is contended and the encode pipeline's normal clusters are starved of CPU for the whole item's streaming duration — slowing a build that mixes huge and ordinary items. Fix: scale the per-item zstdmt worker count down to account for the pipeline's concurrent workers (e.g. `(threads / (1 + active_streaming_tasks)).max(1)`), or gate streaming zstdmt workers by available parallelism rather than the full rayon pool.

- [low] src/writer.rs:2249 — `finalize` re-opens and re-reads the entire output file through `verify_archive` (5 structural passes) on every build, documented as ~1-2 s per GB, on top of step-13's MD5 pass that already hashed the final on-disk bytes from mmap. The verify catches structural invariants MD5 can't, so it isn't pure waste, but it doubles the full-file I/O of an already multi-GB build. Fix if the cost matters: make verification opt-in / gated by env or build profile, or fold the structural checks into the single mmap pass used for the MD5 trailer.

## Coverage
src/mime.rs — clean
src/raw.rs — clean
src/uuid.rs — clean
src/writer.rs — findings: 2
# Pi sweep — performance review — batch-4

## Summary
Reviewed 17 files under `examples/` and `tests/` of the zimru Rust ZIM library.
All but one (`examples/writer_bench.rs`) are integration/unit tests that drive the
public writer and reader APIs against small, fixed-size in-memory fixtures or tiny
temp corpora; the benchmark generates synthetic items to measure the writer.
No production hot paths exist in this batch: loops iterate over small bounded
fixtures (`n` entries that are hard-coded test constants, not user data that grows),
single-pass, and the underlying writer/reader allocations live in `src/` which is
outside this batch's scope. `real_files.rs` exercises full coverage over real wiki
ZIMs, but that whole-cluster decompression walk is the test's explicit purpose, not
a defect. No defensible performance findings (cold-path tests with bounded inputs;
the performance-review skill excludes test setup and one-off validation loops).
External tool (`clippy` perf lints, `ruff`, `staticcheck`) not applicable to Rust
except clippy, which was not run (no cargo build per global constraints).

## Findings
(none)

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
# Batch 5 — performance review

Reviewed: include/zimru.h, src/cffi/archive.rs, src/bin/zimdump.rs, src/cffi/creator.rs
(applying performance-review + rust-performance-review checklists).

## Findings

- [medium] src/cffi/archive.rs:727 — `interned` CString cache in `zimru_archive_metadata` (and the identical append in `zimru_archive_metadata_key`, ~line 835) grows monotonically with **no dedup and no eviction**: every call pushes a fresh `CString` and the Vec lives until the archive is freed. Calling the same metadata key repeatedly (the header/doc explicitly targets "long-running-server telemetry" and "servers serving content", where a hot value like Title/Description would be fetched per request) appends a duplicate copy each time, so memory grows unboundedly with request count for the archive's lifetime — a slow leak that accumulates on any long-lived server process. — Dedup on the key (cache at most one CString per distinct metadata key / per key index) or bound the interned Vec and hand out `Arc`-backed stable pointers; at minimum check whether the key is already interned before pushing.

## Coverage
- include/zimru.h — clean
- src/cffi/archive.rs — findings: 1
- src/bin/zimdump.rs — clean
- src/cffi/creator.rs — clean
# Pi perf sweep — batch-6

## Findings

- [low] src/bin/_index_helper.rs:288 — `write_raw` issues a raw `stdin.write_all(buf)` syscall per `feed_title`/`feed_fulltext` call, and each feed also heap-allocates a fresh JSON `Vec<u8>` in `encode_title_doc`/`encode_fulltext_doc`. zimrecreate feeds title+fulltext per HTML page, so a multi-million-entry archive (5M+ pages) does one write(2) syscall and one small allocation per index record, serialised through the parent — build time grows with entry count × syscall/alloc overhead. Fix: wrap each `ChildStdin` in a `std::io::BufWriter` (flush on `finish()`) so JSONL records are batched into large writes, and/or reuse a reusable `Vec<u8>` buffer across feeds.

## Coverage
src/bin/zimrecreate.rs — findings: 0
src/bin/_index_helper.rs — findings: 1
src/cffi/item.rs — clean
src/cluster.rs — clean
# Pi perf review — batch-7

## Findings

- [low] src/dirent.rs:77-81,97-101,116-120 — `Dirent::parse` recomputes `url.to_string()` a second time for the `title` fallback when the dirent's title field is empty (all three branches: redirect, linktarget/deleted, article). For archives where a large fraction of dirents carry no title (the spec says empty title == reuse url as title), each such entry pays an extra heap allocation + full string copy during bulk materialization (`cmd_list --all`, `cmd_readall`, `get_metadata_keys` loop over millions of entries). Consequence: a constant multiplier on allocation churn in the hottest materialization path — e.g. a 5 M-entry archive with 50% empty titles gets ~2.5 M extra String allocs+copies. Fix: bind `let url_owned = url.to_string();` once, then set `title: if title.is_empty() { url_owned.clone() } else { title.to_string() }` so the empty-title fast path reuses the already-built url allocation instead of re-scanning the C string.

## Coverage
src/bin/zimru.rs — clean
src/dirent.rs — findings: 1

## Run stats

input 679143 tok (+4318208 cached), output 48167 tok, cost $0.15 — 67 files in 21m (188.6 files/h, 3.0 min/batch)
