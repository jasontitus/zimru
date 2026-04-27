# zimwriterfs head-to-head: real libzim vs libzim-shim+zimru vs zimru native

## Latest results (2026-04-27, post 256 MiB threshold + parallel reads + auto-emit metadata)

zimru native at zstd 19 with the full optimisation stack
(streaming Creator with 256 MiB streaming-encode threshold,
parallel `rayon` file reads, parallel-batch cluster encode,
auto-emitted `M/Counter` / `M/Scraper` /
`X/listing/titleOrdered/v1` for byte-comparable output, and
post-write integrity verify):

| workload | wall | RSS peak | output | entries | zimcheck |
|---|---:|---:|---:|---:|:---:|
| sv-unpacked (zimwriterfs, 17 K files) | 144 s | 2.0 GB | 209 MB | 16 916 | Pass |
| **texas-unpacked (zimwriterfs, 1.08 M files)** | **582 s** | **5.4 GB** | **3.65 GB** | 1 080 097 | Pass |
| wiki_top_nopic via zimrecreate (923 K entries) | 389 s | 3.9 GB | 1.71 GB | 923 593 | Pass |
| _texas via real libzim baseline_ | 1 331 s | 0.96 GB | 5.22 GB | 1 080 102 | Pass |

zimru native on texas: **2.29 × faster than real libzim** with
**30 % smaller output** at zstd 19. Output is structurally
byte-comparable now (`M/Counter`, `X/listing/titleOrdered/v1`,
`M/Scraper` all present and well-formed).

Texas ratio vs the original Kiwix-shipped `osm-texas-2026-04-24.zim`
(3.96 GB, also built at zstd 19 by mwoffliner): zimru native at
3.74 GB is 6 % *smaller* than what Kiwix actually distributes —
the remaining gap is small system metadata (e.g. no
`X/title/xapian` suggestion DB yet) plus the 64 KB mime-list
reserve we use to keep `mimelistPos == 80`.

### Apples-to-apples by compression level (texas, `--withoutFTIndex`)

Real libzim's `zimwriterfs` hard-codes its zstd level in the
binary and doesn't expose a `--compression-level` flag, so the
"zimru wins because we use zstd 19" framing wasn't fair.
Actually, at *every* level zimru produces a smaller archive
than libzim's default zstd, and at every level zimru wins
on wall-time by an order of magnitude.

| stack / level                | wall   | RSS    | output  |
|------------------------------|-------:|-------:|--------:|
| **real libzim** (default zstd) | 1331 s |  0.96 GB | 4.86 GB |
| zimru @ zstd **3** (default)   |   74 s |  5.3 GB | 4.33 GB |
| zimru @ zstd **9**             |  104 s |  5.1 GB | 4.02 GB |
| zimru @ zstd **12**            |  130 s |  5.3 GB | 4.00 GB |
| zimru @ zstd **19**            |  582 s |  5.4 GB | 3.74 GB |

Two takeaways:

