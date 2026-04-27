# Shim-side wiring for the writer C ABI (zimru round 4)

zimru round 4 (`claude/libzim-shim-work-d3Rh4`, commit `6388b8a`) ships
the writer C ABI surface that `docs/SHIM_WORK.md` "Writer-side gap"
asked for. With this, the shim's `src/writer/creator.cpp` no longer
needs to throw from `finishZimCreation` — it can forward each buffered
piece of state to a `zimru_creator_*` call and then `zimru_creator_write_to`
to materialise the file. Once that wiring lands, the four zim-tools
writer-side binaries (zimrecreate, zimwriterfs, zimdiff, zimpatch) build
and run through the shim with no source changes, completing the
seamless libzim → zimru swap-in for the full zim-tools 3.6.0 suite.

Header: regenerated `include/zimru.h` (built when the `cffi` feature is
on). Symbols below are available on `libzimru.so` / `libzimru.dylib` /
`zimru.dll` after a `cargo build --release --features cffi`.

## What the zimru C ABI now exposes

All fallible functions follow the same out-pointer error protocol as
the rest of the C ABI: return `false` (or NULL) on failure, write a
heap-allocated `zimru_error_t*` through `**err`, the caller frees it
with `zimru_error_free`. NULL-safe where it makes sense (free, etc.).

### Lifecycle

```c
zimru_creator_t* zimru_creator_new(void);          // always succeeds
void             zimru_creator_free(zimru_creator_t* c); // NULL-safe
```

`zimru_creator_new` returns a creator with default settings (Zstd
level 3, 2 MiB cluster target, time-and-pid-seeded UUID). The shim's
`zim::writer::Creator` constructor maps to this.

### Configuration (call before any `add_*`)

```c
bool zimru_creator_set_compression(zimru_creator_t*,
                                   uint8_t compression_id,
                                   zimru_error_t** err);
// 1 = none, 4 = xz, 5 = zstd. Other IDs error with
// zimru_error_code::UnsupportedCompression.

bool zimru_creator_set_compression_level(zimru_creator_t*,
                                         int32_t level,
                                         zimru_error_t** err);
// Algorithm-specific. Zstd 1..=22, xz 0..=9, ignored for none.

bool zimru_creator_set_cluster_size_target(zimru_creator_t*,
                                           uintptr_t bytes,
                                           zimru_error_t** err);
// Default 2 MiB, matches zimwriterfs / zimrecreate.

bool zimru_creator_set_uuid(zimru_creator_t*,
                            const uint8_t uuid[16],
                            zimru_error_t** err);
// Bytes are copied; caller retains ownership of the input.

bool zimru_creator_set_main_path(zimru_creator_t*,
                                 const char* main_path,
                                 zimru_error_t** err);
// Writes a W/mainPage redirect to C/<main_path> at finalize time.
```

Configurations handled at the shim layer (no zimru primitive needed —
zimru exposes the surface the shim builds on, and the shim does the
domain work):

* `configIndexing(bool, lang)` — implemented shim-side via
  `src/text_indexer.{h,cpp}` (Xapian writer + compact-to-single-file
  Glass). On `finishZimCreation` the shim emits the compacted blob as
  a single ZIM entry at `X/fulltext/xapian` via
  `zimru_creator_add_item` — see the X/ prefix shortcut below.
* `configNbWorkers(unsigned)` — zimru's writer-side parallelism is
  governed by `--threads N` / `RAYON_NUM_THREADS` rather than this
  hint; the shim swallows it.
* `configVerbose(bool)` — zimru's writer doesn't emit progress.

### Adding content

