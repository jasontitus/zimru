//! Microbenchmark for `Archive::get_entry_by_path`.
//!
//! Tracks ZIMRU_PERF.md item P1: identify whether the 4.4× gap vs libzim
//! on path lookup is dominated by allocation (per-call CString /
//! Entry construction) or by memory traffic (cold mmap pages, dirent
//! binary search visiting ~20 distinct pages on a 700 K-entry archive).
//!
//! The two cases compared:
//!   * `same_path_repeated`  — the same path looked up in a tight loop.
//!     Steady state has no mmap fault traffic; cost reflects the
//!     per-call allocation/parsing pipeline.
//!   * `rotating_paths`      — N distinct paths cycled in order.
//!     With a small N each path hits cached pages, so this is still
//!     allocation-dominated. With a large N the working set exceeds
//!     cached pages and per-op cost rises with the page-fault tail.
//!
//! If the two cases are within ~10 %, the per-call CString / Entry
//! allocation pipeline dominates and the fix lives there. If
//! `rotating_paths` is materially slower for large N, the dirent walk
//! is paying mmap faults and a sparse anchor / `madvise(WILLNEED)` is
//! the right next move.
//!
//! Set `ZIMRU_BENCH_ZIM` to a path before running; the bench prints a
//! skip message and exits cleanly otherwise so `cargo bench` on a
//! fresh checkout doesn't fail.

use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use zimru::Archive;

fn bench_zim_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ZIMRU_BENCH_ZIM") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let fallback = PathBuf::from("zim-cache/wikipedia_ba_all_maxi.zim");
    if fallback.exists() {
        return Some(fallback);
    }
    None
}

/// Pull a representative set of paths out of the archive so the
/// rotating-lookup variant exercises a real distribution rather than
/// adjacent dirents. Up to `cap` paths, evenly spaced across the
/// content namespace.
fn sample_paths(arc: &Archive, cap: usize) -> Vec<String> {
    let total = arc.entry_count();
    let mut out = Vec::with_capacity(cap.min(total as usize));
    let stride = (total as usize / cap.max(1)).max(1);
    for i in (0..total).step_by(stride) {
        if let Ok(entry) = arc.entry_by_url_index(i) {
            if !entry.is_redirect() {
                out.push(entry.path().to_string());
                if out.len() >= cap {
                    break;
                }
            }
        }
    }
    out
}

fn bench_get_entry_by_path(c: &mut Criterion) {
    let Some(zim) = bench_zim_path() else {
        eprintln!(
            "skip get_entry_by_path bench: set ZIMRU_BENCH_ZIM=/path/to/file.zim \
             (or place one at zim-cache/wikipedia_ba_all_maxi.zim)"
        );
        return;
    };
    eprintln!("benching against {}", zim.display());
    let arc = Archive::open(&zim).expect("Archive::open");

    let target_path = arc
        .iter_by_path()
        .filter_map(Result::ok)
        .find(|e| !e.is_redirect())
        .map(|e| e.path().to_string())
        .expect("archive has at least one non-redirect entry");

    let mut group = c.benchmark_group("get_entry_by_path");
    group.throughput(Throughput::Elements(1));

    group.bench_function("same_path_repeated", |b| {
        b.iter(|| {
            let _ = arc
                .get_entry_by_path(black_box(target_path.as_str()))
                .expect("lookup");
        })
    });

    for &n in &[8usize, 64, 512, 4096] {
        let paths = sample_paths(&arc, n);
        if paths.len() < n {
            eprintln!(
                "rotating_paths/{n}: archive only has {} non-redirect entries; \
                 skipping",
                paths.len()
            );
            continue;
        }
        group.bench_with_input(BenchmarkId::new("rotating_paths", n), &paths, |b, paths| {
            let mut idx = 0usize;
            b.iter(|| {
                let p = unsafe { paths.get_unchecked(idx) };
                idx = (idx + 1) % paths.len();
                let _ = arc
                    .get_entry_by_path(black_box(p.as_str()))
                    .expect("lookup");
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_get_entry_by_path);
criterion_main!(benches);
