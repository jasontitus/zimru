# Disposition of the DS4 sweep — zimru

Every finding in [`DS4_REVIEW.md`](./DS4_REVIEW.md), checked against the tree
rather than taken on trust. The sweep was written against an earlier commit,
so a number of its findings had already been closed by the 2026-08-13 round
(see [`PI_REVIEW_FIXES_20260813.md`](./PI_REVIEW_FIXES_20260813.md)); those
are marked **Already fixed** with the evidence, not silently ticked off.

One finding is **refuted**.

## Fixed here

| Finding | What was wrong | Fix |
|---|---|---|
| [low→medium] `src/cffi/archive.rs:427` | `zimru_archive_illustrations` called `shrink_to_fit()` and then `mem::forget`, so `zimru_illustrations_free` could rebuild the allocation with `Vec::from_raw_parts(ptr, count, count)`. `shrink_to_fit` is explicitly best-effort: when it leaves capacity > len, that reconstruction is **undefined behaviour** on free. | Collect straight into `Box<[T]>`, whose allocation is *guaranteed* to be exactly the element count, and free the mirror-image `Box::from_raw(slice_from_raw_parts_mut(..))`. |
| [low] `src/archive.rs:279,291` | `pos + 16 > mmap.len()`, where `pos` is the header's `checksum_pos` — attacker-controlled. For `pos` near `usize::MAX` the add wraps, the bounds test passes, and the slice below panics. `zimcheck -C` on a crafted archive aborts instead of reporting a truncated file. | `checked_add(16)`, returning the normal truncated-file error on overflow. |
| [low] `src/archive.rs:349` | `base + i * 4` in `title_listing`, with both `base` (`title_ptr_pos`) and the loop bound (`entry_count`) taken from the header. Overflow wraps to an in-bounds offset, so the read silently returns *the wrong table* rather than failing. Reachable from `zimcheck -I` and `zimru titles`. | `checked_mul` / `checked_add`, erroring on overflow. |
| [low] `src/bin/zimsplit.rs:174` | `advance_suffix` incremented the leading byte unconditionally, so part 27 was named `…{a` (`z` + 1) and everything past that walked out of the alphabet — filenames no concatenating tool recognises, produced silently. A 2 GB `--size` on a 60 GB archive needs 30 parts. | Variable-length suffix: `az`→`ba`, `zz`→`aaa`. Unit tests cover the two rollover points; verified end-to-end by splitting into 35 parts and confirming `cat` reproduces the source byte-for-byte. |
| [medium] `src/cffi/creator.rs:271,322` | Both call `Creator::add_item`, which **panics** on a streaming-mode write error (full disk, I/O error mid-encode). A panic unwinding out of an `extern "C"` function is undefined behaviour, and it bypasses the C ABI's documented "return `false`, set `*err`" contract. | New fallible `Creator::try_add_item`; `add_item` now delegates to it so the two cannot drift. Both FFI entry points map the error onto `false` + `*err`. |

## Already fixed before this sweep was read

| Finding | Evidence |
|---|---|
| [high] `ci.yml:18,30,46,58,69` — unpinned actions | All `uses:` lines carry full 40-char SHAs (`actions/checkout@11d5960…`, `dtolnay/rust-toolchain@4360b52…`, `Swatinem/rust-cache@6323deb…`). |
| [low] `ci.yml:14` — no `permissions:` block | Present at workflow level. |
| [medium] `zimbench.rs:114` — `-d 0` divide-by-zero | Guarded; phase 3 skips with a message. |
| [medium] `zimdump.rs:670` — `dump` path traversal | `safe_path` rebuilds from segments, dropping empty / `.` / `..`. Hardened further here: `exception_dest` now also refuses a `rel` that is exactly `.` or `..`, which survived the `/`-escaping. |
| [medium] `zimdump.rs:374` — unchecked `clusters[cluster_idx]` | `.get()` with a skip on out-of-range dirent values. |
| [low] `cluster.rs:92` — `blob_count` DoS from a crafted first offset | `first == 0 \|\| first as usize > payload.len()` is rejected before `blob_count` is derived, so the count is bounded by the payload. |
| [medium] `writer.rs:965` — `intern_mime` u16 wrap | Explicit error at `MIME_DELETED` (0xFFFD) distinct mimetypes. |
| [medium] `zimwriterfs.rs:455` — `--inflateHtml` silent no-op | Implemented with flate2; gzip magic is detected and inflated, corrupt input is a hard error naming the path. |

## Refuted

**[medium] `src/bin/zimcheck.rs:1002` — `resolve_relative` panics on `..` at
the root.** It does not. The finding assumes `Vec::pop()` panics when the
vector is empty; it returns `Option` and is a no-op. A `..` that would
escape the root is therefore already clamped, which is the wanted behaviour.
No change made.

## Accepted as-is

| Finding | Why |
|---|---|
| [low] `build.rs:14` — generated header written into the source tree | Deliberate: `include/zimru.h` is committed so C consumers get it without running cargo. Noted in `build.rs`. |
| [low] `bench/parity.sh`, `bench/cache_aware_compare.sh` — unquoted interpolation into shell strings | Developer harnesses, not CI, run against paths the developer chooses. Worth tidying, not a vulnerability in the shipped artefact. |
| [low] test-file findings (`tests/*.rs` temp-dir patterns, ×9) | Test hygiene; they do not touch shipped library code. Tracked, not fixed here. |
| [low] `zimrecreate.rs:339` + `cluster.rs:73` — extra memcpy of decoded payload | Fixing it means threading lifetimes through `Cluster`/`Item`/encode — a refactor, not a localized change. Already documented in place, and the worst case (direct access to large uncompressed clusters) was eliminated by the `raw_blob_range` work. |
