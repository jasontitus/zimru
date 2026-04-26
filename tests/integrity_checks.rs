#![cfg(feature = "writer")]
//! Round-trip + corruption tests for the structural-integrity
//! primitives the libzim-shim's `IntegrityCheckList` / `validate()`
//! aggregator composes:
//!
//! * [`zimru::Archive::check_dirent_ptrs`]
//! * [`zimru::Archive::check_dirent_order`]
//! * [`zimru::Archive::check_title_index`]
//! * [`zimru::Archive::check_cluster_ptrs`]
//! * [`zimru::Archive::check_mimetypes`]
//!
//! Plus the small accessors added at the same time:
//!
//! * [`zimru::Archive::main_entry_index`]
//! * [`zimru::Archive::cluster_offset`]
//! * [`zimru::Item::cluster_index`] / [`zimru::Item::blob_index`]
//!
//! Each integrity primitive gets two tests: clean fixture should
//! return `Ok(true)`; deliberately corrupted fixture (one targeted
//! byte rewrite) should return `Ok(false)`.

use std::path::PathBuf;

use zimru::writer::{Creator, Item};
use zimru::{Archive, Compression};

fn tmp_path(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "zimru-integrity-{tag}-{}-{ns}.zim",
        std::process::id()
    ))
}

/// Build a small uncompressed ZIM with three articles and a main page.
/// Returns the file path. Caller is responsible for removing the file
/// when the test ends.
fn build_clean_zim(tag: &str) -> PathBuf {
    let p = tmp_path(tag);
    let mut c = Creator::new();
    c.set_compression(Compression::None);
    c.set_main_path("home");
    c.add_item(Item::html(
        "home",
        "Home",
        "<!doctype html><body>home</body>",
    ));
    c.add_item(Item::html(
        "about",
        "About",
        "<!doctype html><body>about</body>",
    ));
    c.add_item(Item::text("notes", "Notes", "plain text"));
    c.add_metadata("Title", "integrity-test");
    c.write_to(&p).expect("write");
    p
}

#[test]
fn clean_archive_passes_every_integrity_primitive() {
    let p = build_clean_zim("clean");
    let arc = Archive::open(&p).expect("open");

    assert!(arc.check_dirent_ptrs().unwrap(), "dirent_ptrs");
    assert!(arc.check_dirent_order().unwrap(), "dirent_order");
    assert!(arc.check_title_index().unwrap(), "title_index");
    assert!(arc.check_cluster_ptrs().unwrap(), "cluster_ptrs");
    assert!(arc.check_mimetypes().unwrap(), "mimetypes");

    let _ = std::fs::remove_file(&p);
}

#[test]
fn main_entry_index_matches_main_entry_path_round_trip() {
    let p = build_clean_zim("main-index");
    let arc = Archive::open(&p).expect("open");

    let idx = arc.main_entry_index().expect("main entry set");
    let by_index = arc.entry_by_url_index(idx).unwrap();
    let by_main = arc.main_entry().unwrap();
    assert_eq!(by_index.path(), by_main.path());
    assert_eq!(by_index.namespace(), by_main.namespace());

    let _ = std::fs::remove_file(&p);
}

#[test]
fn main_entry_index_returns_none_when_no_main_page_set() {
    let p = tmp_path("no-main");
    let mut c = Creator::new();
    c.set_compression(Compression::None);
    c.add_item(Item::html(
        "only",
        "Only",
        "<!doctype html><body>only</body>",
    ));
    c.write_to(&p).expect("write");

    let arc = Archive::open(&p).expect("open");
    assert!(arc.main_entry_index().is_none());

    let _ = std::fs::remove_file(&p);
}

#[test]
fn cluster_offset_matches_cluster_byte_range_start() {
    let p = build_clean_zim("cluster-offset");
    let arc = Archive::open(&p).expect("open");

    let n = arc.cluster_count();
    assert!(n > 0, "fixture has at least one cluster");
    for i in 0..n {
        let offset = arc.cluster_offset(i).unwrap();
        let range = arc.cluster_byte_range(i).unwrap();
        assert_eq!(offset, range.start, "cluster {i} offset != range start");
        assert!(offset < arc.file_len(), "cluster {i} offset past file end");
    }

    // Out-of-range index errors out.
    assert!(arc.cluster_offset(n).is_err());

    let _ = std::fs::remove_file(&p);
}

#[test]
fn item_cluster_and_blob_indices_route_back_to_the_same_bytes() {
    let p = build_clean_zim("item-indices");
    let arc = Archive::open(&p).expect("open");

    let entry = arc.get_entry_by_path("home").unwrap();
    let item = entry.get_item(false).unwrap();
    let cluster_idx = item.cluster_index();
    let blob_idx = item.blob_index();

    // Hand-fetch the same bytes via the cluster API and compare.
    let cluster = arc.cluster(cluster_idx).unwrap();
    let blob_bytes = cluster.blob(blob_idx).unwrap();
    let item_bytes = item.get_data().unwrap();
    assert_eq!(blob_bytes, item_bytes.data());

    let _ = std::fs::remove_file(&p);
}

