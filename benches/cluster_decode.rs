//! Microbenchmark for ZIMRU_PERF.md item P4: zstd22 cold-cluster
//! decompression (zimru ~25 ms vs libzim ~1 ms on a 2 MB block).
//!
//! Two axes:
//!
//! * **Compression level.** zstd levels 3 (default), 19 (kiwix
//!   default), 22 (max, used by `*-zstd22.zim` archives). Level 22's
//!   window log is 27 (128 MB) vs ≤ 23 for lower levels, so on paper
//!   the decoder has a much bigger working set.
//! * **Decompressor lifetime.** Fresh `bulk::decompress` per call
//!   (allocates a new `DCtx` each time) vs a thread-local
//!   `bulk::Decompressor::new()` reused across calls.
//!
//! ## Findings (2026-04-25, M-series Mac, realistic ~3.7× ratio data)
//!
//! | level | fresh DCtx | pooled DCtx |
//! |------:|-----------:|------------:|
//! |     3 |    644 µs  |     658 µs  |
//! |    19 |    600 µs  |     618 µs  |
//! |    22 |    606 µs  |     619 µs  |
//!
//! Three things to take away:
//!
//! 1. **zstd22 is NOT inherently slow.** Decompression at level 22 is
//!    within 1 % of level 19 — both about 600 µs/2 MB ≈ 3.3 GB/s.
//!    The 25 ms cold gap reported in the perf doc cannot be a raw
//!    libzstd-level effect.
//! 2. **DCtx pooling does not help.** The "pooled" variant ends up
//!    ~10–20 µs slower because reusing the decompressor forces us to
//!    pre-allocate (and zero-fill) a `vec![0u8; size]` output buffer
//!    every call; `bulk::decompress` skips the zero-fill internally.
//! 3. **Most likely root cause for the 25 ms gap.** Cold mmap page
//!    faults on first access to the compressed cluster bytes from
//!    disk — not zstd, but the I/O underneath. Validating that needs
//!    the same `wikipedia_en_movies_nopic_2025-08-zstd22.zim` ZIM the
//!    perf doc benchmarked against, on the same platform; with the
//!    cluster bytes warm in the page cache, our `decode_zstd` path
//!    is already at the libzstd ceiling.
//!
//! The bench harness stays here so future investigators (or anyone
//! pointing zimru at a fresh zstd22 archive on a fresh-boot host)
//! can re-run quickly.

use std::cell::RefCell;
use std::time::Duration;

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};

const PAYLOAD_SIZE: usize = 2 * 1024 * 1024;

thread_local! {
    /// Reusable bulk decompressor — once created, the underlying
    /// libzstd `DCtx` keeps its window buffer across calls.
    static REUSABLE_DCTX: RefCell<Option<zstd::bulk::Decompressor<'static>>> =
        const { RefCell::new(None) };
}

/// Build a 2 MB payload with realistic-text compression characteristics
/// (~3–6× ratio at zstd default, ~6–10× at zstd 22) — pseudo-random
/// noise bytes interleaved with repeated phrases at varied stride so
/// zstd's LZ matcher has something to chew on without the payload
/// collapsing to a near-zero compressed size. Without this, a tiny
/// vocabulary repeated nets 4 000× ratios that don't exercise the
/// decoder's hot path the way real Wikipedia HTML does.
fn synth_payload(target_size: usize) -> Vec<u8> {
    const VOCAB: &[&str] = &[
        "the quick brown fox jumps over the lazy dog. ",
        "alice was beginning to get very tired of sitting by her sister. ",
        "it was the best of times, it was the worst of times. ",
        "all happy families are alike; each unhappy family is unhappy in its own way. ",
        "in a hole in the ground there lived a hobbit. ",
        "call me ishmael. some years ago, never mind how long precisely. ",
        "to be or not to be, that is the question. ",
        "the past is a foreign country: they do things differently there. ",
    ];
    let mut out = Vec::with_capacity(target_size + 1024);
    // Linear-congruential PRNG for deterministic-but-unpredictable noise.
    let mut state: u64 = 0xCAFEBABED00DBEEF;
    let mut step = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    while out.len() < target_size {
        // Alternate between a vocab phrase (compressible) and a 16–64 byte
        // chunk of pseudo-random ASCII (incompressible). Net ratio comes
        // in around 4–8× at zstd default which matches real wiki HTML.
        let idx = (step() as usize) % VOCAB.len();
        out.extend_from_slice(VOCAB[idx].as_bytes());
        let noise_len = (step() as usize) % 48 + 16;
        for _ in 0..noise_len {
            let byte = b'a' + (step() % 26) as u8;
            out.push(byte);
        }
    }
    out.truncate(target_size);
    out
}

fn decompress_fresh(body: &[u8]) -> Vec<u8> {
    let size = zstd::zstd_safe::get_frame_content_size(body)
        .ok()
        .flatten()
        .map(|n| n as usize)
        .unwrap_or(body.len() * 8);
    zstd::bulk::decompress(body, size).expect("decompress")
}

fn decompress_pooled(body: &[u8]) -> Vec<u8> {
    let size = zstd::zstd_safe::get_frame_content_size(body)
        .ok()
        .flatten()
        .map(|n| n as usize)
        .expect("benchmark frames are written with pledgedSrcSize");
    REUSABLE_DCTX.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            *opt = Some(zstd::bulk::Decompressor::new().expect("Decompressor::new"));
        }
        let dec = opt.as_mut().unwrap();
        let mut out = vec![0u8; size];
        let n = dec.decompress_to_buffer(body, &mut out).expect("decompress");
        out.truncate(n);
        out
    })
}

fn bench_cluster_decode(c: &mut Criterion) {
    let payload = synth_payload(PAYLOAD_SIZE);
    let mut group = c.benchmark_group("cluster_decode");
    group.throughput(Throughput::Bytes(PAYLOAD_SIZE as u64));
    // zstd22's 128 MB window means a single fresh-DCtx call can take
    // tens of ms; bump measurement time so criterion can collect a
    // statistically meaningful sample without exploding the bench
    // wall clock.
    group.measurement_time(Duration::from_secs(5));

    for &level in &[3i32, 19, 22] {
        // Compress at the level being benched. Both `bulk::compress`
        // and `stream::encode_all` write the pledgedSrcSize header so
        // `get_frame_content_size` returns the exact value.
        let compressed = zstd::bulk::compress(&payload, level).expect("compress");
        let ratio = payload.len() as f64 / compressed.len() as f64;
        eprintln!("level {level:>2}: compressed {} → {} bytes (×{:.2})",
                  payload.len(), compressed.len(), ratio);

        // Reset the thread-local DCtx so the "cold" measurement at this
        // level isn't pre-warmed by a prior level's window buffer.
        REUSABLE_DCTX.with(|c| { *c.borrow_mut() = None; });

        group.bench_with_input(
            BenchmarkId::new("fresh_dctx", level),
            &compressed,
            |b, body| b.iter(|| black_box(decompress_fresh(body))),
        );
        group.bench_with_input(
            BenchmarkId::new("pooled_dctx", level),
            &compressed,
            |b, body| b.iter(|| black_box(decompress_pooled(body))),
        );
    }

    group.finish();
}

criterion_group!(benches, bench_cluster_decode);
criterion_main!(benches);
