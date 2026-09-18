# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security
- `zimdump dump`: entry paths from the archive are sanitized (`..` and `.`
  segments dropped), closing a path traversal that let a crafted ZIM write
  files outside `--dir`.
- `cluster`: decompressed cluster size is now capped (4 GiB standard /
  16 GiB extended), so a crafted xz/zstd cluster can no longer grow the
  decode buffer without bound or force a multi-GB allocation from a lying
  zstd frame header (decompression bomb).
- CI: third-party actions pinned to full commit SHAs, workflow token
  restricted with `permissions: contents: read`, Dependabot enabled for
  `github-actions`.

### Added
- `writer`: `Creator::set_streaming_encode_threshold` to override the
  256 MiB chunked-item streaming-encode cutoff (used by tests and
  memory-constrained builds).
- `zimwriterfs`: `-x/--inflateHtml` is now implemented (gzipped `.html`
  inputs are inflated via `flate2` before packing) instead of silently
  doing nothing.
- CI: the `cffi` job now also runs `cffi_writer_smoke`, covering the
  `zimru_creator_*` C ABI.
- `writer`: continuous encode pipeline that overlaps compression with item
  production, plus preservation/selection of raw (uncompressed) clusters for
  incompressible content.
- `zimwriterfs`: content-sniffing of MIME types for extensionless files.
- `examples/writer_bench.rs`: standalone writer benchmark harness.
- Benchmark docs: high-compression results (1.49× faster than libzim at
  zstd 19) and a real-archive round-trip benchmark vs zim-tools on the
  1.1 GB Bashkir Wikipedia archive (`docs/bashkir-roundtrip-bench.md`).
- `reader`: `Archive::validate_cluster_references(cluster, &[blob])` checks
  blob references against a cluster's offset table with a single decode
  (uncompressed clusters are validated in place); `zimcheck -I` uses it to
  verify every article's cluster/blob reference.
- `zimru readall --content-md5`: length-framed digest of user entries
  (path, title, MIME, body, redirect target) that excludes regenerated
  `M`/`X`/`W` entries; `bench/recreate-suite.sh` compares this instead of
  concatenated blob bytes.
- Bench harnesses: `bench/verify-common.sh` parses `zimcheck -A` reports
  fail-closed; `ZIMRU_BUILD=0` skips the release build in
  `recreate-suite.sh` / `parity.sh` / `recreate-bench.sh`;
  `tests/benchmark_harnesses.rs` fault-injects stub upstream tools.

### Changed
- `zimrecreate`: reads source content grouped by cluster.
- `zimdump dump`: deterministic shallow-wins collision policy.
- `README` and inline code docs refreshed to match the current writer/tooling.
- `zimsplit`: parts are always cluster aligned; `--size` is a hard maximum
  and the split is planned (and rejected) before any output is created.
  `--force` only permits an indivisible region larger than `--size`; it
  never byte-splits a cluster or overwrites an existing file. Only
  `aa`..`zz` (676) parts are emitted; the prefix gains `.zim` if missing.
  The suffix unit tests were replaced by CLI regressions
  (`tests/archive_cli_preservation.rs`); `zimsplit` no longer builds a
  test harness.
- `zimrecreate`: writes into an exclusively created temp directory beside
  the destination and renames on success; the source and any existing
  destination survive every failure. On non-Unix platforms an existing
  destination is refused (no portable file identity to prove it is not
  the source).
- `zimdump dump`: extraction is descriptor-relative (`openat` +
  `O_NOFOLLOW`, exclusive temp file, `renameat`) on Unix; a pre-existing
  symlink in the output tree can no longer redirect writes. On non-Unix
  platforms `dump` reports `Unsupported`.
- `zimcheck -I`: validates every dirent, the pointer/title/mimetype
  tables, main-page/layout/redirect references (including cycles) and
  every article's cluster/blob reference; content read failures are
  reported (text and JSON) instead of skipped.
- `zimwriterfs`: the output path must lie outside the source tree
  (symlink aliases are resolved); the archive is staged beside the output
  and renamed on success, so an output that hard-links a source file no
  longer truncates the source.
- `reader`: title iteration on archives with a v1 title listing yields the
  front-article subset even when full header/v0 tables coexist; ordinary
  title lookup still finds excluded entries.
- `writer`: item size hints from the C ABI (`zimru_creator_*_size_hint`)
  are nonbinding; the Rust `Some(size)` API is exact and rejects short or
  overrun bodies recoverably.

