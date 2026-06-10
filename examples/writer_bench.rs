//! Synthetic writer benchmark: streams a generated corpus through
//! `Creator` and reports wall time + throughput. Run with:
//!
//! ```text
//! cargo run --release --example writer_bench -- [items] [avg_item_kb] [out.zim]
//! ZIMRU_STATS=1 cargo run --release --example writer_bench
//! ```
//!
//! The corpus is deterministic (xorshift-seeded) and ~3-4× zstd-
//! compressible, mimicking HTML-heavy ZIM payloads, so runs are
//! comparable across machines and across writer changes.

use std::time::Instant;

use zimru::writer::{Creator, Item};

/// Deterministic xorshift64* generator — no rand dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

const WORDS: &[&str] = &[
    "the",
    "quick",
    "brown",
    "fox",
    "jumps",
    "over",
    "lazy",
    "dog",
    "wiki",
    "article",
    "section",
    "header",
    "content",
    "paragraph",
    "reference",
    "history",
    "table",
    "data",
    "<p>",
    "</p>",
    "<div class=\"mw-body\">",
    "</div>",
    "<span>",
    "</span>",
];

fn make_body(rng: &mut Rng, target_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target_len + 16);
    while out.len() < target_len {
        let w = WORDS[(rng.next() % WORDS.len() as u64) as usize];
        out.extend_from_slice(w.as_bytes());
        out.push(b' ');
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let items: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let avg_kb: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(16);
    let out = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "/tmp/zimru-writer-bench.zim".to_string());

    let mut rng = Rng(0x5EED_CAFE_F00D_BEEF);
    let started = Instant::now();
    let mut total_bytes = 0u64;

    let mut c = Creator::new();
    c.set_main_path("article0");
    c.add_metadata("Title", "writer-bench");
    c.add_metadata("Language", "eng");
    c.add_metadata("Description", "synthetic writer benchmark corpus");
    c.add_metadata("Creator", "zimru");
    c.add_metadata("Publisher", "zimru");
    c.add_metadata("Date", "2026-06-10");
    c.add_metadata("Name", "zimru-writer-bench");
    c.start_writing(&out).expect("start_writing");

    for i in 0..items {
        // Spread sizes 0.25×..4× around the average for realistic
        // bin-packing behaviour.
        let jitter = (rng.next() % 16) as usize + 1; // 1..=16
        let len = (avg_kb * 1024 * jitter) / 4;
        let body = make_body(&mut rng, len);
        total_bytes += body.len() as u64;
        let path = format!("article{i}");
        let title = format!("Article {i}");
        c.add_item(Item::html(path, title, body));
        if i % 7 == 0 {
            c.add_redirection(
                format!("alias{i}"),
                format!("Alias {i}"),
                format!("article{i}"),
            );
        }
    }
    let produce_done = started.elapsed();
    c.finish_writing().expect("finish_writing");
    let total = started.elapsed();

    let out_size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!(
        "items={items} raw={:.1} MB out={:.1} MB produce+encode={:.2}s total={:.2}s ({:.1} MB/s raw)",
        total_bytes as f64 / 1048576.0,
        out_size as f64 / 1048576.0,
        produce_done.as_secs_f64(),
        total.as_secs_f64(),
        total_bytes as f64 / 1048576.0 / total.as_secs_f64(),
    );
}
