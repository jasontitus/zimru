# Primitives needed for zim-tools 3.6.0 to build through libzim-shim

The `libzim-shim` repo (separate, GPL) presents a libzim-shaped
C++ API on top of zimru's C ABI. It is currently good enough for
`libkiwix` / `kiwix-tools` / `kiwix-serve` to build and run end-
to-end. It is **not** good enough for the `zim-tools` 3.6.0
binary suite (`zimcheck` / `zimdump` / `zimsplit` / `zimrecreate`
/ `zimwriterfs` / `zimsearch` / `zimbench` / `zimdiff` /
`zimpatch`) — those tools call methods on the C++ surface that
do not currently route through to zimru.

When the gap below is closed, all 9 zim-tools binaries should
build cleanly against the shim and pass head-to-head runs against
the real libzim.

## How to scope this work

The right reference is **`zim-tools` source itself**, at
`/Users/jasontitus/experiments/zim-tools/src/`. For each missing
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

- `zimru_archive_cluster_count` — total cluster count
- `zimru_archive_get_entry_by_ns_path(arc, ns, url, err)` —
  namespace + path lookup
- `zimru_archive_entry_by_url_index(arc, idx, err)` — by-index
- `zimru_archive_entry_by_title_index(arc, idx, err)` — by-title-
  index
- `zimru_archive_main_entry(arc, err)` + `zimru_entry_index(e)` —
  may be enough to compose certain main-entry-index needs without
  a new primitive
- `zimru_item_direct_access` — file offset / size for items in
  uncompressed clusters

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
  grained primitives.

## Companion benchmark doc

`libzim-shim/bench/RESULTS.md` records the latency picture from
running the shim's bench harness against real libzim. Once the
gap above is closed, re-running zim-tools binaries (zimcheck,
zimdump) under both libzim implementations and recording the
deltas there is the natural follow-up.