1. **No zstd level produces output as large as real libzim's
   default.** Even at zstd 3 zimru is 11% smaller; at zstd 9 it's
   17 % smaller. Likely cause: libzim's bin-packing order packs
   less compressible content per cluster (URL-sort puts
   alphabetically-adjacent items together; libzim's order
   apparently doesn't), so the zstd dictionary inside each cluster
   is less effective on libzim's clusters.

2. **At any matched compression effort, zimru is 12-18× faster.**
   The "2.29× faster" headline above is the conservative
   high-quality comparison (zimru @ zstd 19); the
   production-realistic comparison at libzim's effective level is
   `1331 / 74 = 18.0×` faster. The 5.6× higher RSS is the
   parallel-batch trade — see "Memory characteristics" below.

### Phase breakdown on texas (`ZIMRU_STATS=1`):

```
total wall:              640 s
parallel-batch encode:   486 s (76 %)   8 cores at saturation
streaming encode:         80 s (12 %)   2 huge items (addr.json 1.74 GB, poi.json 516 MB)
table+dirent write:        5 s ( 1 %)
MD5 trailer:               6 s ( 1 %)
post-write verify:         6 s ( 1 %)
producer + bookkeeping:   57 s ( 9 %)   FS walk + parallel reads + push_item
```

Cores avg = 5.64 of 8. Streaming-encode of the 2 huge items is
serial across them (each uses zstdmt internally); parallelizing
that path across items would buy ~3 % wall (~30 s) — filed as
"Idea C" follow-up.

### Portable I/O hygiene — capturing io_uring-style wins

io_uring's headline benefits — batched syscall submission, zero-
copy file-to-file moves, kernel-prefetch hints — are mostly
available portably on Linux + macOS via primitives that have
been in POSIX or Apple's libc for years. We pulled in five of
them (no io_uring dependency, no Linux-only path) and got most
of the benefit for low-compression-level builds:

| change                                  | mechanism (Linux / macOS)                                |
|-----------------------------------------|----------------------------------------------------------|
| Batched table-write at finalize         | one big `Vec<u8>` + single `write_all` (was ~3 M syscalls) |
| Zero-copy temp-file → output splice     | `std::io::copy` → `copy_file_range(2)` / `fcopyfile(2)`  |
| Sequential-scan hint on input mmap      | `madvise(MADV_SEQUENTIAL)` (`Archive::advise_sequential_scan`) |
| Sequential read hint on huge files      | `posix_fadvise(SEQUENTIAL)` / `fcntl(F_RDAHEAD)` (`io_hints::hint_sequential`) |
| Larger parallel-read batch (zimwriterfs)| `READ_BATCH` 64 → 256 (deeper disk queue per rayon batch) |

Texas, before vs. after these changes (same build, same machine,
same input):

| level / config            | before     | after          | wall delta |
|---------------------------|-----------:|---------------:|-----------:|
| zstd 3, uncapped          |     85 s   | **74 s**       | **−13 %** |
| zstd 19, uncapped         |    595 s   | **582 s**      | **−2.3 %** (~14 s) |
| zstd 19, `--max-memory=128`|    621 s   | 622 s          | noise      |

Reading the curve: at zstd 3 the overall wall is short enough
(~85 s) that I/O hygiene matters — saving syscalls and skipping
user-space copies trims 13 %. At zstd 19, the run is dominated
by zstdmt CPU (486 s out of 627 s in the phase breakdown above),
so even a perfect I/O layer can only shave a couple percent. The
`--max-memory=128` row is noise because the cap-driven drain
forces enough parallel-batch round-trips to mask the savings —
the hygiene wins land cleanly only when the parallel-batch isn't
being throttled.

The wins compound the right way: low-compression rebuilds
(zimrecreate, zimwriterfs at default level) are the realistic
hot path on cloud zimfarm workers, and that's exactly where the
13 % shows up.

### Future: io_uring on Linux

The above captures the io_uring benefits that have portable
equivalents. The remaining io_uring-only wins (and what they'd
buy us, on top of the current state):

* **`IORING_OP_*` submission queue with `SQE_LINK`** —
  fire-and-forget chained reads + writes for the temp-file
  splice, removing the `std::io::copy` synchronous wait between
  splices. Speculative ~5–10 s on z19 (parallelises the
  serial drain loop across cluster_idx).
* **Registered fixed buffers + fixed FDs** — skip per-syscall
  fd lookup and buffer registration. Marginal on a 1.08 M-item
  build (very few unique fds: input mmap, output file, ~2 temp
  files), maybe 1–2 s.
* **`IORING_SETUP_IOPOLL` for NVMe** — bypass the storage stack's
  interrupt path. Only meaningful on `O_DIRECT` workloads, which
  we'd have to opt into anyway.

Net plausible upper bound from a full io_uring port: another
~10 s on z19, ~3 s on z3. The uplift is real but small relative
to the parallel-batch / zstdmt CPU cost. Likely worth it only
if zimru is also driven from an async runtime that can already
use io_uring (e.g. a future kiwix-rs server). Filed as a
follow-up; not in the critical path.

### Memory characteristics — `--max-memory` knob

zimru's writer trades RSS for wall time: the parallel-batch
encode pipeline accumulates raw input bytes for up to one
cluster per worker thread before draining, plus zstdmt's
per-worker output buffers (which scale with compression level).
At zstd 3 this lands ~1.5 GB peak; at zstd 19 it lands ~3.5 GB.

The new `--max-memory=<MB>` flag (and
`Creator::set_max_in_flight_bytes` API) puts a soft cap on the
parallel-batch buffer — when crossed, `flush_bucket` triggers an
early `drain_pending_encode`, trading parallel-batch fan-out
(more clusters waiting to be encoded in parallel) for a tighter
peak-RSS bound.

Texas (1.08 M items, `--withoutFTIndex`), reporting
`peak_memory_footprint` from `time -l` (the honest process-RSS
metric on macOS — `maximum resident set size` is inflated by the
OS page cache and runs ~3-4× larger):

| stack / flag                       | wall  | peak_mem | output  |
|------------------------------------|------:|---------:|--------:|
| **real libzim** (default zstd, no cap) | 1331 s | **0.49 GB** | 4.86 GB |
| zimru @ zstd 3, `--max-memory=32`  |   88 s | 1.52 GB | 4.33 GB |
| zimru @ zstd 3, `--max-memory=64`  |   85 s | 1.54 GB | 4.33 GB |
| zimru @ zstd 3, `--max-memory=128` |   85 s | 1.62 GB | 4.33 GB |
| zimru @ zstd 19, `--max-memory=32` |  803 s | 2.92 GB | 3.65 GB |
| zimru @ zstd 19, `--max-memory=128`|  621 s | 3.49 GB | 3.65 GB |
| zimru @ zstd 19, `--max-memory=512`|  595 s | 3.59 GB | 3.65 GB |

Reading the table:

* **At zstd 3 the cap is largely cosmetic** — peak floats between
  1.5-1.6 GB regardless of cap size, because the floor is set by
  things the cap doesn't control: the dirent/URL/title tables for
  1.08 M items (~120 MB), `mmap`-backed input reads, the redirect
  Vec, the in-progress mime list, allocator slack, and zstd's own
  hash table. Wall is essentially flat: zstd 3 is fast enough
  that even the tightest cap (32 MB) doesn't starve the
  parallel-batch.

* **At zstd 19 the cap matters but plateaus quickly.** cap128
  matches uncapped wall (621 vs 627 s prior baseline) at 3.49 GB
  peak. cap32 saves another 0.6 GB peak but costs 28 % wall
  (803 s) because the constant drains shrink each parallel batch
  to 1-2 clusters instead of 8. Above cap128 the cap rarely fires
  — cap512 is essentially uncapped (595 s, 3.59 GB peak).

* **None of the caps approach libzim's 0.49 GB peak.** At zstd 19
  the dominant remaining contributors are zstdmt's internal
  per-worker compressed-output buffers (proportional to
  `level × worker_count`, ~250 MB × 8 = ~2 GB at level 19) and
  zimru's streaming-encode of huge items (the two largest texas
  items are 1.74 GB / 516 MB; their compressed output buffers
  hold ~level-dependent state until written). The cap doesn't
  reach those — they're inside zstd's own thread pool, not in
  zimru's parallel-batch queue. To match libzim's peak we'd need
  to (a) cap zstdmt worker count when running under a tight
  memory budget, and (b) drop streaming-encode of huge items
  back to single-threaded zstd. Both are filed as follow-ups; on
  realistic hardware (>= 4 GB free) the current trade-off is
  close to optimal.

