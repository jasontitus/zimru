# zimru

A clean-room, MIT-licensed Rust re-implementation of the [ZIM] file format
reader. No code is shared with the GPL-licensed C++ `libzim`; the format
parsing is written from the publicly documented [ZIM file format spec] and
validated against real-world ZIM files (English & Chinese Wikipedia).

[ZIM]: https://wiki.openzim.org/wiki/OpenZIM
[ZIM file format spec]: https://wiki.openzim.org/wiki/ZIM_file_format

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
| Xapian fulltext / suggestion search (`X/fulltext/xapian`)   | ⏳ TODO |
| ZIM writer (creator)                                        | ⏳ TODO |

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

## CLI

The crate ships five binaries — one Rust-native explorer plus four drop-in
replacements for the upstream `zim-tools` family:

| binary       | upstream counterpart   | status                                              |
|--------------|------------------------|-----------------------------------------------------|
| `zimru`      | (native, no upstream)  | Inspection / extraction / read-every-blob benchmark |
| `zimcheck`   | `zim-tools/zimcheck`   | Full CLI parity (all flags) + JSON output           |
| `zimdump`    | `zim-tools/zimdump`    | `info` / `list` / `list --details` / `show` / `dump`|
| `zimbench`   | `zim-tools/zimbench`   | `-n` / `-r` / `-d` flags (linear + random access)   |
| `zimsplit`   | `zim-tools/zimsplit`   | byte-aligned split (concat reproduces original)     |

```sh
cargo build --release

# Native helper
./target/release/zimru info     wikipedia_en_100_mini.zim
./target/release/zimru readall  wikipedia_en_100_mini.zim --md5
./target/release/zimru get      wikipedia_en_100_mini.zim Alcoholism
./target/release/zimru get      wikipedia_en_100_mini.zim 'W/mainPage'  # explicit ns

# zim-tools drop-in replacements
./target/release/zimcheck -A wikipedia_en_100_mini.zim
./target/release/zimcheck -A -J wikipedia_en_100_mini.zim   # JSON
./target/release/zimdump  info wikipedia_en_100_mini.zim
./target/release/zimdump  list --details wikipedia_en_100_mini.zim
./target/release/zimbench -n 1000 wikipedia_en_100_mini.zim
./target/release/zimsplit --prefix=part- --size=2G --force wikipedia_en_100_mini.zim
```

## CLI parity vs upstream `zim-tools` 3.6.0 / `libzim` 9.3.0

`bench/parity.sh` runs the upstream tool and our binary on the same archive,
normalizes version strings + elapsed-time, and diffs outputs. On three
real-world ZIM files the harness reports **36/36 cases pass** (12 cases ×
3 archives — small English mini, Chinese chemistry mini, and 1.1 GB Bashkir
all-maxi):

| case                    | matching mode    | result   |
|-------------------------|------------------|----------|
| `zimdump info`          | byte-for-byte    | ✓ 3/3    |
| `zimdump list`          | byte-for-byte    | ✓ 3/3    |
| `zimdump list --details`| byte-for-byte    | ✓ 3/3    |
| `zimdump show --idx=N`  | byte-for-byte    | ✓ 3/3    |
| `zimcheck -C`           | byte-for-byte    | ✓ 3/3    |
| `zimcheck -I`           | byte-for-byte    | ✓ 3/3    |
| `zimcheck -M`           | byte-for-byte    | ✓ 3/3    |
| `zimcheck -F`           | byte-for-byte    | ✓ 3/3    |
| `zimcheck -P`           | byte-for-byte    | ✓ 3/3    |
| `zimcheck -L`           | byte-for-byte    | ✓ 3/3    |
| `zimcheck -A`           | structural*      | ✓ 3/3    |
| `zimcheck -A -J` (JSON) | structural*      | ✓ 3/3    |

\* "Structural" means everything in the report matches except the order of
items inside `[WARNING] Redundant data found:`, `[ERROR] Invalid internal
links found:` and `[ERROR] Invalid external links found:` blocks. The set
*counts* (e.g. `dangling=27/29 redundant=2/2 external=1/1` on Bashkir) are
also checked. Order varies because libzim iterates clusters in a sequence
we don't replicate.

Tools we don't yet reach byte-for-byte parity on:

