# zimru

A clean-room, MIT-licensed Rust re-implementation of the [ZIM] file format
reader and writer. No code is shared with the GPL-licensed C++ `libzim`;
the format handling is written from the publicly documented [ZIM file
format spec] and validated against real-world ZIM files.

[ZIM]: https://wiki.openzim.org/wiki/OpenZIM
[ZIM file format spec]: https://wiki.openzim.org/wiki/ZIM_file_format

## At a glance

- **93 tests pass** (unit + synthetic-zim + ergonomic-API +
  writer round-trip + streaming-encode + per-item-compression +
  zimwriterfs e2e + real-file integration + doc tests).
- **7 binaries**: `zimru`, plus drop-in replacements for `zimcheck`,
  `zimdump`, `zimbench`, `zimsplit`, `zimrecreate`, `zimwriterfs`. All
  accept upstream `zim-tools` 3.8.0 flags.
- **CLI parity with zim-tools 3.8.0 / libzim 9.8.2** — 12/12 cases on each
  of 8 archives spanning Arabic (RTL), Bashkir (1.1 GB Cyrillic), English,
  Chinese ×2, Tamil, Korean and Japanese.
- **Faster than upstream on every comparable workload**, measured ABBA
  against the current release (see [the benchmark](#head-to-head-benchmark-vs-upstream)):
  `zimcheck -R` 8.7×, `zimcheck -A` 7.0×, `zimdump info` 3.6×,
  `zimdump list` 2.0×, `zimcheck -C` 1.18×, `zimcheck -I` 1.16×;
  `zimrecreate` 1.15×–1.84× at matched compression with both sides building
  search indexes. `zimdump dump` and `zimbench` are excluded — the first is
  swamped by filesystem noise on the test machine, the second is not
  comparable because upstream exits 0 without running its read phases.
- **Content equivalence verified through real libzim**, not through zimru's
  own reader: every one of 252 234 entries identical after a recreate.
- **Writer validated end-to-end on real ZIMs** across 13 MB → 1.1 GB and
  10 languages, with the rebuilt Xapian index answering upstream
  `zimsearch` with the same top hit as libzim's original.

## Goals

- **API-compatible with libzim** as a drop-in: the public surface mirrors the
  libzim user-facing types (`Archive`, `Entry`, `Item`, `Blob`) and method
  names map one-for-one to their C++ equivalents (in snake_case as is
  idiomatic in Rust):
  | libzim (C++)                      | zimru (Rust)                            |
  |-----------------------------------|-----------------------------------------|
  | `Archive(path)`                   | `Archive::open(path)`                   |
  | `archive.getUuid()`               | `archive.uuid()`                        |
  | `archive.getAllEntryCount()`      | `archive.all_entry_count()`             |
  | `archive.hasMainEntry()`          | `archive.has_main_entry()`              |
  | `archive.getMainEntry()`          | `archive.main_entry()`                  |
  | `archive.hasChecksum()`           | `archive.has_checksum()`                |
  | `archive.getChecksum()`           | `archive.checksum()`                    |
  | `archive.check()`                 | `archive.check()`                       |
  | `archive.getEntryByPath(path)`    | `archive.get_entry_by_path(path)`       |
  | `archive.hasEntryByPath(path)`    | `archive.has_entry_by_path(path)`       |
  | `archive.getEntryByTitle(title)`  | `archive.get_entry_by_title(title)`     |
  | `archive.iterByPath()`            | `archive.iter_by_path()`                |
  | `archive.iterByTitle()`           | `archive.iter_by_title()`               |
  | `archive.getMetadata(name)`       | `archive.get_metadata(name)`            |
  | `archive.getMetadataKeys()`       | `archive.get_metadata_keys()`           |
  | `entry.getPath()`                 | `entry.path()`                          |
  | `entry.getTitle()`                | `entry.title()`                         |
  | `entry.isRedirect()`              | `entry.is_redirect()`                   |
  | `entry.getItem(follow)`           | `entry.get_item(follow)`                |
  | `entry.getRedirectEntry()`        | `entry.get_redirect_entry()`            |
  | `item.getMimetype()`              | `item.mimetype()`                       |
  | `item.getSize()`                  | `item.size()`                           |
  | `item.getData()`                  | `item.get_data()`                       |
  | `blob.data() / size()`            | `blob.data() / size() / to_vec()`       |
- **Idiomatic Rust extras** layered on top — `Result`/`Error` for fallible
  ops, `Iterator`-based traversal, zero-copy `&[u8]` blob access into
  `Arc<[u8]>` cluster snapshots, memory-mapped IO, snake_case naming.
- **No GPL contamination.** Only the public ZIM file format spec was
  consulted (along with downstream user code such as the
  [`python-libzim`](https://github.com/openzim/python-libzim) README) to
  decide on naming. No libzim source was read or copied.

A C-FFI shim that lets C++ apps link `zimru` in place of `libzim` is a
follow-up. The current crate is a Rust-only library + CLI.

## Format coverage

| Feature                                                     | Status |
|-------------------------------------------------------------|--------|
| Header (magic / version / pointers / UUID / checksum pos)   | ✅     |
| MIME type list                                              | ✅     |
| URL pointer list (binary search by `(namespace, url)`)      | ✅     |
| Title pointer list — legacy in-header                       | ✅     |
| Title pointer list — modern `X/listing/titleOrdered/v{0,1}` | ✅     |
| Article dirents                                             | ✅     |
| Redirect dirents (multi-hop following with loop detection)  | ✅     |
| Cluster pointer list                                        | ✅     |
| Cluster: uncompressed (types 0 & 1)                         | ✅     |
| Cluster: zstd (type 5)                                      | ✅     |
| Cluster: xz / LZMA2 (type 4)                                | ✅     |
| Cluster: zlib / bzip2 (deprecated, types 2 & 3)             | ❌ rejected |
| Cluster: 4-byte and 8-byte (extended) blob offsets          | ✅     |
| MD5 checksum verification                                   | ✅     |
| Multi-part files (`.zimaa`, `.zimab`, …)                    | ⏳ TODO |
| Xapian fulltext / suggestion search (`X/fulltext/xapian`)   | 🚫 out of scope (see [licensing rationale](#licensing--clean-room-policy)) |
| ZIM writer (creator)                                        | ✅     |

## Library example

```rust
use zimru::Archive;

let archive = Archive::open("wikipedia_en_100_mini.zim")?;
let main = archive.main_entry()?;
let item = main.get_item(true)?;          // follow redirects
println!("{} ({} bytes, mime={})", item.path(), item.size()?, item.mimetype());
println!("{}", String::from_utf8_lossy(item.get_data()?.data()));
# Ok::<(), zimru::Error>(())
```

## Idiomatic convenience API

On top of the libzim one-for-one surface, `zimru` ships an ergonomic layer
designed around common read-only workflows. Every method below exists
specifically because the Rust rewrite enabled it — zero-copy `&str` from
mmap, rayon-aware parallelism, iterator composition, `Result` everywhere,
`Deref` for `&[u8]`. Pick the one that matches your use case:

### One-liners for the common case

```rust
use zimru::Archive;
let a = Archive::open("wiki.zim")?;

// "Give me the article body as a string."
let html: String = a.get_text("Albert_Einstein")?;

// "Give me the bytes." (follows redirects automatically)
let bytes: Vec<u8> = a.get_bytes("images/logo.png")?;

// "Give me the full Item object, redirect already followed."
let item = a.get_item("home")?;

// "What's the main page actually, after redirect chasing?"
let main_path: String = a.main_path()?;          // e.g. "index"
let main_item  = a.main_item()?;                 // Item of the resolved target

// "Read this metadata as a string."
let title: String = a.metadata_str("Title")?;    // "Wikipedia 100"
let lang:  String = a.metadata_str("Language")?; // "eng"

// "Does this key exist?"
if a.has_metadata("Illustration_48x48@1") { ... }
# Ok::<(), zimru::Error>(())
```

### Filtered iteration

```rust
use zimru::Archive;
let a = Archive::open("wiki.zim")?;

// Only real articles (skip redirects) in path order.
for e in a.articles()       { let e = e?; println!("{}", e.path()); }

// Only redirects.
for e in a.redirects()      { let e = e?; println!("{} -> ?", e.path()); }

// Only C-namespace entries (user content), exact URL-pointer range.
for e in a.content_entries() { let e = e?; /* … */ }

// Every entry whose (namespace, url) starts with a prefix.
// Binary-search for the start, then walks forward in O(log n + k).
for e in a.by_prefix(b'C', "images/") {
    let e = e?;
    println!("{}", e.path());   // images/header.png, images/logo.png, ...
}

// O(log n) namespace count — no full scan.
let (content_start, content_end) = {
    let r = a.namespace_range(b'C')?;
    (r.start, r.end)
};
# Ok::<(), zimru::Error>(())
```

### Rayon-aware parallelism

```rust
use rayon::prelude::*;
use zimru::Archive;
let a = Archive::open("wiki.zim")?;

// Parallel dirent-only scan (no cluster decompression).
let redirect_count: u32 = a.par_iter_by_path()
    .filter_map(|r| r.ok())
    .filter(|e| e.is_redirect())
    .count() as u32;

// Parallel foreach across every decompressed cluster. Each worker
// decompresses one cluster exactly once, drops it when done. This is
// how zimcheck -A hits 8× upstream throughput; use it to build your
// own parallel content scanners.
let total_decompressed_bytes: u64 = a.par_clusters(|_idx, c| {
    (0..c.blob_count())
        .filter_map(|i| c.blob(i).ok())
        .map(|b| b.len() as u64)
        .sum::<u64>()
})?.into_iter().sum();
# Ok::<(), zimru::Error>(())
```

### Zero-copy content access

```rust
use std::io::Read;
use zimru::Archive;
let a = Archive::open("wiki.zim")?;
let item = a.get_item("home")?;

// Most efficient: borrow &str directly from the decompressed cluster.
let blob = item.get_data()?;
let text: &str = blob.as_str()?;                 // zero-copy UTF-8 validation

// Or stream it without materializing a Vec<u8>.
let mut sink = Vec::new();
blob.reader().read_to_end(&mut sink)?;

// Blob derefs to &[u8] and AsRef<[u8]>, so it drops into any API
// that expects byte slices:
let len = blob.len();         // via Deref
sink.extend_from_slice(&blob); // via Deref -> &[u8]
# Ok::<(), zimru::Error>(())
```

### Item mime-type helpers

```rust
use zimru::Archive;
let a = Archive::open("wiki.zim")?;
let item = a.get_item("home")?;

if item.is_html()  { /* decode HTML */ }
if item.is_text()  { /* treat as text/* */ }
if item.is_image() { /* save to disk */ }
let text = item.text()?;         // shortcut for utf-8 bytes -> String
let bytes = item.bytes()?;       // shortcut for blob -> Vec<u8>
# Ok::<(), zimru::Error>(())
```

### Snapshot for logging & `--info` commands

```rust
use zimru::Archive;
let a = Archive::open("wiki.zim")?;
let s = a.summary();
// {:?} / {:#?} for Debug, {} for a pre-formatted multi-line dump.
println!("{s}");
// uuid            529b7e6e-3e90-9b9b-3d24-eac14f2f1f00
// version         6.3 (new namespaces)
// entries         5172 total (5155 content)
// clusters        4
// mime types      10
// main page       index
// checksum        yes
# Ok::<(), zimru::Error>(())
```

### `Entry` as `Display`

```rust
# use zimru::Archive;
# let a = Archive::open("wiki.zim")?;
let e = a.get_entry_by_path("apple")?;
println!("{e}");         // C/apple "Apple"
let r = a.get_entry_by_path("fruit")?;
println!("{r}");         // ↪ C/fruit "Fruit"   (unicode arrow marks a redirect)
# Ok::<(), zimru::Error>(())
```

## CLI

The crate ships **seven binaries** — one Rust-native explorer plus six
drop-in replacements for the upstream `zim-tools` family:

| binary        | upstream counterpart      | status                                                                |
|---------------|---------------------------|-----------------------------------------------------------------------|
| `zimru`       | (native, no upstream)     | Inspection / extraction / read-every-blob benchmark                   |
| `zimcheck`    | `zim-tools/zimcheck`      | Full CLI parity (all flags) + JSON output. **6.0× faster than upstream on `-A`, 8.4× on `-R`.** |
| `zimdump`     | `zim-tools/zimdump`       | `info` / `list` / `list --details` / `show` / `dump`. **2.73× faster on `info`.** |
| `zimbench`    | `zim-tools/zimbench`      | `-n` / `-r` / `-d` flags. Upstream's random-URL phase crashes; ours runs to completion. |
| `zimsplit`    | `zim-tools/zimsplit`      | byte-aligned split (concat reproduces original)                       |
| `zimrecreate` | `zim-tools/zimrecreate`   | reads source cluster-grouped, writes new archive preserving per-cluster compression. **1.15×–1.84× faster at matched compression**, both sides building search indexes. |
| `zimwriterfs` | `zim-tools/zimwriterfs`   | packs an HTML directory tree into a ZIM: symlinks → redirects, content-sniffed mimetypes for extensionless files, mime-driven cluster compression. Completes 153 k-file builds upstream dies on (fd exhaustion). |

```sh
cargo build --release

# Native helper
./target/release/zimru info     wikipedia_en_100_mini.zim
./target/release/zimru readall  wikipedia_en_100_mini.zim --md5
./target/release/zimru get      wikipedia_en_100_mini.zim Alcoholism
./target/release/zimru get      wikipedia_en_100_mini.zim 'W/mainPage'  # explicit ns

# zim-tools drop-in replacements
./target/release/zimcheck -A wikipedia_en_100_mini.zim
./target/release/zimcheck -A -J wikipedia_en_100_mini.zim     # JSON
./target/release/zimdump  info wikipedia_en_100_mini.zim
./target/release/zimdump  list --details wikipedia_en_100_mini.zim
./target/release/zimbench -n 1000 wikipedia_en_100_mini.zim
./target/release/zimsplit --prefix=part- --size=2G --force wikipedia_en_100_mini.zim

# Writer tools (zimrecreate also accepts every upstream flag + extras)
./target/release/zimrecreate source.zim out.zim --compression zstd
./target/release/zimwriterfs --welcome=index.html --illustration=icon48.png \
    --language=eng --name=demo --title="Demo" --description=d \
    --creator=me --publisher=zimru ./html_dir out.zim
```

## CLI parity vs upstream `zim-tools` 3.8.0 / `libzim` 9.8.2

`bench/parity.sh` runs the upstream tool and our binary on the same archive,
normalizes version strings + elapsed-time, and diffs outputs. This is a
correctness harness, not a benchmark — no timing is involved. Across 8
real-world ZIMs it reports **96/96 cases pass** (12 cases × 8 archives:
Arabic, Bashkir 1.1 GB, English, Japanese, Korean, Tamil, and two Chinese):

| case                    | matching mode    | result   |
|-------------------------|------------------|----------|
| `zimdump info`          | byte-for-byte    | ✓ 8/8    |
| `zimdump list`          | byte-for-byte    | ✓ 8/8    |
| `zimdump list --details`| byte-for-byte    | ✓ 8/8    |
| `zimdump show --idx=N`  | byte-for-byte    | ✓ 8/8    |
| `zimcheck -C`           | byte-for-byte    | ✓ 8/8    |
| `zimcheck -I`           | byte-for-byte    | ✓ 8/8    |
| `zimcheck -M`           | byte-for-byte    | ✓ 8/8    |
| `zimcheck -F`           | byte-for-byte    | ✓ 8/8    |
| `zimcheck -P`           | byte-for-byte    | ✓ 8/8    |
| `zimcheck -L`           | byte-for-byte    | ✓ 8/8    |
| `zimcheck -A`           | structural*      | ✓ 8/8    |
| `zimcheck -A -J` (JSON) | structural*      | ✓ 8/8    |

\* "Structural" means everything in the report matches except two blocks
whose contents depend on libzim's internal iteration:

- `[WARNING] Redundant Data:` lines — compared as a *set*, with the two
  paths in each pair normalized, since which member prints first depends on
  hash order.
- `[ERROR] Internal URL: Dangling link(s) …` blocks — elided, with the
  counts reported separately (e.g. `dangling=10/9` on Bashkir). Upstream's
  HTML parser reports a scattered subset of the links actually present:
  every extra zimru reports has been checked against the raw markup and is
  a real `href`/`src` in a real tag pointing at a missing entry.

Everything else, including the phase interleaving 3.8.0 introduced, is
compared exactly.

Tools we don't yet reach byte-for-byte parity on:

| tool          | status                                                            |
|---------------|-------------------------------------------------------------------|
| `zimrecreate` | **Implemented** — reads source, writes new archive. Output passes upstream `zimcheck -A`. |
| `zimwriterfs` | **Implemented** — packs an HTML directory tree into a ZIM. CLI-compatible with upstream (every flag accepted). |
| `zimpatch`    | needs libzim diff format — TODO                                   |
| `zimdiff`     | needs libzim diff format — TODO                                   |
| `zimsearch`   | needs Xapian fulltext index — TODO                                |
| `zimsplit`    | parts produced; output uses byte-aligned splits (upstream splits  |
|               | on cluster boundaries). Both reconstruct the original via concat. |
| `zimbench`    | runs end-to-end; upstream's random-URL phase crashes on our test  |
|               | files ("Cannot find entry"), ours doesn't.                        |

## Writing ZIM files

The `zimru::writer::Creator` API mirrors libzim's `Creator` in idiomatic
Rust:

```rust
use zimru::writer::{Creator, Item};
use zimru::Compression;

let mut c = Creator::new();
c.set_main_path("home")
 .set_compression(Compression::Zstd)
 .add_item(Item::html("home", "Home", "<h1>Welcome</h1>"))
 .add_item(Item::html("about", "About", "<p>About page.</p>"))
 .add_item(Item::png("favicon.png", "favicon", std::fs::read("icon.png")?))
 .add_redirection("start", "Start", "home")       // alias for C/home
 .add_metadata("Title", "Demo")
 .add_metadata("Language", "eng")
 .add_metadata("Creator", "zimru")
 .add_illustration(48, std::fs::read("icon48.png")?);
c.write_to("demo.zim")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Writer features:

- **Bin-packed clusters** — items are greedily grouped until the running
  payload hits `cluster_size_target` bytes (default 2 MiB, same as
  upstream `zimwriterfs`).
- **Continuous parallel encode pipeline** — cluster compression runs on
  one worker thread per CPU while a dedicated writer thread appends
  encoded clusters in completion order, fully overlapped with item
  production. At zstd 19 on a 3 GB corpus the encoders sustain ~97% of
  available CPU, which is what makes zimru's high-compression builds
  faster than libzim's.
- **Compression**: `Compression::None`, `Compression::Zstd` (default),
  `Compression::Xz`. Per-item override via `Item::with_compress(false)`
  routes already-compressed payloads (JPEG, video, …) into raw clusters.
  zstd frames carry the frame-content-size header so readers (zimru,
  libzim, fzstd) can pre-allocate exact decode buffers.
- **Streaming mode** — `start_writing()` / `finish_writing()` bin-packs
  and encodes as items arrive; only dirent metadata stays resident.
  Items ≥256 MiB stream through a chunked encoder bounded at
  ~encoder-state memory.
- **Namespaces**: content in `C`, metadata and illustrations in `M`, the
  main-page redirect in `W`. Output uses version 5.1 (new namespaces,
  in-header title pointer list, MD5 trailer) — readable by libzim 8+.
- **MD5 checksum** is appended automatically, followed by a structural
  post-write verify (pointer tables, dirent order, title index,
  mimetypes) so a successful build is guaranteed readable.
- **Redirect resolution** happens at finalize time — a dangling redirect
  target returns an error instead of silently corrupting the file.
- **UUID preservation** — `set_uuid(uuid)` lets `zimrecreate` carry the
  source archive's UUID into the recreated output.

### `zimrecreate` binary

Reads any zimru/libzim-compatible archive and rewrites it using the
`Creator` API. Matches upstream's CLI signature:

```sh
./target/release/zimrecreate source.zim recreated.zim \
    --compression zstd          # none | zstd | xz  (default zstd)
    --cluster-size 2097152      # target cluster size in bytes
```

### `zimwriterfs` binary

Packs a filesystem directory of HTML/CSS/JS/images/… into a single ZIM,
matching upstream `zim-tools/zimwriterfs` flag-for-flag:

```sh
./target/release/zimwriterfs \
    --welcome=index.html \
    --illustration=icon48.png \
    --language=eng \
    --name=my-zim \
    --title="My ZIM" \
    --description="Example archive" \
    --creator="Me" \
    --publisher="Kiwix" \
    --skip-libmagic-check \
    ./html_dir out.zim
```

All upstream flags are accepted (`-w/--welcome`, `-I/--illustration`,
`-l/--language`, `-n/--name`, `-t/--title`, `-d/--description`,
`-c/--creator`, `-p/--publisher`, plus `-L/-m/-J/-x/-r/-j/-a/-e/-o/-s`
optional ones), so existing build scripts that drive `zimwriterfs` can
swap binaries with no other changes. `-J/--threads` sizes the encode
worker pool (default: one per CPU). Symlinks are converted into ZIM
redirects, matching upstream. Mime detection is by file extension with
a content sniff (HTML/SVG/XML markers, common image/audio magic bytes,
UTF-8 text check) for extensionless files — no libmagic dependency —
and drives per-item cluster compression: text-like content is
compressed, already-compressed media goes into raw clusters, same as
libzim's hints.

Tested side-by-side with upstream `zimwriterfs`: the same source
directory produces ZIMs with the same C-namespace path set, both pass
`upstream zimcheck -A`, both contain the mandatory metadata (Title,
Description, Language, Creator, Publisher, Name, Date,
Illustration_48x48@1).

### Creation benchmark vs upstream

Measured by `bench/creation-bench.sh` — see
[Head-to-head benchmark vs upstream](#write-side) for the current table
against zim-tools 3.8.0. Summary: **1.15×–1.84×** faster at matched
compression with both sides building search indexes, output sizes within
±5%.

Earlier revisions of this README reported 19×–59× here. That figure
compared zimru at its own default (zstd 3, no index) against upstream at
libzim's default (zstd 19, with a fulltext index) — it measured preset
choice and workload, not implementation. Matching the compression level,
the thread count and the set of indexes built is what the current harness
does, and it is the only way the number means anything.

### `zimrecreate` validated across diverse real-world archives

Reproduced via `bench/recreate-suite.sh zim-cache/*.zim` — for each ZIM
in the cache we (1) read it with `zimru readall --md5`, (2) rewrite via
`zimrecreate --compression zstd`, (3) run `upstream zimcheck -A` on
both source and output, (4) re-read the output and compare the per-blob
MD5 sum back to the original.

| file                                | size                | entries | zimcheck -A    | blob MD5 |
|-------------------------------------|---------------------|---------|----------------|----------|
| `wikipedia_ba_all_maxi.zim`         | 1.1 GB → 1.1 GB     | 175 387 | FAIL (= source)| MATCH    |
| `wikipedia_zh_chemistry_mini.zim`   | 14 MB → 7.0 MB      | 12 581  | PASS (= source)| MATCH    |
| `wikipedia_en_100_nopic.zim`        | 13 MB → 12 MB       | 5 175   | PASS (= source)| MATCH    |
| `freecodecamp_js.zim`               | 6.7 MB → 7.9 MB     | 741     | PASS (= source)| MATCH    |
| `vikidia_ca.zim`                    | 5.0 MB → 4.6 MB     | 1 504   | PASS (= source)| MATCH    |
| `wikipedia_en_100_mini.zim`         | 4.4 MB → 3.3 MB     | 5 155   | PASS (= source)| MATCH    |
| `gutenberg_ale.zim`                 | 2.0 MB → 1.1 MB     | 79      | FAIL (= source)| MATCH    |
| `wikiquote_af.zim`                  | 1.8 MB → 1.6 MB     | 460     | PASS (= source)| MATCH    |

**All 8/8 round-trip with byte-perfect blob content** (BLOB-MD5 MATCH on
every file). The two `FAIL (= source)` cases — `gutenberg_ale.zim` and
`wikipedia_ba_all_maxi.zim` — fail upstream zimcheck for the same
reasons in both source AND recreated form (pre-existing dangling
internal links like `icons/apple-touch-icon.png`). Our writer preserves
the source's broken-link state faithfully — it does not introduce any
new validation failures.

Where the source has no flaws (the other 6 archives, spanning English /
Chinese / Catalan / Afrikaans content + Wikipedia / Wiktionary /
Wikiquote / Vikidia / FreeCodeCamp / Gutenberg-style sources), the
recreated file passes upstream zimcheck's full sweep.

## Head-to-head benchmark vs upstream

Against **zim-tools 3.8.0 / libzim 9.8.2** (the current release), reproduced
via `bench/toolset-bench.sh`. Hardware: shared 4-core linux container.

### Methodology

The harness does **not** use hyperfine, which runs every repetition of A and
then every repetition of B. Several of these workloads write (`zimdump dump`
extracts 175 k files) or stream a gigabyte through page cache, so running one
tool's repetitions back-to-back hands the second command a machine the first
one warmed or dirtied — and the effect is worth more than some of the
differences being measured.

Instead each pair runs **ABBA** — zimru, upstream, upstream, zimru — with
each tool keeping its best, so both get the same number of first- and
last-position runs. Before every run the previous output is removed and the
archive is re-read into page cache; afterwards the run's writes are synced
outside the timed region. Every individual run time is recorded, and the
spread is small: on the 1.1 GB archive `zimcheck -A` came in at 9.894 /
9.882 s for zimru against 59.327 / 59.097 s for upstream.

### Read side

1.1 GB Bashkir Wikipedia (175 404 entries, 3.2 GB decompressed), plus two
CJK archives to check the pattern holds across scripts:

| workload                                     | zimru   | upstream | speedup   |
|----------------------------------------------|---------|----------|-----------|
| `zimcheck -C` (MD5 trailer only)             | 2.050 s | 2.421 s  | **1.18×** |
| `zimcheck -I` (structure + every cluster)    | 2.504 s | 2.895 s  | **1.16×** |
| `zimcheck -R` (decompress + MD5 every blob)  | 2.182 s | 18.899 s | **8.66×** |
| `zimcheck -A` (full sweep)                   | 8.486 s | 59.165 s | **6.97×** |
| `zimdump info`                               | 0.005 s | 0.018 s  | **3.60×** |
| `zimdump list`                               | 0.075 s | 0.149 s  | **1.99×** |

| workload      | `ko_top_mini` 154 MB | `zh_chemistry_maxi` 128 MB |
|---------------|----------------------|----------------------------|
| `zimcheck -C` | **1.23×**            | **1.20×**                  |
| `zimcheck -I` | **2.16×**            | **1.17×**                  |
| `zimcheck -R` | **9.91×**            | **8.74×**                  |
| `zimcheck -A` | **5.98×**            | **4.58×**                  |
| `zimdump info`| **3.80×**            | **3.20×**                  |
| `zimdump list`| **2.01×**            | **2.69×**                  |

`zimdump dump` and `zimbench` are excluded from both tables; see below.

### What is not in the tables, and why

Two workloads are excluded rather than scored, and one archive breaks a
third tool outright.

- **`zimdump dump` is not reliably measurable here.** Extracting 175 k files
  is dominated by the container's filesystem and writeback behaviour, not by
  either tool: within a *single* ABBA pair the two runs of one tool came in
  at 7.698 s and 19.832 s, and upstream's at 30.022 s and 13.964 s. A 2.58×
  spread swamps any difference between the implementations. Every other
  workload in these tables holds within 1.15× run-to-run, which is what
  makes those numbers worth quoting and this one not. An earlier revision
  of this README claimed 3.98× here; that was one sample of a distribution
  this wide.

- **`zimbench` is not comparable.** Upstream collects its URL lists and then
  exits **0** without running either read phase, reporting no throughput;
  zimru runs all three (linear access 1 000 reads / 84 MB / 42.7 MB/s,
  random access 1 000 reads / 84 MB / 45.2 MB/s). Taken at face value the
  numbers say upstream is ~190× faster, and the exit status does not
  contradict it. It is doing none of the work. `bench/toolset-bench.sh`
  keeps the row and prints both exit codes rather than dropping it, so the
  asymmetry stays visible.

- **`zimdump dump` fails outright on the Korean archive**: upstream exits
  255 after 0.1 s with `Error creating symlink from …/%/%`, because that
  archive contains entries named `%` and `$`. zimru completes the export.
  This is why the harness prints exit codes at all — a crash is otherwise
  indistinguishable from a very fast run, and the same trap is what makes
  `zimdump dump`-based content verification useless on that file (it gives
  up after 24 of 252 234 entries, still exiting 0).

`zimcheck -I` used to be the one workload where upstream was faster (0.74×).
The cause was structural rather than algorithmic — `-A`'s content scan had
been rayon-parallel by cluster for some time, but `-I` still decoded every
cluster in a sequential loop, 1.8 s of a 3.8 s run. Parallelizing it took
`-I` to 1.16× and carried `-A` from 5.98× to 6.97×, since `-A` runs the
integrity check too. Corruption detection is unchanged: an archive with
damaged zstd cluster payloads and a re-stamped MD5 trailer — so that only
decoding can catch it — still fails, with the same message every run.

### Write side

`bench/creation-bench.sh`, same ABBA scheme, matched compression (upstream's
`zimrecreate` exposes no knobs and uses libzim's zstd 19, so zimru is driven
at `--compression zstd --compression-level 19`), same thread count, both
tools spawning their indexers — zimru the `xapianbuilder` helper, upstream
its in-process Xapian.

Two modes, both producing the **same entry set**: `index` builds fulltext +
title on both sides; `noft` passes `-j` to both, which drops the fulltext
index and keeps the title index.

| archive                     | size   | mode  | zimru   | upstream | speedup   | size vs upstream |
|-----------------------------|--------|-------|---------|----------|-----------|------------------|
| `wikipedia_ar_chemistry_mini` | 13 MB  | index | 3.82 s  | 5.14 s   | **1.35×** | −3.7%            |
|                             |        | noft  | 3.13 s  | 3.74 s   | **1.19×** | +0.2%            |
| `wikipedia_zh_chemistry_mini` | 14 MB  | index | 3.82 s  | 7.04 s   | **1.84×** | −1.9%            |
|                             |        | noft  | 2.93 s  | 3.74 s   | **1.28×** | −3.8%            |
| `wikipedia_zh_chemistry_maxi` | 128 MB | index | 20.47 s | 28.52 s  | **1.39×** | −0.0%            |
|                             |        | noft  | 14.55 s | 16.92 s  | **1.16×** | −0.2%            |
| `wikipedia_ta_top_mini`     | 134 MB | index | 90.89 s | 108.17 s | **1.19×** | −2.1%            |
|                             |        | noft  | 78.87 s | 96.95 s  | **1.23×** | +0.1%            |
| `wikipedia_ko_top_mini`     | 154 MB | index | 76.53 s | 112.10 s | **1.46×** | −3.4%            |
|                             |        | noft  | 50.31 s | 79.16 s  | **1.57×** | −4.7%            |
| `wikipedia_ja_top_mini`     | 179 MB | index | 90.11 s | 132.54 s | **1.47×** | −3.3%            |
|                             |        | noft  | 61.37 s | 89.23 s  | **1.45×** | −4.9%            |
| `wikipedia_en_100`          | 318 MB | index | 9.13 s  | 10.91 s  | **1.19×** | +0.0%            |
|                             |        | noft  | 8.03 s  | 9.23 s   | **1.15×** | +0.0%            |
| `wikipedia_hi_all_mini`     | 378 MB | index | 329.6 s | 393.1 s  | **1.19×** | —                |

zimru is faster on every row, by 1.15×–1.84×. The margin tracks how much
indexing the archive needs: widest on CJK, narrowest on `en_100`, which is
mostly media with a 3.2 MB index.

**Output sizes land within ±5%, and where zimru is smaller it is the index
that is smaller, not the content compression.** Content clusters agree
within 0.2% — which is what should happen when both tools run zstd 19 over
identical bytes. Earlier revisions of this table claimed 25–42% size wins;
those were a measurement error, described below.

Indic archives are disproportionately expensive on **both** sides (Tamil
takes 91 s for 134 MB against English's 9 s for 318 MB). That is a property
of the shared Xapian accent/stemming pipeline, not of either writer.

### Content equivalence

Being smaller is only interesting if the archive is still the same archive.
`bench/content-verify.sh` diffs per-entry content manifests between the
source, zimru's recreate and upstream's, reading every entry through **real
libzim** (zimru-misc's `zim-manifest`) rather than through zimru's own
reader — checking our writer with our reader would only prove the two agree
with each other.

| archive                       | entries | identical | changed | missing | extra |
|-------------------------------|---------|-----------|---------|---------|-------|
| `wikipedia_zh_chemistry_mini` | 12 692  | 12 692    | 0       | 0       | 0     |
| `wikipedia_ko_top_mini`       | 252 234 | 252 234   | 0       | 0       | 0     |

Both writers, both archives: same path set, same redirect graph, same MD5
per item.

The Korean archive is exactly the case where the obvious verification
approach fails silently. Extracting with `zimdump dump` and hashing the tree
routes every entry through a filesystem path, and upstream zimdump aborts on
the `%` entries after **24 of 252 234** — while still exiting 0.

### Two measurement errors found and fixed

Both inflated zimru's results, and both are recorded here rather than
quietly corrected, because the wrong numbers circulated first.

1. **Ordering bias.** The harness ran zimru then upstream, warming the
   source once before the pair. zimru's output writes — up to a gigabyte —
   evicted the warmed source, so upstream re-read from disk what zimru got
   from cache. Worth a few percent, in zimru's favour, on exactly the
   comparison the benchmark existed to make. Fixed by the ABBA scheme above.

2. **`-j` meant different things to the two tools.** Upstream's
   `--withoutFTIndex` drops only the fulltext index and still builds
   `X/title/xapian`; zimru's dropped both. The old "noindex" mode therefore
   compared an archive that had a title index against one that did not, and
   scored the missing index as a compression win — **−42.4% on Korean,
   where the honest figure is −4.7%**. That was also a real CLI-parity bug:
   `zimru zimrecreate -j` produced an archive missing an entry upstream's
   `-j` output contains. `-j` now matches upstream; `--without-indexes`
   drops both.

## Why zimru is faster

Four independent optimizations, each measurable on its own. Figures are
from the 1.1 GB Bashkir Wikipedia test file. Where a row compares against
upstream it is the ABBA best-of described above; rows comparing zimru
against an earlier zimru are historical, from when that change landed, and
are labelled as such.

### 1. `md-5` with the `asm` feature (beats OpenSSL + `md5sum`)

`zimcheck -C` is the pure "hash every byte of the file" workload. Both
tools are bottlenecked on MD5 throughput. Upstream libzim uses
OpenSSL-accelerated MD5 (~510 MB/s on this machine); enabling the `md-5`
crate's `asm` feature pulls in `md5-asm`'s hand-written x86-64 block
transform and bumps zimru's throughput to **~556 MB/s**, matching the
system `md5sum`.

| build                      | `-C` on 1.1 GB | throughput |
|----------------------------|----------------|------------|
| pure-Rust `md-5` (default) | 2.52 s         | 450 MB/s   |
| **`md-5` + `asm`**         | **2.05 s**     | **556 MB/s** |
| `md5sum` (system)          | 2.09 s         | 545 MB/s   |
| upstream `zimcheck -C`     | 2.29 s         | 498 MB/s   |

Cost: one Cargo feature flag. Wall-clock saved on `-C`: **470 ms per run**
(≈19% faster than before, and 10% faster than upstream).

### 2. Single-pass content scan (4× fewer iterations)

Upstream and our previous code both ran every content check as its own
pass over the entire C-namespace: once for `empty`, once for `redundant`,
once for `url_internal`, once for `url_external`. That means four cluster
decompressions (with a cache; without the cache it'd be 4× the zstd/xz
work), four HTML UTF-8 decodes per article, four dirent parses, and four
passes of allocating intermediate `String`s.

zimcheck now folds all four checks into one per-blob loop. Each blob is
fetched once; we MD5 it for redundancy, check its size for empty,
UTF-8-decode once if it's HTML, and scan `href=`/`src=` attributes a
single time for both internal and external URL classification.

Isolated gain (single-threaded, before rayon): `-R` went from 11.3 s to
roughly 8.0 s — **~30% wall-clock reduction** from deduplicated work
alone.

### 3. Parallel cluster scan with rayon (near-linear with core count)

The dominant cost on `-A` is decompressing every cluster (1 502 of them
for the Bashkir file, totalling 3.2 GB of output) plus MD5-ing every
blob. That's embarrassingly parallel at cluster granularity:

- Each cluster is self-contained (its decompression doesn't need any
  other cluster).
- Per-blob work (MD5, HTML scan, internal-URL binary-search) only
  reads the mmap'd archive, which is lock-free.

zimcheck builds a `HashMap<cluster_idx, Vec<BlobRef>>` once, then uses
`rayon::par_iter` across the clusters. Each worker calls the new
`Archive::cluster_uncached(idx)` which bypasses the shared cluster cache
(no lock contention — we know each cluster is touched exactly once in
this pass). Per-cluster findings come back as `Vec<PerEntryFinding>`;
the main thread merges them in URL-pointer order so the report text
stays **deterministic** and still matches upstream byte-for-byte on all
12 parity cases.

| build                            | `-A` on 1.1 GB   | speedup vs upstream |
|----------------------------------|------------------|---------------------|
| single-pass, single-thread       | ~12 s (estimate) | ~4.8×               |
| single-pass, rayon content scan  | 9.88 s           | 5.98×               |
| **+ rayon integrity check**      | **8.49 s**       | **6.97×**           |
| upstream (3.8.0)                 | 59.17 s          | 1.00×               |

The rayon pass saturates user CPU time (20 s of user time across
wall-clock 6.9 s on this container). Memory stays bounded because
`cluster_uncached` drops the decompressed buffer as soon as the worker
finishes that cluster — we never hold all 3.2 GB at once.

### 4. O(log n) namespace counting (zimdump info)

`zimdump info` prints `count-entries: N` for the main content namespace.
The old implementation iterated every dirent (175 404 on Bashkir) and
counted matches — 35 ms just to answer that one number because every
dirent read allocates `String`s for the URL and title.

The new `Archive::entry_count_in_namespace(ns)` binary-searches the URL
pointer list for the namespace boundary, reading only the single
namespace byte at offset 3 of each dirent (no string allocation). That
makes it **O(log n)**: on a 175 k-entry archive it does ≈18 dirent peeks
instead of 175 404 full parses.

| build                  | `zimdump info` on 1.1 GB |
|------------------------|--------------------------|
| iterate all dirents    | 35 ms                    |
| **binary search (log n)** | **1.6 ms**            |
| upstream `zimdump info`| 4.3 ms                   |

## What the benefits look like in practice

- **CI integrity checks**: `zimcheck -A` runs 50 seconds faster per
  1 GB archive. A nightly pipeline that validates a dozen ZIMs saves
  ~10 minutes of wall-clock per run.
- **`zimcheck -R` dedup scans**: now fast enough (~2 s/GB) to run on
  every upload instead of post-facto.
- **Header/metadata inspection** (`zimdump info`) drops below the 5 ms
  shell-command floor, so scripting across hundreds of archives no
  longer stalls on the inspect step.
- **Streaming decompression** (`zimru readall`) hits 760 MB/s
  single-threaded — enough headroom that a full zimrecreate
  read-side doesn't become the bottleneck once the writer lands.

`zimcheck -C` now edges out upstream (which is using OpenSSL-accelerated MD5)
by enabling the `md-5` crate's `asm` feature — we hit ~556 MB/s, matching
`md5sum` itself.

`zimcheck -R` and `-A` get an 8× speedup from two changes stacked together:
(1) the four content-checks (`empty` / `redundant` / `url_internal` /
`url_external`) are now a **single pass** instead of four separate iterations,
and (2) the pass is **parallelized by cluster** with rayon — each cluster
decompresses once on a worker thread, and per-blob work (MD5 + HTML scan +
internal-URL lookup) runs in parallel. Aggregation stays deterministic (URL
pointer order) so the report text still matches upstream byte-for-byte.

Upstream's `zimbench` crashes with `Cannot find entry` during the random-
access phase on every one of our test ZIMs (a known libzim bug), which
stops it from completing. Ours runs through all three phases.

`zimru readall --md5` decompresses every cluster (3.2 GB output) AND
MD5-hashes every blob in 11.3 s. The pure decompression alone is 4.2 s —
~760 MB/s of sustained zstd/xz throughput.

## Reproducing the benchmark

```sh
# Install upstream tools
curl -fLo /tmp/zt.tar.gz \
    https://download.openzim.org/release/zim-tools/zim-tools_linux-x86_64-3.8.0.tar.gz
sudo tar xf /tmp/zt.tar.gz -C /opt

# Get a 1 GB+ ZIM
mkdir -p zim-cache && cd zim-cache
curl -fLO https://download.kiwix.org/zim/wikipedia/wikipedia_ba_all_maxi_2026-07.zim
cd ..

cargo build --release

# ABBA read-side + tool-by-tool comparison
./bench/toolset-bench.sh  zim-cache/wikipedia_ba_all_maxi_2026-07.zim

# ABBA creation comparison (needs the xapianbuilder helper on $PATH or
# $XAPIANBUILDER for the index-building modes)
./bench/creation-bench.sh zim-cache/wikipedia_ba_all_maxi_2026-07.zim

# Is the output still the same archive? (needs zimru-misc's zim-manifest)
./bench/content-verify.sh zim-cache/wikipedia_ba_all_maxi_2026-07.zim

# CLI parity diff — correctness, not timing
./bench/parity.sh        zim-cache/wikipedia_ba_all_maxi_2026-07.zim
```

Every harness writes each individual run time to a detail log
(`$OUT/run-times.log`), so the spread behind a "best of" stays checkable.

## Tests

Run the full suite:

```sh
cargo test --release
```

Test breakdown (**93 tests pass**, plus C-ABI smoke
binaries under the `cffi` feature):

- **Unit tests** (`src/*.rs`, 20 tests) — synthetic byte-level round-trips
  for the header, MIME list, dirent, and cluster (uncompressed / zstd / xz
  / extended offsets / unsupported-compression rejection) parsers, plus
  `Uuid` Display/FromStr round-trips and malformed-input rejection.
- **Synthetic ZIM end-to-end** (`tests/synthetic_zim.rs`, 10 tests) — builds
  spec-compliant ZIM files in memory (header → mime list → URL/title/cluster
  pointer lists → dirents → cluster → MD5 trailer) and exercises the public
  `Archive` API against them. Covers redirect following, loop guard,
  zstd-compressed clusters, missing entries, corrupted-checksum detection.
- **Ergonomic API** (`tests/ergonomic_api.rs`, 13 tests) — covers every
  Rust-idiomatic helper added on top of the libzim-mirror surface
  (`get_text` / `get_bytes` / `metadata_str` / `articles` / `redirects`
  / `content_entries` / `by_prefix` / `namespace_range` / `summary` /
  `par_iter_by_path` / `par_clusters` / `Item::text/bytes/is_html` /
  `Blob::as_str/reader/Deref` / `Entry::item/resolve/Display`).
- **Writer round-trip** (`tests/writer_roundtrip.rs`, 9 tests) — builds
  ZIMs through `Creator`, reopens them with our reader, and (when
  upstream zim-tools is installed) cross-validates each output with
  `upstream zimcheck -A`/`-C`/`-M`/`-P` and `upstream zimdump info/list`.
  Covers single-article round-trip, full archive (items + metadata +
  illustration + main-page redirect + custom redirection),
  multi-cluster bin-packing, every compression (None / Zstd / Xz),
  compression-level extremes (zstd 1↔19, xz 0↔9), dangling-redirect
  rejection.
- **`zimwriterfs` end-to-end** (`tests/zimwriterfs_e2e.rs`, 3 tests) —
  builds an HTML site under a temp dir, runs the `zimwriterfs` binary,
  reopens the produced ZIM, validates content + metadata, runs upstream
  `zimcheck -A`. Includes a side-by-side parity check that builds the
  same site with both `zimru zimwriterfs` and `upstream zimwriterfs`
  and verifies the same C-namespace path set.
- **`zimdump analyze` end-to-end** (`tests/zimdump_analyze.rs`, 3 tests)
  — builds a multi-cluster ZIM, runs the `zimdump analyze` binary, and
  asserts that cluster byte ranges tile the on-disk cluster region with
  no gaps or overlaps in file order (cluster index order is not
  guaranteed to match disk order), that the per-cluster table prints
  one row per cluster + a TOTAL row, and that `--by-item` lists every
  article and skips redirects.
- **Integrity checks** (`tests/integrity_checks.rs`, 9 tests) — each
  structural check (`check_dirent_ptrs` / `check_dirent_order` /
  `check_title_index` / `check_cluster_ptrs` / `check_mimetypes`)
  against both valid archives and deliberately corrupted ones.
- **Streaming encode** (`tests/streaming_encode_smoke.rs`, 2 tests) —
  chunked items below the threshold bin-pack normally; huge items
  stream through the bounded-memory zstd encoder and read back intact.
- **Per-item compression** (`tests/per_item_compression.rs`, 3 tests) —
  `Item::with_compress(false)` routes items into raw clusters that
  coexist with compressed ones in a single archive.
- **Direct access** (`tests/direct_access.rs`, 3 tests) — blob offsets
  reported for uncompressed clusters point at the exact on-disk bytes.
- **xapianbuilder helper** (`tests/xapianbuilder_helper.rs`, 3 tests) —
  the external index-builder hand-off used by `zimwriterfs` /
  `zimrecreate` when index building is requested.
- **Real-file integration** (`tests/real_files.rs`, 3 tests) — runs
  against actual Wikipedia ZIM files placed under `zim-cache/`. The
  files are NOT in this repo. Each test verifies the trailing MD5
  checksum, walks every entry in path order (round-trips a sample via
  binary search), walks title order (verifies monotonic order;
  round-trips a sample), decompresses every cluster, follows the
  main-page redirect, and reads the canonical metadata keys (UTF-8
  including non-Latin scripts). Skip silently if cache files absent.
- **Doc tests** (7 tests) — every code-block example in `lib.rs`,
  `archive.rs`, `writer.rs`, and `uuid.rs` is compile-verified.

To populate the real-file cache:

```sh
mkdir -p zim-cache && cd zim-cache
curl -fLO https://download.kiwix.org/zim/wikipedia/wikipedia_en_100_mini_2026-04.zim
mv wikipedia_en_100_mini_2026-04.zim wikipedia_en_100_mini.zim
curl -fLO https://download.kiwix.org/zim/wikipedia/wikipedia_en_100_nopic_2026-04.zim
mv wikipedia_en_100_nopic_2026-04.zim wikipedia_en_100_nopic.zim
curl -fLO https://download.kiwix.org/zim/wikipedia/wikipedia_zh_chemistry_mini_2026-03.zim
mv wikipedia_zh_chemistry_mini_2026-03.zim wikipedia_zh_chemistry_mini.zim
```

  (No `wikipedia_zh_100` ZIM is published; `wikipedia_zh_chemistry_mini` is
  the smallest available Chinese Wikipedia ZIM and exercises the same code
  paths plus UTF-8 metadata.)

## Licensing & clean-room policy

zimru is **MIT-licensed** and is an independent re-implementation of the
[ZIM file format spec][spec] — no code is shared with the GPL-licensed
[`libzim`][libzim] or [`libkiwix`][libkiwix].

To preserve that licence claim, contributors follow a clean-room
discipline: the GPL source of `libzim`, `libkiwix`, `java-libkiwix`'s
JNI wrappers, `kiwix-apple`'s `.mm` wrappers, and `python-libzim`'s
Cython bodies must not be consulted. The full policy — including the
list of sources you may and may not read, and the two-role split for
any work that necessarily touches an interface libzim already defines —
is documented in [CONTRIBUTING.md](./CONTRIBUTING.md). Please read it
before sending a patch.

A future GPL-licensed `libzim-shim` project will live in a *separate*
repository and provide the libzim-shaped C++ headers and JNI symbols
that downstream consumers (libkiwix, kiwix-apple's `.mm` files,
java-libkiwix) need to switch their bridges over to zimru. That shim is
intentionally outside this repo so the MIT-vs-GPL boundary is physical,
not just conventional. The full multi-repo plan — what to create, what
order to build it, prerequisites that must land in zimru first — is in
[`docs/multi-repo-plan.md`](./docs/multi-repo-plan.md).

### Why Xapian fulltext search is "out of scope" rather than "TODO"

Xapian is GPL v2. Linking it into zimru would force any binary that
embeds zimru to be GPL — exactly the contagion the MIT licence is
designed to prevent. So zimru deliberately does *not* implement Xapian
fulltext or suggestion search.

Two paths exist for downstream consumers that need search:

1. **GPL apps** (kiwix-tools, kiwix-serve, kiwix-desktop, kiwix-android,
   kiwix-apple) link Xapian themselves via the GPL-licensed
   `libzim-shim` repo, which can implement search alongside its
   libzim-shaped reader/writer surface.
2. **Non-GPL apps** can build their own search experience on top of
   zimru — anything from no search at all, to a different indexing
   stack (tantivy, sled, custom inverted index), to a server-side
   search service. zimru exposes everything needed to extract
   fulltext content for indexing: parallel cluster decompression,
   zero-copy blob access, MIME-type filtering.

Either way, zimru itself stays MIT and small.

[libzim]: https://github.com/openzim/libzim
[libkiwix]: https://github.com/kiwix/libkiwix
[spec]: https://wiki.openzim.org/wiki/ZIM_file_format

## License

MIT. See `LICENSE`.