```c
bool zimru_creator_add_item(zimru_creator_t*,
                            const char* path,
                            const char* title,
                            const char* mimetype,
                            const uint8_t* content,
                            uintptr_t content_len,
                            zimru_error_t** err);
// User-content namespace (C/) by default. Bytes are copied. Empty
// payloads (content_len == 0, content can be NULL) are permitted.
//
// Namespace-prefix shortcut: a path of the form "X/<rest>" (with
// non-empty <rest>) routes the item to the X namespace with the
// dirent URL set to <rest>. This is the surface the shim's fulltext-
// index emission uses — "X/fulltext/xapian" becomes
// {ns:'X', url:"fulltext/xapian"} so reader-side xapian_loader.cpp
// finds it. Duplicate
// paths surface as a dirent-order failure at write_to time.

bool zimru_creator_add_item_in_namespace(zimru_creator_t*,
                                         uint8_t namespace,
                                         const char* url,
                                         const char* title,
                                         const char* mimetype,
                                         const uint8_t* content,
                                         uintptr_t content_len,
                                         zimru_error_t** err);
// Explicit-namespace variant. `namespace` is the single-byte ZIM
// namespace identifier ('C', 'M', 'W', 'X', 'Z', …); `url` is used
// verbatim as the dirent URL — no prefix peeling. Use this when the
// caller already knows the target namespace (e.g. a future title-
// suggestion DB at X/title/xapian) or needs to write into a namespace
// other than C/X.

bool zimru_creator_add_metadata(zimru_creator_t*,
                                const char* name,
                                const char* mimetype,
                                const uint8_t* content,
                                uintptr_t content_len,
                                zimru_error_t** err);
// M/<name>. The mimetype is recorded verbatim — pass
// "text/plain;charset=utf-8" for textual metadata (Title, Language,
// Description, …) or "image/png" for legacy-archive favicons.

bool zimru_creator_add_illustration(zimru_creator_t*,
                                    uint32_t side,
                                    const uint8_t* png,
                                    uintptr_t png_len,
                                    zimru_error_t** err);
// M/Illustration_<side>x<side>@1. Square only — non-square inputs
// the shim already rejects.

bool zimru_creator_add_redirection(zimru_creator_t*,
                                   const char* path,
                                   const char* title,
                                   const char* target_path,
                                   zimru_error_t** err);
// C/<path> → C/<target_path>. Resolved at finalize time; an
// unresolved target is a hard write_to failure.

bool zimru_creator_add_alias(zimru_creator_t*,
                             const char* path,
                             const char* title,
                             const char* target_path,
                             zimru_error_t** err);
// Today wired as a redirection (the shim degrades gracefully here
// per SHIM_WORK.md). True alias semantics (type-preserving copy of
// the target dirent) wait on a follow-up zimru release.
```

### Finalize

```c
bool zimru_creator_write_to(zimru_creator_t*,
                            const char* path,
                            zimru_error_t** err);
// Materializes the buffered work to `path`. Consumes the inner
// Creator — subsequent set_*/add_*/write_to calls on the same
// handle return false with *err set ("already finalized — call
// free, not write_to/add"), rather than silently producing a
// second corrupt archive.
```

The spec calls this `zimru_creator_finalize`; it landed under the
existing C symbol name `zimru_creator_write_to` (matching the Rust
`Creator::write_to` it wraps and the existing stub). Semantically
it's the spec's `_finalize` — same shape, same consume-on-success.

## What to change in `src/writer/creator.cpp`

The shim already buffers items, redirections, metadata, and
illustrations during the `startZimCreation` … `addItem` /
`addRedirection` / `addMetadata` / `addIllustration` … window. Per
SHIM_WORK.md, "the shim's `finishZimCreation` implementation is a
one-call wiring change — no header refactoring, no zim-tools rebuild
required." Concretely, in the `finishZimCreation` path:

1. Allocate a `zimru_creator_t*` via `zimru_creator_new()`.
2. Translate the buffered `Creator::configCompression` /
   `configClusterSize` / `setMainPath` / `setUuid` settings into
   `zimru_creator_set_*` calls. Map libzim's `Compression` enum to
   the ZIM info-byte ID:

   ```cpp
   switch (libzimCompression) {
     case Compression::None:    id = 1; break;
     case Compression::Xz:      id = 4; break;
     case Compression::Zstd:    id = 5; break;
     // Compression::Zlib / Bzip2 are deprecated — error or pick zstd.
   }
   ```

