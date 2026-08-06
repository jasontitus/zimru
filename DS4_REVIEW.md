# DS4 sweep review — zimru

Exhaustive per-file pass: 57 code files across 6 batches.

## Findings

# Batch 1 findings — zimru

- [high] .github/workflows/ci.yml:18,30,46,58,69 — unpinned third-party GitHub Actions (`dtolnay/rust-toolchain@stable` and `Swatinem/rust-cache@v2`) — `@stable` is a mutable branch ref and `@v2` is a mutable version tag; either can be repointed to malicious code without a lockfile, and CI runs this code on every push/PR — pin each to a full 40-char commit SHA (e.g. `dtolnay/rust-toolchain@<sha>`).

- [low] .github/workflows/ci.yml:14 — no `permissions:` block at workflow or job level — every job inherits the broad default `GITHUB_TOKEN` on `pull_request`; jobs here reference no secrets, but a future edit that adds a secret-using step gets the wide token by default — add an explicit `permissions: contents: read` (and `packages: read` for rust-cache) at the top level.

- [medium] src/bin/zimbench.rs:114 — `let idx = (rng.next_u64() as usize) % random_urls.len();` divides by zero when `random_urls` is empty, which happens when the user passes `-d 0` (`actual_d = d.min(urls.len())` = 0, so the `while random_urls.len() < actual_d` fill loop never runs) — `zimbench -d 0` (with default `-r`/`-n` 1000) panics with "attempt to divide by zero" instead of exiting gracefully — guard `if random_urls.is_empty() { return Ok(()); }` before Phase 3 (and guard `urls.len()` at line 113 the same way).

- [low] src/archive.rs:279,291,296 — `pos + 16` is unchecked `u64`-to-`usize` arithmetic where `pos` comes from the archive header (`checksum_pos`, attacker-controlled in a crafted ZIM); a crafted `checksum_pos` ≥ `usize::MAX - 15` wraps `pos + 16` so the `> mmap.len()` guard passes, then `&self.core.mmap[pos..pos + 16]` (line 283) or `&self.core.mmap[..pos]` (line 296) slices out of bounds and panics — `zimcheck -C`/`check()` on a malicious file aborts instead of returning `Error::Truncated` — use checked/saturating arithmetic (`pos.checked_add(16)` returning `Error::Truncated` on overflow, or `pos.saturating_add(16)`) before the bounds tests.

- [low] bench/parity.sh:95-112,135-136 — commands are stored as strings that interpolate `$UPSTREAM_DIR`/`$OUR_DIR` unquoted and `$ZIM` inside single quotes (e.g. `"$UPSTREAM_DIR/zimdump info '$ZIM'"`), then executed via `bash -c "${UP_CMD[$label]}"` — a ZIM path or UPSTREAM_DIR containing a quote or whitespace injects arguments/commands into the shell — dev-only harness (not CI), but still — build the command vector with separate argv elements (`bash -c` with an array, or reject `'`/whitespace in `$ZIM`/`$UPSTREAM_DIR`).

- [low] bench/cache_aware_compare.sh:100-141,157 — hyperfine `--prepare` and command strings interpolate `$ZIM`/`$SITE`/`$OUT`/`$ICON_NAME` unquoted (e.g. `REAL_PREPARE="rm -f \"$OUT/wfs_real.zim\" && find \"$SITE\" -type f -exec cat {} \\; > /dev/null"` and `"$REAL_ZIMCHECK -C $ZIM"`), and hyperfine runs those strings through a shell — a site/ZIM path containing whitespace or a quote splits/injects — dev-only benchmark, but quote the interpolated variables or reject whitespace in the paths.

- [low] build.rs:14-24 — writes the generated header to the source tree (`include/zimru.h`) instead of `OUT_DIR`, so `cargo build --features cffi` mutates a git-tracked file and dirties the working tree / makes rebuilds non-reproducible; deliberate (the committed header is regenerated in place) but breaks clean-build isolation — generate into `OUT_DIR` and have consumers/cbindgen emit the committed header separately, or at least document the side effect.

## Coverage

