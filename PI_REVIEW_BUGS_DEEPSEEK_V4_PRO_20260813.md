# Pi sweep review — zimru-f516830a

Exhaustive per-file pass: 57 code files across 6 batches.

## Findings

- [high] .github/workflows/ci.yml:18 — third-party actions are not SHA-pinned: `dtolnay/rust-toolchain@stable` is a moving alias (tracks latest stable Rust) and `Swatinem/rust-cache@v2` is a mutable version tag (repeated at lines 30/31, 46/49, 58, 69/70). `actions/checkout@v4` (first-party tag) is lower risk but also unpinned. — A compromised or repointed action tag executes on every push to `main` and, combined with the missing `permissions:` block, runs with the default `contents: write` GITHUB_TOKEN, allowing a supply-chain compromise to push to the repo. — Pin every action to a full 40-char commit SHA (`uses: dtolnay/rust-toolchain@<sha>`, `Swatinem/rust-cache@<sha>`) and add Dependabot updates for `github-actions`.
- [medium] .github/workflows/ci.yml:1 — no `permissions:` block at workflow or job level, so the workflow uses the repo's default GITHUB_TOKEN grants; on `push` to `main` the token defaults to `contents: write` (and possibly more) even though the job only builds/tests. — Any step (including the third-party actions above, or a future edit) silently holds a write token, widening the injection/compromise blast radius. — Add `permissions: { contents: read }` (nothing here needs write).
- [low] src/bin/zimbench.rs:138 — `(rng.next_u64() as usize) % random_urls.len()` panics with a divide-by-zero when `-d 0` is passed (then `actual_d = 0`, `random_urls` is empty, and `r` defaults to `n = 1000`, so the Phase-3 loop still runs). — `zimbench -d 0 file.zim` crashes with `attempt to calculate the remainder with a divisor of zero` instead of benchmarking. — Guard the phase: `if random_urls.is_empty() { return Ok(()); }` (or `continue`) before the random-access loop.
- [low] src/bin/zimbench.rs:112 — random-URL sampling does not enforce distinctness: `while random_urls.len() < actual_d { random_urls.push(urls[idx].clone()) }` can select the same index multiple times, contradicting the documented "picks -d distinct random articles" behavior. — The random-access phase over-reads hot articles and under-reads cold ones, skewing the reported MB/s and defeating the benchmark's purpose. — Reject duplicates (track a seen-set or use a partial Fisher–Yates shuffle) so exactly `d` distinct URLs are chosen.
- [low] bench/parity.sh:9 — `set -uo pipefail` omits `-e`, so a failed `cargo build --release` does not abort the script; every subsequent `bash -c "${US_CMD[...]}"` then runs against a missing/stale `./target/release/{zimdump,zimcheck}`. — A broken build produces 100% "DIFF" rows that look like real regressions instead of a loud build failure, defeating the parity harness's purpose. — Add `set -e` (or an explicit `cargo build --release --quiet || exit 1`).
- [low] bench/recreate-bench.sh:15 — `set -uo pipefail` omits `-e`; if `cargo build --release` fails the loop still runs and `$ZIMRU_RECREATE` is not-found, printing "FAIL" per row for a build problem rather than aborting. — Build failures are misreported as per-file tool failures, misleading benchmark interpretation. — Add `set -e` (or guard `cargo build` with `|| exit 1`).
- [low] bench/recreate-suite.sh:16 — `set -uo pipefail` omits `-e`; if `cargo build --release` fails, `$ZIMRU readall`/`$ZIMDUMP info`/`$ZIMRECREATE` are missing and the script emits empty/DIFF MD5 and entry counts instead of stopping. — A broken build is surfaced as "BLOB-MD5 DIFF"/entry mismatches, which read as data-regression failures. — Add `set -e` (or guard `cargo build` with `|| exit 1`).

