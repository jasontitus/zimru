# Fixes for the 2026-08-13 review sweeps

Disposition of every finding from `reviews/2026-08-13`
(`PI_REVIEW_BUGS_DEEPSEEK_V4_PRO_20260813.md`, 30 findings;
`PI_REVIEW_PERF_DEEPSEEK_V4_PRO_20260813.md`, 21 findings), following the
verified verdicts in `PI_REVIEW_VERDICTS_20260813.md` — the one refuted
finding is skipped, the two partials are handled per their corrected detail.

## Bug sweep (30 findings)

| # | Finding | Disposition |
|---|---------|-------------|
| [high] ci.yml:18 | unpinned third-party actions | **Fixed** — all actions pinned to full commit SHAs (checkout v4.4.0, rust-toolchain stable branch w/ explicit `toolchain: stable`, rust-cache v2.9.2); `.github/dependabot.yml` added for `github-actions` |
| [medium] ci.yml:1 | no `permissions:` block | **Fixed** — workflow-level `permissions: contents: read` |
| [low] zimbench.rs:138 | `-d 0` divide-by-zero panic | **Fixed** — phase 3 skips with a message when no random urls were selected |
| [low] zimbench.rs:112 | random sampling not distinct | **Fixed** — partial Fisher–Yates over indices; exactly `d` distinct urls |
| [low] bench/parity.sh:9, recreate-bench.sh:15, recreate-suite.sh:16 | build failure not fatal | **Fixed** — `cargo build … \|\| exit 1` guard (chosen over blanket `set -e`, which would abort the harnesses on legitimately non-zero diff/compare commands) |
| [high] zimdump.rs:474/670 | path traversal in `dump` | **Fixed** — `safe_path` rebuilds from segments, dropping empty/`.`/`..`; all-traversal paths land at `_` inside the root |
| [medium] zimdump.rs:374 | unchecked `clusters[cluster_idx]` | **Fixed** — `clusters.get(...)` with skip on out-of-range dirent values |
| [medium] zimcheck.rs:557 | `touch_cluster` no-op | **Fixed** — decodes each cluster via `cluster_uncached` and walks its blob-offset table |
| [medium] cffi/archive.rs:718 | binary metadata NUL corruption | **Fixed** — raw bytes stored verbatim (+ trailing NUL for C convenience), `*out_len` reports the true length |
| [medium] cluster.rs:153/172 | decompression bomb | **Fixed** — decode capped at 4 GiB (standard, format-implied by u32 offsets) / 16 GiB (extended); zstd's declared frame size validated before allocation; limit-hit tests added |
| [low] zimwriterfs.rs:734 | mime table rebuilt per call | **Fixed** — `OnceLock` map |
| [low] zimwriterfs.rs:845 | `-x/--inflateHtml` non-functional | **Fixed** — implemented with `flate2` (pure-Rust backend, writer-feature dep); corrupt gzip input is a hard error with the path |
| [low] zimsplit.rs:102 | `parse_size` unchecked multiply | **Fixed** — `checked_mul`, overflow rejected |
| [low] zimru.rs:220 | `readall` unbounded cache | **Skipped per verdict** (partial) — explicitly documented, intentional trade-off for the dev tool |
| [medium] writer.rs:1901 | `M/Counter` dedup misses pending metadata | **Fixed** — guard also scans every bucket's `pending` list; regression test added (`caller_supplied_counter_suppresses_auto_generated_one`) |
| [medium] writer.rs:2045 | listing dedup same defect | **Skipped per verdict** — REFUTED (the listing guard runs after `flush_all_buckets`, so pending entries are already committed) |
| [low] raw.rs:14 | offset add overflow | **Fixed** — `checked_add` in `u16_at`/`u32_at`/`u64_at`/`cstr_at` (hygiene per verdict: release build was already memory-safe) |
| [low] writer.rs:965 | `intern_mime` u16 wrap | **Fixed** — explicit error at 0xFFFD distinct mimetypes (indices 0xFFFD–0xFFFF are reserved) |
| [medium] tests/streaming_encode_smoke.rs:33 | test never exercises streaming path | **Fixed** — new `Creator::set_streaming_encode_threshold` lowered to 1 MiB in the test; added a single-blob-cluster shape assertion; stale “4 MiB” comments corrected |
| [low] tests/cffi_smoke.rs:23, cffi_writer_smoke.rs:21 | hardcoded `target/release` | **Fixed** — profile dir derived from the test executable's location; missing dylib now skips with an actionable message instead of panicking |
| [low] tests/cffi_writer_smoke.rs (CI) | writer C-ABI test not in CI | **Fixed** — `--test cffi_writer_smoke` added to the cffi job |
| [high] zimrecreate.rs:284 | legacy archives recreate empty | **Fixed** — legacy namespaces (`A B I J - U V`) normalized to `C` before gating, per verdict |
| [medium] zimrecreate.rs:301 | metadata mimetype discarded | **Fixed** — `add_metadata_with_mimetype` with the source mimetype |
| [low] zimrecreate.rs:254 | redirect to non-`C` target dangles | **Fixed** — skipped with a warning naming the redirect and target |
| [low] zimrecreate.rs:203 | helper children/tmp dir leak on error | **Fixed** — `Drop` on `IndexHelper` kills+reaps children; `TmpDirCleanup` guard removes the temp dir on all exit paths (both bins) |
| [low] _index_helper.rs:288 | blocking write to dead child | **Fixed** — `try_wait` liveness check before each write; feed disabled once the child exits |
| [low] _index_helper.rs:318 | stderr pipe deadlock | **Fixed** — stderr drained by a dedicated thread from spawn; `finish` reads the collected buffer |