.github/workflows/ci.yml — findings: 2
bench/cache_aware_compare.sh — findings: 1
bench/parity.sh — findings: 1
bench/recreate-bench.sh — clean
bench/recreate-suite.sh — clean
bench/run.sh — clean
benches/cluster_decode.rs — clean
benches/get_entry_by_path.rs — clean
benches/warmup.rs — clean
build.rs — findings: 1
Cargo.toml — clean
cbindgen.toml — clean
include/zimru.h — clean
src/archive.rs — findings: 1
src/bin/_index_helper.rs — clean
src/bin/zimbench.rs — findings: 1
# Batch 2 findings — zimru (src/bin/*, src/cffi/*, core reader modules)

- [medium] src/bin/zimcheck.rs:1002 — `resolve_relative` calls `parts.pop()` to drop the base's file component and again on every `..` target segment, with no empty-guard — when `base` has no directory components (a root-level article path like `home`, `a.url` in the untrusted dirent, line 648) the first `parts.pop()` empties the vec and a `..` in the link target (`target.split('/')` from attacker-controlled HTML, line 998) panics `parts.pop()` on an empty `Vec` — `zimcheck -U`/`-A` on a crafted ZIM aborts the whole tool instead of reporting a dangling link — replace `parts.pop()` with `if !parts.is_empty() { parts.pop() }` (or `parts.pop().filter(|_| !parts.is_empty())`) so out-of-root `..` segments are clamped.

- [medium] src/bin/zimdump.rs:374 — `let info = &clusters[cluster_idx as usize];` indexes the per-cluster `clusters` vector (sized `n_clusters` = header `cluster_count`) with `cluster_idx` taken from the article dirent (`Dirent::Article(a) => (a.cluster, a.blob)`, line 371) — `Dirent::parse` (src/dirent.rs:106-108) stores the cluster field verbatim with no validation against `cluster_count`, so a crafted ZIM whose dirent `cluster` >= `cluster_count` panics out-of-bounds — `zimdump analyze` (and `--by-item`) on a malicious file aborts — guard `if cluster_idx < clusters.len()` (else `continue`/report) before indexing.

- [medium] src/bin/zimdump.rs:670-672 — `safe_path` only strips leading `/` and otherwise keeps `rel` verbatim, and `cmd_dump` writes via `dir.join(rel)` (line 474) and `exception_dest` (line 658-667, which escapes `/`→`%2f` and `%`→`%25` but leaves `..` intact) — a ZIM entry whose path is `../evil` or `../../etc/...` is written outside the requested dump `--dir` (path traversal), since `..` components are never collapsed and `dir.join` does not prevent escaping the root — `zimdump dump --dir=DIR` on a crafted ZIM writes files outside `DIR` — normalize each `rel` (reject or `..`-collapse components that escape `DIR`) before `join`, and escape `.`/`..` components in `exception_dest`.

- [low] src/cffi/archive.rs:427-431,441 — `zimru_archive_illustrations` does `boxed.shrink_to_fit()` then `std::mem::forget(boxed)` and returns `ptr`, relying (comment line 429) on `capacity == length` so `zimru_illustrations_free` (line 441) can `drop(Vec::from_raw_parts(ptr, count, count))` — `shrink_to_fit` is best-effort and is not guaranteed to reduce capacity to `len`, so when capacity > count the `from_raw_parts(ptr, count, count)` reconstructs the Vec with a smaller capacity than the buffer, and dropping it is UB (double-free/OOB on the freed allocation) — use `Vec::into_raw_parts()` (or `Box::into_raw`+`slice_from_raw_parts`) to carry the exact `(ptr, len, cap)` triple, or store the capacity alongside `count`.

- [medium] src/bin/zimru.rs:220 — `arc.set_cluster_cache_max_bytes(usize::MAX);` disables the cluster-cache byte budget (default 64 MB) for `readall`, so every decompressed cluster stays resident — a crafted ZIM with a decompression bomb (one cluster that expands to hundreds of MB/GB) pins that whole payload in RAM and the cache never evicts it — `zimru readall` on a malicious file exhausts memory instead of bounding RSS — keep a finite cap (e.g. the default) for untrusted files, or evict after each entry.

