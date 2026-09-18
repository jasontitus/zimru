# Primitives needed for zim-tools 3.6.0 to build through libzim-shim

> **Historical note.** This document describes an earlier revision and its
> measurements or plans at the time of writing. It is not a statement of
> current behavior, support or open bugs; for those see the README
> compatibility matrix, the generated API docs and CHANGELOG.md.

The `libzim-shim` repo (separate, GPL) presents a libzim-shaped
C++ API on top of zimru's C ABI. It is currently good enough for
`libkiwix` / `kiwix-tools` / `kiwix-serve` to build and run end-
to-end. It is **not yet** good enough for the `zim-tools` 3.6.0
binary suite (`zimcheck` / `zimdump` / `zimsplit` / `zimrecreate`
/ `zimwriterfs` / `zimsearch` / `zimbench` / `zimdiff` /
`zimpatch`) — those tools call methods on the C++ surface that
need to route through to zimru.

This doc tracks the gap for that suite. zimru exposes the
underlying primitives; the libzim-shim repo wires them into C++
methods named to match libzim's API.

## How to scope this work

The right reference is **`zim-tools` source itself**, at
a local checkout of `zim-tools` (`src/`). For each missing
method (the C++ build errors below list them) find the call
sites in `zim-tools` and reason about what the *caller* needs to
know about the underlying ZIM file. Then expose a primitive in
zimru's C ABI that lets the caller learn that fact, named
however reads naturally from the [ZIM file format spec].

[ZIM file format spec]: https://wiki.openzim.org/wiki/ZIM_file_format

The shim's C++ wrappers (which mirror libzim's method names by
necessity) call into the new C ABI primitives. Naming on the
shim side is for the shim repo's contributors to decide; naming
on the zimru side flows from the spec and zimru's existing API
conventions.

## Build errors that block zim-tools right now

Compiling zim-tools' `meson` tree against an install of libzim-
shim that lacks these surfaces produces these errors (verbatim
from `meson compile`):

```
src/zimrecreate.cpp:123: no matching member function for call to 'getIllustrationItem'
src/zimsplit.cpp:116:    no member named 'getClusterCount' in 'zim::Archive'
src/zimsplit.cpp:118:    no member named 'getClusterOffset' in 'zim::Archive'
src/zimdump.cpp:122:    no member named 'getEntryByPathWithNamespace' in 'zim::Archive'
src/zimdump.cpp:134:    no member named 'getClusterCount' in 'zim::Archive'
src/zimdump.cpp:147:    too few arguments to function call (hasIllustration / getIllustrationItem default arg)
src/zimcheck/checks.cpp:97:  no member named 'getClusterIndex' in 'zim::Item'
src/zimcheck/checks.cpp:97:  no member named 'getBlobIndex'    in 'zim::Item'
src/zimcheck/checks.cpp:245: no type named 'IntegrityCheckList' in namespace 'zim'
src/zimcheck/checks.cpp:247: no member named 'validate' in namespace 'zim'
src/zimcheck/checks.cpp:283: no member named 'getMainEntryIndex' in 'zim::Archive'
```

Each error names a call site. Read the call site in `zim-tools`
to understand what the caller is trying to do, then design the
zimru primitive accordingly.

`zimcheck`/`zimdump`/`zimsplit` `#define ZIM_PRIVATE` at the top
of their `.cpp` files — the shim will need to expose the
corresponding methods inside the same `#ifdef` guard so callers
opt in to them deliberately.

## What's already in `zimru.h`

Don't duplicate. Existing primitives an implementer may find
useful:

- `zimru_archive_cluster_count` — total cluster count (covers
  `getClusterCount`)
- `zimru_archive_get_entry_by_ns_path(arc, ns, url, err)` —
  namespace + path lookup (covers `getEntryByPathWithNamespace`)
- `zimru_archive_entry_by_url_index(arc, idx, err)` — by-index
- `zimru_archive_entry_by_title_index(arc, idx, err)` — by-title-
  index
- `zimru_archive_main_entry(arc, err)` + `zimru_entry_index(e)` —
  also see the new `zimru_archive_main_entry_index` below
- `zimru_item_direct_access` — file offset / size for items in
  uncompressed clusters
- `zimru_archive_illustrations` + `zimru_archive_illustrations`
  → `zimru_illustration_t[]` — cover/thumbnail descriptors. The
  shim wraps this into `getIllustrationItem(width)` /
  `hasIllustration(width)`; libzim's overloads carry default
  args (`scale=1`), so the shim's C++ wrappers must declare the
  defaults explicitly to match the call sites in
  `zimrecreate.cpp:123` and `zimdump.cpp:147`.

## What's now in `zimru.h` for this batch

Added in this round, mapping one-for-one to the build-error list
above:

- `zimru_archive_main_entry_index(arc) -> uint32_t` —
  URL-pointer index of the archive's main entry, or
  `NO_MAIN_PAGE` (0xFFFFFFFF) when none. Covers libzim's
  `Archive::getMainEntryIndex` (`zimcheck/checks.cpp:283`).
  Cheaper than `zimru_archive_main_entry + zimru_entry_index`
  because it skips the dirent parse and the entry handle alloc.
- `zimru_archive_cluster_offset(arc, idx, err) -> uint64_t` —
  absolute on-disk byte offset where cluster `idx` begins
  (the cluster's leading info-byte). Covers libzim's
  `Archive::getClusterOffset` (`zimsplit.cpp:118`,
  `zimdump.cpp:134`).
- `zimru_item_cluster_index(it) -> uint32_t` — cluster index
  for an item. Covers libzim's `Item::getClusterIndex`
  (`zimcheck/checks.cpp:97`).
- `zimru_item_blob_index(it) -> uint32_t` — blob index within
  the item's cluster. Covers libzim's `Item::getBlobIndex`
  (`zimcheck/checks.cpp:97`).
- `zimru_archive_check_dirent_ptrs(arc, err) -> bool`
- `zimru_archive_check_dirent_order(arc, err) -> bool`
- `zimru_archive_check_title_index(arc, err) -> bool`
- `zimru_archive_check_cluster_ptrs(arc, err) -> bool`
- `zimru_archive_check_mimetypes(arc, err) -> bool` — five
  fine-grained structural checks. Together with the existing
  `zimru_archive_check` (MD5 verify), they let the shim
  re-implement libzim's `IntegrityCheckList` /
  `zim::validate(path, list)`
  (`zimcheck/checks.cpp:245-247`) by composition: build a
  bit-set on the C++ side, run the per-check primitive for
  each enabled bit, AND the results. Each primitive returns
  `true` on a clean archive, `false` on a structural defect,
  and `false`-with-`*err`-set when the read itself fails.

## Shim-side wiring sketch

For the integrity-check aggregator, a sketch on the shim side
(pseudocode, not a real header):

```cpp
namespace zim {
enum class IntegrityCheck { CHECKSUM, DIRENT_PTRS, DIRENT_ORDER,
                            TITLE_INDEX, CLUSTER_PTRS,
                            DIRENT_MIMETYPES, COUNT };
class IntegrityCheckList { /* bitset over IntegrityCheck */ };
bool validate(const std::string& path, IntegrityCheckList list) {
    auto* a = zimru_archive_open(path.c_str(), &err);
    bool ok = true;
    if (list.test(IntegrityCheck::CHECKSUM))
        ok &= zimru_archive_check(a, &err);
    if (list.test(IntegrityCheck::DIRENT_PTRS))
        ok &= zimru_archive_check_dirent_ptrs(a, &err);
    /* … one branch per check kind … */
    zimru_archive_close(a);
    return ok;
}
}
```

For `getMainEntryIndex` / `getClusterOffset` /
`Item::getClusterIndex` / `Item::getBlobIndex` the wrapping is
a one-liner — just call the matching `zimru_*` primitive and
return the result.

## Tests to add alongside any new primitive

For each primitive added:

1. **Unit / integration test in zimru** — open a fixture
   (synthetic via `Creator` or a real-file under `zim-cache/`,
   silently skipped if absent), call the new C function, assert
   against a reference value derivable from the existing public
   Rust API.
2. **Synthetic-corruption test** for any primitive that reports
   structural validity — build a clean fixture, deliberately
   corrupt the relevant byte region, reopen, assert the primitive
   reports the defect.

This batch of additions is covered by `tests/integrity_checks.rs`
(round-trip + corruption tests for every new primitive) and
extensions to `tests/cffi_smoke.{c,cpp}` (C and C++ link/call
smoke through the regenerated header).

The shim repo carries its own smoke + parity tests for the C++
wrappers; those are in scope of the shim repo's CI, not zimru's.

## Out of scope

- Anything that requires consulting libzim source. The zimru
  side stays clean-room; primitives are designed from the ZIM
  spec and the shape of the consumer's call sites in `zim-tools`,
  not from libzim's API.
- The libzim-shim wiring itself.
- Aggregate "validate the whole archive" entry points that
  could just as easily be composed by the caller from finer-
  grained primitives — see the
  `zimru_archive_check_*` set above for the granular surface
  the shim composes into libzim's `IntegrityCheckList` /
  `validate()` shape.

## Notes about zim-tools 3.6.0 itself

Discovered while trying to build zim-tools against the shim. None
of these depend on libzim source — they're properties of
`zim-tools` and its build glue.

### `zimbench`'s random-URL phase crashes

`zimbench`'s second phase ("collect random urls") fails on every
ZIM tested — both legacy and modern, against real libzim ("Cannot
find entry") and against builds that get past the shim's
buildable subset ("entry not found"). The first phase ("linear
urls") completes fine on either backend.

Practical consequence: `zimbench` is **not usable as a head-to-
head benchmark harness**. Either drive it with `-n 0` to skip
the random phase, or build a custom bench (see
`libzim-shim/bench/bench.cpp` for the shape) to measure
read/lookup/search latencies. zimru's own `bench/run.sh` works
around this by exercising different binaries (`zimcheck`,
`zimdump`, `zimru readall`) instead.

### Files that `#define ZIM_PRIVATE`

These three `.cpp` files at the top of zim-tools/src use
libzim's `ZIM_PRIVATE`-gated internal API:

- `src/zimcheck/checks.cpp`
- `src/zimdump.cpp`
- `src/zimsplit.cpp`

The shim must expose its corresponding additions inside the same
`#ifdef ZIM_PRIVATE` guard so the boundary stays explicit on the
caller side too.

### Build-glue quirks (macOS + homebrew)

Reproducing a release build of zim-tools 3.6.0 against the
homebrew install of libzim 9.6.0 needs three workarounds beyond
"meson setup":

- **`mustache.hpp` not on the default include path.** zim-tools'
  `src/zimcheck/meson.build` does `compiler.has_header('mustache.hpp')`
  with no `-I` hint. On homebrew it ships at `/opt/homebrew/
  include/mustache.hpp` and meson won't find it without
  `CXXFLAGS=-I/opt/homebrew/include`.
- **`icu-i18n.pc` doesn't propagate `icuuc` for shared linkage.**
  `Requires.private: icu-uc` only adds `-licuuc` for static
  builds. zim-tools' `metadata.cpp` references `icu_NN::Unicode
  String::fromUTF8` which lives in libicuuc, so a release build
  fails with "Undefined symbols ... fromUTF8". Workaround:
  `LDFLAGS="-L/opt/homebrew/Cellar/icu4c@78/78.3/lib -licuuc"`
  during `meson setup`.
- **`PKG_CONFIG_PATH` must include both libzim's and icu4c's pc
  directories.** The brew prefix's flat
  `/opt/homebrew/lib/pkgconfig` does not contain `libzim.pc`
  (it's keg-only) or `icu-uc.pc` (the @78 keg is also keg-only).

A working invocation:

```sh
PKG_CONFIG_PATH=/opt/homebrew/Cellar/libzim/9.6.0/lib/pkgconfig:\
/opt/homebrew/Cellar/icu4c@78/78.3/lib/pkgconfig:\
/opt/homebrew/lib/pkgconfig:\
/opt/homebrew/opt/zstd/lib/pkgconfig \
CXXFLAGS="-I/opt/homebrew/include" \
LDFLAGS="-L/opt/homebrew/Cellar/icu4c@78/78.3/lib -licuuc" \
meson setup build --buildtype=release
meson compile -C build
```

### Built-binary layout

meson emits zim-tools binaries at *two different* locations:

- `build/src/zimcheck/zimcheck` (subdir per binary — `zimcheck`
  and `zimwriterfs` follow this layout).
- `build/src/zimbench`, `build/src/zimdump`, `build/src/zimrecreate`,
  etc. (flat at `src/`).

zimru's `bench/run.sh` and similar scripts that take an
`UPSTREAM_DIR` expect a flat directory of binaries. Either
symlink them flat, or set `UP_*` env vars individually:

```sh
mkdir -p /tmp/upstream-flat
ln -sf $PWD/build/src/zimcheck/zimcheck /tmp/upstream-flat/zimcheck
ln -sf $PWD/build/src/zimdump            /tmp/upstream-flat/zimdump
ln -sf $PWD/build/src/zimbench           /tmp/upstream-flat/zimbench
UPSTREAM_DIR=/tmp/upstream-flat ./bench/run.sh path/to/file.zim
```

### Building libzim-shim as a libzim drop-in

For zim-tools to find the shim via `pkg-config --libs libzim`,
configure with `LIBZIM_SHIM_USE_LIBZIM_SONAME=ON` and write a
`libzim.pc` into the install prefix's `lib/pkgconfig`:

```sh
cmake -B build -S . -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=/tmp/shim-prefix \
    -DLIBZIM_SHIM_USE_LIBZIM_SONAME=ON
cmake --build build --parallel
cmake --install build
```

Then `PKG_CONFIG_PATH=/tmp/shim-prefix/lib/pkgconfig:...` ahead
of any system libzim path picks up the shim.

## Companion benchmark doc

`libzim-shim/bench/RESULTS.md` records the latency picture from
running the shim's bench harness against real libzim. Once the
gap above is closed, re-running zim-tools binaries (zimcheck,
zimdump) under both libzim implementations and recording the
deltas there is the natural follow-up.