## Perf sweep (21 findings)

| # | Finding | Disposition |
|---|---------|-------------|
| [high] archive.rs:1043 (+ cffi/item.rs:155, warmup — duplicates) | direct-access materializes whole cluster | **Fixed** — `raw_blob_range` reads the info byte + offset table straight from the mmap; compressed clusters answered from the compression id without decoding |
| [medium] _index_helper.rs:241 | index slurped whole into RAM | **Fixed** — `IndexBlob` is file-backed; `add_to` streams via `begin_chunked_item`/`chunked_item_chunk` in 4 MiB chunks (both bins) |
| [low] archive.rs:1236/1259 | per-dirent String in order checks | **Fixed** — `prev` borrows `(u8, &str)` from the mmap |
| [high] zimdump.rs:379 | `analyze --by-item` re-decodes per blob | **Fixed** — per-blob sizes retained from pass 1; no cluster access in the item loop |
| [low] zimdump.rs:255 | `--idx` linear scan | **Fixed** — `namespace_range` + direct index |
| [low] zimcheck.rs:638 | content-scan clone churn | **Fixed** — borrow the dirent, clone only `url`, store `is_html: bool` |
| [low] zimcheck.rs:541 | `touch_cluster` wasted loop | **Fixed** — same fix as the bug-sweep finding (real per-cluster decode) |
| [medium] zimru.rs:220 | `readall` cache pinning | **Skipped per verdict** — documented intentional trade-off |
| [low] zimwriterfs.rs:734 | mime table per call | **Fixed** — `OnceLock` (same as bug sweep) |
| [medium] zimwriterfs.rs:774 → **:814 per verdict** | full-body lowercase for `<title>` | **Fixed at the corrected line** — ASCII-case-insensitive byte search, no copy (`sniff_mime`'s bounded 1 KB window left as-is per verdict) |
| [low] cffi/archive.rs:816 | O(n) metadata count | **Fixed** — `entry_count_in_namespace(NS_METADATA)`, O(log n) |
| [medium] cffi/archive.rs:829 | O(k·n) key enumeration | **Fixed** — key list computed once per handle (`OnceLock<Vec<CString>>`); `get_metadata_keys` itself now walks only the `M` range |
| [low] cffi/archive.rs:727 | unbounded intern growth | **Fixed** — per-key `HashMap<String, Box<[u8]>>`; repeat reads return the cached pointer |
| [medium] writer.rs:1745 | `bucket_key` alloc per item | **Fixed** — reusable scratch `String` on the streamer; alloc only on new-bucket insert |
| [low] tests/real_files.rs:65 | all paths materialized for a 200-sample check | **Fixed** — stride computed up front, only sampled entries stored |
| [low] tests/real_files.rs:94 | unconditional title clone | **Fixed** — `key` moves into `prev`; clone only on the sample branch |
| [medium] zimrecreate.rs:232 | pass-1 Strings for whole archive | **Fixed** — compact `(cluster, blob, url_index)`; pass 2 re-resolves from the mmap |
| [low] zimrecreate.rs:339 + cluster.rs:73 | decoded-payload extra memcpy | **Documented as known** — per the review, the fix is a lifetime-threading refactor of `Cluster`/`Item`/encode (not a localized change); a comment marks the copy. The worst case (direct-access of huge uncompressed clusters) is eliminated by the archive.rs:1043 fix |
| [low] dirent.rs:114 | double url alloc for empty titles | **Fixed** — raw title stored (empty `String` doesn't allocate); fallback deferred to the `title()` accessors, per the verdict's correction |

## Validation

`cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
and `cargo test --all-features` (15 suites) all pass; the cffi smoke tests
additionally verified in a debug profile (`cargo build --features cffi` +
`cargo test --features cffi`), which previously hard-failed.
