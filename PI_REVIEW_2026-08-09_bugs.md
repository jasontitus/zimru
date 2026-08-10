# Pi sweep review — zimru-c9bc4e8a

Exhaustive per-file pass: 57 code files across 6 batches.

## Findings

# Pi sweep — batch-1 findings

## Findings

- [medium] src/archive.rs:1144 — `cluster_uncached` / `load_cluster` feed untrusted, attacker-controlled cluster bytes to an unbounded decompressor: `decode_zstd` pre-allocates `zstd::bulk::decompress(body, size)` using the frame header's declared `pledgedSrcSize` with no upper cap, and `decode_xz` runs `read_to_end` with no size bound (both in src/cluster.rs, reached via this read path). A crafted/malicious ZIM can declare a huge FCS (or emit an xz bomb) so that merely calling `get_data`/`cluster` on one item allocates many GB and OOMs/aborts the host process. There is no `max` bound anywhere on this path. — concrete consequence: remote DoS (memory exhaustion / abort) on any process that opens an untrusted archive and reads a single item. — smallest safe fix: when decoding, cap the output size (e.g. reject `pledgedSrcSize` > some multiple of the file size or a documented max, and bound `read_to_end` with a `take(max)`), returning `Error::Decompression` instead of allocating. The same reasoning applies to the untrusted header count sizes used in `title_listing` (legacy `Vec::with_capacity(entry_count)`) and the iteration spans driven by `entry_count`/`cluster_count`, which are also unbounded against a crafted header.

- [medium] src/archive.rs:1041 — `blob_direct_access` calls `self.cluster(cluster_idx)` (→ `load_cluster` → `Cluster::parse`), which for an uncompressed cluster copies the *entire* cluster body into a heap `Arc<[u8]>` (`Arc::from(body.to_vec())`) and inserts it into the LRU cache — purely to return the blob's `file_offset`/`size`. This defeats the documented zero-copy direct-access design (the doc says "without copying any bytes") and on large uncompressed clusters (exactly the multi-GB Xapian fulltext DBs this API exists for, e.g. `warmup()`) forces a full heap duplicate of the whole cluster plus LRU-cache pollution/eviction churn on every call. — concrete consequence: memory spike and cache thrash on the direct-access/warmup path that is supposed to be zero-copy. — smallest safe fix: compute the blob's file range directly from the cluster's on-disk info byte and offset table without materialising the payload (read the offset table from the mmap slice `cluster_raw`), or parse the header only instead of the full decompressed payload.

## Coverage
.github/workflows/ci.yml — clean
Cargo.toml — clean
bench/cache_aware_compare.sh — clean
bench/parity.sh — clean
bench/recreate-bench.sh — clean
bench/recreate-suite.sh — clean
bench/run.sh — clean
benches/cluster_decode.rs — clean
benches/get_entry_by_path.rs — clean
benches/warmup.rs — clean
build.rs — clean
cbindgen.toml — clean
include/zimru.h — clean
src/archive.rs — findings: 2
src/bin/_index_helper.rs — clean
src/bin/zimbench.rs — clean
# Pi sweep review — batch-2

## Findings

- [high] src/bin/zimdump.rs:513,516,474,670 — `cmd_dump` writes every archive entry to `dir.join(rel)` where `rel = safe_path(e.path())`, and `safe_path` only strips leading slashes (`p.trim_start_matches('/')`) without normalizing `..` components or rejecting absolute/drive paths. A malicious ZIM whose entry URL is `../escape/foo` (or `../../tmp/pwned`) is written verbatim via `fs::create_dir_all(parent)` + `fs::write` outside `--dir`, i.e. a zip-slip-style arbitrary-file-write. The `_exceptions/` fallback does not help (it is reached only after the traversal already happened, and its path is also under `dir`). — sanitize the entry URL before writing: reject any path containing `..` components or that resolves outside `dir` (e.g. build the relative path with `Component::Normal`-only segments), and skip/error on escapes.

- [medium] src/bin/zimcheck.rs:553 — `touch_cluster(arc, _c)` ignores the `_c` cluster index and merely calls `arc.entry_by_url_index(0)`; the `check_integrity` loop `for c in 0..h.cluster_count { touch_cluster(arc, c)? }` therefore does not actually open/parse the clusters it claims to validate, so the `-I` integrity check reports "structure valid" even when a cluster's info byte, pointer table, or blob bounds are corrupt (the code comment concedes the false-negative). The advertised "every cluster parses" guarantee is not upheld. — make `touch_cluster` actually load cluster `_c` via `cluster_uncached(_c)` (and walk its blob ranges) instead of ignoring the argument.