### Changed (performance)
- `Archive::blob_direct_access` (and the C ABI's `zimru_item_direct_access`
  / `zimru_item_warmup`) answers from the on-disk offset table instead of
  decoding and caching the whole cluster — warming a multi-GB uncompressed
  Xapian-index cluster no longer copies it into RAM.
- `zimrecreate`: pass 1 keeps a compact `(cluster, blob, url_index)` per
  entry instead of three owned Strings, bounding peak RSS on multi-million-
  entry archives; the built Xapian indexes are streamed into the writer in
  bounded chunks instead of being slurped whole (also in `zimwriterfs`).
- `zimdump analyze --by-item`: per-blob sizes are retained from the first
  cluster pass, eliminating per-blob cluster re-decodes; `--idx` lookups
  use the O(log n) namespace range instead of a linear scan.
- Reader/tooling allocation churn: dirent parsing no longer allocates a
  second url copy for empty-title entries; `check_dirent_order` /
  `check_title_index` borrow keys from the mmap; `zimcheck`'s content scan
  stops cloning dirents and mimetype strings; `zimwriterfs` builds its
  mime table once and finds `<title>` without lowercasing whole bodies;
  the writer's non-`Single` bucket strategies reuse a scratch key buffer;
  cffi metadata key enumeration is cached (was O(k·n) full-archive scans)
  and the metadata count is O(log n).

### Fixed
- `zimdump` / `zimwriterfs`: dump collisions and symlink round-trip; `dump` is
  now cluster-parallel.
- `writer`: dropped redundant finalize I/O and corrected temp-file placement.
- `zimrecreate`: legacy (v5 namespace) archives are namespace-normalized so
  their content is recreated instead of silently dropped; metadata keeps its
  original mimetype (PNG favicons were being re-labelled text/plain);
  a redirect whose target lies outside the content namespace fails the
  build with an explicit error instead of being silently rewritten to a
  dangling `C/` path.
- `writer`: the auto-generated `M/Counter` now counts still-pending bucket
  entries (it was empty on small archives and undercounted every build's
  tail) and counts only `C`-namespace articles, matching libzim; a
  caller-supplied `M/Counter` suppresses the auto-generated one even while
  it sits in an un-flushed bucket (previously the output could carry two
  `(M, Counter)` dirents); `finish_writing` refuses to finalize with a
  chunked item still in flight; interning more than 65,532 distinct
  mimetypes errors explicitly instead of silently wrapping.
- `zimcheck -I`: cluster validation actually decodes every cluster (the
  old `touch_cluster` was a no-op, letting corrupt payloads Pass on
  checksum-less archives).
- C ABI: `zimru_archive_metadata` returns binary metadata verbatim
  (interior NUL bytes were replaced with spaces while the reported length
  kept lying); the per-archive intern store no longer grows unboundedly on
  long-lived handles.
- `zimru`/`_index_helper` subprocess handling: child stderr is drained
  concurrently (deadlock), writes stop when the child exits early, and
  children + temp dirs are cleaned up on early error returns.
- `zimbench`: `-d 0` no longer panics with a divide-by-zero; random URLs
  are now sampled distinctly (partial Fisher–Yates) as documented.
- `zimsplit`: overflowing `--size` suffixes are rejected instead of
  silently wrapping.
- `raw`: offset arithmetic in the binary readers uses `checked_add`, so a
  crafted field near `usize::MAX` returns `Error::Truncated` instead of a
  debug-build panic.
- Tests: the streaming-encode smoke test now actually exercises the
  streaming path (it silently took the buffered path before); cffi smoke
  tests resolve the dylib for the current build profile instead of
  hard-coding `target/release`; bench harnesses abort when the build
  fails instead of misreporting DIFF/FAIL rows.
- `reader`: ZIM 6.0 archives are treated as legacy (`-/`, `A/`, `I/`
  namespaces) — the new-namespace boundary is 6.1, not 6.0; qualified
  legacy paths resolve; `checksum()` on a header claiming `u64::MAX`
  returns a clean error instead of overflowing; pointer-table bounds are
  checked before allocation; cluster offsets must be aligned, monotonic,
  and inside the payload; oversized zstd size declarations stream instead
  of pre-allocating.
- `reader`: the structural title check validates every present table
  (header, v0, v1) independently for sortedness, uniqueness, bounds and
  v1 `C`-namespace membership.