## Coverage
.github/workflows/ci.yml — findings: 2
bench/cache_aware_compare.sh — clean
bench/parity.sh — findings: 1
bench/recreate-bench.sh — findings: 1
bench/recreate-suite.sh — findings: 1
bench/run.sh — clean
benches/cluster_decode.rs — clean
benches/get_entry_by_path.rs — clean
benches/warmup.rs — clean
build.rs — clean
Cargo.toml — clean
cbindgen.toml — clean
include/zimru.h — clean
src/archive.rs — clean
src/bin/_index_helper.rs — clean
src/bin/zimbench.rs — findings: 2
- [high] src/bin/zimdump.rs:474 — path traversal in `zimdump dump`: `safe_path` (src/bin/zimdump.rs:670) only strips leading `/` and leaves `..` components intact, so `write_with_fallback` computes `dir.join(rel)` with an attacker-controlled `rel`; an entry path like `../../../../home/user/.bashrc` escapes the `--dir` root and `write_entry` creates parent dirs + writes there — dumping a malicious/corrupt ZIM performs arbitrary file writes outside the target directory (e.g. overwriting a shell rc or a later-executed script) — normalize/reject `..` and `..`-leading paths and verify the resolved destination stays under `dir` (e.g. canonicalize the parent and check `starts_with`).
- [medium] src/bin/zimdump.rs:374 — unchecked indexing: `clusters[cluster_idx as usize]` uses `cluster_idx` read from the untrusted ZIM dirent (`a.cluster`) with no bounds check against `clusters.len()`; a corrupt dirent pointing at `cluster >= cluster_count` panics before the `get_item` error path runs — `zimdump analyze --by-item` crashes on malformed input — bounds-check first (`clusters.get(cluster_idx).ok_or(Error::BadClusterIndex {..})`) or skip the entry.
- [medium] src/bin/zimcheck.rs:557 — integrity check is a no-op for clusters: `touch_cluster` ignores its `_c` argument and only re-reads dirent 0 (`arc.entry_by_url_index(0)`), so the `for c in 0..cluster_count { touch_cluster(...) }` loop in `check_integrity` never decompresses or parses any cluster despite the comment claiming it "validates every cluster parses" — `zimcheck -I` / `-A` reports Pass for archives whose cluster payloads are corrupt — actually load/parse each cluster (`arc.cluster_uncached(c)?`) or delete the misleading loop.
- [medium] src/cffi/archive.rs:718 — `zimru_archive_metadata` silently corrupts binary metadata: when a value contains an interior NUL byte (e.g. `M/Illustration_*` PNG data), `CString::from_vec_with_nul` fails and the fallback replaces every NUL with a space while `*out_len` still reports the original length — C consumers read corrupted bytes for binary metadata — store the raw `Vec<u8>` in the interned cache and return its pointer + length instead of forcing it through `CString`.
- [medium] src/cluster.rs:153 — unbounded decompression (decompression bomb): `decode_xz` pre-allocates `body.len() * 4` and grows via `read_to_end` with no cap, and `decode_zstd` (src/cluster.rs:172) allocates the attacker-declared frame content size via `zstd::bulk::decompress(body, size as usize)` with no limit — a malicious cluster can decompress to gigabytes and OOM the process when any untrusted ZIM is opened — cap the decompressed size (check the declared size against a maximum, or stream with `.take(limit)`) before allocating.
- [low] src/bin/zimwriterfs.rs:734 — `mime_for_path` rebuilds a `HashMap` from the static `TABLE` on every call (once per input file, in both the parallel batch path and the big-file path) — avoidable allocation churn on large input trees — hoist the table into a `static OnceLock<HashMap<&str,&str>>` or replace the lookup with a `match`.
- [low] src/bin/zimwriterfs.rs:845 — `-x/--inflateHtml` is advertised but non-functional: `flate2_decoder` always returns `Err(Unsupported)`, so gzipped `.html` files are packed still-compressed while still labeled `text/html` (title derivation and fulltext indexing then see gzip bytes) — readers receive gzip bytes for `text/html` items — either implement inflation (add `flate2`) or reject the flag with an explicit error instead of silently no-oping.
- [low] src/bin/zimsplit.rs:102 — `parse_size` computes `n * mult` with unchecked multiplication; a suffixed value such as `--size=20000000000G` wraps silently in release builds — produces a wrong (possibly tiny) split size, and with `--force` can generate an enormous number of part files — use `checked_mul` and reject overflow.
- [low] src/bin/zimru.rs:220 — `readall` sets `set_cluster_cache_max_bytes(usize::MAX)`, which disables cache eviction entirely, so every decompressed cluster stays resident for the whole run — running `zimru readall` on a multi-GB ZIM holds the entire decompressed archive in memory and OOMs the process — use a large finite budget (or an uncached single-pass read) instead of `usize::MAX`.