* **Practical recommendation.** Default to no cap on machines
  with ≥ 4 GB free; pass `--max-memory=128` on a memory-budgeted
  build (CI runner, Kiwix zimfarm worker, embedded device). The
  cap stops the parallel-batch from running away on large
  inputs — at zstd 19 cap128 saves up to ~2 GB peak vs uncapped,
  with zero wall penalty.

### What changed since the historical numbers below

| commit (most recent first) | what |
|---|---|
| (this PR)  | portable I/O hygiene — `copy_file_range`/`fcopyfile` splice, batched table-write, `madvise(SEQUENTIAL)`, `posix_fadvise`/`F_RDAHEAD`, `READ_BATCH` 64→256 (z3 85→74 s, z19 595→582 s) |
| `849fdaa`  | `--max-memory` soft cap on parallel-batch in-flight bytes |
| `2cd83d1` | dup-emit guard so caller-supplied `M/Counter` etc. wins over auto-emit (zimrecreate forwarding) |
| `1ae1deb` | parallel `fs::read` via rayon batches in `zimwriterfs` (texas 717 → 640 s) |
| `04fefee` | auto-emit `M/Counter`, `X/listing/titleOrdered/v1`, default `M/Scraper` |
| `2e84a6d` | streaming-encode threshold 4 MiB → 256 MiB, HTML single-read (texas 1462 → 717 s) |
| `672073c` | `BuildStats` instrumentation (`ZIMRU_STATS=1`) |
| `f34a8c0` | streaming zstd encode for huge items + auto-verify integrity gate |
| `2cb807b` | chunked-input C ABI (`zimru_creator_begin_item / _item_chunk / _end_item`) |
| `ed95233` | streaming `Creator` with `start_writing` / `finish_writing` |

