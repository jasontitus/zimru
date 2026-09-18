# Multi-repo roadmap

> **Historical note.** This document describes an earlier revision and its
> measurements or plans at the time of writing. It is not a statement of
> current behavior, support or open bugs; for those see the README
> compatibility matrix, the generated API docs and CHANGELOG.md.

This document describes the repo-level architecture needed to make zimru
a viable replacement for libzim under existing GPL-licensed consumers
(libkiwix, kiwix-tools, kiwix-serve, kiwix-desktop, kiwix-android,
kiwix-apple, python-libzim) **while keeping zimru itself MIT-clean**.

## Goal

```
                            ┌── kiwix-tools / kiwix-serve / kiwix-desktop
                            │       (rebuilt from source)
   ┌── libkiwix ─────────── ┤
   │   (rebuilt from source)│
   │                        └── kiwix-android (via java-libkiwix)
   │                                kiwix-apple (via .mm bridges)
   │
libzim-shim (GPL v2)        ← libzim-shaped C++ headers + JNI symbols
   │
   ↓ calls C ABI of ↓
   │
zimru (MIT)                 ← pure Rust core (this repo)
```

The licensing wall sits at the zimru ↔ libzim-shim boundary. zimru's
MIT licence claim depends on its developers never having read libzim or
libkiwix source. libzim-shim is GPL v2 (matching libzim) and *its*
developers can freely consult libzim/libkiwix to mirror the surface.

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for the full clean-room
policy that applies inside this repo.

## Repos to create

### 1. `jasontitus/libzim-shim` (the only new repo strictly required)

