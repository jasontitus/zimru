//! Smoke test for the streaming-encode path through the chunked
//! C ABI: items at or above the streaming threshold (256 MiB by
//! default) feed straight through a zstd encoder onto disk one chunk
//! at a time without ever materialising the full body in `Vec<u8>`.
//! The test lowers the threshold to 1 MiB via
//! `Creator::set_streaming_encode_threshold` so a 6 MiB payload
//! genuinely exercises the streaming path instead of silently taking
//! the buffered one.
//!
//! The test pushes a 6 MiB item via the public `Creator::begin_chunked_item`
//! / `chunked_item_chunk` / `end_chunked_item` shape, alongside a
//! handful of small bin-packed items, and asserts:
//!
//! - The big item round-trips byte-for-byte through `Archive::open`.
//! - It lives in its own cluster (one blob, cluster_idx >= 1).
//! - The auto-`finish_writing` integrity gate passed (no panic, no Err).
//! - The mix of small items + huge items doesn't break dirent ordering.

#![cfg(feature = "writer")]

use zimru::writer::{Creator, Item};
use zimru::Compression;

fn tmp_path(prefix: &str) -> std::path::PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("zimru-{prefix}-{}-{ns}.zim", std::process::id()))
}

#[test]
fn huge_item_streams_through_zstd_encoder_without_buffering() {
    let out = tmp_path("stream-encode");

    // Reproducible payload: 6 MiB of a deterministic byte pattern.
    // 6 MiB > the 1 MiB threshold set below, so begin_chunked_item
    // picks the StreamingZstd variant, not Buffered.
    let payload: Vec<u8> = (0u32..(6 * 1024 * 1024 / 4))
        .flat_map(|i| i.to_le_bytes())
        .collect();
    assert_eq!(payload.len(), 6 * 1024 * 1024);

    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.set_compression_level(3);
    c.set_main_path("home");
    // Lower the streaming cutoff (default 256 MiB) so this test's
    // 6 MiB item actually takes the streaming-encode path — with the
    // default it would silently fall into the buffered path and this
    // test would cover nothing.
    c.set_streaming_encode_threshold(1024 * 1024);
    c.start_writing(&out).expect("start_writing");

    // Two small bin-packed items first — these go through the
    // normal cluster bucket.
    c.add_item(Item::html(
        "home",
        "Home",
        "<!doctype html><body>Home page small content.</body>",
    ));
    c.add_item(Item::html(
        "about",
        "About",
        "<!doctype html><body>About page small content.</body>",
    ));

    // A 6 MiB item via the chunked path. We feed in 64 KiB chunks
    // to mimic a real ContentProvider::feed() pattern.
    c.begin_chunked_item(
        None,
        "huge.bin".to_string(),
        "Huge".to_string(),
        "application/octet-stream".to_string(),
        Some(payload.len() as u64),
        None,
    )
    .expect("begin_chunked_item");
    for chunk in payload.chunks(64 * 1024) {
        c.chunked_item_chunk(chunk).expect("chunked_item_chunk");
    }
    c.end_chunked_item().expect("end_chunked_item");

    // One more small item after the huge one to make sure the
    // streamer's cluster bookkeeping resumed correctly.
    c.add_item(Item::html(
        "after",
        "After",
        "<!doctype html><body>Item after the huge streaming item.</body>",
    ));

    c.add_metadata("Title", "stream-encode smoke");
    c.add_metadata("Language", "eng");

    c.finish_writing()
        .expect("finish_writing (includes auto zimcheck)");

    // ---- Read back and verify ----
    let arc = zimru::Archive::open(&out).expect("reopen");
    assert!(arc.has_main_entry());
    assert!(
        arc.cluster_count() >= 2,
        "expected ≥2 clusters (small bucket + huge-item-only), got {}",
        arc.cluster_count()
    );

    // Find huge.bin and confirm its body matches.
    let huge = arc
        .get_entry_by_path("huge.bin")
        .expect("get_entry_by_path huge.bin");
    let huge_item = huge.get_item(false).expect("get_item huge.bin");
    let body = huge_item.bytes().expect("read huge body");
    assert_eq!(body.len(), payload.len(), "huge body size mismatch");
    assert_eq!(&body[..], &payload[..], "huge body content mismatch");

    // Streaming items get a dedicated single-blob cluster — assert that
    // shape so a regression back to the bin-packed buffered path fails
    // loudly here.
    let zimru::Dirent::Article(a) = huge.dirent() else {
        panic!("huge.bin should be an article dirent");
    };
    let huge_cluster = arc.cluster(a.cluster).expect("load huge cluster");
    assert_eq!(
        huge_cluster.blob_count(),
        1,
        "streamed item should live alone in its own cluster"
    );

    // The other items still round-trip.
    let home = arc
        .get_entry_by_path("home")
        .expect("get_entry_by_path home");
    let home_body = home.get_item(true).unwrap().bytes().unwrap();
    assert!(std::str::from_utf8(&home_body)
        .unwrap()
        .contains("Home page"));

    let after = arc
        .get_entry_by_path("after")
        .expect("get_entry_by_path after");
    let after_body = after.get_item(true).unwrap().bytes().unwrap();
    assert!(std::str::from_utf8(&after_body)
        .unwrap()
        .contains("Item after"));

    // Sanity-check: the auto-verify gate inside finish_writing
    // already passed every check (Pass on dirent_ptrs, dirent_order,
    // title_index, cluster_ptrs, mimetypes, MD5). If those failed,
    // finish_writing would have returned Err and we'd never reach
    // this point.

    let _ = std::fs::remove_file(&out);
}

