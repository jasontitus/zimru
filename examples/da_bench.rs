//! Micro-benchmark: `Archive::blob_direct_access` on a large
//! uncompressed cluster. The cluster cache budget is set to 1 byte so
//! repeated calls cannot be answered from a previously-cached decode —
//! each call pays whatever the primitive itself costs.
//!
//! Usage: da_bench FILE.ZIM ENTRY_PATH [ITERS]

use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args
        .next()
        .expect("usage: da_bench FILE.ZIM ENTRY_PATH [ITERS]");
    let entry_path = args
        .next()
        .expect("usage: da_bench FILE.ZIM ENTRY_PATH [ITERS]");
    let iters: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);

    let arc = zimru::Archive::open(&file).expect("open archive");
    arc.set_cluster_cache_max_bytes(1);
    let item = arc
        .get_entry_by_path(&entry_path)
        .expect("entry")
        .get_item(false)
        .expect("item");
    let (c, b) = (item.cluster_index(), item.blob_index());

    let t = Instant::now();
    let da = arc.blob_direct_access(c, b).expect("direct access");
    let first = t.elapsed();

    let t = Instant::now();
    for _ in 0..iters {
        let _ = arc.blob_direct_access(c, b).expect("direct access");
    }
    let avg = t.elapsed() / iters;

    println!(
        "blob_direct_access {entry_path}: is_direct={} size={} bytes | first_call={first:?} avg_of_{iters}={avg:?}",
        da.is_direct, da.size,
    );
}
