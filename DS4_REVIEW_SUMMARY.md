# zimru — Executive Review Summary

Decision-ready analysis of the DS4 sweep (57 files, 6 batches). Severity re-weighted on
two axes — **exploitability** and **path-criticality** — for a Rust ZIM library whose threat
surface is (a) readers parsing untrusted archives, (b) the C FFI boundary, and (c) CI
supply-chain. This is analysis and prioritization, not a restatement of every finding.

---

## 1. Verdict

**Ship-blocking? No — but two items should be fixed before the next tagged release**: the
unpinned CI actions (supply-chain) and the `zimdump` path traversal (arbitrary file write from
an untrusted archive). Neither is exotic; both are cheap to fix.

The library core (parse/decode) is reasonably solid — most untrusted-input findings degrade to
**panic/DoS** rather than memory corruption, because Rust bounds-checks slices. The real
memory-safety exposure is concentrated at the **C FFI boundary**, which is small and auditable.
Overall this reads as a maturing codebase with good instincts (it already fixed the tmp-file race
once) that needs a hardening pass on the untrusted-input parse layer and an FFI-soundness audit.

| Severity | Count | Character |
|----------|-------|-----------|
| High     | **1**  | Supply-chain (CI) |
| Medium   | **9**  | Untrusted-archive DoS/traversal, FFI UB/abort, silent corruption |
| Low      | **29** | FFI hygiene, robustness, dev/bench scripts, test flakiness |
| **Total**| **39** | |

Of the 29 lows, **10 are test/bench-only** (9 are the *same* tmp-file pattern) and do not touch
shipped library code. The substantive surface is smaller than the raw count suggests.

---

## 2. Fix first (highest opportunity)

Ranked by exploitability × path-criticality. Supply-chain and genuinely-exploitable items lead.

### A. Pin CI actions to commit SHAs — supply-chain
- **[high] .github/workflows/ci.yml:18,30,46,58,69**
- **What/why:** `dtolnay/rust-toolchain@stable` and `Swatinem/rust-cache@v2` are mutable refs.
  A compromised (or repointed) upstream action runs with the repo's `GITHUB_TOKEN` on every push/PR.
  This is the classic, real supply-chain vector — highest leverage, lowest cost to fix.
- **Fix:** pin each to a full 40-char commit SHA (`…@<sha>`), optionally with a Dependabot/Renovate
  policy to bump the pins. Pair with the `permissions:` hardening in §4.

### B. `zimdump dump` path traversal — arbitrary file write from an untrusted ZIM  *(elevate: medium → treat as high)*
- **[medium] src/bin/zimdump.rs:516-519 (`safe_path`), sinks at :456, :497; exception path :658-667**
- **Verified in source.** `safe_path` only does `p.trim_start_matches('/')` — `..` components pass
  through untouched — then `dir.join(safe_path(e.path()))` writes the file. A ZIM entry named
  `../../etc/…` (or any `../` chain) writes **outside `--dir`**. `exception_dest` escapes `/`→`%2f`
  but leaves `..` intact. This is zip-slip: a malicious archive achieves arbitrary file write when a
  user runs `zimdump dump`. Higher real-world impact than any other code finding here.
- **Fix:** normalize each relative path and reject/clamp any component that escapes the root
  (resolve components, drop `..` that would ascend above `dir`; verify the final canonical path is
  still under `dir` before writing). Apply to both the normal and exception destinations.

### C. FFI illustrations free relies on an unguaranteed `shrink_to_fit` — potential UB  *(elevate: low → medium)*
- **[low] src/cffi/archive.rs:427-431, free at :441**
- **Verified in source.** `boxed.shrink_to_fit(); … mem::forget(boxed)`; the matching
  `zimru_illustrations_free` does `Vec::from_raw_parts(ptr, count, count)`. `shrink_to_fit` is
  **best-effort** — the API does not guarantee `capacity == len`. If capacity > count, reconstructing
  the Vec with the wrong capacity and dropping it is undefined behavior (heap corruption/double-free).
  In practice it usually shrinks for these small POD elements, which is exactly why this is a latent
  time-bomb rather than an obvious crash. It sits on the memory-safety FFI boundary → elevated.
- **Fix:** carry the exact triple. Use `Vec::into_raw_parts()` (or `Box::into_raw` +
  `slice_from_raw_parts`) and free with the real `(ptr, len, cap)`; do not infer capacity.

