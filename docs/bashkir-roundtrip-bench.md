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
| `zimdump dump` (3.2 GB, 175 k entries) | **4.2–5.5 s** | 19.1 s (3.6.0) / 38.6 s (3.2.0) | 3.5× / 7× |
| `zimwriterfs` (3.2 GB tree → zim) | **9.4 s** (zstd 3, 1.087 GB) | did not finish¹ | n/a |
| `zimwriterfs --compression-level 19` | **209 s**, 1.029 GB | did not finish¹ | n/a |
| `zimrecreate` (defaults) | **9.0 s** (zstd 3, 1.111 GB) | 320.5 s (zstd 19, 1.057 GB) | 35× |
| `zimrecreate --compression-level 19` | **214.8 s**, **1.036 GB** | 320.5 s, 1.057 GB | **1.49×, smaller output** |

¹ Upstream zimwriterfs accumulates one open fd per in-flight item and
exhausted the container's 4096-fd hard limit on this 153,708-file tree
in every configuration tried: 3.2.0 at `-J 4` (died at 114 s), 3.2.0 at
`-J 1` (died at 376 s), 3.6.0 at `-J 4` (died at 156 s). zimru's
zimwriterfs reads files in bounded 256-file batches and completed with
default limits.

The high-compression numbers come from three writer-side changes
measured on this corpus (zimrecreate@19 was 334 s before, time-parity
with upstream):

* a continuous encode pipeline (workers + dedicated writer thread,
  clusters written in completion order) replacing join-spawn batches —
  encode CPU utilisation reached 389% of the 400% available;
* preserving the source's per-cluster compression in zimrecreate —
  861 MB of already-compressed media (JPEG/WebP/WebM) stays in raw
  clusters instead of being run through zstd 19 for a ~2% size gain;
* mime-driven cluster compression in zimwriterfs (text-like formats
  compress, media goes raw), matching libzim's hints.

## Fidelity

* File export: byte-identical to upstream zimdump for all ~153,700
  non-colliding files; the 5 path-collision pairs (file `Foo` vs
  directory `Foo/`) are resolved into `_exceptions/` with a
  deterministic shallow-entry-wins policy (link-optimal: upstream
  resolves by write order, which can exile a page that hundreds of
  articles link to), and all 21,679 redirect symlinks resolve
  (upstream: same).
* zimru zimrecreate output reports the identical 29 invalid-link blocks
  that upstream `zimcheck -A` reports for the *source archive itself*
  and for upstream zimrecreate's own output — i.e. content artifacts
  inherited faithfully, no structural errors introduced.
* zimru zimwriterfs tree round-trip output: structurally clean per
  upstream `zimcheck -A`; 51 invalid-link blocks vs the source's
  inherent 29, where the delta is entirely dotfile-named articles
  (`.ru`, `.az`, … — skipped by the dotfile filter, same as upstream
  zimwriterfs) and one side of each path-collision pair. Articles are
  correctly `text/html` with real `<title>`-derived titles via content
  sniffing (extension-only detection used to label every extensionless
  article `application/octet-stream`).

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