- `writer`: uncompressed article title listings (`X/listing/titleOrdered/v1`)
  carry the final URL indices and are always stored raw, as are all `X`
  items; directory/MIME strings with control characters or embedded NULs
  are rejected; a recoverable error before finalization starts (e.g. C ABI
  `finish_writing` before `start_writing`) leaves the buffered work
  intact; duplicate metadata is a recoverable
  error; the encode queue is bounded by actual retained bytes (Mutex +
  Condvar byte budget), not item count.
- `zimrecreate`: legacy 5.0/6.0 archives keep namespace-qualified content
  paths (`C/A/…`, `C/I/…`, `C/-/…`) so in-body links stay valid; main page,
  redirect targets and colliding paths map consistently.
- `zimdump dump --redirect`: HTML redirects compute depth from the actual
  fallback destination, percent-encode the URL and escape HTML.
- `zimwriterfs`: symlink chains are resolved once; dangling, cyclic,
  outside-tree and skipped-target chains are skipped consistently; MIME
  sniffing precedes HTML-vs-streaming routing, so large extensionless HTML
  is title-extracted and indexed while media stays chunked even under
  `--inflateHtml`; temp dirs are created exclusively and never remove
  pre-existing paths.
- Bench harnesses: `recreate-suite.sh` no longer reports MATCH on two
  empty `--quiet` digests; `content-verify.sh` rejects failed or truncated
  manifests (row count is cross-checked against `zimdump info`) and
  handles an empty archive explicitly; `parity.sh` compares exit status
  and normalized stderr as well as stdout, fails on missing executables
  and crashes (exit > 1), and no longer discards redundant-data findings;
  `creation-bench.sh` / `recreate-bench.sh` / `toolset-bench.sh` distinguish
  checker crashes and incomplete reports from findings and exit non-zero.
- Tests: cffi, zimdump analyze, zimwriterfs and CLI preservation suites use
  the Cargo-built binaries/dylib (`CARGO_BIN_EXE_*` / deps dir) and fail
  instead of silently skipping when a compiler or artifact is missing.
- C ABI: the Rust error enum is `zimru_error_code_t` (C spelling
  `zimru_error_code_t_<Variant>`); `zimru_error_code()` is unchanged.
- `zimwriterfs`: `<title>` text spanning multiple lines is whitespace-
  normalized instead of aborting the build as a control character.
- `zimrecreate`: only `Illustration_NxN@1` maps onto `add_illustration`;
  other scales (`@2`, …) are copied as metadata under their own key
  instead of colliding with the `@1` key. Legacy `Z/` (old Xapian index)
  is no longer re-packed as content; `zimru readall --content-md5` applies
  the same rule.
- `zimwriterfs` / `zimrecreate`: writer validation failures (control
  characters, invalid MIME, duplicate keys) are reported as clean errors
  (exit 1) via the `try_*` API instead of panics.
- `reader`: a title lookup miss against a full-table listing returns
  immediately instead of scanning every dirent.
- `writer`: duplicate-key detection keeps a 16-byte digest per entry
  rather than a second copy of every path.
- `cluster`: the xz decoder's memory limit is tied to the cluster cap, so
  a tiny cluster declaring a multi-GiB LZMA dictionary is rejected before
  allocation.
- `zimdump dump`: a `--dir` that is itself a symlink (e.g. macOS `/tmp`)
  is canonicalized once and accepted; archive-controlled components below
  it remain `O_NOFOLLOW`.
- Bench harnesses: commands are argv vectors / `printf %q`-escaped, and
  scratch directories default to `mktemp -d`.
- `zimdump dump`: without `--ns`, modern archives dump only the `C`
  namespace (as `list`/`info` already did and as upstream 3.8.0 does), so
  `M/Title` no longer flattens onto `C/Title`; legacy archives still dump
  every namespace. `dump_errors.log` is written only when an entry failed.
  Verified on Linux against official zim-tools 3.8.0: dump trees identical
  on all three fixtures.
- `writer`: the auto-generated `M/Counter` folds MIME parameters onto the
  bare media type (`text/html; charset=iso-8859-1` counts as `text/html`),
  matching libzim and zimcheck's Counter grammar; previously such an item
  produced an invalid Counter entry on recreate.
- Known difference: `zimcheck -A` reports every dangling internal link;
  upstream 3.8.0 omits some on certain archives (see README tool table).

[Unreleased]: https://github.com/jasontitus/zimru/compare/main...HEAD