### D. Panic across the `extern "C"` boundary on streaming push error — process abort / ABI violation
- **[medium] src/cffi/creator.rs:271** (root cause: `add_item`'s `panic!` at src/writer.rs:585-587)
- **What/why:** `(*inner).add_item(item)` panics on any streaming `push_item` error (e.g. disk-full).
  Unwinding through `extern "C"` aborts the process, violating the documented "return `false`, set
  `*err`" contract — a hostile-input-independent but real reliability/ABI defect for embedders.
- **Fix:** in `zimru_creator_add_item`, use the error-propagating `push_streaming_item` (returns
  `Result`) when streaming and map failure to `false` + `set_err`. Fixes the FFI symptom of the
  systemic panic pattern in §3.

### E. Decompression-bomb memory exhaustion in `zimru readall`
- **[medium] src/bin/zimru.rs:220** — `set_cluster_cache_max_bytes(usize::MAX)` disables the 64 MB
  cluster-cache budget, so a crafted cluster that expands to GBs pins in RAM and never evicts.
  A malicious ZIM exhausts memory. **Fix:** keep a finite cap for untrusted files (default budget),
  or evict per entry. Cheap, and directly on the untrusted-input path.

### F. Silent data corruption when interning >65 535 mimetypes
- **[medium] src/writer.rs:965** — `mimes.len() as u16` wraps and collides the mime index; the ZIM's
  mime list and per-dirent `mime_idx` silently corrupt (no error). Low exploitability (needs a
  pathological *author*, not an attacker) but the output is a silently-bad published archive.
  **Fix:** return an `Error` when `mimes.len() >= u16::MAX`, or widen the index to `u32` and the
  dirent field.

### G. `zimwriterfs --inflateHtml` silently does nothing
- **[medium] src/bin/zimwriterfs.rs:455** — `flate2_decoder` (:845) always returns
  `Err(Unsupported)`, so the advertised "gunzip *.html" is a no-op; gzipped HTML is packed as-is and
  title extraction fails. Correctness/trust bug (advertised feature broken), not security.
  **Fix:** implement inflate, or hard-error when `-x` is passed without flate2 rather than silently
  passing through.

---

## 3. Systemic patterns (fix the root, not each instance)

**P1 — Untrusted header/dirent fields are trusted verbatim at the parse boundary.** A cluster of
medium/low findings all share one root cause: attacker-controlled offsets/counts from the ZIM header
are used in unchecked arithmetic and indexing, so a crafted file **panics or over-reads within the
mmap** (DoS on the reader tools, not memory corruption):
- src/bin/zimdump.rs:374 — dirent `cluster_idx` indexes `clusters[]` unchecked (OOB panic).
- src/bin/zimcheck.rs:1002 — `resolve_relative` `parts.pop()` on empty vec (panic on `..` link).
- src/archive.rs:279/291/296 — `checksum_pos + 16` unchecked `u64→usize` wrap defeats the bounds guard.
- src/archive.rs:349 (via zimcheck `-I`) — `title_ptr_pos + i*4` overflow → arbitrary in-mmap read.
- src/cluster.rs:92-96 — `blob_count = first/ptr_size - 1` from attacker offset → billions of
  probe iterations (CPU DoS).
- **Recommendation:** add a validation pass on `Archive`/`Cluster` construction that checks every
  header-derived offset/count against `mmap.len()` using `checked_add`/`checked_mul` and returns
  `Error::Truncated`. This closes the whole class and makes `zimcheck`/`zimdump` safe to point at
  hostile files — which is precisely their job.

**P2 — Streaming `add_item` cannot return an error, so every call site papers over it with a panic.**
`writer.rs:585-587` panics; the pattern then repeats at src/bin/zimwriterfs.rs:509,
src/bin/zimrecreate.rs:359, and (worst) src/cffi/creator.rs:271 (§2-D). A disk-full mid-build aborts
the process everywhere. **Recommendation:** thread the error — either store the first error on the
`Creator` and surface it from `finish_writing`, or make `add_item` return `Result` — and route all
call sites (and the error-propagating `push_streaming_item`) through it.

**P3 — FFI hygiene is uneven.** Beyond §2-C/§2-D: missing `// SAFETY:` on raw-pointer ops
(src/cffi/creator.rs:195, and `from_raw_parts` at :88/:657), silent empty-string substitution on
interior-NUL paths/mimetypes (src/cffi/item.rs:22-23), and inconsistent NULL handling —
`zimru_item_size` returns 0 without setting `*err` (src/cffi/item.rs:104-105) unlike its siblings.
**Recommendation:** one FFI-soundness pass — SAFETY comments on every `unsafe`, propagate `CString`
NUL errors instead of `""`, and make NULL handling uniform (set `*err` everywhere).

**P4 — Test tmp-file races (test-quality only).** Nine test helpers reuse the nanos+pid temp-name
scheme (cffi_smoke.rs, cffi_writer_smoke.rs, direct_access.rs, integrity_checks.rs,
per_item_compression.rs, streaming_encode_smoke.rs, writer_roundtrip.rs, zimdump_analyze.rs,
zimwriterfs_e2e.rs). The repo **already fixed this exact bug** in tests/ergonomic_api.rs:245 with an
`AtomicU64` counter. **Recommendation:** one mechanical sweep — reuse that counter or switch to the
`tempfile` crate. Flaky tests only; no shipped-code impact.

---

## 4. Lower priority / hygiene

- **[low] .github/workflows/ci.yml:14** — no `permissions:` block; jobs inherit the broad default
  token. Add `permissions: contents: read` (+ `packages: read`) at top level. Pairs with §2-A.
- **[medium] src/bin/zimbench.rs:114** — divide-by-zero panic on `zimbench -d 0`. Benchmark utility,
  not library; guard `random_urls.is_empty()`. (Model-rated medium; effective impact is low.)
- **[low] src/writer.rs:2514-2524** — `default_uuid()` from `SystemTime::now()`+pid, not a CSPRNG →
  predictable/colliding archive IDs. Use `getrandom`/`Uuid::new_v4`.
- **[low] src/bin/zimsplit.rs:169-175** — `advance_suffix` has no rollover past 26 parts (`zz`→`{a`),
  producing unrecognizable part names. Extend to variable-length suffixes.
- **[low] src/bin/zimwriterfs.rs:194** — `value_or_next` yields `""` for a trailing value-flag, so
  `--welcome` with no value silently sets an empty main path. Treat missing value as a CLI error.
- **[low] src/cffi/item.rs:100-114** & **src/bin/zimwriterfs.rs:734** — perf: `zimru_item_size`
  decodes+copies a whole cluster just to return a length (use direct-access size when uncompressed);
  `mime_for_path` rebuilds a ~40-entry `HashMap` per file (use a `match`/static scan). Matter at
  Wikipedia scale.
- **[low] src/bin/_index_helper.rs:288** — `write_raw` can block forever on a hung xapianbuilder
  child (no timeout on `write_all`/`wait_with_output`). Bound the write / add a timeout.
- **[low] build.rs:14-24** — generates `include/zimru.h` into the git-tracked source tree, dirtying
  the tree and breaking clean-build isolation. Emit into `OUT_DIR` (deliberate today; at least
  document).
- **[low] bench/parity.sh:95-112,135-136** & **bench/cache_aware_compare.sh:100-141,157** —
  shell-injection via unquoted `$ZIM`/`$UPSTREAM_DIR`/`$SITE` interpolated into `bash -c` strings.
  **Dev-only harnesses, not CI** — low real risk; use argv arrays or reject quotes/whitespace.
- **[low] examples/writer_bench.rs:95** — unchecked `avg_kb*1024*jitter` overflow (example code).

**`set -e` / `pipefail` check (explicitly requested): clean.** I inspected all five shell scripts.
None has `set -e` without `pipefail`: `bench/run.sh` and `bench/cache_aware_compare.sh` use
`set -euo pipefail`; `bench/parity.sh`, `bench/recreate-bench.sh`, and `bench/recreate-suite.sh` use
`set -uo pipefail` (no `-e`, but `pipefail` is present). No action needed.

---

## 5. Caveats

- **Model-generated sweep (DeepSeek).** Findings and severities were assigned by an automated model,
  not exhaustively source-verified. Line numbers and reasoning may drift from the current tree.
- **Severity is my two-axis re-weighting**, differing from the raw labels where the model under- or
  over-rated impact. Notably I **elevated** the FFI illustrations-free UB (§2-C, labeled low) and the
  `zimdump` traversal (§2-B, labeled medium), and note the `zimbench` divide-by-zero (labeled medium)
  is effectively low.
- **Verified directly in source for this summary:** the CI unpinned actions (§2-A), the `zimdump`
  `safe_path` traversal (§2-B — `safe_path` at src/bin/zimdump.rs:516-519 confirmed to pass `..`
  through), the FFI `shrink_to_fit` reliance (§2-C — confirmed at src/cffi/archive.rs:427-441), and
  the shell-script `pipefail` status (§4). The remaining items are **not** independently confirmed.
- **Before acting, verify the leads in §2** (re-read each cited span) — especially the path-traversal
  and FFI-memory items, which are the ones that warrant a fix before the next release.
- **No committed-credential findings** surfaced in this sweep. Had any appeared, the guidance would be
  verify-first, then rotate and move to an env var — not an automatic high.
