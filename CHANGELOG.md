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

### Changed
- `zimrecreate`: reads source content grouped by cluster.
- `zimdump dump`: deterministic shallow-wins collision policy.
- `README` and inline code docs refreshed to match the current writer/tooling.

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
  redirects whose target is outside the content namespace are skipped with
  a warning instead of failing the build.
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

[Unreleased]: https://github.com/jasontitus/zimru/compare/main...HEAD