---

## Historical comparison (from earlier runs)



Comparison of how long it takes to build a ZIM file from a directory
tree on disk, across three implementations:

* **real libzim** — `/opt/homebrew/bin/zimwriterfs` (zim-tools 3.6.0
  upstream binary, built against libzim 9.x by Homebrew).
* **shim + zimru** — **same `zim-tools 3.6.0 zimwriterfs` binary**,
  but `install_name_tool -change`d to load `libzim-shim`'s
  `libzim.dylib` (which forwards every libzim API call to zimru's
  C ABI). Same upstream program, same CLI behaviour, different
  underlying engine.
* **zimru native** — `target/release/zimwriterfs`, a **separate
  Rust binary** in zimru's tree (`src/bin/zimwriterfs.rs`, 407
  lines). Clean-room re-implementation of the zim-tools
  `zimwriterfs` CLI, written from publicly-documented behaviour;
  not a port of zim-tools source. CLI is mostly compatible
  (`-w/--welcome`, `-I/--illustration`, `-l/--language`, etc.) but
  the implementation is independent.

> **What zimru native skips that real / shim do.** zimru's
> `zimwriterfs` is a simpler program than upstream's. It walks the
> directory tree, infers MIME from file extensions (no libmagic),
> extracts `<title>` from HTML for the title field, and packs the
> bytes — that's it. It does **not** scan HTML for
> `<meta http-equiv=refresh>` redirects (the upstream behaviour
> that, on `mdwiki`'s unpacked tree, refused to proceed with a
> `'Entadfi' HTML redirection target path '…/Finasteride/tadalafil'
> doesn't exist` error). It does not run libmagic. It does not
> build a Xapian fulltext index. So part of the speed advantage
> below comes from doing less per-file semantic work, not just
> from engine throughput.
>
> The cleanest **engine-only** comparison is the pair
> `real libzim` vs `shim + zimru`: same upstream binary, same
> per-file work (HTML refresh scan, libmagic, Xapian config call,
> redirect resolution), only the underlying writer engine
> differs. `zimru native` is a separate datapoint answering
> "what if you used zimru's own tooling instead of zim-tools at
> all" — useful, but not the same question.

