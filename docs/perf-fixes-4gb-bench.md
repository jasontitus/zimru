# Performance-fix validation: 4.8 GB `zimwriterfs` build, before vs after

Validates the five hot-path fixes from the writer performance review
(commit `eebcd92`) on a realistic >4 GB ZIM creation workload.

## Fixes under test

1. `zimwriterfs::mime_for_path` — was rebuilding a `HashMap` from the
   static mime table on **every call** (once per input file); now a
   binary search over a sorted static table.
2. `zimwriterfs::derive_title_from_bytes` — was allocating a lowercase
   copy of the **entire HTML body** to find `<title>`; now an
   allocation-free ASCII case-insensitive scan.
3. `writer::encode_cluster` — was creating a fresh zstd compression
   context per cluster; each pipeline worker now reuses one context
   across clusters.
4. `archive::check_dirent_order` / `check_title_index` — were
   allocating a `String` per entry in the post-write verify; now borrow
   the keys from the mmap.
5. `zimwriterfs` big-file streaming loop — 64 KiB → 1 MiB read buffer
   (16× fewer read syscalls / encoder calls per GB).

## Workload

Synthetic corpus, 5.7 GB / 60,752 files, shaped to exercise every
creation path:

| slice | count | size | path exercised |
|---|---|---|---|
| HTML articles (~15 KB, real `<title>` tags) | 60,000 | ~0.9 GB | parallel batch read, title derivation, mime lookup, zstd-19 pipeline |
| media, 8 MiB random bytes (`.webp`) | 550 | 4.4 GB | one-at-a-time chunked path, raw clusters |
| media, 2 MiB random bytes (`.webp`) | 200 | 0.4 GB | batch path, raw clusters |

Build: `zimwriterfs -j` (no xapian indexes), defaults otherwise
(zstd level 19, 2 MiB clusters). Output ZIM: **5,108,151,522 bytes
(4.76 GiB)**, byte-identical size across all four runs.

## Method

* Host: 4 cores, 15 GB RAM, single NVMe-backed volume.
* **ABBA ordering** — before, after, after, before — so any monotonic
  drift (thermal, disk aging, cache creep) cancels instead of biasing
  one condition.
* **Cache control** — every run starts with `sync; echo 3 >
  /proc/sys/vm/drop_caches`, then re-reads the full corpus, so each
  build begins from the identical warm-corpus / no-output-residue page
  cache state. Output deleted between runs.
* Binaries built from the same toolchain at the two commits
  (`ba2ec05`+docs = before, `eebcd92` = after), `--release`.

## Results

| run | binary | wall (s) | peak RSS (MB) | encode busy (s, 4 workers) | MD5 (s) |
|---|---|---|---|---|---|
| A1 | before | 151.44 | 5,082 | 490.4 | 9.3 |
| B1 | after  | 147.38 | 5,074 | 475.8 | 9.5 |
| B2 | after  | 149.58 | 5,119 | 487.0 | 9.2 |
| A2 | before | 148.75 | 5,139 | 481.3 | 9.1 |

* **before mean: 150.10 s** (spread 148.75–151.44)
* **after mean: 148.48 s** (spread 147.38–149.58)
* nominal delta: **−1.6 s (−1.1 %)**

(Peak RSS is dominated by the MD5 pass mmapping the 4.9 GB output;
it is not a resident-heap number.)

## Conclusion

The fixes are real but the end-to-end wall time on this workload is
**statistically unchanged**: the −1.1 % nominal improvement is about
the same size as the run-to-run spread (~2 %), so it cannot be
distinguished from noise with two runs per condition.

That is the expected shape: this build spends its time in zstd-19
compression of the text slice (~480 s of encoder busy time across 4
workers ≈ the entire 150 s of wall time) and in raw I/O for the 4.8 GB
of media, and none of the five bugs sat in those two dominant costs.
The fixes remove per-file/per-cluster overheads — 60 k HashMap builds,
~0.9 GB of redundant lowercase copies, 1,203 zstd context setups,
~120 k verify-pass allocations, 16× the needed read syscalls — which
together were worth only a couple of seconds against a
compression-bound 150 s build. On workloads where those paths weigh
more (many more, smaller files; low compression levels; HTML-only
corpora; slower storage where syscall count matters), the relative win
grows; here they are dwarfed by compression.

Output correctness is unaffected: all four runs produced the same
output size, every build passed the writer's post-write structural
verify, and the full test suite passes on the fixed code.