- [medium] src/bin/zimcheck.rs:345-349 (reaching src/archive.rs:349) — `Archive::title_listing` computes `base + i * 4` with `base = title_ptr_pos as usize` and `i` up to `entry_count` (both attacker-controlled header fields), unchecked — a crafted `title_ptr_pos` near `usize::MAX` wraps `base + i*4`, so `raw::u32_at(&mmap, wrapped_off)` reads from an arbitrary in-mapped offset (out-of-bounds read past the intended title table) — reachable from `zimcheck -I` (`check_title_index` → `title_listing`, src/bin/zimcheck.rs:531-551) and `zimru titles` — bounds-check `title_ptr_pos` and `entry_count` against `mmap.len()` and use `checked_mul/checked_add` before the loop (return `Error::Truncated` on overflow).

- [low] src/cluster.rs:92-96 — `blob_count` is derived from the attacker-controlled first offset (`first / ptr_size - 1`) with no validation that the payload actually contains that many pointers — a crafted cluster sets `first = u32::MAX` (4-byte ptrs) yielding `blob_count ≈ 1e9`, and callers iterate `0..c.blob_count()` (e.g. `zimdump analyze` src/bin/zimdump.rs:351-353, `zimcheck`), each `blob(i)` failing `read_off` bounds but still doing a per-index probe — `zimdump analyze`/`zimcheck` on a small crafted file spin through billions of error calls (CPU DoS) — validate `first` against the payload length (require `(N+1)*ptr_size <= payload.len()` and `first % ptr_size == 0`) before accepting `blob_count`.

- [low] src/bin/zimsplit.rs:169-175 — `advance_suffix` increments `s[0]` when `s[1] == b'z'` with no upper bound, so after 26 parts (`az`→`ba`…`zz`) the 27th part gets `s[0] = b'{'` (0x7B), producing a broken part filename (`{a`) instead of rolling over to a 3-letter suffix — `zimsplit --size` small enough to need 27+ parts writes misnamed parts that no concatenating tool recognizes — extend `advance_suffix` to a variable-length suffix (roll `z`→`aa`, `zz`→`aaa`, …) or reject files needing more than 26 parts.

## Coverage

src/bin/zimcheck.rs — findings: 3
src/bin/zimdump.rs — findings: 3
src/bin/zimrecreate.rs — clean
src/bin/zimru.rs — findings: 1
src/bin/zimsplit.rs — findings: 1
src/bin/zimwriterfs.rs — clean
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
# Batch 3 findings — zimru (src/mime.rs, src/raw.rs, src/uuid.rs, src/writer.rs)

- [medium] src/writer.rs:965 — `intern_mime` casts the mime-list length to `u16` (`let i = mimes.len() as u16;`) before pushing the new mimetype, so once the build intern more than 65535 distinct mimetypes the index silently wraps and collides with an existing entry (the BTreeMap insert overwrites the old `(mime, index)` mapping). — a caller that feeds >65535 distinct `Item::mimetype` / `add_metadata_with_mimetype` strings (or generated per-item mimetypes) produces a ZIM whose mime list and per-dirent `mime_idx` are corrupted without any error: `MimeList::get` (src/mime.rs:28) resolves those entries to the wrong mimetype, and the `M/Counter` histogram (build_counter_string, writer.rs:976) attributes counts to the wrong mime — silent data corruption in a published archive — check `mimes.len() < u16::MAX` (else return an `Error` from `push_item`/`begin_chunked_item`), or store the index as `u32`/`usize` and widen the dirent field.

- [low] src/writer.rs:2514-2524 — `default_uuid()` builds the 16-byte archive UUID from `SystemTime::now()` nanos + `pid.wrapping_mul(0x9E3779B97F4A7C15)` instead of a CSPRNG — the archive identifier is predictable and enumerable, and archives created by the same process at the same nanosecond collide (identical 16 bytes). — downstream readers/tools that key on the archive UUID (cache/dedup, cross-archive correlation) can misidentify or collide two archives produced in the same instant; not a secret, but a uniqueness/quality defect — derive the UUID from `getrandom`/`rand` (or `Uuid::new_v4`) instead of time+pid.

