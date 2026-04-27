# Wikipedia ZIM build — the highest-value target for zimru

The Kiwix zim farm at <https://farm.openzim.org> builds the world's
public ZIM corpus on a fleet of worker bots. The biggest, slowest,
and most user-visible job in that fleet is **English Wikipedia
(`wikipedia_en_all_maxi`)**: the recipe ships a ~120 GB ZIM
quarterly to public mirrors. Cutting that build's wall time is the
single biggest production win we can deliver.

## How a Wikipedia ZIM is actually built (verified via the public Zimfarm API)

- The farm dispatches a Docker container running
  `ghcr.io/openzim/mwoffliner:1.17.5`. **mwoffliner**, not
  `zimwriterfs` or `zimrecreate`, is the entry point.
- mwoffliner is a Node.js MediaWiki scraper that uses
  `@openzim/libzim` (Node bindings to libzim's C++ Creator API)
  to produce the output ZIM directly — no filesystem extract step
  in between.
- **No `--threads`, `--clusterSize`, or `--compression-level` flags
  are passed.** ZIM-write parameters are baked into mwoffliner's
  source at the version level (1.17.5 today).

Verified command for `wikipedia_en_all_maxi` (fetched from
`https://api.farm.openzim.org/v2/schedules/wikipedia_en_all_maxi`):

```
mwoffliner \
  --mwUrl=https://en.wikipedia.org/ \
  --adminEmail=contact@kiwix.org \
  --customMainPage=User:The_other_Kiwix_guy/Landing \
  --customZimTitle=Wikipedia \
  --customZimDescription='The free encyclopedia' \
  --customZimFavicon=…png \
  --publisher=openZIM \
  --format=novid:maxi \
  --optimisationCacheUrl=… \
  --addNamespaces=100 \
  --osTmpDir=/dev/shm \
  --outputDirectory=/output \
  --requestTimeout=300 \
  --verbose=log \
  --webp \
  --forceRender=ActionParse
```

Resource budget per task:

- **6 CPU**
- **22 GB RAM** (23 622 320 128 bytes allocated)
- **150 GB disk**

`withoutZimFullTextIndex: null` in the config means **Xapian
fulltext indexing IS on** for Wikipedia builds — the resulting
ZIM ships with `X/fulltextIndex/xapian` populated.

## Why this is zimru's highest-value target

1. The English Wikipedia build is widely reported to take **6–10
   hours of wall time** on a 6-CPU, 22 GB worker. A 2× wall-time
   speedup saves the farm a meaningful fraction of its compute
   budget per quarter; a 4× speedup recovers a full machine-day
   per release.
2. mwoffliner is the most complex consumer of libzim's writer API
   in production. If the shim handles mwoffliner correctly, every
   smaller scraper (sotoki for Stack Exchange, gutenberg, ted,
   phet, …) is downstream of the same surface.
3. zimwriterfs and zimrecreate matter for archival flows but
   aren't on the critical path for the public ZIM corpus.
4. Wikipedia's ZIM is the canonical demo. If kiwix-serve's first
   `/search` on a 48 GB Wikipedia is faster on shim+zimru (the
   P7 cold-mmap target), or if the build itself is faster, those
   are the headlines that move adoption.

## What the shim must do to run mwoffliner

mwoffliner's `@openzim/libzim` bindings exercise libzim's full
writer-side API. At minimum:

1. **Every Creator method mwoffliner calls must be exposed by the
   shim.** Today's `WRITER_WIRING_HANDOFF.md` round covers
   `configCompression`, `configClusterSize`, `setMainPath`,
   `addItem`, `addRedirection`, `addMetadata`, `addIllustration`,
   `finishZimCreation`. mwoffliner additionally needs:
   - `configCompressionLevel(int)` — currently the shim has no
     way to forward an explicit level (we worked around with
     `ZSTD_CLEVEL` env var on zimru's side; mwoffliner sets the
     level via API, not env).
   - `addItem(shared_ptr<Item>, …)` with full `HintKeys` map
     (frontArticle / etc.) — partially covered.
   - `Creator::Status` / progress callbacks if mwoffliner reads
     them.
   - `setIndexing(true, lang)` — currently a no-op on the shim
     (zimru doesn't index). Wikipedia builds will produce
     unsearchable archives until this is real.
2. **Xapian writer-side integration in the shim.** mwoffliner
   expects `X/fulltextIndex/xapian` populated. The shim is GPL
   and can link `libxapian` directly to build the index in C++
   during `addItem`, then ship the finished DB as an entry via
   `zimru_creator_add_item("X/fulltextIndex/xapian", …)`. zimru
   itself stays Xapian-free (and MIT) — the index lives in the
   shim layer. This is the headline missing piece.
3. **Streaming cluster compression on the zimru side.** The 22 GB
   RAM budget per task is exactly what `zimru native` hit on the
   17 GB osm-texas test (22.9 GB RSS peak, OOM-adjacent).
   English Wikipedia is ~10× bigger; current buffer-everything-
   then-write would OOM the worker. Stream-fill clusters,
   compress, free, repeat.
4. **`--threads N` plumbing.** Native's `--threads N` is a
   no-op stub today; wire it to `rayon::ThreadPoolBuilder`. Shim
   should forward libzim's `configNbWorkers(N)` similarly. The 6
   CPU budget should saturate.

## How to run the production-equivalent test

Once items 1–4 above land:

1. Rebuild mwoffliner Docker with libzim-shim's
   `libzim.{so,dylib}` swapped in for upstream libzim. Both have
   `SONAME libzim.9.dylib`; `LIBZIM_SHIM_USE_LIBZIM_SONAME=ON` at
   build time gives us the right name.
2. Run mwoffliner on a small wiki first
   (`wikipedia_simple_all_maxi` is ~1 GB and uses the same
   pipeline). Validate output ZIM with real `zimcheck -A` — must
   pass and must contain `X/fulltextIndex/xapian`.
3. Bench head-to-head on `simple` (small enough to iterate):
   real libzim mwoffliner image vs shim+zimru mwoffliner image.
   Compare wall time, peak RSS, output size, search results
   parity.
4. Once `simple` is clean, pilot on `en_simple` then `en_all`
   under a Zimfarm test worker.

## Summary

The drop-in surface is **mwoffliner + libzim-shim + zimru**.
The headline blockers are (a) Xapian writer integration in the
shim, (b) streaming cluster compression in zimru. The CLI
flags Kiwix uses are uninteresting (no tuning exposed); the work
lives in the API layer mwoffliner calls into.