3. Replay the buffered items / redirections / metadata /
   illustrations through `zimru_creator_add_*`. For each call, if the
   bool return is false, surface the `zimru_error_t*` message in a
   thrown `ZimFileFormatError` (or whatever the shim's error type is)
   and stop replaying.
4. `zimru_creator_write_to(c, output_path, &err)`. On success, the
   `zimru_creator_t*` is in its consumed state — call
   `zimru_creator_free(c)` and return success.
5. On any failure, free the creator and propagate the error. The
   partially-written file at `output_path` should be unlinked by the
   shim if it exists (zimru's writer doesn't clean up after itself
   on a mid-write error).

That's the entire change. The `zim::writer::*` headers don't move,
the `addItem` / `addMetadata` / etc. APIs don't change, and the four
writer-side zim-tools binaries don't need a rebuild.

## Gotchas

* **`add_metadata` mimetype is now per-entry**, not hard-coded.
  Pass `"text/plain;charset=utf-8"` for the typical case (Title,
  Language, Description, Date, Creator, Publisher, Name, Source,
  Tags, Flavour, Scraper, LongDescription) and `"image/png"` for
  legacy-archive favicons stored as metadata. Whatever you pass is
  written verbatim into the dirent's mimetype index.

* **`add_alias` is wired as `add_redirection` for now.** If the shim
  needs true alias semantics (preserving the target's mimetype +
  cluster/blob pointers under a new path/title), file a follow-up
  zimru issue. Until then, callers see a redirect — readable, just
  not byte-identical to libzim's behaviour.

* **Fulltext / title Xapian index at write time** — implemented
  shim-side via `text_indexer.{h,cpp}` (Xapian writer + compact-to-
  single-file Glass), emitted through `zimru_creator_add_item` with
  the `"X/fulltext/xapian"` prefix shortcut. See the configuration
  notes earlier in this doc. Title-suggestion DB
  (`X/title/xapian`) is the next follow-up — same path through
  `zimru_creator_add_item_in_namespace('X', "title/xapian", …)`
  once the shim grows that emission.

* **Streaming `ContentProvider` handoff is not implemented.** The
  shim should slurp each `ContentProvider` into a single `Vec<u8>`
  (well, `std::vector<uint8_t>` C++-side) before calling
  `zimru_creator_add_item`. Memory cost is bounded by the largest
  individual item. If huge media items start mattering this becomes
  a follow-up — the shim's API doesn't change, just the internal
  call pattern.

* **Post-finalize calls fail loudly.** Once `zimru_creator_write_to`
  returns successfully, the inner `Creator` has been consumed by
  `Option::take` and any further `set_*` / `add_*` / `write_to` on
  the same handle returns `false` with `*err` set to "already
  finalized — call free, not write_to/add". The shim should treat
  the C++ Creator as one-shot.

* **Empty payloads are permitted** (`content_len == 0`, `content`
  may be NULL). zim-tools writers occasionally emit zero-byte
  placeholders.

## Recommended shim-side test additions

Mirror what zimru itself ships at `tests/cffi_writer_smoke.{c,rs}`:

1. **Round-trip test.** Build a tiny ZIM through the shim's
   `zim::writer::Creator`, reopen with the shim's `zim::Archive`,
   assert main-entry redirect resolves, metadata round-trips,
   redirect entry exists and is a redirect, illustration enumerated,
   MD5 verifies.
2. **`zimcheck -A` on the output.** Either the shim's own
   `zimcheck` build, or system `zimcheck`, should report "Pass" on
   the round-trip ZIM. Cross-validation against real libzim if it's
   on PATH.
3. **`zimrecreate <input.zim> <output.zim>`** through the shim.
   Pick a small Wikipedia ZIM, recreate, run `zimcheck -A` on the
   output, diff entry counts and main-page targets against the
   source.
4. **`zimwriterfs <html-dir> <output.zim>`** through the shim.
   Same shape, different content source.

If the shim's CI has access to the upstream `zim-tools` 3.6.0
binaries built against real libzim, a head-to-head on byte counts
+ `zimcheck` output makes regressions obvious.

## What's deferred (not in this batch)

From SHIM_WORK.md:

* §5.1 `zimru_item_open_xapian_db` — conditional on shim-side §1.1
  verification of the F_RDAHEAD=0 / POSIX_FADV_RANDOM mitigation on
  the 48 GB Wikipedia cold-restart bench.
* §5.2 `madvise(MADV_WILLNEED)` opt-in at archive open — conditional
  on shim-side §1.3 verification of the zstd22 cold-cache
  hypothesis.

Both stay paused until the shim side reruns those benches. Both are
well-bounded follow-ups when the verification calls them in.

True alias semantics on the writer side (see `add_alias` gotcha
above) is the other follow-up — file an issue against zimru when
the shim hits a downstream consumer that requires it.

## Pointer for the shim agent

The zimru changes that make all of this real:

* Branch: `claude/libzim-shim-work-d3Rh4`
* Commit: `6388b8a`
* Files of interest:
  * `src/cffi/creator.rs` — the 11 new entry points
  * `include/zimru.h` — regenerated header (lines ~555–706)
  * `tests/cffi_writer_smoke.{c,rs}` — reference C consumer
  * `docs/multi-repo-plan.md` — P1 status update
