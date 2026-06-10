# Real-archive round-trip benchmark: 1.1 GB Bashkir Wikipedia

Date: 2026-06-10. Environment: 4-core x86-64 Linux container, 15 GB RAM,
4096 fd hard limit. Reference: official zim-tools static builds
(3.6.0 musl; 3.2.0 from Ubuntu apt where noted). Source archive:
`wikipedia_ba_all_maxi_2026-04.zim` — 1,141,821,754 bytes, 175,387
entries (153,708 articles + 21,679 redirects), 1,502 clusters, 3.2 GB
decompressed.

Workflow exercised end-to-end, matching the upstream tool chain:

```
zimdump dump --dir=DIR --ns=C --redirect SOURCE.zim   # export to files
zimwriterfs [metadata flags] -j -J 4 DIR OUT.zim      # rebuild from files
zimrecreate SOURCE.zim OUT.zim -j                     # rebuild from archive
zimcheck -A OUT.zim                                   # cross-validate
```

## Results

| Step | zimru | upstream zim-tools | speedup |
|---|---|---|---|
| `zimdump dump` (3.2 GB, 175 k entries) | **5.5 s** | 19.1 s (3.6.0) / 38.6 s (3.2.0) | 3.5× / 7× |
| `zimwriterfs` (3.2 GB tree → 1.09 GB zim) | **9.4–12.7 s** | did not finish¹ | n/a |
| `zimrecreate` (defaults) | **9.0 s** (zstd 3, 1.111 GB) | 320.5 s (zstd 19, 1.057 GB) | 35× |
| `zimrecreate --compression-level 19` | 334 s, **1.035 GB** | 320.5 s, 1.057 GB | parity² |

¹ Upstream zimwriterfs accumulates one open fd per in-flight item and
exhausted the container's 4096-fd hard limit on this 153,708-file tree
in every configuration tried: 3.2.0 at `-J 4` (died at 114 s), 3.2.0 at
`-J 1` (died at 376 s), 3.6.0 at `-J 4` (died at 156 s). zimru's
zimwriterfs reads files in bounded 256-file batches and completed with
default limits.

² At matched compression level the writers are within 4% on wall time
and zimru's output is ~2% smaller. The 35× default-settings gap is a
defaults trade-off: zimru defaults to zstd 3 (fast, output ~5% larger
than upstream's zstd 19 default); pass `--compression-level 19` for
size parity.

## Fidelity

* File export: byte-identical to upstream zimdump for all ~153,700
  non-colliding files; the 5 path-collision pairs (file `Foo` vs
  directory `Foo/`) are resolved into `_exceptions/` exactly like
  upstream, and all 21,679 redirect symlinks resolve (upstream: same).
* zimru zimwriterfs output passes upstream `zimcheck -A` (Overall Test
  Status: Pass).
* zimru zimrecreate output reports the identical 29 invalid-link blocks
  that upstream `zimcheck -A` reports for the *source archive itself*
  and for upstream zimrecreate's own output — i.e. content artifacts
  inherited faithfully, no structural errors introduced.

## What made zimru fast (see git history for details)

* Writer: cluster compression batches run on a background thread,
  overlapped with item production; frames carry the zstd
  frame-content-size header so readers pre-allocate exact buffers;
  no redundant MD5 re-read in post-write verify; dirents render
  directly into one staging buffer.
* zimdump/zimrecreate: source content is read grouped by cluster
  (each cluster decompressed exactly once) instead of URL order,
  which thrashed the decoded-cluster LRU on real archives where
  cluster packing follows crawl order, not URL order.