- [medium] src/bin/zimwriterfs.rs:456,845 — `--inflateHtml`/`-x` is accepted and documented, but `flate2_decoder` is a stub that always returns `Err(Unsupported)` and the `if let Ok(...)` swallows that, so gzipped `*.html` files are never decompressed and get packed raw (with `.html` extension ⇒ mime `text/html`) into the archive, silently corrupting the content/links of an advertised feature. — either wire in an actual gzip decoder or reject `--inflateHtml` at startup instead of silently no-oping.

- [medium] src/cffi/archive.rs:407-441 — `zimru_archive_illustrations` allocates a `Vec`, calls `shrink_to_fit()`, `mem::forget`s it, and hands out `as_mut_ptr()`; the matching `zimru_illustrations_free` rebuilds it with `Vec::from_raw_parts(ptr, count, count)`. Reconstructing a Vec with `capacity == count` is only sound if the underlying allocation's capacity is exactly `count`, which `shrink_to_fit` does **not** guarantee (allocators may round up), so the layout passed to the deallocator on `drop` can mismatch the real allocation — UB / potential invalid free from the C side. — store the Vec in the opaque handle (or return `count` copies / a separately-allocated buffer) so capacity is never assumed.

- [low] src/bin/zimru.rs:316 — in `cmd_show` the `--url` branch defaults the namespace to legacy `b'A'` (`arc.entry_by_ns_path(opts.ns.unwrap_or(b'A'), url)`), while the `--idx` branch of the same command and `cmd_list` default to `b'C'`. On modern archives (`C/M/W/X`) `zimru show --url=foo <file>` cannot resolve content and returns EntryNotFound. — use `b'C'` for consistency with the rest of the CLI.

- [low] src/bin/zimwriterfs.rs:691,734 — `mime_for_path` rebuilds the `HashMap<&str,&str>` from `TABLE` on every single call, and it is invoked once per ingested file in the hot per-item loop (tens of thousands to millions of files on real builds), adding avoidable per-file allocation. — hoist the table into a `static`/lazy map (or `match` on the extension) so it is built once.

## Coverage
src/bin/zimcheck.rs — findings: 1
src/bin/zimdump.rs — findings: 1
src/bin/zimrecreate.rs — clean
src/bin/zimru.rs — findings: 1
src/bin/zimsplit.rs — clean
src/bin/zimwriterfs.rs — findings: 2
src/cffi/archive.rs — findings: 1
src/cffi/blob.rs — clean
src/cffi/creator.rs — clean
src/cffi/entry.rs — clean
src/cffi/error.rs — clean
src/cffi/item.rs — clean
src/cffi/mod.rs — clean
src/cffi/uuid.rs — clean
src/cluster.rs — clean
src/dirent.rs — clean
src/error.rs — clean
src/header.rs — clean
src/io_hints.rs — clean
src/lib.rs — clean
# Pi review — batch-3

## Findings

- [medium] src/writer.rs:1901-1919 — The auto-generated `M/Counter` histogram and its caller-supplied dedup check are evaluated against `self.dirents` *before* `flush_all_buckets()` (step 2b) is called, so in a buffered build (the `write_to` path, where all content items and metadata are still sitting un-flushed in buckets) `self.dirents` is empty at this point. `build_counter_string(&self.mimes, &self.dirents)` therefore emits an empty `"mime=count"` string for every archive built through the plain buffered API (write_to/buffered add_*), so zimcheck's mime report and kiwix stats read an empty histogram instead of real counts. Independently, the same mis-ordering makes the `counter_already_set` dedup ineffective for the documented use case (zimrecreate forwarding a source ZIM's `M/Counter` via `add_metadata`): that entry is also still in a bucket, not in `dirents`, so `counter_already_set` is false and the writer emits a *second* `M/Counter` dirent, leaving two dirents with the same (ns,url) key. Fix: run `flush_all_buckets()` (step 2b) first, then check `counter_already_set` and push the auto Counter, then flush once more so the Counter dirent commits before the final sort — mirroring how the `X/listing/titleOrdered/v1` dedup at step 6b is already done *after* the flush at 2b.

- [low] src/writer.rs:1398,1701 — Streaming-encode items write per-item scratch files `.zimru-stream-<pid>-<idx>-<nanos>.tmp` into the output directory, and `drain_streaming_tasks` removes them only best-effort on the successful path. On any error returned from finalize (an encode/IO failure, a size-mismatch, or a process kill), the multi-GB temp files are left behind in the output directory with no cleanup, so a failed build can litter arbitrarily large `.zimru-stream-*.tmp` files. Fix: register the temp paths and remove them on the error path (e.g. guard the finalize body so completed temp files are deleted even when a later splice/encode step fails, or create them in the platform temp dir and document the leftover), and/or create them with `O_EXCL` semantics so a worst-case filename collision cannot truncate a user file.