- [low] src/writer.rs:586 — streaming-mode `add_item` does `panic!("streaming add_item: {e}")` on any `Streamer::push_item` error (the `&mut Self` return shape can't propagate), and `push_item` errors are reachable from real IO failures (`flush_bucket` → `send_cluster` → pipeline encode/write error, e.g. disk-full mid-build). — a disk-full / broken-pipe during a streaming `Creator::add_item` call aborts the whole process instead of returning the error (the documented `push_streaming_item` error-propagating helper exists but most call sites use `add_item`) — store the first error on the `Creator` and return it from `finish_writing`, or have `add_item` return a `Result`.

## Coverage
src/mime.rs — clean
src/raw.rs — clean
src/uuid.rs — clean
src/writer.rs — findings: 3
# Batch 4 findings — zimru (examples/writer_bench.rs, tests/*)

All files in this batch are example/dev/test code (not shipped library code). Findings are
correctness/maintainability issues in the test & benchmark harnesses; nothing reaches the
published library.

- [low] examples/writer_bench.rs:95 — `let len = (avg_kb * 1024 * jitter) / 4;` multiplies the argv-parsed `avg_kb` (usize, default 16, from `args.get(2).and_then(|s| s.parse().ok()).unwrap_or(16)`) by 1024 then by `jitter` (1..=16) with no `checked_*`; the doc comment says runs use `cargo run --release`, where integer overflow wraps silently. — a large `avg_kb` argument silently wraps `len` (and `make_body`'s `Vec::with_capacity(target_len + 16)` then allocates a wrong-size/giant buffer), so the benchmark measures a corrupted corpus instead of erroring — use `checked_mul`/`saturating_mul` on the argv-derived sizes (or reject oversized args).

- [low] tests/cffi_smoke.rs:67-73 — `unique_tmp` derives the temp filename from only `SystemTime::now().as_nanos()` + pid, with no per-call counter. The repo fixed the identical pattern in tests/ergonomic_api.rs:245 with an `AtomicU64` SEQ counter whose comment states "`SystemTime::now().as_nanos()` is not coarse enough to disambiguate two parallel test threads in the same process, which used to cause one test to delete another's tmp file mid-run". — under `cargo test` (parallel test binaries/threads) two tests can land on the same name and one's `std::fs::remove_file(&bin)`/`remove_file(&zim)` (lines 130/149/162) deletes the other's in-flight compiled binary or ZIM, causing a spurious failure — apply the same AtomicU64 counter (or the `tempfile` crate) to this helper.

- [low] tests/cffi_writer_smoke.rs:46-52 — `unique_tmp` uses the same nanos-only+pid temp-name scheme as cffi_smoke.rs (fixed in ergonomic_api.rs:245 with an AtomicU64 counter); here `std::fs::remove_file(&bin)` at :109 and `remove_file(&zim)` at :121 delete the named file. — two parallel tests colliding on the nanos value delete each other's compiled binary / ZIM mid-run → spurious failure — add the AtomicU64 counter to this helper.

- [low] tests/direct_access.rs:17-26 — `tmp_path` names temp ZIMs from nanos+pid only (same pattern fixed at ergonomic_api.rs:245); `std::fs::remove_file(&out)` at :59/:87/:108 removes the shared name. — parallel tests can collide and delete another test's ZIM mid-read → flaky failure — add the AtomicU64 counter.

- [low] tests/integrity_checks.rs:27-36 — `tmp_path` uses the nanos-only+pid scheme (fixed at ergonomic_api.rs:245); `std::fs::remove_file` on the shared names at :73/:87/:105/:125/:144/:177/:188/:195/:216/:223/:236/:243/:262. — a parallel test deleting a colliding temp file mid-run (the corruption tests read/write the same path) makes the suite flaky — add the AtomicU64 counter.

- [low] tests/per_item_compression.rs:21-33 — `tmp_path` nanos-only+pid scheme (fixed at ergonomic_api.rs:245); `remove_file(&out)` at :94/:126/:181. — parallel collision deletes another test's ZIM mid-run → flaky failure — add the AtomicU64 counter.

- [low] tests/streaming_encode_smoke.rs:20-26 — `tmp_path` nanos-only+pid scheme (fixed at ergonomic_api.rs:245); `remove_file(&out)` at :130/:179. — parallel collision deletes another test's ZIM mid-run → flaky failure — add the AtomicU64 counter.

- [low] tests/writer_roundtrip.rs:20-32 — `tmp_path` nanos-only+pid scheme (fixed at ergonomic_api.rs:245); `remove_file(&out)` at :134/:218/:254/:294/:337/:357/:395/:486/:499. — parallel collision deletes another test's ZIM mid-run → flaky failure — add the AtomicU64 counter.

- [low] tests/zimdump_analyze.rs:19-31 — `tmp_path` nanos-only+pid scheme (fixed at ergonomic_api.rs:245); `remove_file(&out)` at :94/:143/:187. — parallel collision deletes another test's ZIM mid-run → flaky failure — add the AtomicU64 counter.

- [low] tests/zimwriterfs_e2e.rs:80-92 — `tmp_dir` names its temp dir from nanos+pid only (fixed at ergonomic_api.rs:245); `fs::remove_dir_all(&site)` at :176/:221/:312 and `remove_file(&zim)` at :177/:222/:313/:314 remove the shared name. — two parallel tests colliding on the nanos value delete each other's site dir / ZIM mid-run → spurious failure — add the AtomicU64 counter to `tmp_dir`.

## Coverage
examples/writer_bench.rs — findings: 1
tests/cffi_smoke.c — clean
tests/cffi_smoke.cpp — clean
tests/cffi_smoke.rs — findings: 1
tests/cffi_writer_smoke.c — clean
tests/cffi_writer_smoke.rs — findings: 1
tests/direct_access.rs — findings: 1
tests/ergonomic_api.rs — clean
tests/integrity_checks.rs — findings: 1
tests/per_item_compression.rs — findings: 1
tests/real_files.rs — clean
tests/streaming_encode_smoke.rs — findings: 1
tests/synthetic_zim.rs — clean
tests/writer_roundtrip.rs — findings: 1
tests/xapianbuilder_helper.rs — clean
tests/zimdump_analyze.rs — findings: 1
tests/zimwriterfs_e2e.rs — findings: 1
# batch-5 findings

## Findings

- [low] src/bin/zimwriterfs.rs:509 — `creator.add_item(item)` panics on streaming-mode push error — after `start_writing` (line 250) every `add_item` goes through `Creator::add_item` which on a `push_item` error executes `panic!("streaming add_item: {e}")` (writer.rs:585-587); a disk-full / I/O failure mid-build aborts the process with a panic instead of the clean `Err(zimru::Error)` that `run`/`main` (lines 172-178) would print — use the error-propagating `push_streaming_item` (returns `Result`) and return it from `run`.
- [low] src/bin/zimrecreate.rs:359 — `creator.add_item(item)` panics on streaming-mode push error — same `add_item` panic path as above (writer.rs:585-587); an I/O error (e.g. full disk) during the recreate aborts the CLI with a panic rather than the `Err` path handled at main (lines 147-152) — replace with `push_streaming_item` and propagate the `Result`.
- [medium] src/cffi/creator.rs:271 — `(*inner).add_item(item)` can panic across the `unsafe extern "C"` boundary — `add_item` panics on any streaming-mode `push_item` error (writer.rs:585-587); a panic unwinding through an `extern "C"` function is UB / process abort, violating the C ABI's documented "return false and set `*err`" contract (e.g. `zimru_creator_add_item` on a full disk while in streaming mode) — in `zimru_creator_add_item` use `push_streaming_item` (returns `Result`) when streaming and map failure to `false` + `set_err`.
- [low] src/bin/zimwriterfs.rs:734 — `mime_for_path` rebuilds a fresh `HashMap<&str,&str>` of ~40 entries on every call — it is invoked once per ingested file at line 385 (streaming path) and line 466 (parallel batch), so a Wikipedia-scale build (millions of files) allocates millions of tiny HashMaps; replace the per-call `TABLE.iter().copied().collect()` with a `match` on the extension or a static linear scan (`TABLE.iter().find()`).
- [medium] src/bin/zimwriterfs.rs:455 — `--inflateHtml` silently does nothing — `flate2_decoder` (line 845) unconditionally returns `Err(Unsupported)`, so the `if let Ok(mut dec) = flate2_decoder(&content)` branch never inflates and gzipped `.html` files are packed as-is; the advertised "gunzip *.html" behavior (help line 198) is a silent no-op and title extraction on gzipped bytes fails; emit a warning/error when `-x` is set without flate2 enabled (or implement inflate) instead of silently passing through.
- [low] src/bin/zimwriterfs.rs:194 — `value_or_next` returns "" when a value-taking flag (`-w`, `-I`, `-t`, …) is the last argument with no following value — `args.get(*i).cloned().unwrap_or_default()` yields an empty String, and the mandatory-argument check (lines 144-158) only tests `.is_some()`, so `--welcome` with no value passes and `set_main_path("")` writes an empty main-page redirect; treat a missing flag value as a CLI error.
- [low] src/cffi/creator.rs:195 — raw-pointer copy `std::ptr::copy_nonoverlapping(uuid, bytes.as_mut_ptr(), 16)` has no `// SAFETY:` comment — the invariant (uuid points to 16 readable bytes) is stated only in the doc comment (line 180), so a C caller passing a shorter buffer is instant UB; add an explicit `// SAFETY:` justification (and likewise for `from_raw_parts` at lines 88 and 657 in `ptr_to_vec` / `zimru_creator_item_chunk`, whose length invariants are also only in doc comments).
- [low] src/bin/_index_helper.rs:288 — `write_raw` blocks forever on a hung xapianbuilder child — `stdin.write_all(buf)` has no timeout, so if the helper stops reading stdin (crash, hang) the caller (zimwriterfs/zimrecreate, which feed every entry) stalls indefinitely with no way to detect the dead child; write in a non-blocking loop / check `try_wait` or bound the write, and add a timeout to `finish()`'s `wait_with_output`.

## Coverage

- src/bin/zimwriterfs.rs — findings: 4
- src/cffi/creator.rs — findings: 2
- src/bin/zimrecreate.rs — findings: 1
- src/bin/_index_helper.rs — findings: 1
# Batch 6 findings — src/cffi/item.rs

## Findings

- [low] src/cffi/item.rs:100-114 — `zimru_item_size` calls `(*it).inner.size()`, which (archive.rs:1528) calls `get_data()` → `pinned_blob()` → `load_cluster()`, decompressing/copying the **entire cluster** (and `Blob` retains a clone of the whole `Arc<[u8]>` payload) just to return a length. For a large item in a big cluster (e.g. a multi-MB article, or a direct-access item in an uncompressed cluster) each size query pays O(cluster) decompression + a full payload copy. Consequence: a hot-path C consumer that asks for size before reading pays a full cluster decode/copy even when the bytes themselves are direct-access. Smallest safe fix: for direct-access (uncompressed) clusters return `archive().blob_direct_access(cluster, blob).size` without decoding; only decompress via `get_data` when `is_direct` is false.

- [low] src/cffi/item.rs:22-23 — `CString::new(it.path())` / `CString::new(it.mimetype())` use `.unwrap_or_else(|_| CString::new("").unwrap())`, silently substituting an empty string if the path or mimetype contains an interior NUL byte. Consequence: a C caller receives an empty `const char*` with no error, silently corrupting the returned path/mimetype. Smallest safe fix: propagate the `CString` error (e.g. return a null/error) instead of substituting `""`.

- [low] src/cffi/item.rs:104-105 — `zimru_item_size` returns `0` without setting `*err` on a NULL item, unlike every other fallible entry point in this file (`zimru_item_get_data` line 174, `zimru_item_blob_view` line 220, `zimru_item_warmup` line 265) which set `EntryNotFound` on NULL. Consequence: a C caller that checks only `*err` (not the return value) cannot distinguish a NULL item from a genuine zero-length item. Smallest safe fix: `set_err(err, crate::Error::EntryNotFound)` before returning 0 on a NULL `it`.

## Coverage
src/cffi/item.rs — findings: 3
