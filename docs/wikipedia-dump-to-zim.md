# Wikipedia ZIM from dumps — feasibility study

The current Wikipedia ZIM build (zimfarm's `wikipedia_en_all_maxi`)
is a **~10 day live crawl** of `en.wikipedia.org` via the
MediaWiki Action API. The writer is a tiny fraction of that wall
time; even a perfect zimru replacement of libzim only trims
minutes off a multi-day job. The leverage move — if we want to
materially shrink Wikipedia build time — is to skip the crawl
entirely and build directly from Wikimedia's existing public
**HTML dumps**.

This document evaluates the feasibility, scope, and effort.
It was written 2026-04-27 against the snapshot of mwoffliner /
zimfarm / Wikimedia Enterprise visible at that date.

## TL;DR

- **The dumps exist, are free, and are usable.** Wikimedia
  Enterprise's Snapshot API publishes the full English Wikipedia
  in NDJSON (one article per line, with parsed HTML in-band) on
  the 2nd and 21st of every month. ~50–100 GB compressed for
  English; ~915 GB across all projects.
- **No one has built a dump → ZIM tool.** mwoffliner's
  `Downloader.ts` only knows how to call live Action API
  endpoints. There's no open issue actively pursuing dump-based
  ingestion, and no existing OSS pipeline that does it
  end-to-end.
- **The hard problem is image bytes, not article text.** The
  HTML dump references images by URL but doesn't include the
  bytes — those still have to be fetched, from
  `upload.wikimedia.org` or from Commons bulk dumps. About 60 %
  of the 10-day crawl wall time is image fetching, so eliminating
  it requires a real solution here, not just dump ingestion.
- **MVP effort: 4–6 weeks** for a "good enough English Wikipedia
  ZIM, no images" prototype that proves the writer pipeline.
  **Production parity with mwoffliner: 6+ months** because of
  HTML rewrite edge cases, redirect handling, search index
  generation, and asset pipeline.
- **Realistic time cut: 10 days → ~1–2 days end-to-end** on the
  same hardware, dominated by image fetch bandwidth.

## What the dumps actually contain

Wikimedia Enterprise's [Snapshot API](https://enterprise.wikimedia.com/docs/snapshot/)
ships compressed `tar.gz` archives, each containing a single
`.ndjson` file. One line = one article. Free tier with bearer
token, no special agreement needed.

Schedule and size:

- **Cadence**: 2nd and 21st of every month (paid tier: daily at
  12:00 UTC).
- **Total archive size**: 915.84 GB across 6,540 files
  (March 2025 census across all Wikimedia projects).
- **English Wikipedia (enwiki_namespace_0)**: not separately
  documented but estimated 50–100 GB compressed based on the
  published density.
- **Format**: gzip-compressed NDJSON, one parsed article per
  line.

The schema (per [Wikimedia Enterprise data
dictionary](https://enterprise.wikimedia.com/docs/data-dictionary/))
includes for each article:

- `name`, `url`, `identifier` — the title and canonical URL.
- `article_body.html` — **rendered HTML, ready to embed in a
  ZIM** (this is the entire point — Parsoid output, templates
  resolved, citations expanded).
- `article_body.wikitext` — also included if you want it.
- `image.content_url` — main article image URL (Commons or
  upload.wikimedia.org). Bytes NOT included.
- `categories[]` — `{name, url}` per category.
- `references[]` (Structured Contents beta) — citations with
  identifier/type/text/metadata.
- `sections[]` (Structured Contents beta) — hierarchical
  decomposition with `has_parts` recursion.
- `infoboxes[]` (Structured Contents beta) — parsed infobox tree.
- Plus revision metadata, license, redirects target.

The presence of `article_body.html` is the magic ingredient. It
makes a dump-based ZIM build **fundamentally a transform-and-
rewrite job**, not a render job. We do not need Parsoid; we do
not need a wikitext parser; we do not need template resolution.
Wikimedia has already done all of that.

## What mwoffliner does that we'd need to replicate

mwoffliner is ~10,000 lines of TypeScript. Most of it is fetch
machinery (which we'd throw away) and HTML transformation
(which we'd keep). A rough decomposition by lines of code:

| stage | LOC est. | needed for dump-based path? |
|---|---:|---|
| MediaWiki API discovery / auth | ~1500 | **No** — replaced by NDJSON read |
| Article HTML download (Action API, REST API, Parsoid) | ~2000 | **No** |
| Article queue + retry + rate limit | ~800 | **Reduced** — only image fetcher needs it |
| HTML rewriting (links, edit-buttons, scripts) | ~2500 | **Yes — port directly** |
| Image / media URL extraction + fetch | ~1500 | **Yes for fetch; HTML processing reused** |
| Redirect graph collection | ~400 | **Yes — derive from dump's redirect entries** |
| ZIM emission via @openzim/libzim | ~600 | **Replaced by zimru's chunked-input API** |
| Title index / search index hooks | ~300 | **Yes — covered by zimru's auto-emit** |
| CSS/JS bundling for offline rendering | ~400 | **Yes — port the asset payload** |

So roughly **half** of mwoffliner's logic is reusable shape; the
other half goes away. The remaining half is the real cost.

### The HTML transform pass — where the work actually is

Each article's `article_body.html` is *almost* but not quite
ready to drop into a ZIM. mwoffliner does these transforms,
and a dump-based tool needs to do them too:

1. **Rewrite internal links.** `<a href="/wiki/Foo">` becomes
   `<a href="A/Foo">` (or whatever ZIM path scheme the consumer
   tools expect — kiwix-serve has historical assumptions here).
   Anchored links (`/wiki/Foo#bar`) preserve the fragment.
2. **Strip live-only chrome.** Edit-section buttons, "log in"
   prompts, talk-page tabs, "watch this article" UI, references
   to live JS endpoints. Most of these are class-tagged
   (`mw-editsection`, `mw-empty-elt`, etc.) so it's CSS-selector
   driven removal.
3. **Rewrite resource URLs.** `<img src="//upload.wikimedia.org/.../File.jpg">`
   becomes `<img src="I/File.jpg">`. Same for video/audio
   posters where present.
4. **Strip or rewrite client-side scripts.** Inline `<script>`
   blocks that hook into MediaWiki's RL (ResourceLoader) get
   removed; some get replaced by tiny offline shims (e.g. for
   collapsible infoboxes, math rendering).
5. **Inject offline CSS bundle.** A pre-built `style.css` bundle
   is added under `-/assets/` and `<link>` tags injected into
   each article's `<head>`. Prevents every article from being a
   visual disaster offline.
6. **Resolve redirects.** mwoffliner emits ZIM redirect
   entries (`Item::redirection`) for every redirect article.
   The dump includes redirect targets in its metadata, so this
   is straightforward — but it's a separate per-article
   bookkeeping step.

None of these are conceptually hard, but the cumulative edge
cases (special-case templates, language variants, math/chemistry
extensions, citation formats) are why mwoffliner has been worked
on for ~7 years. A clean re-implementation for English would
hit 80 % parity in 4–6 weeks; the last 20 % is years of polish.

## The image problem

Article HTML in the dump references images by URL:

```
<img src="//upload.wikimedia.org/wikipedia/commons/thumb/.../Foo.jpg/800px-Foo.jpg" ...>
```

The image bytes are **not in the dump**. To produce a ZIM
that's actually readable offline, a dump-to-ZIM tool has three
options for getting the bytes:

### Option A: fetch from upload.wikimedia.org

Roughly the same approach mwoffliner takes today. Parallel HTTP
GETs to the canonical image hosts.

- **Pros**: gets exactly the rendered thumbnail sizes
  referenced by the HTML; no over-fetching.
- **Cons**: ~6 M images per English Wikipedia; ~50 KB avg
  thumbnail size = ~300 GB egress. At 100 Mbit/s sustained
  fetch rate (a generous assumption — Wikimedia rate-limits
  bots) that's ~7 hours of pure download. Realistic with
  retries: 10–15 hours.
- **Net**: turns the 10-day API crawl into a ~12-hour image
  fetch + ~1-hour ZIM build. **Order of magnitude wall-time
  improvement.**

### Option B: Wikimedia Commons bulk dumps

Commons publishes daily-incremental zip dumps of all uploads.

- **Pros**: bulk download, no API rate limit.
- **Cons**: dumps include every Commons upload (40+ M files,
  multi-TB) without thumbnail variants. We'd need to download
  the whole archive, run our own thumbnailer, then pick which
  thumbnails our ZIM references. The disk and CPU cost is
  enormous.
- **Net**: only worth it if we're producing many ZIMs (Wikipedia
  in 100+ languages) — amortise the bulk download once.

### Option C: skip images (pic-less ZIM)

- mwoffliner already produces `nopic` and `mini` formats. A
  dump-based pipeline could trivially emit the same.
- 5 % of users want this format; 95 % want pictures.

**My recommendation**: implement Option A first (gets the
realistic Wikipedia user experience), keep Option B in mind as
a future optimisation when we're producing multiple language
ZIMs in the same pipeline.

## Architecture sketch

```
┌─────────────────────────────────┐
│ Wikimedia Enterprise Snapshot   │
│   GET /v2/snapshots/enwiki/     │
│   download                      │  ~50–100 GB tar.gz
└──────────────┬──────────────────┘
               │ stream-decompress
               ▼
┌─────────────────────────────────┐
│ NDJSON line reader              │  serde_json::Deserializer<Read>
│   one article = one line        │  no full-file slurp
└──────────────┬──────────────────┘
               │
               ▼
┌─────────────────────────────────┐
│ HTML transform pass             │  scraper / kuchikiki crate
│   - rewrite internal links      │  pure CPU work, parallelisable
│   - strip live chrome           │  via rayon par_iter on lines
│   - rewrite img src to I/...    │
│   - emit redirect map entries   │
│   - extract image URLs          │──────┐
└──────────────┬──────────────────┘      │
               │                          ▼
               │                ┌─────────────────────┐
               │                │ Parallel image      │
               │                │ fetcher (rayon +    │
               │                │ reqwest::blocking)  │
               │                │ → I/<filename>.bin  │
               │                └─────────┬───────────┘
               │                          │
               ▼                          ▼
┌─────────────────────────────────────────────────────┐
│ zimru Creator (chunked-input API)                   │
│   begin_chunked_item / chunked_item_chunk /         │
│   end_chunked_item — one call per article + image   │
│   Auto-emits M/Counter, X/listing/titleOrdered/v1   │
└──────────────────┬──────────────────────────────────┘
                   │
                   ▼
              wikipedia.zim
```

zimru's existing chunked-input writer is a perfect fit. We
already prove this works for streaming large items (the texas
benchmark pushes a 1.74 GB JSON through it). Adding ~6 M small
items + ~6 M small image blobs is the same shape, just more
of them.

## Effort estimate by milestone

### Phase 1 — End-to-end MVP (3–4 weeks)

Goal: produce a `wikipedia_en_simple.zim` from a Simple Wikipedia
dump (~200 K articles), no images, basic link rewriting. Proves
the pipeline.

- Week 1: dump downloader + NDJSON streaming reader. ~200 LOC.
- Week 1–2: HTML transform pass for links + chrome strip,
  using `kuchikiki` (Mozilla's `html5ever`). ~600 LOC.
- Week 2–3: redirect handling, ZIM emission via zimru's
  Creator, integration tests. ~300 LOC.
- Week 3–4: zimcheck + manual review against a kiwix-serve
  instance loading the output. Bug fixing.

End state: a 100 MB English Simple Wikipedia ZIM that opens
and reads correctly in Kiwix, no images. **~1 hour build time
on a workstation.**

### Phase 2 — English Wikipedia, with images (3–4 weeks)

Goal: full `wikipedia_en_all_maxi`-equivalent, image-bearing.

- Week 1–2: parallel image fetcher with proper backoff for
  Wikimedia's rate limits (it caps at ~50 req/s for bots).
  ~400 LOC.
- Week 2–3: WebP transcoding (mwoffliner emits WebP via
  `--webp`). ~200 LOC plus image2webp library binding.
- Week 3: scale-up testing on full English Wikipedia.
  Memory profiling, retry-on-network-blip handling.
- Week 4: side-by-side validation against an existing
  `wikipedia_en_all_maxi.zim`: Are 99 % of pages
  byte-comparable on the article HTML? Are image counts
  matching? Are redirect counts matching?

End state: a tool that produces a Wikipedia ZIM in **~1–2 days
end-to-end**, dominated by the image fetch wall.

### Phase 3 — Production parity (3–6 months, optional)

Goal: full mwoffliner replacement. The ~20 % polish that takes
the dominant share of the time.

- Search index emission (`X/fulltext/xapian`,
  `X/title/xapian`). zimru-shim already has the C ABI hooks;
  we'd need to drive the Xapian builder from the dump pass.
- All language editions (mwoffliner handles 300+; corner cases
  with right-to-left scripts, CJK, etc.).
- Math rendering, chemistry diagrams, music notation —
  Wikipedia uses `<math>` (rendered to MathML/SVG by Parsoid,
  preserved in dumps), but offline rendering may need extra
  CSS / fonts.
- Special pages (Recent changes, Categories pages, etc.) —
  mwoffliner generates synthetic versions; we'd need to match.
- Custom main page, custom favicon, "namespaces" addition
  (zimfarm passes `--addNamespaces=100`).

### Phase 4 — image bytes from Commons dumps (optional, later)

If we're producing Wikipedia in N languages, fetching ~6 M
images N times is wasteful. A one-time Commons bulk download +
local thumbnail server amortises across all languages.

This is a 1–2 month project on its own, only worth it once we
have multi-language production traffic.

## Risks and mitigations

| risk | likelihood | impact | mitigation |
|---|---|---|---|
| HTML format changes between dumps | low | medium | Pin schema version; smoke-test against latest dump |
| Wikimedia changes free-tier access policy | medium | high | Cache dumps locally; ensure the tool works offline once dump is downloaded |
| Image-fetch rate limits tighten | medium | high | Cooperative throttling; retry-with-backoff; partner with Wikimedia for image-dump access |
| HTML transform misses an edge case | high | low | Side-by-side diff against mwoffliner output for a sample |
| Search index missing → kiwix-serve `/search` empty | low | medium | Pulled from libzim-shim's existing Xapian path |
| Commons URLs change format | low | high | Use the dump's full canonical URL, not assumptions |

## Comparison: dump-to-ZIM vs. current crawl

|                     | mwoffliner crawl | dump-to-zim (proposed) |
|---------------------|-----------------:|-----------------------:|
| Wall time (en Wiki) | ~10 days         | ~1–2 days              |
| Wikipedia API load  | high             | none                   |
| Reproducibility     | snapshot drift   | bit-stable per dump    |
| Storage required    | ~150 GB          | ~150 GB (dump + work)  |
| Update cadence      | quarterly        | bi-monthly possible    |
| Developer cost      | sunk             | ~2 months (MVP path)   |
| Maintenance burden  | tracks API drift | tracks dump schema     |

The "tracks dump schema" maintenance is dramatically lighter
than "tracks API drift" — schemas change quarterly at most;
mwoffliner has had to chase MediaWiki API quirks every few
months for years.

## Recommendation

**Worth doing if** any of these are true:

1. We actually want to ship Wikipedia ZIMs faster than
   quarterly. The current cadence is API-bound; dumps change
   that.
2. We want a more reproducible build pipeline (e.g. for ZIMs
   used as ML training data).
3. We're building zimru into a broader "ZIM fabric" tool and
   Wikipedia is the headline use case.

**Don't bother if** the goal is just to make existing zimfarm
faster — the leverage there is on the writer (zimru already
gets us 2× faster + 30 % smaller at the same compression
level), not the source-of-truth swap.

The MVP is small enough (3–4 weeks) that it would be
**worth doing as a proof-of-concept** even if production
adoption is uncertain. The artifact would be:

- A standalone Rust binary `wikidumpzim` that takes
  `--dump=path/to/enwiki.tar.gz --out=enwiki.zim` and produces
  a kiwix-serveable ZIM without ever touching the live API.
- Clear measurement of how long each phase takes (download,
  transform, image fetch, ZIM write) so we know the realistic
  ceiling vs. the current 10-day baseline.
- A go/no-go on whether the production-parity work is worth
  doing.

## Sources

- [Wikimedia Enterprise Snapshot API documentation](https://enterprise.wikimedia.com/docs/snapshot/)
- [Wikimedia Enterprise data dictionary](https://enterprise.wikimedia.com/docs/data-dictionary/)
- [Wikimedia Enterprise HTML Dump Archive (now-deprecated public mirror)](https://dumps.wikimedia.org/other/enterprise_html/)
- [mwparserfromhtml — Wikimedia Foundation's Python library for HTML dump parsing](https://pypi.org/project/mwparserfromhtml/)
- [Wikimedia Tech Blog: From hell to HTML (2023)](https://techblog.wikimedia.org/2023/02/24/from-hell-to-html/)
- [mwoffliner source on GitHub (Downloader.ts)](https://github.com/openzim/mwoffliner/blob/main/src/Downloader.ts)
- [Zimfarm `wikipedia_en_all_maxi` recipe configuration](https://farm.openzim.org/recipes/wikipedia_en_all_maxi/config)
- [Wikipedia:Database download](https://en.wikipedia.org/wiki/Wikipedia:Database_download)
- [Wikimedia Commons download tools](https://commons.wikimedia.org/wiki/Commons:Download_tools)
- [Parsoid — MediaWiki](https://www.mediawiki.org/wiki/Parsoid)
- [openzim/mwoffliner on GitHub](https://github.com/openzim/mwoffliner)
