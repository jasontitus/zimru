//! Bench: `Item::warmup()` on a real multi-GB ZIM.
//!
//! Verifies P7 (libzim-shim's `ZIMRU_PERF.md`): the shim sees a 12 s
//! hang on first `/search` after kiwix-serve restart on the 48 GB
//! Wikipedia, caused by Xapian's eager DB-validation reads scattered
//! across the cold mmap. zimru's `Item::warmup` forces those pages
//! resident upfront; this bench measures
//!
//!   1. cold cost — first read of the item's bytes after a fresh
//!      process (no warmup),
//!   2. warm cost — second read in the same process (page cache
//!      already populated),
//!   3. warmup cost — the explicit `Item::warmup()` call itself,
//!      which should match (1) within noise: warmup *is* the cold
//!      read, just done on purpose.
//!
//! If `cold ≈ warmup`, the primitive does what it advertises (pays
//! the disk-read cost upfront). The shim then calls it once at
//! kiwix-serve start; the first user `/search` no longer waits.
//!
//! Set `ZIMRU_BENCH_ZIM` to the path of a multi-GB archive. Optional
//! `ZIMRU_BENCH_PATH` selects which entry to warm (default: tries
//! `X/listing/fulltextIndex/xapian` then `X/fulltextIndex/xapian`,
//! then falls back to the first direct-access non-redirect item).
//!
//! Best run on a freshly-rebooted machine or after `purge` so the
//! page cache is cold. Without that, "cold" measurements collapse
//! to "warm" since the OS already cached the bytes.

use std::path::PathBuf;
use std::time::Instant;

use zimru::{Archive, Item};

fn bench_zim() -> Option<PathBuf> {
    std::env::var("ZIMRU_BENCH_ZIM").ok().map(PathBuf::from).filter(|p| p.exists())
}

fn pick_item(arc: &Archive) -> Option<Item> {
    if let Ok(path) = std::env::var("ZIMRU_BENCH_PATH") {
        return arc.get_item(&path).ok();
    }
    // Try the conventional Xapian fulltext-index paths first.
    for (ns, url) in [
        (b'X', "listing/fulltextIndex/xapian"),
        (b'X', "fulltextIndex/xapian"),
        (b'X', "fulltext/xapian"),
        (b'X', "title/xapian"),
    ] {
        if let Ok(entry) = arc.entry_by_ns_path(ns, url) {
            if let Ok(item) = entry.get_item(false) {
                return Some(item);
            }
        }
    }
    // Fallback: first direct-access non-redirect entry we can find.
    for i in 0..arc.entry_count() {
        let Ok(entry) = arc.entry_by_url_index(i) else {
            continue;
        };
        if entry.is_redirect() {
            continue;
        }
        let Ok(item) = entry.get_item(false) else {
            continue;
        };
        if arc
            .blob_direct_access(item.cluster_index(), item.blob_index())
            .map(|d| d.is_direct)
            .unwrap_or(false)
        {
            return Some(item);
        }
    }
    None
}

/// Touch one byte per 4 KB page through the item's bytes. Returns the
/// (acc, elapsed) so the compiler doesn't dead-code-eliminate the read.
fn touch_pages(item: &Item) -> (u64, std::time::Duration) {
    let blob = item.get_data().expect("get_data");
    let bytes = blob.data();
    let start = Instant::now();
    let mut acc: u64 = 0;
    let mut i = 0;
    while i < bytes.len() {
        acc = acc.wrapping_add(bytes[i] as u64);
        i += 4096;
    }
    (acc, start.elapsed())
}

fn main() {
    let Some(zim) = bench_zim() else {
        eprintln!(
            "skip warmup bench: set ZIMRU_BENCH_ZIM=/path/to/file.zim \
             (multi-GB archive recommended; mdwiki, big Wikipedia, …)"
        );
        return;
    };
    eprintln!("benching warmup against {}", zim.display());
    let arc = Archive::open(&zim).expect("Archive::open");

    let item = match pick_item(&arc) {
        Some(it) => it,
        None => {
            eprintln!("skip: no direct-access item found in archive");
            return;
        }
    };

    let direct = arc
        .blob_direct_access(item.cluster_index(), item.blob_index())
        .expect("blob_direct_access");
    let size_mb = direct.size as f64 / (1024.0 * 1024.0);
    eprintln!(
        "item: {} ({:.1} MB direct-access, file_offset={})",
        item.path(),
        size_mb,
        direct.file_offset
    );

    // 1. Time Item::warmup itself. On a cold OS page cache this is
    //    where the disk-read cost lives.
    let start = Instant::now();
    item.warmup().expect("warmup");
    let warmup_elapsed = start.elapsed();
    eprintln!("warmup       : {:?}", warmup_elapsed);

    // 2. Time the page-touch right after warmup. Pages should now be
    //    resident, so this is the warm cost — the time saved on the
    //    first user-facing read.
    let (_, warm_elapsed) = touch_pages(&item);
    eprintln!("warm touch   : {:?}", warm_elapsed);

    // 3. Print throughput numbers.
    if size_mb > 0.0 {
        let warmup_mbps = size_mb / warmup_elapsed.as_secs_f64();
        let warm_mbps = size_mb / warm_elapsed.as_secs_f64();
        eprintln!("warmup MB/s  : {:.1}", warmup_mbps);
        eprintln!("warm MB/s    : {:.1}", warm_mbps);
        if warm_elapsed.as_nanos() > 0 {
            let speedup = warmup_elapsed.as_secs_f64() / warm_elapsed.as_secs_f64();
            eprintln!(
                "speedup      : {:.1}× ({:.0} ms saved per first-read)",
                speedup,
                (warmup_elapsed.saturating_sub(warm_elapsed)).as_millis()
            );
        }
    }
}
