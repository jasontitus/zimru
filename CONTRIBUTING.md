# Contributing to zimru

Thanks for considering a contribution. Before sending a patch, please read
the clean-room policy below — it's the foundation of zimru's MIT licence
claim and is non-negotiable.

## Project licence

zimru is **MIT-licensed**. It is an independent re-implementation of the
ZIM file format, not a port of GPL-licensed [`libzim`][libzim] or
[`libkiwix`][libkiwix]. To preserve that licence claim:

- All code in this repository must be original work, written from publicly
  documented specifications.
- No GPL source from `libzim`, `libkiwix`, or their downstream native
  bindings may be consulted while writing zimru code.

If you want to write a libzim-shaped C++ shim, JNI bindings whose
implementations mirror libzim's, or Obj-C++ wrappers whose bodies look
like the kiwix-apple ones, that work belongs in a **separate, GPL-licensed
project** (e.g. a future `libzim-shim` repo). It does not go in this repo.

## Clean-room rules

### Sources you may consult

- The [ZIM file format specification][spec] on the openzim wiki.
- Public READMEs and architecture docs of `libzim`, `libkiwix`,
  `python-libzim`, `java-libkiwix`, and `kiwix-apple`.
- API surface declarations exposed in **non-C++ languages** by downstream
  projects:
  - `python-libzim`'s Python-facing API (the `.pyx` *interface*, not its
    Cython implementation body).
  - `java-libkiwix`'s Kotlin/Java class declarations
    (`org.kiwix.libzim.Archive` and friends — declarations only).
  - `kiwix-apple`'s Swift `ZimFileService` / `ZimContentProvider` API.
- Public openzim issues, RFCs, and design discussions.
- Output of upstream binary tools (`zimcheck`, `zimdump`, `zimwriterfs`,
  `zimrecreate`) treated as black-box references — running them on the
  same input as zimru and diffing outputs is fine and encouraged.
  Functional behaviour is not copyrightable.

### Sources you must NOT consult

- `libzim`'s `src/**/*.cpp` and `src/**/*.h` (any file under the project
  except the rendered wiki spec linked above).
- `libkiwix`'s source (any file).
- `java-libkiwix`'s `lib/src/main/cpp/**` JNI implementations
  (the Kotlin/Java surface in `lib/src/main/java/**` is OK as documented
  above — the C++ wrappers are not).
- `kiwix-apple`'s `Model/ZimFileService/*.mm` Objective-C++ wrapper
  bodies (the Swift API consuming them is OK).
- `python-libzim`'s `.pyx` Cython implementation bodies
  (the Python-facing interface declarations are OK).
- Any libzim, libkiwix, or downstream **test code**, ever.

If you have already read any of the above, please do not contribute to
zimru.

### Two-role split (only if writing FFI / JNI / Obj-C bridge surfaces)

For work that necessarily touches an interface shape libzim already
defines (e.g. a JNI symbol that must be named
`Java_org_kiwix_libzim_Archive_setNativeArchive` because Kotlin requires
that name), use the standard Phoenix-style clean-room methodology:

- **Describer** — may read the *declaration* (Kotlin signature, Swift
  protocol, Python `.pyx` interface) and write an English-language
  description of behaviour. The description contains no code.
- **Implementer** — reads only the description and the public ZIM spec.
  Never opens the GPL source. Writes the Rust.

The describer and implementer must be different people, and the English
description should be retained as a paper trail (commit it under
`docs/clean-room/`).

In practice this is rarely needed within zimru itself — most such bridge
work belongs in the GPL-licensed shim project, not here.

## Tests

Tests in this repo are written from the ZIM spec and from observable
behaviour of upstream binaries. Do not translate `libzim`'s GTest cases
into Rust. If you want to verify zimru handles the same edge case as
upstream:

1. Construct an input that triggers the edge case (synthetic ZIM in
   memory, or a real-world ZIM you can describe).
2. Run upstream's binary and zimru against it, observe both outputs.
3. Write an original Rust test that asserts zimru's behaviour matches
   the observed contract.

The test code should be your own, not a transliteration of upstream's.

## Style and scope

- Format with `cargo fmt` and lint with `cargo clippy --all-targets`.
- Add tests for behaviour you change. Prefer the existing test patterns
  in `tests/synthetic_zim.rs` (in-memory spec-compliant fixtures) and
  `tests/writer_roundtrip.rs` (Creator-built archives).
- Keep changes scoped — bundle independent fixes into separate PRs.

## Reporting bugs

If you observe an issue with a real-world ZIM that zimru misreads, open
a GitHub issue with:

- The exact `zimru` / `zimcheck` / `zimdump` invocation that fails.
- A minimal description of the input archive (size, source, version).
- The output you got vs. the output you expected.

Don't paste libzim source into the issue — describe the discrepancy in
your own words.

[libzim]: https://github.com/openzim/libzim
[libkiwix]: https://github.com/kiwix/libkiwix
[spec]: https://wiki.openzim.org/wiki/ZIM_file_format