## Coverage
src/bin/zimcheck.rs — findings: 1
src/bin/zimdump.rs — findings: 2
src/bin/zimrecreate.rs — clean
src/bin/zimru.rs — findings: 1
src/bin/zimsplit.rs — findings: 1
src/bin/zimwriterfs.rs — findings: 2
src/cffi/archive.rs — findings: 1
src/cffi/blob.rs — clean
src/cffi/creator.rs — clean
src/cffi/entry.rs — clean
src/cffi/error.rs — clean
src/cffi/item.rs — clean
src/cffi/mod.rs — clean
src/cffi/uuid.rs — clean
src/cluster.rs — findings: 1
src/dirent.rs — clean
src/error.rs — clean
src/header.rs — clean
src/io_hints.rs — clean
src/lib.rs — clean
- [medium] src/writer.rs:1901 — the "skip if caller supplied M/Counter" guard only inspects `self.dirents` (committed dirents), but caller-supplied metadata is still sitting un-flushed in `self.buckets[*].pending` at that point: `finalize` step 1 pushes `self.metadata` through `push_item` into a bucket, and only step 2b calls `flush_all_buckets`. So `add_metadata("Counter", …)` (a documented supported path) is never detected and the auto-emitted `M/Counter` is written too, producing two dirents with the same `(namespace, url)` pair. — the output archive violates the unique-URL invariant and readers (`entry_by_ns_path`) return whichever duplicate binary-search finds. — smallest safe fix: before generating the counter, also scan the drained metadata buffer and each bucket's `pending` list for `(b'M', "Counter")` (or emit the counter after flushing metadata and re-check committed + pending entries).

- [medium] src/writer.rs:2045 — same dedup defect for `X/listing/titleOrdered/v1`: the guard checks only `self.dirents`, but a caller-added `Item::in_namespace(b'X', "listing/titleOrdered/v1", …)` normally lives in a pending bucket (it is committed only if its bucket happened to overflow the cluster target). When it has not flushed yet, the auto-emitted listing entry is still generated, yielding two `X/listing/titleOrdered/v1` dirents. — duplicate dirent; the reader's `listing` lookup (archive.rs `entry_by_ns_path`) returns an arbitrary one, so the title index may silently point at the caller's stale copy. — smallest safe fix: check both `self.dirents` and every bucket's `pending` articles for `(b'X', "listing/titleOrdered/v1")` before auto-emitting.

- [low] src/raw.rs:14 — `u16_at`/`u32_at`/`u64_at` compute `off + N` without `checked_add`. Offsets reaching these helpers are ultimately file-controlled (e.g. header `url_ptr_pos`/`cluster_ptr_pos` or cluster-pointer values cast to `usize`), so a crafted ZIM with a field near `usize::MAX` overflows the addition — a panic (DoS) in debug builds, and in release it only survives because the wrapped `end` makes `get(off..end)` return `None` via `start > end`. — malformed input aborts the process instead of returning `Error::Truncated`. — smallest safe fix: use `off.checked_add(N).ok_or(Error::Truncated(off as u64))?` (and `start.checked_add(nul + 1)` in `cstr_at`).