| field | value |
|---|---|
| Licence | **GPL v2** (matches libzim — the shim *is* a derivative work of libzim's API) |
| Language | C++ (with optional CMake or Meson build) |
| Purpose | Expose libzim-shaped C++ headers (`zim::Archive`, `zim::Entry`, …) and JNI symbols (`Java_org_kiwix_libzim_*`) backed internally by zimru's C ABI |
| Depends on | `zimru` ≥ 0.2.0 (must publish a C ABI first — see prerequisites below) |

**Initial structure:**

```
libzim-shim/
├── README.md                  # describes purpose, links to zimru + libzim
├── LICENSE                    # GPL-2.0
├── CMakeLists.txt             # (or meson.build)
├── include/
│   └── zim/
│       ├── archive.h          # mirrors libzim's public reader headers
│       ├── entry.h
│       ├── item.h
│       ├── blob.h
│       ├── uuid.h
│       └── writer/
│           ├── creator.h
│           └── item.h
├── src/
│   ├── archive.cpp            # impls call zimru's C ABI
│   ├── entry.cpp
│   ├── item.cpp
│   ├── blob.cpp
│   ├── uuid.cpp
│   └── writer/
│       ├── creator.cpp
│       └── item.cpp
├── jni/
│   └── libzim/
│       ├── archive.cpp        # JNI wrappers w/ Java_org_kiwix_libzim_*
│       ├── entry.cpp
│       └── ...
└── tests/                     # GPL-licensed shim tests; may freely
                               # consult libzim test patterns
```

**What to populate it with on first commit:**

1. `LICENSE` — text of GPL-2.0.
2. `README.md` — short summary, link to this roadmap, link to zimru,
   note that this is the GPL-licensed shim layer and contributors here
   are *not* bound by zimru's clean-room policy.
3. `CMakeLists.txt` — find `libzimru` (the C library produced by zimru
   when built as `cdylib`/`staticlib`), build a shared library
   `libzim.so` exposing the libzim-shaped API.
4. Stub `include/zim/archive.h` mirroring libzim's `zim::Archive` class
   declaration (signatures only — implementations stay in `.cpp`).
5. Stub `src/archive.cpp` that opens a zimru archive via the C ABI and
   stashes the handle inside the shim's `zim::Archive` instance.

The shim author may freely consult libzim's headers to get class shapes
and method signatures right. They should NOT consult libkiwix or any
upstream test code unless the shim is also re-licensed appropriately.

### 2. (Optional, later) `jasontitus/libkiwix-zimru-fork`

If maintaining a libkiwix fork that consumes libzim-shim turns out to
be necessary, that's a separate repo (GPL v3 to match libkiwix). Most
likely we don't need it: libkiwix should build unchanged against the
shim if the shim is faithful to libzim's API.

### 3. (Optional, later) `jasontitus/zimru-search`

If we go the route of adding Xapian-equivalent fulltext + suggestion
search to zimru rather than to the shim, this could be a separate crate.
Could also live inside zimru as a feature flag. Decide later — see
"Open architectural questions" below.

## Prerequisites in this repo (zimru)

Before libzim-shim work can start in earnest, zimru needs to ship:

### P1. Stable C ABI (blocker) — reader + writer LANDED

Reader-side C ABI shipped first (branch
`claude/review-libzim-issues-SGsq0`); writer-side C ABI shipped on
branch `claude/libzim-shim-work-d3Rh4`. The writer surface mirrors
what the libzim-shim's `src/writer/creator.cpp` wrapper expects from
`docs/SHIM_WORK.md` (writer-side gap section), which is what unblocks
the four `zim-tools` writer-side binaries (`zimrecreate`,
`zimwriterfs`, `zimdiff`, `zimpatch`) building through the shim.

See [tracking issue #7][i7]. Reader smoke harness at
`tests/cffi_smoke.{c,rs}`; writer smoke harness at
`tests/cffi_writer_smoke.{c,rs}`.

[i7]: https://github.com/jasontitus/zimru/issues/7

#### What's done

- `cffi` Cargo feature (off by default, implies `writer`).
- `[lib] crate-type = ["rlib", "cdylib", "staticlib"]`.
- `cbindgen` build-dep + `build.rs` emit `include/zimru.h` whenever
  the feature is on.
- `src/cffi/` modules: `error`, `archive`, `entry`, `item`, `blob`,
  `creator` (stub). All `extern "C"` functions prefixed `zimru_*`.
- Conventions documented in module preamble: string lifetimes (tied to
  parent handle), out-pointer error protocol, `*_free` ownership.
- `tests/cffi_smoke.c` + `tests/cffi_smoke.rs` end-to-end test:
  builds a ZIM with the writer, compiles + links a C consumer against
  the generated header and built dylib, runs it and asserts exit 0.
  Passes in `cargo test --features cffi`.
- CI gains a `cffi` job that builds + smoke-tests the feature.

#### What still needs to happen before the shim work can fully start

- ~~**Writer C ABI**~~ — landed. The full surface (`zimru_creator_new`
  / `_free` / `_set_compression` / `_set_compression_level` /
  `_set_cluster_size_target` / `_set_uuid` / `_set_main_path` /
  `_add_item` / `_add_metadata` / `_add_illustration` /
  `_add_redirection` / `_add_alias` / `_write_to`) is in `src/cffi/
  creator.rs` with smoke coverage at `tests/cffi_writer_smoke.{c,rs}`.
  One acknowledged gap: `zimru_creator_add_alias` is wired as a
  redirection until the underlying writer grows true alias semantics
  — the shim degrades gracefully here, per `docs/SHIM_WORK.md`.
- **Cut zimru 0.2.0** — pin a published version the shim repo can
  declare as its `zimru = "0.2.0"` dependency.
- **Header SOVERSION discipline** — decide before downstream apps link
  against `libzimru.so.0`.

#### Original spec (kept for reference)

Add a `cffi` Cargo feature that builds zimru as a `cdylib` and
`staticlib` and exposes an `extern "C"` API plus a cbindgen-generated
header.

- New module `src/cffi.rs` with `#[no_mangle] extern "C"` functions for
  every reader API the shim needs.
- Add `[lib] crate-type = ["rlib", "cdylib", "staticlib"]`.
- Add `cbindgen` as a build dep + `build.rs` that emits `zimru.h`.
- Symbol prefix: `zimru_*`. Do **not** name them `zim_*` — that's
  libzim's namespace.
- Test: a tiny C program in `tests/cffi_smoke.c` that opens an archive
  and reads one entry. Run via a build-script-driven test.

Suggested first-cut surface (mirror existing Rust API shapes):

```c
zimru_archive_t* zimru_archive_open(const char* path, zimru_error_t** err);
void             zimru_archive_close(zimru_archive_t*);
uint32_t         zimru_archive_entry_count(const zimru_archive_t*);
zimru_entry_t*   zimru_archive_get_entry_by_path(const zimru_archive_t*,
                                                 const char* path,
                                                 zimru_error_t** err);
const char*      zimru_entry_path(const zimru_entry_t*);
const char*      zimru_entry_title(const zimru_entry_t*);
bool             zimru_entry_is_redirect(const zimru_entry_t*);
zimru_item_t*    zimru_entry_get_item(const zimru_entry_t*, bool follow,
                                       zimru_error_t** err);
const char*      zimru_item_mimetype(const zimru_item_t*);
zimru_blob_t*    zimru_item_get_data(const zimru_item_t*, zimru_error_t**);
const uint8_t*   zimru_blob_data(const zimru_blob_t*);
size_t           zimru_blob_size(const zimru_blob_t*);
/* … iterators, search, writer, etc. */
```

### P2. Xapian — DECIDED: out of scope for zimru

**Decision:** zimru deliberately does not implement Xapian fulltext or
suggestion search. Xapian is GPL v2; linking it into zimru would force
every binary that embeds zimru to be GPL, defeating the MIT licence.

Search is therefore pushed to the layer above zimru:

- **GPL consumers** (kiwix-tools, kiwix-serve, kiwix-desktop,
  kiwix-android, kiwix-apple) get Xapian via `libzim-shim`. The shim
  is already GPL v2, so it can link Xapian freely and implement
  `zim::Searcher` / `zim::SuggestionSearcher` natively.
- **Non-GPL consumers** build their own search stack on top of zimru
  — Tantivy, a custom index, server-side search, or no search at all.
  zimru exposes everything needed to harvest fulltext for indexing
  (parallel cluster decompression, zero-copy blobs, MIME filtering).

Implications for the shim repo: it grows a `searcher.cpp` etc. that
links Xapian directly. zimru's surface area stays tight.

This decision is also documented in zimru's
[`README.md` → Licensing & clean-room policy](../blob/main/README.md#why-xapian-fulltext-search-is-out-of-scope-rather-than-todo).

### P3. (Optional, but desirable) Stable Rust API

zimru is at 0.1.0 today. The breaking changes from issue #3 (Uuid type)
are the kind of thing we don't want recurring once shim and JNI
consumers depend on us. Suggest cutting a 0.2.0 once C ABI lands and
tagging it as the first "shim-eligible" version.

## Sequencing

Ordered by dependency, not by calendar:

1. **In zimru**: P1 — C ABI + cbindgen header. Cut zimru 0.2.0.
2. **Create libzim-shim repo**, license + README + CMake skeleton.
3. **In libzim-shim**: implement reader headers + impls. Test by
   building a trivial C++ program against `<zim/archive.h>` from the
   shim and confirming it reads a real ZIM via zimru.
4. **In libzim-shim**: implement writer headers + impls. Test by
   running libzim's `zimwriterfs` source rebuilt against the shim
   (or just our own test harness building ZIMs).
5. **Decide P2** (Xapian path A / B / C). If A: implement in zimru
   (probably as a feature flag), then add `searcher.cpp` to shim.
6. **In libzim-shim**: implement JNI symbol layer
   (`Java_org_kiwix_libzim_*`). Test by checking out `java-libkiwix`,
   replacing its native build with the shim's, running its Java tests.
7. **Validate libkiwix builds against shim**: clone libkiwix, point its
   pkg-config at libzim-shim's pkg-config file, run `meson setup +
   ninja test`. Iterate on shim until libkiwix's own test suite passes.
8. **Validate downstream apps**: kiwix-serve, kiwix-tools, kiwix-apple,
   kiwix-android. Each is a separate validation pass.

Steps 1–4 are tractable for a single team in 2–4 weeks of focused
work. Step 5 (Xapian) is the largest variable. Steps 6–8 are
integration / debugging.

## Cross-repo conventions

- Both repos use `claude/<topic>` branches matching this repo's pattern.
- libzim-shim depends on a published zimru version, not a git ref.
  Pin `zimru = "0.2.0"` (etc.) in its build config.
- Issue tracking: cross-link issues across repos by URL. The shim repo
  may file feature requests against zimru when its C ABI needs to grow.
- When the shim discovers a behavioural divergence between zimru and
  libzim, file the bug **against zimru**, with a black-box reproducer
  (input ZIM + expected vs. observed output). Do not paste libzim
  source into the bug report — describe the discrepancy in your own
  words. (Per the clean-room policy.)

## What the other Claude instance should do first

Concrete first-session checklist when running on the machine that can
create repos:

1. `gh repo create jasontitus/libzim-shim --public --license GPL-2.0`
   (or `--private` if not ready to publish).
2. Clone it locally.
3. Add a stub `README.md` that says: "C++ shim exposing libzim's API
   surface, backed by [zimru](https://github.com/jasontitus/zimru).
   GPL v2. Contributors here are *not* bound by zimru's clean-room
   policy and may freely consult libzim source."
4. Add a stub `CMakeLists.txt` that finds `libzimru` and builds an
   empty `libzim.so`.
5. Open issue #1 on the new repo: "Implement reader-side shim headers
   (Archive, Entry, Item, Blob, Uuid)".
6. Come back to *this* repo and open a tracking issue: "Land C ABI
   (P1) — blocker for libzim-shim work" and link the shim repo's
   issue #1 to it.

Beyond that, work proceeds per the sequencing list above.

## Open architectural questions

Before committing to specific designs, decide:

1. ~~**Xapian path**~~ — **decided**: out of scope for zimru, lives in
   the shim. See P2 above.
2. **Stable C ABI vs. evolving** — do we commit to ABI stability from
   zimru 0.2.0, or version it (`zimru-cffi-0.2.so`) and let the shim
   target a specific minor?
3. **Build system for the shim** — CMake (matches libzim), Meson
   (matches libkiwix), or both? CMake is probably simpler since
   downstream consumers (kiwix-build) already use it.
4. **JNI strategy** — shim exposes JNI symbols itself, OR a separate
   smaller `zimru-jni` Rust crate using the `jni` crate. The latter
   could be MIT (since JNI symbol names alone are interface, not
   expression) — but it's safer and clearer for it to live under the
   GPL umbrella with the rest of the libzim-mimicking work.
5. **Apple bridge** — kiwix-apple's `.mm` files include libzim headers
   directly. Once the shim is built into a `CoreKiwix.xcframework`,
   no kiwix-apple changes are strictly needed. Confirm this assumption
   by walking `Model/ZimFileService/ZimService.mm`'s includes (only
   public API surface, per clean-room rules — don't read its body).
