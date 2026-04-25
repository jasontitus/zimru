#![cfg(feature = "writer")]
//! Round-trip test for [`zimru::Archive::blob_direct_access`].
//!
//! Builds an archive with a known blob in an *uncompressed* cluster,
//! asks for direct-access info, then `pread`s the reported offset+size
//! out of the file and asserts the bytes match the original. Also
//! verifies that a blob in a *compressed* cluster reports
//! `is_direct=false` and zero offset/size.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use zimru::writer::{Creator, Item};
use zimru::{Archive, Compression};

fn tmp_path(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "zimru-direct-{tag}-{}-{ns}.zim",
        std::process::id()
    ))
}

#[test]
fn uncompressed_blob_round_trips_via_direct_access() {
    let out = tmp_path("uncompressed");
    let payload = b"DIRECT-ACCESS-PAYLOAD-MARKER-12345".to_vec();
    let mut c = Creator::new();
    c.set_compression(Compression::None);
    c.add_item(Item::new(
        "blob",
        "Blob",
        "application/octet-stream",
        payload.clone(),
    ));
    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    let entry = a.get_entry_by_path("blob").expect("entry by path");
    let item = entry.get_item(false).expect("get_item");
    let info = a
        .blob_direct_access(item.cluster_index(), item.blob_index())
        .expect("direct access");

    assert!(info.is_direct, "expected uncompressed cluster to be direct");
    assert_eq!(info.size, payload.len() as u64);

    // pread the reported offset+size out of the file and compare.
    let mut f = OpenOptions::new().read(true).open(&out).unwrap();
    f.seek(SeekFrom::Start(info.file_offset)).unwrap();
    let mut buf = vec![0u8; info.size as usize];
    f.read_exact(&mut buf).unwrap();
    assert_eq!(buf, payload, "bytes at reported offset don't match");

    let _ = std::fs::remove_file(&out);
}

#[test]
fn compressed_blob_reports_not_direct() {
    let out = tmp_path("compressed");
    let payload = b"this lives in a zstd-compressed cluster".to_vec();
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.add_item(Item::new(
        "blob",
        "Blob",
        "application/octet-stream",
        payload,
    ));
    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    let entry = a.get_entry_by_path("blob").expect("entry by path");
    let item = entry.get_item(false).expect("get_item");
    let info = a
        .blob_direct_access(item.cluster_index(), item.blob_index())
        .expect("direct access");

    assert!(!info.is_direct, "compressed cluster must not be direct");
    assert_eq!(info.file_offset, 0);
    assert_eq!(info.size, 0);

    let _ = std::fs::remove_file(&out);
}

#[test]
fn item_archive_accessor_returns_same_archive() {
    let out = tmp_path("archive-accessor");
    let mut c = Creator::new();
    c.set_compression(Compression::None);
    c.add_item(Item::text("home", "Home", "hi"));
    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    let item = a
        .get_entry_by_path("home")
        .unwrap()
        .get_item(false)
        .unwrap();
    // Item::archive() should yield the same checksum as the original.
    let direct = item.archive().has_checksum();
    assert_eq!(direct, a.has_checksum());

    let _ = std::fs::remove_file(&out);
}
