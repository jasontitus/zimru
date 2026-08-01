# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
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

### Fixed
- `zimdump` / `zimwriterfs`: dump collisions and symlink round-trip; `dump` is
  now cluster-parallel.
- `writer`: dropped redundant finalize I/O and corrected temp-file placement.
- Five hot-path inefficiencies in the creation path: per-file mime-table
  `HashMap` rebuild and whole-body lowercasing for `<title>` in
  `zimwriterfs`, per-cluster zstd context creation in the encode pipeline,
  per-entry `String` allocation in the post-write verify checks, and a
  64 KiB (now 1 MiB) big-file read buffer. Validated on a 4.8 GB build
  with an ABBA cache-controlled benchmark
  (`docs/perf-fixes-4gb-bench.md`): end-to-end wall time is
  compression-bound and statistically unchanged (−1.1 %, within noise).

[Unreleased]: https://github.com/jasontitus/zimru/compare/main...HEAD
