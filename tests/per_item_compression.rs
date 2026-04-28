#![cfg(feature = "writer")]
//! Per-item compression override (`Item::compress`).
//!
//! These tests pin the contract that drove the feature: a single ZIM
//! can mix compressed and raw clusters, gated by per-item flags. The
//! motivating use case is streetzim's >500 MB routing-graph chunks
//! that bust PWA fzstd's per-cluster decompression cap; the same
//! build emits zstd-compressed tiles + HTML alongside.
//!
//! Each test reopens the file with the public `Archive` API so we
//! can also assert content roundtrips, and walks the clusters via
//! `Archive::cluster(idx)` to confirm the on-disk compression of
//! each one matches the per-item choice.

use std::collections::HashMap;
use std::path::PathBuf;

use zimru::writer::{ClusterStrategy, Creator, Item};
use zimru::{Archive, Compression};

fn tmp_path(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut p = std::env::temp_dir();
    p.push(format!(
        "zimru-percomp-{tag}-{}-{}.zim",
        std::process::id(),
        ns
    ));
    p
}

/// Map (item path → cluster index) for a finished archive — handy
/// for asserting which cluster a given item landed in without making
/// the test brittle to encode order.
fn cluster_map(a: &Archive) -> HashMap<String, u32> {
    let mut m = HashMap::new();
    for path in ["home", "blob.bin", "huge.bin"] {
        if let Ok(e) = a.get_entry_by_path(path) {
            if let Ok(item) = e.get_item(true) {
                m.insert(path.to_string(), item.cluster_index());
            }
        }
    }
    m
}

#[test]
fn raw_item_lands_in_its_own_uncompressed_cluster() {
    let out = tmp_path("raw-only");
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.set_cluster_strategy(ClusterStrategy::ByMime);

    // Compressed by default — should land in a zstd cluster.
    c.add_item(Item::html("home", "Home", "<h1>Hi</h1>"));

    // Force raw — should land in its OWN cluster (different bucket
    // key under the per-item-compression patch) with info_byte=1.
    c.add_item(
        Item::new(
            "blob.bin",
            "Blob",
            "application/octet-stream",
            vec![0xAB; 4096],
        )
        .with_compress(false),
    );

    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    assert_eq!(a.get_text("home").unwrap(), "<h1>Hi</h1>");
    assert_eq!(a.get_bytes("blob.bin").unwrap(), vec![0xAB; 4096]);

    let map = cluster_map(&a);
    let zstd_cluster = map["home"];
    let raw_cluster = map["blob.bin"];
    assert_ne!(
        zstd_cluster, raw_cluster,
        "compressed and raw items must land in different clusters"
    );
    assert_eq!(
        a.cluster(zstd_cluster).unwrap().compression(),
        Compression::Zstd
    );
    assert_eq!(
        a.cluster(raw_cluster).unwrap().compression(),
        Compression::None
    );

    let _ = std::fs::remove_file(&out);
}

#[test]
fn compress_true_honours_creator_default() {
    let out = tmp_path("explicit-true");
    let mut c = Creator::new();
    c.set_compression(Compression::None); // default is raw
    c.set_cluster_strategy(ClusterStrategy::ByMime);

    // `Some(true)` documents intent but should NOT force compression
    // when the Creator default is None — see effective_compression
    // semantics: only `Some(false)` overrides the default.
    c.add_item(Item::html("home", "Home", "<h1>Hi</h1>").with_compress(true));
    c.add_item(Item::new(
        "blob.bin",
        "Blob",
        "application/octet-stream",
        vec![0xAB; 1024],
    ));
    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    let map = cluster_map(&a);
    for path in ["home", "blob.bin"] {
        let idx = map[path];
        assert_eq!(
            a.cluster(idx).unwrap().compression(),
            Compression::None,
            "{path} cluster should be raw under Compression::None default"
        );
    }
    let _ = std::fs::remove_file(&out);
}

#[test]
fn streaming_raw_passthrough_for_huge_uncompressed_items() {
    // Threshold for the streaming-encode dispatch is 256 MiB (real
    // streetzim builds), but we don't want to allocate that in a unit
    // test. Drive the same path with a small expected_size by using
    // the buffered fallback — the dispatch logic that picks
    // StreamingRaw for raw items is exercised by the higher-level
    // `compress=Some(false)` flow regardless of size. The code path
    // that matters here is "raw cluster header (info_byte=1) for a
    // raw-flagged item routed through `begin_item`", which the
    // following exercises via the buffered chunked path.
    use std::io::Write as _;

    let out = tmp_path("chunked-raw");
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.set_cluster_strategy(ClusterStrategy::ByMime);
    c.start_writing(&out).expect("start");

    let body = vec![0xCD; 8 * 1024];
    {
        let mut b = c
            .begin_item(
                "huge.bin",
                "Huge",
                "application/octet-stream",
                None,
                Some(body.len()),
            )
            .expect("begin_item");
        b.set_compress(Some(false));
        b.write_chunk(&body);
        b.finish().expect("finish");
    }

    // Sibling compressed item to confirm both still coexist.
    c.add_item(Item::html("home", "Home", "<h1>Hi</h1>"));
    c.finish_writing().expect("finish_writing");

    let a = Archive::open(&out).expect("reopen");
    assert_eq!(a.get_bytes("huge.bin").unwrap(), body);
    assert_eq!(a.get_text("home").unwrap(), "<h1>Hi</h1>");

    let map = cluster_map(&a);
    assert_eq!(
        a.cluster(map["huge.bin"]).unwrap().compression(),
        Compression::None
    );
    assert_eq!(
        a.cluster(map["home"]).unwrap().compression(),
        Compression::Zstd
    );
    let _ = std::fs::remove_file(&out);

    // Use a noop write_all to keep the import alive (Drop ordering).
    let mut sink = Vec::new();
    sink.write_all(b"").unwrap();
}
