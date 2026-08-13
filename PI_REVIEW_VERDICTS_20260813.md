# Review verdicts — 2026-08-13

Reviewed a **clean `origin/main` clone** (HEAD `b6e9440`). Independent
verification (Claude subagents, adversarial re-read with exact
offset/ownership recomputation) of the two DeepSeek-V4-Pro sweep reports.

- `PI_REVIEW_BUGS_DEEPSEEK_V4_PRO_20260813.md` — 30 findings:
  **28 confirmed / 1 refuted / 1 partial / 0 unsampled** (all 30 checked).
- `PI_REVIEW_PERF_DEEPSEEK_V4_PRO_20260813.md` — 21 findings:
  **20 confirmed / 0 refuted / 1 partial / 0 unsampled** (all checked).

Both sweeps were unusually accurate for a Rust binary-format library — one
refutation and two right-bug/wrong-detail partials across 51 findings. The
right lens is **untrusted-archive safety** (bugs) and **large-archive memory /
throughput** (perf).

## Fix first — untrusted-input safety (bugs)

1. **[high] src/bin/zimdump.rs:474** — path traversal. `safe_path` (670) only
   trims a leading `/`, so a crafted ZIM with `../`-laden entry paths flows
   through `dir.join(rel)` → `fs::write` and writes arbitrary files outside
   `--dir`. Real arbitrary write on dump.
2. **[medium] src/cluster.rs:153/172** — decompression bomb, widest blast radius
   (whole reader lib). `decode_xz` grows via uncapped `read_to_end`;
   `decode_zstd` allocates the untrusted `declared_FCS` via `bulk::decompress`.
   Any crafted cluster → multi-GB alloc / OOM / abort. Cap decompressed size.
3. **[medium] src/bin/zimdump.rs:374** — `clusters[cluster_idx as usize]` indexes
   with an untrusted dirent field before the error path; `analyze --by-item`
   panics on a corrupt cluster index. Use checked `.get()`.
4. **[high] src/bin/zimrecreate.rs:284** — legacy (major=5, A/I/J/-) archives:
   the `ns==b'C'` gate silently drops all content Articles, so recreate is
   metadata-only and `finish_writing` then fails on the dangling `W/mainPage`
   redirect. (Namespace-normalize legacy dirents before gating.)
5. **[medium] src/cffi/archive.rs:718** — `zimru_archive_metadata` maps interior
   NULs to spaces on the fallback path while reporting the original length →
   silent binary-metadata corruption over the C ABI.
6. **[medium] src/writer.rs:1901** — the auto `M/Counter` dedup guard inspects
   only committed `self.dirents`, but finalize pushes caller metadata into
   still-pending buckets first → a duplicate `(M,Counter)` dirent (invariant
   violation / `finish_writing` error). Fix-advice note: the report's cited
   trigger is wrong — zimrecreate skips `M/Counter`; the real trigger is a
   streaming-API caller adding its own Counter.

Also confirmed: `add_metadata` discards the original mimetype for binary M
entries (`zimrecreate.rs:301`); `zimcheck touch_cluster` never parses clusters
despite its "every cluster parses" doc, so `-I`-only checks can false-Pass on
checksum-less archives (`zimcheck.rs:557`); `zimsplit` unchecked `n*mult`
overflow (`zimsplit.rs:102`); `zimbench -d 0` divide-by-zero panic
(`zimbench.rs:138`); CI actions unpinned + no `permissions:` block
(`ci.yml`); several `_index_helper` subprocess-pipe deadlock/leak edge cases.

## Fix first — large-archive memory / throughput (perf)

1. **[high] src/archive.rs:1043** — direct-access / `warmup` of a multi-GB
   uncompressed cluster (e.g. the Xapian index) does `Arc::from(body.to_vec())`
   (via `cluster.rs:73`), copying+caching the whole cluster and defeating
   zero-copy → OOM risk. Localized mmap-read fix; the compressed case can even
   be answered from the info-byte nibble without loading a cluster.
2. **[high] src/bin/zimdump.rs:379** — `analyze --by-item` decodes every cluster
   in pass 1, then calls `item.size()` → `pinned_blob` → `load_cluster` in URL
   order, thrashing the 64 MB LRU up to once per blob. The sibling `cmd_dump`
   already groups by cluster; retain per-blob sizes from pass 1.
3. **[medium] src/bin/zimrecreate.rs:232** — pass-1 holds 3 owned Strings per
   entry (`PendingContent`) for the whole archive → ~1.5–2 GB peak on a 10M-entry
   Wikipedia, undermining the writer's bounded-RSS design. Compact to
   `(cluster, blob, url_index)` and re-resolve in pass 2.
4. **[medium] src/bin/_index_helper.rs:241** — `std::fs::read` slurps the whole
   multi-GB Xapian DB then `add_item` buffers it again (~2× index in RAM); the
   streaming `begin_item`/`chunked_item_chunk` API already exists and is used
   for regular files.
5. **[medium] src/cffi/archive.rs:829** — `zimru_archive_metadata_key` re-runs
   the O(n) full-dirent scan on every call → O(k·n) (~200M Dirent::parse on 10M
   entries × 20 keys). Cache the key list (also the O(n) count at :816).

## Refuted / corrected — do NOT act on as stated

- **[medium bugs] src/writer.rs:2045** — REFUTED. The listing-dedup `.any(...)`
  guard runs *after* `flush_all_buckets()` (1919), which commits every bucket to
  `self.dirents`, so a caller-supplied `X/listing/...` IS detected — no
  duplicate. The report mis-applied the genuine Counter defect (whose guard at
  1901 precedes the flush) to the listing case.
- **[medium perf] src/bin/zimwriterfs.rs:774** — PARTIAL, wrong line. Line 774
  (`sniff_mime`) lowercases only a bounded 1 KB window (not a problem). The real
  full-body `to_ascii_lowercase` is `derive_title_from_bytes` at **:814**
  (per-HTML-file). Substance/fix correct; severity ~low (I/O+compress-bound
  build dominates).
- **[low bugs] src/bin/zimru.rs:220** — PARTIAL. The unbounded
  `set_cluster_cache_max_bytes(usize::MAX)` is real but an explicitly
  documented, intentional tradeoff for the `readall` dev tool — not a defect.
- **Fix-advice corrections:** `raw.rs:14`/`zimsplit.rs:102` overflow is
  debug-panic / release-wrapped (release stays memory-safe — the wrapped `end`
  makes `get(off..end)` return `None`), so it's hygiene not a live bug;
  `dirent.rs:114`'s "bind `url` once and reuse" still allocates twice (you can't
  move one String into two owned fields) — only the accessor-deferral removes
  the second alloc.

## Duplicates (one fix each)

Bugs: writer Counter/listing dedup (one real at 1901, one refuted at 2045);
zimrecreate namespace/target gating (:284 + :254); cffi hardcoded
`target/release` in two smoke tests; `_index_helper` pipe-deadlock (:288 + :318);
bench scripts missing `set -e` (3 files); CI hardening (2).
Perf: `blob_direct_access` whole-cluster copy (`archive.rs:1043` + `cffi/item.rs:155`
+ `warmup`); `get_metadata_keys` O(n) scan (`cffi/archive.rs:816` + :829);
decoded-payload copy (`cluster.rs:73` + `zimrecreate.rs:339`).

## Provenance & cost

Model `deepseek/deepseek-v4-pro:high` (api.deepseek.com), 4-wide live sweeps
over 57 files + refeed, on a clean `origin/main` clone. Bugs: $0.32. Perf:
$0.25. Repo total ≈ **$0.57** at 2026-08-13 list prices.