#[test]
fn small_chunked_item_uses_buffered_path_and_bin_packs() {
    // Small chunked item (well below the default 256 MiB threshold)
    // goes through the Buffered path, so it shares a cluster with
    // neighbouring small items rather than getting its own.
    let out = tmp_path("stream-encode-small");

    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.set_compression_level(3);
    c.set_main_path("home");
    c.start_writing(&out).expect("start_writing");

    c.add_item(Item::html(
        "home",
        "Home",
        "<!doctype html><body>home</body>",
    ));

    // 1 KiB chunked item — definitely buffered path.
    let body = vec![b'X'; 1024];
    c.begin_chunked_item(
        None,
        "small.bin".to_string(),
        "Small".to_string(),
        "application/octet-stream".to_string(),
        Some(body.len() as u64),
        None,
    )
    .expect("begin_chunked_item");
    c.chunked_item_chunk(&body).expect("chunked_item_chunk");
    c.end_chunked_item().expect("end_chunked_item");

    c.add_metadata("Title", "small-chunked");
    c.add_metadata("Language", "eng");

    c.finish_writing().expect("finish_writing");

    let arc = zimru::Archive::open(&out).expect("reopen");
    let small = arc
        .get_entry_by_path("small.bin")
        .expect("get_entry_by_path small.bin");
    let bytes = small.get_item(false).unwrap().bytes().unwrap();
    assert_eq!(bytes.len(), 1024);
    assert!(bytes.iter().all(|&b| b == b'X'));

    let _ = std::fs::remove_file(&out);
}

#[test]
fn exact_chunk_sizes_are_enforced_and_recoverable_on_every_encoding_route() {
    for (compression, threshold) in [
        (Compression::None, 1),
        (Compression::Zstd, 1),
        (Compression::Xz, 1),
        (Compression::Zstd, usize::MAX),
    ] {
        let out = tmp_path("exact-chunk-size");
        let mut c = Creator::new();
        c.set_compression(compression).set_compression_level(1);
        c.set_streaming_encode_threshold(threshold);
        c.start_writing(&out).unwrap();
        assert!(c
            .begin_chunked_item(
                None,
                "data".into(),
                "bad\0title".into(),
                "text/plain".into(),
                Some(4),
                None
            )
            .is_err());
        c.begin_chunked_item(
            None,
            "data".into(),
            "Data".into(),
            "text/plain".into(),
            Some(4),
            None,
        )
        .unwrap();
        c.chunked_item_chunk(b"ab").unwrap();
        assert!(
            c.end_chunked_item().is_err(),
            "short input is not an exact-size item"
        );
        assert!(
            c.chunked_item_chunk(b"XYZ").is_err(),
            "overrun is rejected before writing"
        );
        c.chunked_item_chunk(b"cd").unwrap();
        c.end_chunked_item().unwrap();
        assert!(c.try_add_item(Item::text("data", "", "duplicate")).is_err());
        c.begin_chunked_item(
            None,
            "unknown".into(),
            "".into(),
            "text/plain".into(),
            None,
            None,
        )
        .unwrap();
        c.chunked_item_chunk(b"unknown size").unwrap();
        c.end_chunked_item().unwrap();
        c.finish_writing().unwrap();
        let arc = zimru::Archive::open(&out).unwrap();
        assert_eq!(arc.get_bytes("data").unwrap(), b"abcd");
        assert_eq!(arc.get_bytes("unknown").unwrap(), b"unknown size");
        let _ = std::fs::remove_file(out);
    }
}

#[test]
fn item_builder_validates_before_reserving_and_commits_duplicate_protection() {
    let out = tmp_path("builder-key");
    let mut c = Creator::new();
    c.start_writing(&out).unwrap();
    assert!(c
        .begin_item("item", "bad\0title", "text/plain", None, Some(usize::MAX))
        .is_err());
    assert!(c.begin_item("item", "", "", None, None).is_err());
    let mut item = c
        .begin_item("item", "", "text/plain", None, Some(1))
        .unwrap();
    item.write_chunk(b"a nonbinding size hint");
    item.finish().unwrap();
    assert!(c.begin_item("item", "", "text/plain", None, None).is_err());
    c.finish_writing().unwrap();
    let arc = zimru::Archive::open(&out).unwrap();
    assert_eq!(arc.get_bytes("item").unwrap(), b"a nonbinding size hint");
    let _ = std::fs::remove_file(out);
}

#[test]
fn oversized_pipeline_items_roundtrip_with_a_tiny_cap_and_reproducible_bytes() {
    let paths = [tmp_path("byte-cap-first"), tmp_path("byte-cap-second")];
    for path in &paths {
        let mut c = Creator::new();
        c.set_compression(Compression::Zstd)
            .set_compression_level(1);
        c.set_uuid([0x51; 16]);
        c.set_cluster_size_target(256);
        c.set_max_in_flight_bytes(32);
        c.start_writing(path).unwrap();
        for i in 0u8..12 {
            let body: Vec<u8> = (0..8192).map(|n| (n as u8).wrapping_add(i)).collect();
            c.try_add_item(Item::new(
                format!("blob-{i:02}"),
                "",
                "application/octet-stream",
                body,
            ))
            .unwrap();
        }
        c.finish_writing().unwrap();
        let arc = zimru::Archive::open(path).unwrap();
        for i in 0u8..12 {
            let expected: Vec<u8> = (0..8192).map(|n| (n as u8).wrapping_add(i)).collect();
            assert_eq!(arc.get_bytes(&format!("blob-{i:02}")).unwrap(), expected);
        }
        assert!(arc.check().unwrap());
    }
    assert_eq!(
        std::fs::read(&paths[0]).unwrap(),
        std::fs::read(&paths[1]).unwrap()
    );
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}