## Branch
(no branch created; read-only review)

## Coverage
src/mime.rs — clean
src/raw.rs — clean
src/uuid.rs — clean
src/writer.rs — findings: 2
# Pi sweep — batch 4 (examples + writer/reader integration tests)

## Findings

- [low] tests/real_files.rs:127-140 — Cluster-touch loop records a permanent `Compression::None` placeholder for every cluster, with a comment claiming it will be "overwrite[n] below", but nothing ever overwrites it. The stated goal of the block ("Tracks observed compression types for reporting") is not met and the value stored is always `None`; only the map's `.len()` (unique-cluster count) is meaningful. This is lying/misleading dead code: a future reader of the test would believe compression-type stats are being collected. — either actually derive the cluster's compression (re-parse or via an archive accessor) or drop the placeholder and use a `HashSet<u32>` for the unique-cluster count.

Note: this file also uses `Compression` import solely for the placeholder dead value; simplifying removes the misleading claim.

## Coverage
examples/writer_bench.rs — clean
tests/cffi_smoke.c — clean
tests/cffi_smoke.cpp — clean
tests/cffi_smoke.rs — clean
tests/cffi_writer_smoke.c — clean
tests/cffi_writer_smoke.rs — clean
tests/direct_access.rs — clean
tests/ergonomic_api.rs — clean
tests/integrity_checks.rs — clean
tests/per_item_compression.rs — clean
tests/real_files.rs — findings: 1
tests/streaming_encode_smoke.rs — clean
tests/synthetic_zim.rs — clean
tests/writer_roundtrip.rs — clean
tests/xapianbuilder_helper.rs — clean
tests/zimdump_analyze.rs — clean
tests/zimwriterfs_e2e.rs — clean
# Pi sweep — batch 5

## Findings

- [high] src/bin/zimrecreate.rs:250-302 — Pass 1 only handles entries whose namespace is `C` or `M`; every other namespace is silently dropped. `Dirent::Article` handles `ns == b'C'` (pushed to `pending`) and `ns == b'M'` (metadata), with **no `else` branch** for any other namespace; `Dirent::Redirect` is only rebuilt when `ns == b'C'`. zimru's `Archive` explicitly supports legacy archives where content lives in `A/` (articles) and `I/`/`J/` (images/files) (see archive.rs NS_ARTICLES_LEGACY = b'A' and the split-legacy-path helpers), and `iter_by_path()` walks all of them. Running `zimrecreate` on any legacy-format source therefore produces an output archive with essentially the entire article/image set silently missing (content loss), and all non-C redirects dropped too. — Add an `else` arm for legacy/other namespaces: map `A`->content (push to `pending` like C) and `I`/`J`/other articles to content items as well, and rebuild redirects in any content namespace, not just `C`.

- [low] src/bin/zimrecreate.rs:97 — `--cluster-size` parse failure is silently swallowed (`args.get(i).and_then(|s| s.parse().ok())` falls back to `None`/default), unlike `--compression-level` which errors with exit code 2. A user typo like `--cluster-size 2MB` silently proceeds with the default 2 MiB target instead of failing. — Return an error (like `--compression-level`) when the value fails to parse.

- [low] src/bin/zimrecreate.rs:203-209 — After `IndexHelper::spawn` (which spawns two live `xapianbuilder` child processes) and `make_index_tmp_dir` succeed, many later steps use `?` (`entry?` on iter, `source.entry_by_url_index(r.redirect_index)?`, `entry.get_item(false)?.bytes()?`, `source.cluster_uncached(cidx)?`, `cluster.blob(p.blob)?`). Any of these errors returns from `run` early, dropping `indexer` without waiting: the spawned children are never reaped (their `ChildStdin` is dropped so they see EOF and exit, leaving zombies), and the `.`<dst>`.xapianbuilder.<pid>` temp dir is never removed (cleanup only happens after `finish`). — Create a guard that reaps/cancels the child jobs and removes `index_tmp` on the error path (e.g. wrap `run` so cleanup happens on early return).

## Coverage

include/zimru.h — clean
src/cffi/creator.rs — clean
src/bin/zimrecreate.rs — findings: 3
src/bin/_index_helper.rs — clean
# Pi sweep — batch-6

## Findings

(none)

## Coverage
src/cffi/item.rs — clean
src/dirent.rs — clean

## Run stats

input 608352 tok (+1917952 cached), output 62619 tok, cost $0.1 — 63 files in 27m (135.6 files/h, 4.6 min/batch)
