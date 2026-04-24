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

```sh
cargo build --release
./target/release/zimru info   wikipedia_en_100_mini.zim
./target/release/zimru check  wikipedia_en_100_mini.zim
./target/release/zimru list   wikipedia_en_100_mini.zim --limit 20
./target/release/zimru titles wikipedia_en_100_mini.zim --limit 20
./target/release/zimru meta   wikipedia_en_100_mini.zim Title
./target/release/zimru get    wikipedia_en_100_mini.zim Alcoholism
./target/release/zimru get    wikipedia_en_100_mini.zim 'W/mainPage'  # explicit ns
./target/release/zimru mimes  wikipedia_en_100_mini.zim
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