// ---------------- corruption tests ---------------------
//
// Each test reads the clean ZIM file into memory, surgically rewrites
// one or more bytes, writes it back to a fresh path, reopens, and
// asserts the corresponding integrity primitive returns `Ok(false)`.
// Using a fresh path per corruption keeps the OS page cache clean
// across reopens.

fn read_bytes(p: &PathBuf) -> Vec<u8> {
    std::fs::read(p).expect("read clean zim")
}

fn write_bytes(p: &PathBuf, bytes: &[u8]) {
    std::fs::write(p, bytes).expect("write corrupt zim");
}

/// Header layout (see src/header.rs): u64 url_ptr_pos at offset 32,
/// u64 cluster_ptr_pos at offset 48.
fn read_u64(bytes: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap())
}

fn write_u64(bytes: &mut [u8], off: usize, v: u64) {
    bytes[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn check_dirent_ptrs_flags_an_out_of_bounds_url_pointer() {
    let clean = build_clean_zim("dirent-ptrs-clean");
    let mut bytes = read_bytes(&clean);
    let _ = std::fs::remove_file(&clean);

    let url_ptr_pos = read_u64(&bytes, 32) as usize;
    // Clobber the first url-pointer to point past EOF.
    let bogus = (bytes.len() as u64) + 4096;
    write_u64(&mut bytes, url_ptr_pos, bogus);

    let dirty = tmp_path("dirent-ptrs-dirty");
    write_bytes(&dirty, &bytes);
    let arc = Archive::open(&dirty).expect("open corrupt");
    assert!(!arc.check_dirent_ptrs().unwrap(), "should flag OOB pointer");
    let _ = std::fs::remove_file(&dirty);
}

#[test]
fn check_dirent_order_flags_a_swapped_pointer_pair() {
    let clean = build_clean_zim("dirent-order-clean");
    let mut bytes = read_bytes(&clean);
    let _ = std::fs::remove_file(&clean);

    let url_ptr_pos = read_u64(&bytes, 32) as usize;
    // Swap the first two url-pointer slots so the dirent-order
    // invariant breaks. Both slots still point at parseable dirents,
    // so check_dirent_ptrs would still pass — only check_dirent_order
    // catches this. The fixture has 5 entries (3 articles + Title +
    // Counter metadata); index 0 sorts before index 1, swapping puts
    // them in the wrong order.
    let p0 = read_u64(&bytes, url_ptr_pos);
    let p1 = read_u64(&bytes, url_ptr_pos + 8);
    write_u64(&mut bytes, url_ptr_pos, p1);
    write_u64(&mut bytes, url_ptr_pos + 8, p0);

    let dirty = tmp_path("dirent-order-dirty");
    write_bytes(&dirty, &bytes);
    let arc = Archive::open(&dirty).expect("open corrupt");
    assert!(
        !arc.check_dirent_order().unwrap(),
        "should flag out-of-order dirent pair"
    );
    let _ = std::fs::remove_file(&dirty);
}

#[test]
fn check_cluster_ptrs_flags_an_out_of_bounds_cluster_pointer() {
    let clean = build_clean_zim("cluster-ptrs-clean");
    let mut bytes = read_bytes(&clean);
    let _ = std::fs::remove_file(&clean);

    let cluster_ptr_pos = read_u64(&bytes, 48) as usize;
    let bogus = (bytes.len() as u64) + 1;
    write_u64(&mut bytes, cluster_ptr_pos, bogus);

    let dirty = tmp_path("cluster-ptrs-dirty");
    write_bytes(&dirty, &bytes);
    let arc = Archive::open(&dirty).expect("open corrupt");
    assert!(
        !arc.check_cluster_ptrs().unwrap(),
        "should flag OOB cluster pointer"
    );
    let _ = std::fs::remove_file(&dirty);
}

#[test]
fn check_mimetypes_flags_an_out_of_range_dirent_mimetype() {
    let clean = build_clean_zim("mimetypes-clean");
    let mut bytes = read_bytes(&clean);
    let _ = std::fs::remove_file(&clean);

    // Find the first url-pointer, follow to the dirent, rewrite its
    // mimetype field (u16 at dirent offset 0) to something larger
    // than the mime list. The fixture only registers two mimetypes
    // (text/html, text/plain), so 100 is comfortably out of range
    // and short of the 0xFFFx reserved values that check_mimetypes
    // intentionally tolerates.
    let url_ptr_pos = read_u64(&bytes, 32) as usize;
    let dirent_off = read_u64(&bytes, url_ptr_pos) as usize;
    bytes[dirent_off..dirent_off + 2].copy_from_slice(&100u16.to_le_bytes());

    let dirty = tmp_path("mimetypes-dirty");
    write_bytes(&dirty, &bytes);
    let arc = Archive::open(&dirty).expect("open corrupt");
    assert!(
        !arc.check_mimetypes().unwrap(),
        "should flag out-of-range mimetype"
    );
    let _ = std::fs::remove_file(&dirty);
}
