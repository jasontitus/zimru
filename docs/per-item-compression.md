# Per-item compression override

The writer's compression mode is per-cluster, not per-build. By default
every cluster uses the Creator-wide `Compression` setting. When a build
needs to mix compressed and uncompressed clusters in a single ZIM,
items can opt out of compression individually via `Item::compress`.

## Why this exists

The motivating use case is streetzim: continent-sized routing graphs
(>500 MB) bust PWA `fzstd`'s per-cluster decompression cap. Those items
must land in raw clusters (`info_byte = 1`) while the same archive's
tiles, HTML, and JS continue to use zstd. Without per-item override the
choice is forced one way or the other for the whole build.

## Semantics

```rust
pub struct Item {
    /* … */
    pub compress: Option<bool>,
}
```

| `compress`      | Effective cluster compression                         |
|-----------------|--------------------------------------------------------|
| `None`          | Creator default (`set_compression` value)             |
| `Some(true)`    | Creator default — explicit, identical to `None`       |
| `Some(false)`   | `Compression::None` regardless of Creator default     |

Only `Some(false)` overrides the default. `Some(true)` is for callers
that want to record intent (e.g. round-tripping a `compress=true`
manifest field) without changing behaviour.

## Cluster routing

Items are bucketed by `(strategy_key, effective_compression)`. Two items
with the same path/mime but differing compression land in different
buckets — and therefore different clusters — so the writer can encode
each cluster with its own compression setting at flush time.

Bucket keys are namespaced by a one-character compression tag (`n` /
`z` / `x`) which keeps the BTreeMap iteration order interleaved
across modes — useful when scanning clusters in tools or tests.

## Streaming-encode dispatch

`Creator::begin_chunked_item` accepts a `compress: Option<bool>` and
picks one of three paths based on `expected_size` and the effective
compression:

| condition                                                 | path             |
|-----------------------------------------------------------|------------------|
| `expected_size >= STREAMING_ENCODE_THRESHOLD` & zstd      | `StreamingZstd`  |
| `expected_size >= STREAMING_ENCODE_THRESHOLD` & none      | `StreamingRaw`   |
| anything else                                             | `Buffered`       |

`StreamingRaw` mirrors `StreamingZstd`'s temp-file splice contract but
skips the encoder entirely: chunks are written straight through to the
per-item temp file (after the `info_byte = 1` + ptr table), and the
temp file is concatenated into the main output in `cluster_idx` order
at finalize time. Memory peak is the chunk size.

## API

### `Item::with_compress(bool)`

```rust
let raw = Item::new("graph.bin", "Routing", "application/octet-stream", body)
    .with_compress(false);
creator.add_item(raw);
```

### `ItemBuilder::set_compress(Option<bool>)`

For chunked feeds:

```rust
let mut b = creator.begin_item("graph.bin", "Routing",
                               "application/octet-stream",
                               None, Some(size))?;
b.set_compress(Some(false));
b.write_chunk(&chunk);
b.finish()?;
```

### `Creator::begin_chunked_item(..., compress)`

The internal C-ABI-shape entrypoint takes the override directly:

```rust
creator.begin_chunked_item(
    None,                       // namespace
    "graph.bin".into(),
    "Routing".into(),
    "application/octet-stream".into(),
    Some(size),                 // expected_size
    Some(false),                // compress
)?;
```

## Backwards compatibility

- `Item::new`, `Item::html`, `Item::text`, `Item::png`, and
  `Item::in_namespace` default to `compress: None`. Existing callers
  see no behaviour change.
- `Creator::begin_chunked_item` gained a trailing `Option<bool>`
  argument. Existing callers must pass `None` (or `Some(true)`) for
  identical behaviour.
- The C ABI's `zimru_creator_begin_chunked_item` continues to default
  to `None` until/unless the C surface is extended.

## Tests

`tests/per_item_compression.rs` covers:

1. A raw item lands in its own `Compression::None` cluster while a
   sibling lands in zstd (`raw_item_lands_in_its_own_uncompressed_cluster`).
2. `Some(true)` honours the Creator default — flipping the default to
   `Compression::None` keeps everything raw
   (`compress_true_honours_creator_default`).
3. The chunked-streaming `StreamingRaw` path produces a raw cluster
   while a sibling buffered HTML lands in zstd
   (`streaming_raw_passthrough_for_huge_uncompressed_items`).

The full test suite (70 tests) passes after the change.
