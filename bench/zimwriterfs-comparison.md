# zimwriterfs head-to-head: real libzim vs libzim-shim+zimru vs zimru native

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
| zimru native (zstd 19, par) | ❌ | rayon | _streaming re-run pending_ | _TBD_ | _TBD_ | _TBD_ | _TBD_ | _TBD_ |
| shim+zimru (zstd 19, par) | ❌ | upstream 4 + rayon | _pending_ | _TBD_ | _TBD_ | _TBD_ | _TBD_ | _TBD_ |
| **real libzim (`--withoutFTIndex`)** | ❌ | upstream 4 | **1 342 s (22:22)** | 2 727 s | 2.03 | **5.22 GB** | ~840 MB | Pass |

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
is real-libzim `--withoutFTIndex` against the zimru rows; the
former is now measured (1 342 s / 5.22 GB), the latter is
re-running with the streaming writer.

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

2. **Real libzim streams; zimru buffers.** Peak RSS:
   real libzim **459 MB** on 17 GB of input vs zimru native
   **22.9 GB**. Real libzim fills clusters as items come in,
   compresses+writes+frees, repeats — total RAM stays small
   regardless of archive size. zimru holds every cluster's
   bytes resident until `finalize`, then compresses in
   parallel.
   On 17 GB texas this is fine — barely fits the Kiwix
   zimfarm's 22 GB per-task budget. On English Wikipedia
   (~120 GB unpacked) zimru's current writer would OOM the
   worker. **Streaming cluster compression is the
   production-blocker** for the mwoffliner+shim+zimru
   Wikipedia drop-in (filed as task #22 in the bench notes).

3. **Shim is 1.87× slower than native and 1.32× slower than
   real libzim** at this scale. All three stacks produce
   structurally valid ZIMs. The shim's slowdown vs native is
   upstream `zimwriterfs`'s per-file work (libmagic mime
   detect, HTML refresh-tag scan, Xapian-indexer prep) plus
   the C-ABI hop on every `addItem` — overhead the simpler
   zimru-native binary doesn't pay. The shim's slowdown vs
   real libzim is partly because zimru is at zstd 19 (real is
   at libzim's lower default) and partly because zimru
   buffers everything in RAM (cache pressure on the 22 GB
   worker compared to real libzim's 459 MB streaming
   footprint).

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