- [low] src/writer.rs:965 — `intern_mime` stores the mime index as `mimes.len() as u16`, silently wrapping once more than 65535 distinct mimetypes are added; the wrapped value collides with an earlier index and `build_counter_string`/dirents then attribute the wrong mime. This is currently masked because `finalize` step 11 rejects the oversized mime list (>64 KiB reserve) with a misleading "exceeds reserved region" message, but the truncation happens silently before that. — silent index corruption (today surfacing only as a confusing error) instead of an explicit "too many mime types" failure. — smallest safe fix: reject with a clear error when `mimes.len() >= u16::MAX as usize` before casting.

## Coverage
src/mime.rs — clean
src/raw.rs — findings: 1
src/uuid.rs — clean
src/writer.rs — findings: 3
- [medium] tests/streaming_encode_smoke.rs:33 — `huge_item_streams_through_zstd_encoder_without_buffering` feeds a 6 MiB payload (line 38/66) and its name/doc claim it exercises the streaming-encode path, but `src/writer.rs:311` defines `STREAMING_ENCODE_THRESHOLD = 256 * 1024 * 1024`, so `begin_chunked_item` (writer.rs:1339, `big_enough = expected_size >= 256 MiB`) takes the Buffered branch for a 6 MiB item — the streaming-encode code path for >=256 MiB items is never exercised by this test, so a regression in the bounded-memory streaming path would still pass CI while the test name/doc falsely imply coverage — fix: use a payload > 256 MiB (or add a test-only threshold override) and correct the stale "4 MiB" comments at lines 2, 33, 135.

- [low] tests/cffi_smoke.rs:23 — `target_dir()` hardcodes `target/release`, so running the documented `cargo test --features cffi` in the default (debug) profile builds `libzimru.dylib` under `target/debug` and the `assert!(dylib_path.exists(), …)` at line 87 panics ("libzimru.dylib not found") instead of skipping — a contributor running plain `cargo test` gets a hard failure with a misleading message — fix: derive the profile-relative dir (e.g. from `env!("PROFILE")` or `target/debug` fallback) or gate the test on a release build.

- [low] tests/cffi_writer_smoke.rs:21 — same hardcoded `target/release` in `target_dir()`; the `assert!(dylib_path.exists(), …)` at line 66 panics under a debug `cargo test --features cffi` even though the cdylib exists at `target/debug` — fix: same as `tests/cffi_smoke.rs` (profile-relative target dir).

- [low] tests/cffi_writer_smoke.rs — this writer-C-ABI smoke test is never executed in CI: the `cffi` job in `.github/workflows/ci.yml` runs only `cargo test --release --features cffi --test cffi_smoke`, and the `build-test` job runs without the `cffi` feature, so `cffi_writer_smoke.rs` (and `tests/cffi_writer_smoke.c`) are excluded everywhere — regressions in the `zimru_creator_*` C ABI would go uncaught — fix: add `--test cffi_writer_smoke` to the cffi CI job.

## Coverage
examples/writer_bench.rs — clean
tests/cffi_smoke.c — clean
tests/cffi_smoke.cpp — clean
tests/cffi_smoke.rs — findings: 1
tests/cffi_writer_smoke.c — clean
tests/cffi_writer_smoke.rs — findings: 2
tests/direct_access.rs — clean
tests/ergonomic_api.rs — clean
tests/integrity_checks.rs — clean
tests/per_item_compression.rs — clean
tests/real_files.rs — clean
tests/streaming_encode_smoke.rs — findings: 1
tests/synthetic_zim.rs — clean
tests/writer_roundtrip.rs — clean
tests/xapianbuilder_helper.rs — clean
tests/zimdump_analyze.rs — clean
tests/zimwriterfs_e2e.rs — clean
# Batch 5 findings