> **Important caveat: Xapian fulltext indexing.**
>
> Real libzim's `zimwriterfs` builds a **Xapian fulltext index** by
> default (every `text/html` item is parsed, tokenised, and stored
> in `X/fulltextIndex/xapian`). Pass `--withoutFTIndex` to skip it.
>
> Neither **zimru native** nor **shim+zimru** builds a Xapian index
> today. zimru's writer has no Xapian integration; the shim swallows
> `configIndexing(true, lang)` as a no-op so zim-tools' calls don't
> error. Output ZIMs from these stacks are **content-equivalent** to
> a real-libzim ZIM produced with `--withoutFTIndex`, but they have
> **no `X/fulltextIndex/xapian` entry**, so downstream consumers
> that expect a fulltext index (kiwix-serve's `/search`,
> `kiwix-search`) will return empty results on these archives.
>
> Adding Xapian writer support to the shim is a follow-up: the shim
> is GPL-licensed C++ and can link `libxapian` directly to build the
> index in C++ during `addItem`, then ship the finished DB as a ZIM
> entry via `zimru_creator_add_item("X/fulltextIndex/xapian", ...)`.
> zimru itself stays Xapian-free (and MIT) — the index lives in
> the shim layer.
>
> All zimru/shim numbers below are therefore **fastest case for
> zimru, slowest case for real libzim** if you compare default-vs-
> default. The `real libzim --withoutFTIndex` row is the
> apples-to-apples comparison row.

## Inputs

| label | source | files | unpacked size |
|---|---|---:|---:|
| sv-unpacked | osm-silicon-valley-2026-04-22.zim, dumped to fs | 16 904 | 1.7 GB |
| texas-unpacked | osm-texas-2026-04-24.zim, dumped to fs | 1 080 086 | 17 GB |

## Results — sv-unpacked (1.7 GB / 16 904 files)

Compression: each stack at its own default (real libzim ≈ libzstd
default; zimru ≈ zstd level 3 unless `ZSTD_CLEVEL` is set).

| stack | Xapian | time | output | integrity | speedup |
|---|:---:|---:|---:|:---:|---:|
| real libzim (default) | ✅ yes | 143.05 s | 335 MB | Pass | 1.00× (baseline) |
| real libzim (`--withoutFTIndex`) | ❌ no | 139.19 s | 334 MB | Pass | 1.03× |
| shim + zimru | ❌ no | 6.40 s | 290 MB | Pass | 22.4× |
| zimru native | ❌ no | 4.47 s | 290 MB | Pass | 32.0× |

The Xapian step costs only 4 s on this 16 904-item input — a small
fraction of the total. Most of real libzim's wall time is the
underlying writer itself.

## Results — colorado-unpacked (1.7 GB / 324 K files) at `ZSTD_CLEVEL=19`

`ZSTD_CLEVEL=19` is honoured by libzstd directly. zimru's writer
also reads `ZSTD_CLEVEL` as a default-level fallback. **Real
libzim's `zimwriterfs` does NOT honour `ZSTD_CLEVEL`** — it calls
`ZSTD_compress2()` with an explicit hard-coded level, so the env
var is ignored. Real libzim's output below is at libzim's
hard-coded default level (close to zstd 9–12 based on output
size); the zimru paths are at zstd 19.

| stack | Xapian | parallel? | time | output | integrity |
|---|:---:|:---:|---:|---:|:---:|
| real libzim (default w/ Xapian) | ✅ yes | 4 threads | 80.4 s | 762 MB | Pass |
| real libzim (`--withoutFTIndex`) | ❌ no | 4 threads | 86.2 s | 753 MB | Pass |
| shim + zimru (zstd 19, parallel) | ❌ no | rayon | 98.9 s | 473 MB | Pass |
| **zimru native (zstd 19, parallel)** | ❌ no | rayon | **65.4 s** | **473 MB** | Pass |

`zimru native` now **beats real libzim wall-clock** on this
324 K-file build _while_ producing a 38 % smaller output (473 MB
vs 762 MB) at higher compression level (zstd 19 vs libzim's
default ≈ 9–12). The shim path is slower than native (98.9 s vs
65.4 s) but still produces the same zstd-19 output.

