# Before/after benchmarks for the 2026-08-13 review fixes

Measured old `main` (`b6e9440`) vs post-fix `main` (`c67e9d7`) on the same
machine with generated archives, ABBA-ordered (each pair run old→new cold,
then new→old with the page cache pre-warmed via `cat file > /dev/null`).
Results were order-independent; ranges below span both orders.

## Setup

- `raw.zim` — `cargo run --release --example gen_raw_zim -- raw.zim 256`:
  one 256 MiB incompressible item stored uncompressed (a stand-in for the
  conventionally-uncompressed Xapian index / media clusters that
  `blob_direct_access` / `warmup` are used on).
- `many.zim` — `cargo run --release --example writer_bench -- 300000 1 many.zim`:
  300 k ~1 KiB items (623 MB raw, 95 MB compressed).
- `shuffled.zim` — `cargo run --release --example gen_shuffled_zim -- shuffled.zim 100000 3`:
  100 k ~3 KiB items with random hex path names, so URL-sorted order is
  decorrelated from cluster order — the access pattern of real
  scraper-packed archives.
- Latency/RSS via `/usr/bin/time -v` (GNU time); microbenches via the
  `da_bench` example and the existing criterion benches. Rust 1.97.1.

## Results

| Benchmark | Old (`b6e9440`) | New (`c67e9d7`) | Delta |
|---|---|---|---|
| `blob_direct_access` on the 256 MiB uncompressed cluster, first call, cache budget 1 B (`da_bench raw.zim bigraw`) | 0.99–1.38 s, 788 MB peak RSS | 3.7–4.7 µs, 2.3 MB peak RSS | ~10⁵× faster, ~340× less memory |
| `zimdump analyze --by-item shuffled.zim` | 2 m 22 s – 2 m 24 s, 128 MB | 0.71–0.75 s, 41 MB | ~190× faster; output byte-identical |
| `zimrecreate many.zim out.zim -j` peak RSS | 446–448 MB | 397–398 MB | −49 MB (−11%); ~160 B/entry ⇒ ~1.6 GB at 10 M entries |
| `zimrecreate` wall time (same runs) | 1:36 / 1:46 | 1:36 / 1:46 | equal (encode-bound) |
| `get_entry_by_path/same_path_repeated` (criterion, `many.zim`) | 183.9 ns | 180.6 ns | within noise |
| `cluster_decode` fresh-DCtx zstd 3 / 19 / 22 (criterion) | 1.014 ms / 942 µs / 919 µs | 1.019 ms / 900 µs / 923 µs | within noise (old-vs-old rerun variance was ±2.5%) |

## Interpretation

- **Direct access / warmup** (`archive.rs` fix): the old code decoded and
  cached the entire cluster to answer an offset/size query; a single
  `warmup` of a big archive's index materialized (and pinned, being over
  the 64 MB budget) the whole multi-hundred-MB cluster. The new code reads
  the info byte and two offsets from the mmap.
- **`analyze --by-item`** needed the shuffled archive to show its effect:
  sequentially-packed archives don't thrash the 64 MB cluster LRU, real
  crawl-order-packed ones do. Per-blob sizes are now retained from pass 1,
  so pass 2 touches no clusters at all (also visible in the RSS drop).
- **`zimrecreate` RSS**: pass 1 now keeps 12 bytes per content entry
  instead of three owned Strings. Wall time is unchanged because encoding
  dominates.
- **`cluster_decode` parity** confirms the new decompression-bomb caps
  (declared-size validation + `take(limit)`) cost nothing measurable.
- Not individually benchmarked: the dirent empty-title allocation removal
  and mmap-borrowing order checks (wins live on full-iteration paths the
  criterion benches don't cover), the cffi metadata key caching
  (O(k·n) → O(k) needs a C harness), and `zimcheck -I`, which is
  intentionally *slower* now — it validates every cluster instead of
  no-opping.