- [high] src/bin/zimrecreate.rs:284 — pass-1 only copies `C`-namespace articles (`if ns == b'C'`) and `M`-namespace entries, and only re-adds `C`-namespace redirects (line 251); legacy (major=5, minor=0, `uses_new_namespaces()==false`) ZIMs store user content in `A` (articles) and `I`/`J`/`-` (media), so on a legacy archive every article/media entry is silently dropped — the recreated archive ends up with metadata only, and the regenerated `W/mainPage -> C/<main>` redirect dangles and makes `finish_writing` fail with "redirect ... has no target" — gate the content namespace on `uses_new_namespaces()` (use `A` for legacy and copy the other legacy media namespaces), or refuse legacy input with a clear error.
- [medium] src/bin/zimrecreate.rs:301 — `creator.add_metadata(path, data)` records `text/plain;charset=utf-8` (the writer's `DEFAULT_METADATA_MIMETYPE`) for every non-square-illustration `M` entry, discarding the original mimetype already held in the local `mime` var (line 280); non-text metadata (e.g. PNG favicons, non-square `Illustration_WxH@1` where W≠H) is rewritten as text/plain, so downstream readers mis-handle it — use `creator.add_metadata_with_mimetype(path, mime, data)`.
- [low] src/bin/zimrecreate.rs:254 — a `C`-namespace redirect is re-emitted via `add_redirection`, whose finalize hard-codes `target_ns: b'C'` (writer.rs:1935); if `r.redirect_index` resolves to an entry in a different namespace the rewritten `C/<target_path>` never resolves and `finish_writing` fails with "redirect ... has no target" (and redirects living in non-`C` namespaces are already silently dropped by the `ns == b'C'` gate) — skip (or emit an explicit-namespace redirect for) redirects whose target namespace isn't `C`.
- [low] src/bin/zimrecreate.rs:203 — on any error path after `IndexHelper::spawn` (e.g. `entry?` at line 234 or `cluster_uncached(cidx)?` at line 330 on a corrupt source), `run` returns `Err` and drops `IndexHelper` without `finish()`/`wait`, leaking the two `xapianbuilder` children (un-reaped; they also never have their output read) and leaving the `.{stem}.xapianbuilder.{pid}` temp dir on disk — kill/wait the children and remove the temp dir on early return (e.g. a `Drop` impl on `IndexHelper` or a scope guard around the iteration).

## Coverage
src/archive.rs — clean
include/zimru.h — clean
src/cffi/creator.rs — clean
src/bin/zimrecreate.rs — findings: 4
- [low] src/bin/_index_helper.rs:288 — `write_raw` performs a blocking `stdin.write_all(buf)` with no timeout and no `try_wait()` liveness check, so if `xapianbuilder` wedges without exiting (alive but stops draining stdin) the parent blocks forever once the 64 KB stdin pipe fills — `zimwriterfs`/`zimrecreate` hang with no error surfaced and no way to distinguish a dead helper from a slow one — smallest safe fix: poll `job.child.try_wait()` in `write_raw` (and `finish`) and, on `Some(status)`, take the stdin handle and stop feeding, as the existing broken-pipe error path already does.
- [low] src/bin/_index_helper.rs:318 — child stderr is configured `Stdio::piped()` but is only consumed in `finish()` via `wait_with_output()` (line 240), so during the potentially long feeding phase any helper output exceeding the ~64 KB pipe buffer (e.g. diagnostics `--quiet` does not suppress) makes the child block writing stderr, which stops it reading stdin and deadlocks the parent's `write_all` — smallest safe fix: drain stderr concurrently (spawn a reader thread into a `Vec<u8>`), or set stderr to `Stdio::null()`/`Stdio::inherit()` since `--quiet` already suppresses normal output.

## Coverage
src/bin/_index_helper.rs — findings: 2
src/cffi/item.rs — clean
src/dirent.rs — clean

## Run stats

input 379896 tok (+5096576 cached), output 155928 tok, cost $0.32 — 64 files in 15m (248.8 files/h, 2.6 min/batch)