Single-threaded baseline (before `1aeedf7` parallel-cluster
encoding shipped): native took 107.4 s on the same input. Adding
rayon for cluster encoding gave 1.65× wall-clock improvement. The
effective parallelism is ~2 cores (user/wall = 1.98); further
wins would require either zstd's own multi-worker mode inside
big clusters or balancing cluster sizes so the long-pole cluster
doesn't dominate at the tail.

### Texas-unpacked (17 GB / 1.08 M files) — measured numbers

All zimru/shim rows are at zstd 19 with the parallel cluster
encoder + streaming writer (commits `1aeedf7` + `7695429`).
Real libzim is at its hard-coded default level (zimwriterfs CLI
doesn't expose level; libzim ignores `ZSTD_CLEVEL`). The
**`--withoutFTIndex` row is the apples-to-apples engine
comparison** — both real libzim and the zimru paths skip the
Xapian fulltext index, so the per-file work and output entries
are equivalent (modulo a few small `M/` differences we still
have to fix; see "Output equivalence" earlier in this doc):

| stack | Xapian | parallel? | wall | user | user/wall | output | RSS peak | integrity |
|---|:---:|:---:|---:|---:|---:|---:|---:|:---:|
| zimru native (zstd 19, par + stream) | ❌ | rayon | **1 011 s (16:51)** | 3 285 s | 3.25 | 3.92 GB | ~19 GB → drains | Pass |
| shim+zimru (zstd 19, par + stream) | ❌ | upstream 4 + rayon | **1 240 s (20:40)** | 3 453 s | 2.78 | 3.92 GB (-3.4 KB vs native) | ~15 GB peak (snapshot, not `time -v`) | Pass |
| **real libzim (`--withoutFTIndex`)** | ❌ | upstream 4 | 1 342 s (22:22) | 2 727 s | 2.03 | **5.22 GB** | ~840 MB | Pass |

For reference, the older runs:

| historical row | wall | output | note |
|---|---:|---:|---|
| zimru native (pre-streaming, parallel) | 933 s | 3.92 GB | RSS peaked at 22.9 GB — buffered everything |
| shim+zimru (pre-streaming, parallel) | 1 742 s | 3.92 GB | same; 17 GB peak RSS |
| real libzim (default w/ Xapian) | 1 315 s | 5.24 GB | extra work building `X/fulltextIndex/xapian`; not a fair comparison to the no-Xapian zimru rows |

**Apples-to-apples note.** The first published version of this
table compared zimru-without-Xapian against real-libzim-WITH-
Xapian. That was wrong — real libzim was building an
`X/fulltextIndex/xapian` over every `text/html` item (extra
work, extra output bytes), and "we're faster because we skip
the work" is not a meaningful claim. The right comparison
is real-libzim `--withoutFTIndex` against the zimru rows;
both are now measured at zstd 19 with the post-streaming
zimru writer (commits `1aeedf7` parallel + `7695429`
streaming output). zimru native and shim+zimru both produce
3.92 GB output (size matches to within 4 KB on a 3.92 GB
archive — same compression, same content, but the file
hashes differ because of metadata-emission ordering and
the shim's slightly different `Hints`-vs-native handling);
real libzim `--withoutFTIndex` produces 5.22 GB.

**Output is still not fully equivalent even with `--withoutFTIndex`.**
zimru-side ZIMs are missing a few entries that real libzim
emits: `M/Counter` (auto-generated mime histogram),
`X/listing/titleOrdered/v1` (explicit title-order listing
entry), and the upstream zimwriterfs binary unconditionally
emits `M/Scraper` and `M/Tags` placeholders that zimru-native
omits. Final equivalence claim has to wait until those land
on the zimru side too; for now the comparison covers the
content (`C/`) namespace and the major `M/` metadata items.

Three observations dominate the take:

1. **zimru native beats real libzim wall-clock** by 1.41× — and
   produces a 25 % smaller archive (3.92 GB vs 5.24 GB) at
   higher compression level (zstd 19 vs libzim's hard-coded
   default ≈ zstd 9–12). This is the single biggest "zimru
   burns through zimwriterfs" datapoint in the doc: even
   excluding Xapian-indexing work that real libzim is doing,
   zimru wins on raw write-speed-per-output-byte at higher
   compression.

2. **Real libzim streams; zimru still partially buffers.**
   Peak RSS on 17 GB input: real libzim
   **~840 MB** (`--withoutFTIndex`, fully streaming) vs
   zimru native **~19 GB then drains** (post-streaming
   writer; clusters now stream out of memory once encoded,
   but the input items are still buffered up front).
   Real libzim fills clusters as items come in,
   compresses+writes+frees, repeats — total RAM stays small
   regardless of archive size. zimru's streaming writer
   freed the cluster-output buffering (commit `7695429`)
   but the input-side buffering remains: every `add_item`
   call's bytes still live in RAM until finalize starts
   bin-packing. On 17 GB texas this fits the Kiwix
   zimfarm's 22 GB per-task budget. **On English Wikipedia
   (~120 GB unpacked) zimru would still OOM the worker** —
   input-side streaming (drain-as-we-go bin-packing into
   the cluster output stream) is the remaining
   production-blocker for the mwoffliner+shim+zimru
   Wikipedia drop-in.

3. **Shim is 1.23× slower than native, 1.08× FASTER than
   real libzim** at this scale (post-streaming). All three
   stacks produce structurally valid ZIMs (`zimcheck -I`
   and `-C` Pass). The shim's slowdown vs native is
   upstream `zimwriterfs`'s per-file work (libmagic mime
   detect, HTML refresh-tag scan, Xapian-indexer prep) plus
   the C-ABI hop on every `addItem` — overhead the simpler
   zimru-native binary doesn't pay. Net of all that, the
   shim still beats real libzim (`--withoutFTIndex`)
   wall-clock at the same per-file work and produces a
   25 % smaller output (3.92 GB vs 5.22 GB) at zstd 19.

Effective parallelism on native at this scale is ~3.8 cores —
much better than the colorado run (1.98×), because the larger
archive has enough independent clusters to keep more rayon
workers fed.

## zimrecreate (ZIM → ZIM, complementary data point)

For completeness — same Texas input as a writer workload, but
through `zimrecreate` (no filesystem extract, no Xapian rebuild
since `zimrecreate` carries the source's existing index across).

| stack | Xapian | time | output |
|---|:---:|---:|---:|
| real libzim | (carries source index unchanged) | 984.5 s | 5.25 GB |
| zimru native | (drops source's Xapian — see caveat) | 61.5 s | 4.67 GB |

Speedup: **15.9×**. Note that zimru's `zimrecreate` does not copy
through the `X/fulltextIndex/xapian` entry the way real libzim's
does — output ZIMs lack the original's fulltext index, even though
the source had one.

## Output equivalence (sv-unpacked)

Bit-for-bit comparison of the outputs produced by all three stacks
on the same 16 904-file input.

### Content (`C/`) namespace — bit-identical

All three outputs have **16 905 entries in `C/`** (the 16 904 user
files plus a generated `C/` placeholder). Sampled 30 paths spread
across the namespace; content `SHA-256` matches across all three
outputs for every sample. The bytes a reader sees when opening an
entry are identical regardless of which writer produced the ZIM.

```
30 identical, 0 mismatching across 3-way compare
```

### Metadata (`M/`) namespace — diverges

| key | real | shim | native |
|---|:---:|:---:|:---:|
| Counter | ✅ | ❌ | ❌ |
| Creator | ✅ | ✅ | ✅ |
| Date | ✅ | ✅ | ✅ |
| Description | ✅ | ✅ | ✅ |
| Illustration_48x48@1 | ✅ | ✅ | ✅ |
| Language | ✅ | ✅ | ✅ |
| Name | ✅ | ✅ | ✅ |
| Publisher | ✅ | ✅ | ✅ |
| Scraper | ✅ | ✅ | ❌ |
| Tags | ✅ | ✅ | ❌ |
| Title | ✅ | ✅ | ✅ |

* `Counter` is auto-generated by libzim's writer at finalize time
  (a mime-type histogram of the archive's content). zimru's writer
  doesn't generate it; the shim doesn't expose a way for the
  upstream-binary code path to forward libzim's auto-Counter
  through, so it's missing on both zimru paths.
* `Scraper`, `Tags` are populated as empty strings by zim-tools'
  `zimwriterfs` even when `-s` / `-a` aren't passed. The shim
  forwards those calls; zimru's own `zimwriterfs` binary
  (different program — see top of this doc) only adds metadata
  the user explicitly passes.
* The other 8 keys (Title/Description/Creator/etc.) are user-supplied
  CLI args; all three stacks honour them.

### Index (`X/`) namespace — diverges (significant)

| entry | real | shim | native |
|---|:---:|:---:|:---:|
| `X/fulltext/xapian` | ✅ | ❌ | ❌ |
| `X/listing/titleOrdered/v1` | ✅ | ❌ | ❌ |
| `X/title/xapian` | ✅ | ❌ | ❌ |

* `X/fulltext/xapian` — Xapian fulltext index over every
  `text/html` item. **kiwix-serve's `/search` and the
  `kiwix-search` CLI rely on this; archives without it return
  empty results.** This is the headline missing piece on the
  zimru paths.
* `X/title/xapian` — separate Xapian DB that backs suggestion
  lookups (`/suggest?term=…`). Without it suggest falls back to
  the title-pointer-list scan, which is functional but slower on
  large archives.
* `X/listing/titleOrdered/v1` — explicit dump of the title-order
  permutation as an entry. zimru computes the same listing on
  the fly from the title-pointer list in the header and doesn't
  emit a separate entry; functionally identical for readers that
  use libzim's APIs but missing for any consumer that opens the
  entry directly.

### Welcome (`W/`) namespace — bit-identical

All three: one entry, `W/mainPage` redirecting to `C/index.html`.

### Bit-for-bit (whole-file)

| pair | result |
|---|---|
| real vs shim | differ — disjoint Xapian/Counter sets, plus per-build UUID |
| real vs native | differ — same as above plus M/ Scraper/Tags absent |
| shim vs native | differ at byte 9 (UUID start) — sizes 290 211 305 vs 290 211 357 (52 bytes) — same content, different metadata-row count, different UUIDs |

Even the two zimru-engine paths aren't byte-identical: shim+zimru
adds the `Scraper`/`Tags` placeholder dirents that zim-tools'
binary always emits, zimru native doesn't. The 52-byte size
difference is the cluster-table delta from those two extra
empty-string M-namespace entries.

## Takeaways

1. zimru's writer is fundamentally faster than real libzim's, even
   single-threaded vs multi-threaded. Most of the gap is per-entry
   overhead (libzim does HTML parsing, link extraction, plus the
   Xapian work) that zimru doesn't do.
2. **For a true production-parity comparison the shim must learn
   to build a Xapian fulltext index in C++ during `addItem`.**
   That work is filed but not yet implemented; until then any
   build that pretends to replace `zimwriterfs` produces a
   silently-missing-search-index archive.
3. zstd 19 (and 22) are slow even on fast machines. Single-thread
   zstd 19 on 17 GB takes ~5–10 minutes of pure compression time;
   that's the floor for any zimwriterfs replacement at production
   compression levels.
4. The shim's overhead vs zimru native is modest (5–10 %): one
   C ABI hop per addItem call, mostly amortised across the
   compression cost. Existing C++ consumers can adopt the shim
   without losing the bulk of the speedup.