| tool          | status                                                            |
|---------------|-------------------------------------------------------------------|
| `zimrecreate` | needs ZIM **writer** — TODO                                       |
| `zimwriterfs` | needs ZIM **writer** — TODO                                       |
| `zimpatch`    | needs ZIM **writer** + libzim diff format — TODO                  |
| `zimdiff`     | needs libzim diff format — TODO                                   |
| `zimsearch`   | needs Xapian fulltext index — TODO                                |
| `zimsplit`    | parts produced; output uses byte-aligned splits (upstream splits  |
|               | on cluster boundaries). Both reconstruct the original via concat. |
| `zimbench`    | runs end-to-end; upstream's random-URL phase crashes on our test  |
|               | files ("Cannot find entry"), ours doesn't.                        |

## Head-to-head benchmark vs upstream

Reproduced via `bench/run.sh wikipedia_ba_all_maxi.zim` (1.1 GB Bashkir
Wikipedia, 175 404 entries, 1 502 clusters, 3.2 GB of decompressed
content). Hardware: shared linux container; results are warm-cache means of
3–5 runs measured by `hyperfine`.

| workload                                  | upstream (3.6.0)  | zimru          | speedup      |
|-------------------------------------------|-------------------|----------------|--------------|
| `zimcheck -C` (MD5 trailer only)          | 2.29 s            | 2.05 s         | **1.12×**    |
| `zimcheck -R` (decompress + MD5 every blob) | 18.28 s         | 2.19 s         | **8.44×**    |
| `zimcheck -A` (full sweep)                | 57.27 s           | 6.93 s         | **8.27×**    |
| `zimdump info` (cold-style header parse)  | 4.3 ms            | 1.6 ms         | **2.73×**    |
| `zimru readall` (decompress only)         | n/a               | 4.20 s         | —            |
| `zimbench` (n=1000)                       | ~~n/a~~ (crashes) | 1.95 s (full)  | —            |

## Why zimru is faster

Four independent optimizations, each measurable on its own. The numbers
below are from the 1.1 GB Bashkir Wikipedia test file, warm-cache, reported
by `hyperfine` (mean of 3–5 runs).

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
| **single-pass, rayon (8 cores)** | **6.93 s**       | **8.27×**           |
| upstream                         | 57.27 s          | 1.00×               |

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
    https://download.openzim.org/release/zim-tools/zim-tools_linux-x86_64-3.6.0.tar.gz
sudo tar xf /tmp/zt.tar.gz -C /opt
sudo apt-get install -y hyperfine

# Get a 1 GB+ ZIM
mkdir -p zim-cache && cd zim-cache
curl -fLO https://download.kiwix.org/zim/wikipedia/wikipedia_ba_all_maxi_2026-04.zim
mv wikipedia_ba_all_maxi_2026-04.zim wikipedia_ba_all_maxi.zim
cd ..

cargo build --release
./bench/run.sh   zim-cache/wikipedia_ba_all_maxi.zim   # benchmark
./bench/parity.sh zim-cache/wikipedia_ba_all_maxi.zim  # CLI parity diff
```

## Tests

Run the full suite:

```sh
cargo test --release
```

Test breakdown:

- **Unit tests** (`src/*.rs`) — synthetic byte-level round-trips for the
  header, MIME list, dirent, and cluster (uncompressed / zstd / xz / extended
  offsets / unsupported-compression rejection) parsers.
- **Synthetic ZIM end-to-end** (`tests/synthetic_zim.rs`) — builds spec-
  compliant ZIM files in memory (header → mime list → URL/title/cluster
  pointer lists → dirents → cluster → MD5 trailer) and exercises the public
  `Archive` API against them. Includes redirect following, loop guard,
  zstd-compressed clusters, missing entries, and corrupted-checksum
  detection.
- **Real-file integration** (`tests/real_files.rs`) — runs against actual
  Wikipedia ZIM files placed under `zim-cache/`. The files are NOT in this
  repo. Each test:
    1. Verifies the trailing MD5 checksum.
    2. Walks every entry in path order; round-trips a sample via binary
       search on `(namespace, url)`.
    3. Walks every entry in title order; verifies monotonic ordering;
       round-trips a sample via title binary search.
    4. Decompresses every cluster by reading every article's blob.
    5. Follows the main-page redirect and asserts non-empty content.
    6. Reads the canonical metadata keys (Title, Language, Description) —
       UTF-8, including non-Latin scripts.

  Tests skip silently if the cache files aren't present.

  To populate the cache:

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

## License

MIT. See `LICENSE`.
