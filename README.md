# zimru

An independent, MIT-licensed Rust reader and writer for the [ZIM file
format](https://wiki.openzim.org/wiki/ZIM_file_format), used for offline
websites and reference collections.

zimru provides a Rust library, an optional C API, and seven command-line
tools. Its API vocabulary and CLI conventions are familiar to libzim and
zim-tools users, but **it is not a binary-compatible or fully equivalent
replacement for either project**. See the compatibility matrix below before
switching an existing workflow.

## Build

Install a current stable Rust toolchain, a C/C++ toolchain, and the XZ/liblzma
development package. On Debian/Ubuntu, install `build-essential pkg-config
liblzma-dev`; on macOS, install the Xcode command-line tools and `xz`.

```sh
cargo build --release
cargo test
```

The default `writer` feature includes the reader, `Creator`, `zimrecreate`,
and `zimwriterfs`. A reader-only build omits the writer and those two tools:

```sh
cargo build --release --no-default-features
```

Build the optional C API and generated header with:

```sh
cargo build --release --features cffi
```

This produces `include/zimru.h` and the platform's `libzimru` shared/static
libraries under `target/release`. `cffi` implies `writer`. The C ABI is
zimru's own API: linking an application written for libzim's C++ ABI requires
a separate adapter. The crate is pre-1.0; Rust and C API stability is not yet
a long-term compatibility guarantee.

## Compatibility and current limitations

This section is the authoritative current scope. Historical review reports
and benchmark notes are not a current support matrix.

| Area | Supported behavior | Limits / differences |
|---|---|---|
| ZIM versions | Legacy 5.0 and 6.0 namespaces; modern 6.1+ namespaces | Major versions other than 5 and 6 are rejected. Recognizing a future minor version does not imply support for every future feature. |
| Cluster compression | Uncompressed (0/1), XZ (4), Zstd (5); standard and extended offsets | Deprecated zlib/bzip2 clusters are rejected. Decompression has explicit size limits. |
| Reader | Path/title lookup, metadata, redirects, cluster access, checksum and structural checks | Input files are memory-mapped: do not modify or truncate them while open. |
| Title listings | Modern article-only v1 listing, with legacy listing fallback | Article title iteration is not an enumeration of all resources; use path iteration for all entries. |
| Writer | ZIM 6.3 output, metadata, illustrations, redirects, raw/XZ/Zstd clusters | Front articles are HTML entries and redirects resolving to them. Directory fields must be valid format strings; duplicate entry keys are rejected. |
| Multipart archives | `zimsplit` creates cluster-boundary-aligned parts | The reader does not transparently join `.zimaa`, `.zimab`, etc. Join them in suffix order before opening with zimru. |
| C API | Reader and writer handles, blobs, error codes, metadata, chunked input | Not libzim's C++ ABI. Follow the generated header's ownership and error contracts. |
| Search | Optional external `xapianbuilder` integration during CLI creation | No built-in Xapian query engine. A working external helper is required to produce search indexes. |
| Platforms | Linux and macOS are exercised by CI | Windows is not a tested support target. Filesystem-specific operations may be unavailable there. |
| CLI compatibility | Common zim-tools-style commands and options | Output text, error categories, exit codes, indexing, MIME detection and overwrite policies can differ. Test your actual invocations. |

The reader caps decompressed standard clusters at 4 GiB and extended
clusters at 16 GiB. These are format/implementation ceilings, not a promise
that a machine can allocate those amounts. The cluster-cache budget is
separate from decompression working memory. Large archives still require
memory for entry indexes, worker state and the largest active clusters.

### Tool inventory

| Binary | Purpose | Important distinction |
|---|---|---|
| `zimru` | Inspect, fetch content, verify checksums, read all blobs | Native interface; put the archive path before `readall` flags. |
| `zimcheck` | Structural, checksum, metadata and content checks | `-I` checks structure and references; `-C` checks MD5; neither establishes the correctness of HTML itself. |
| `zimdump` | `info`, `list`, `show`, `dump`, `analyze` | `analyze` is a zimru extension. Extraction rejects unsafe filesystem destinations; HTML redirects differ from symlink redirects. |
| `zimbench` | Sequential/random read workloads | Its workload and reports are not a general speed comparison with upstream. |
| `zimsplit` | Split a ZIM at cluster boundaries | Refuses existing output parts and source aliases. A cluster may exceed the requested part-size target. |
| `zimrecreate` | Re-encode a single-file archive | Regenerates metadata/listings and optionally indexes; not byte-identical output. Legacy namespace prefixes are preserved in content paths. |
| `zimwriterfs` | Pack an HTML directory | Uses extension/signature-based MIME detection, not libmagic. Output must not alias input files. Invalid symlink chains are skipped with diagnostics. |

Use each executable's `--help` for its accepted options. Avoid installing
these binaries over an existing zim-tools installation until your workflow
has been tested; explicit paths make comparisons unambiguous.

## Read an archive

```rust
use zimru::Archive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let archive = Archive::open("wiki.zim")?;
    let main = archive.main_entry()?.get_item(true)?;
    let blob = main.get_data()?;
    println!("{}: {} bytes ({})", main.path(), blob.len(), main.mimetype());
    println!("{}", blob.as_str()?);
    Ok(())
}
```

Modern content paths omit the namespace: `get_entry_by_path("home")`
addresses `C/home`. For legacy archives use namespace-qualified paths such
as `A/home` and `-/style.css`. To address any namespace explicitly, use
`entry_by_ns_path(b'M', "Title")` or the dedicated metadata methods.

Useful convenience APIs:

```rust
use zimru::Archive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let archive = Archive::open("wiki.zim")?;
    let html = archive.get_text("home")?; // follows redirects
    let image = archive.get_bytes("images/logo.png")?;
    let title = archive.metadata_str("Title")?;
    println!("{title}: {} HTML bytes, {} image bytes", html.len(), image.len());
    for result in archive.iter_by_path() {
        let entry = result?;
        println!("{}/{}", char::from(entry.namespace()), entry.path());
    }
    Ok(())
}
```

`articles()` excludes redirects; it does not mean only front-page HTML.
`content_entries()` selects content namespaces, while `iter_by_title()` uses
an archive's article listing when available. `by_prefix(namespace, prefix)`
and `namespace_range(namespace)` avoid a full path scan.

`Item::get_data()` returns a `Blob` backed by a shared cluster snapshot.
Borrow `blob.data()` or `blob.as_str()` to avoid copying the body. Keep the
blob alive for the lifetime of the borrow. `bytes()`/`to_vec()` deliberately
copy. A blob reader reads an already materialized cluster; it is not a
streaming decompressor.

For parallel workloads, `par_iter_by_path()` scans entries and
`par_clusters()` processes clusters. Propagate errors rather than silently
using `filter_map(Result::ok)` in validation code. The generated API docs
contain the complete method contracts:

```sh
cargo doc --all-features --no-deps --open
```

## Write an archive

```rust
use zimru::writer::{Creator, Item};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut creator = Creator::new();
    creator.set_main_path("home");
    creator.try_add_item(Item::html("home", "Home", "<h1>Welcome</h1>"))?;
    creator.try_add_metadata("Title", "Demo")?;
    creator.try_add_metadata("Language", "eng")?;
    creator.write_to("demo.zim")?;
    Ok(())
}
```

For larger builds, configure the creator, call `start_writing(path)`, add
items, and call `finish_writing()`. Prefer fallible `try_add_*` methods when
input or I/O can fail. Builder-style `add_*` methods cannot return errors and
may panic; they are not an error-handling substitute.

Compression defaults to Zstd. `set_compression` selects raw, Zstd or XZ;
`set_compression_level` selects the algorithm's level.
`Item::with_compress(false)` keeps already-compressed content in raw
clusters. Index/listing entries in `X` are stored uncompressed as required
by the [index specification](https://wiki.openzim.org/wiki/Search_indexes).
See [per-item compression](docs/per-item-compression.md) for details.

The cluster-size target is a packing target, not a maximum item size. The
queued raw-byte budget controls compression backpressure, not total process
RSS: entry metadata, compressed results and encoder state need additional
memory. Oversized items must still be processed without deadlocking.

`Creator::begin_chunked_item` with `Some(expected_size)` uses an **exact**
size contract. The C API's `size_hint` is instead **non-binding**; arbitrary
hints must not change which input lengths are accepted. Unknown-size input
may be buffered. Known large items can use the exact-size Rust path for
streaming encoding.

A successful finish writes tables and a checksum and performs structural
checks. This is not a guarantee that external HTML links, custom metadata,
or an external search index are semantically correct. The library writer
creates its requested destination directly; applications needing atomic
replacement should use their own temporary-output workflow.

### CLI examples

```sh
./target/release/zimru info wiki.zim
./target/release/zimru readall wiki.zim --md5
./target/release/zimdump show --url=home wiki.zim
./target/release/zimcheck -I wiki.zim
./target/release/zimcheck -A wiki.zim
./target/release/zimdump dump --dir=export wiki.zim

./target/release/zimrecreate source.zim recreated.zim \
    --compression zstd --cluster-size 2097152 --without-indexes

./target/release/zimwriterfs \
    --welcome=index.html --illustration=icon48.png \
    --language=eng --name=my-zim --title="My ZIM" \
    --description="Example archive" --creator="Example author" \
    --publisher="Example publisher" --without-indexes \
    ./html_dir out.zim
```

`zimrecreate` rejects source/destination aliases and publishes a completed
temporary output. Legacy `A/home` and `I/icon` become modern content paths
`A/home` and `I/icon`, rather than losing their namespace components. This
preserves ordinary relative links without guessing how to rewrite HTML,
CSS or JavaScript. Legacy auxiliary entries are preserved where possible;
regenerated indexes and well-known entries are not copied byte-for-byte.

## C and C++ integration

Include `include/zimru.h` and link with `libzimru`. Build the library with
`--features cffi` in the same profile used by your application/tests. See
`tests/cffi_smoke.c`, `tests/cffi_smoke.cpp` and
`tests/cffi_writer_smoke.c` for complete original consumers.

- Handles have matching `*_free` functions; archives use
  `zimru_archive_close`.
- Borrowed strings/bytes remain valid for the lifetime documented in the
  header; do not free borrowed pointers.
- Fallible calls accept a `zimru_error_t **` out-parameter. Initialize it to
  NULL, check the return value, and free returned errors. Successful calls
  leave the error slot unchanged.
- Binary metadata is returned verbatim with an explicit byte length;
  embedded zero bytes are valid.
- Calling `finish_writing` before `start_writing` is recoverable. A failure
  after finalization has actually begun can consume the creator; consult
  the specific function contract before retrying.

## Testing and independent verification

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test --no-default-features --lib --tests
cargo build --all-features
cargo test --all-features
cargo build --release --all-features
cargo test --release --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps
```

Native smoke tests require working C and C++ compilers and the matching
profile's C-enabled library. Missing prerequisites are failures, not proof
of a passing C ABI.

Real-archive tests use downloaded fixtures under gitignored `zim-cache/`.
A normal source checkout can run without those files, but a passing suite
with absent fixtures is **not** a real-archive interoperability result.
The CI fixture job downloads pinned archives and requires their tests to
run. Upstream comparisons use released binary tools as black-box references;
no upstream implementation or test sources are needed.

### Benchmark evidence

The former blanket speedup and "8/8 byte-perfect" claims have been withdrawn.
The old digest harness compared empty strings under `--quiet`, and a later
manifest harness could accept failed commands as empty results. Historical
numbers remain in clearly labelled notes, not as evidence for this revision.

The maintained harnesses are:

- `bench/parity.sh`: selected CLI outputs, exit statuses and diagnostics.
- `bench/recreate-suite.sh`: explicit round-trip verification.
- `bench/content-verify.sh`: user-content manifests from real libzim via the
  separately built `zim-manifest` helper in `zimru-misc`.
- `bench/creation-bench.sh` and `bench/toolset-bench.sh`: creation/read timing.

Consult each script's header for prerequisites and environment variables.
Run correctness checks before timing. Record archive checksums, revision,
upstream versions, compression levels, indexes built, thread counts, hardware
and individual runs. A user-content match does not mean identical metadata,
search results or output bytes. Missing helpers and failed comparisons must
exit unsuccessfully, never produce an equivalence verdict.

## Contribution policy and project history

zimru is MIT-licensed. Contributions must be original work from the public
format specification, permitted interface documentation and black-box
observations. Do not copy or translate upstream implementation/test code.
[CONTRIBUTING.md](CONTRIBUTING.md) defines the source-use policy and review
process. This policy describes the required contribution process; it is not
an independently verified assertion about every historical contributor's
reading history.

Search querying is outside this library's scope. External adapters/helpers
have their own licenses; using a separate process does not by itself settle
all licensing obligations for a downstream distribution.

Historical design plans, review-fix notes and benchmark notes under `docs/`
carry a "Historical note" banner and describe older revisions. For current
behavior use this README, the generated API/header contracts, and
[CHANGELOG.md](CHANGELOG.md). Historical reports do not constitute a list of
currently open bugs.

## License

MIT. See [LICENSE](LICENSE). Archive content has its own copyright and
licensing requirements; the library license does not grant rights to the
content being packaged.
