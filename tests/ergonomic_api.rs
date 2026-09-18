//! Tests for the idiomatic convenience APIs added on top of the
//! libzim-mirror surface. Reuses the synthetic ZIM builder from
//! `synthetic_zim.rs`, but tests the ergonomic shortcuts.
//!
//! Every public API in this batch gets at least one test here.

use std::io::{Read, Write};

use md5::{Digest, Md5};
use zimru::{Archive, Error};

const MAGIC: u32 = 0x44D495A;

struct Article {
    ns: u8,
    url: &'static str,
    title: &'static str,
    mime: u16,
    blob: u32,
}
struct Redirect {
    ns: u8,
    url: &'static str,
    title: &'static str,
    target: u32,
}
enum D {
    A(Article),
    R(Redirect),
}

fn write_dirent(out: &mut Vec<u8>, d: &D) {
    match d {
        D::A(a) => {
            out.extend_from_slice(&a.mime.to_le_bytes());
            out.push(0);
            out.push(a.ns);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // cluster 0
            out.extend_from_slice(&a.blob.to_le_bytes());
            out.extend_from_slice(a.url.as_bytes());
            out.push(0);
            out.extend_from_slice(a.title.as_bytes());
            out.push(0);
        }
        D::R(r) => {
            out.extend_from_slice(&0xFFFFu16.to_le_bytes());
            out.push(0);
            out.push(r.ns);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&r.target.to_le_bytes());
            out.extend_from_slice(r.url.as_bytes());
            out.push(0);
            out.extend_from_slice(r.title.as_bytes());
            out.push(0);
        }
    }
}

fn uncompressed_cluster(blobs: &[&[u8]]) -> Vec<u8> {
    let n = blobs.len();
    let header_len = (n + 1) * 4;
    let mut out = Vec::new();
    out.push(0x01);
    let mut cursor = header_len as u32;
    for b in blobs {
        out.extend_from_slice(&cursor.to_le_bytes());
        cursor += b.len() as u32;
    }
    out.extend_from_slice(&cursor.to_le_bytes());
    for b in blobs {
        out.extend_from_slice(b);
    }
    out
}

/// Build a small but realistic ZIM with:
///   C/apple, C/banana, C/fig, C/fruit (redirect -> apple),
///   C/images/logo.png, C/images/header.png,
///   M/Title, M/Language,
///   W/mainPage (redirect -> apple),
///   X/listing/titleOrdered/v1 (placeholder — we still use legacy
///       in-header title list so this isn't actually used by the reader).
fn build_test_zim() -> Vec<u8> {
    let blobs: Vec<&[u8]> = vec![
        b"An apple.",                    // 0 apple
        b"A banana.",                    // 1 banana
        b"A fig.",                       // 2 fig
        b"Demo",                         // 3 Title
        b"eng",                          // 4 Language
        &[0x89, 0x50, 0x4E, 0x47],       // 5 logo.png (fake PNG bytes)
        &[0x89, 0x50, 0x4E, 0x47, 0xAA], // 6 header.png
    ];
    let cluster = uncompressed_cluster(&blobs);

    // Dirents in (ns, url) order. blob indices point into `blobs`.
    let dirents = vec![
        D::A(Article {
            ns: b'C',
            url: "apple",
            title: "Apple",
            mime: 0,
            blob: 0,
        }),
        D::A(Article {
            ns: b'C',
            url: "banana",
            title: "Banana",
            mime: 0,
            blob: 1,
        }),
        D::A(Article {
            ns: b'C',
            url: "fig",
            title: "Fig",
            mime: 0,
            blob: 2,
        }),
        D::R(Redirect {
            ns: b'C',
            url: "fruit",
            title: "Fruit",
            target: 0,
        }),
        D::A(Article {
            ns: b'C',
            url: "images/header.png",
            title: "header",
            mime: 2,
            blob: 6,
        }),
        D::A(Article {
            ns: b'C',
            url: "images/logo.png",
            title: "logo",
            mime: 2,
            blob: 5,
        }),
        D::A(Article {
            ns: b'M',
            url: "Language",
            title: "Language",
            mime: 1,
            blob: 4,
        }),
        D::A(Article {
            ns: b'M',
            url: "Title",
            title: "Title",
            mime: 1,
            blob: 3,
        }),
        D::R(Redirect {
            ns: b'W',
            url: "mainPage",
            title: "mainPage",
            target: 0,
        }),
    ];
    let mimes = ["text/html", "text/plain", "image/png"];

    // --- compute layout ---
    let header_size = 80usize;
    let mut mime_bytes: Vec<u8> = Vec::new();
    for m in &mimes {
        mime_bytes.extend_from_slice(m.as_bytes());
        mime_bytes.push(0);
    }
    mime_bytes.push(0);

    let entry_count = dirents.len() as u32;
    let cluster_count = 1u32;
    let url_ptr_pos = header_size as u64 + mime_bytes.len() as u64;
    let title_ptr_pos = url_ptr_pos + entry_count as u64 * 8;
    let cluster_ptr_pos = title_ptr_pos + entry_count as u64 * 4;
    let dirents_pos = cluster_ptr_pos + cluster_count as u64 * 8;

    let mut drawn: Vec<Vec<u8>> = Vec::new();
    let mut dirent_offsets: Vec<u64> = Vec::new();
    let mut cursor = dirents_pos;
    for d in &dirents {
        let mut buf = Vec::new();
        write_dirent(&mut buf, d);
        dirent_offsets.push(cursor);
        cursor += buf.len() as u64;
        drawn.push(buf);
    }
    let cluster_offset = cursor;
    let after_clusters = cluster_offset + cluster.len() as u64;
    let checksum_pos = after_clusters;

    // Title order: sort by (ns, title) and record original indices.
    let mut title_order: Vec<u32> = (0..dirents.len() as u32).collect();
    title_order.sort_by_key(|&i| match &dirents[i as usize] {
        D::A(a) => (a.ns, a.title.to_string()),
        D::R(r) => (r.ns, r.title.to_string()),
    });

    // main_page = index in url-order of W/mainPage = last entry (index 8)
    let main_page = 8u32;

    // --- render file ---
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&6u16.to_le_bytes()); // major
    out.extend_from_slice(&1u16.to_le_bytes()); // version 6.1: new namespaces
    let uuid: [u8; 16] = *b"zimru-api-tests!";
    out.extend_from_slice(&uuid);
    out.extend_from_slice(&entry_count.to_le_bytes());
    out.extend_from_slice(&cluster_count.to_le_bytes());
    out.extend_from_slice(&url_ptr_pos.to_le_bytes());
    out.extend_from_slice(&title_ptr_pos.to_le_bytes());
    out.extend_from_slice(&cluster_ptr_pos.to_le_bytes());
    out.extend_from_slice(&(header_size as u64).to_le_bytes()); // mime_list_pos
    out.extend_from_slice(&main_page.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes()); // layout_page (deprecated)
    out.extend_from_slice(&checksum_pos.to_le_bytes());
    debug_assert_eq!(out.len(), header_size);

    out.extend_from_slice(&mime_bytes);
    for o in &dirent_offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    for i in &title_order {
        out.extend_from_slice(&i.to_le_bytes());
    }
    out.extend_from_slice(&cluster_offset.to_le_bytes());
    for d in &drawn {
        out.extend_from_slice(d);
    }
    out.extend_from_slice(&cluster);

    let mut h = Md5::new();
    h.update(&out);
    let digest: [u8; 16] = h.finalize().into();
    out.extend_from_slice(&digest);
    out
}

fn write_tmp(bytes: &[u8]) -> std::path::PathBuf {
    // Unique counter — `SystemTime::now().as_nanos()` is not coarse
    // enough to disambiguate two parallel test threads in the same
    // process, which used to cause one test to delete another's
    // tmp file mid-run.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "zimru-ergo-{}-{}-{}.zim",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        seq,
    ));
    std::fs::File::create(&p).unwrap().write_all(bytes).unwrap();
    p
}

#[test]
fn get_bytes_and_text_one_liners() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    assert_eq!(a.get_bytes("apple").unwrap(), b"An apple.");
    assert_eq!(a.get_text("banana").unwrap(), "A banana.");
    // Redirect follows automatically.
    assert_eq!(a.get_bytes("fruit").unwrap(), b"An apple.");

    // Missing path -> EntryNotFound, not panic.
    match a.get_bytes("nonexistent") {
        Err(Error::EntryNotFound) => {}
        other => panic!("unexpected {other:?}"),
    }
    let _ = std::fs::remove_file(p);
}

#[test]
fn metadata_str_and_has_metadata() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    assert_eq!(a.metadata_str("Title").unwrap(), "Demo");
    assert_eq!(a.metadata_str("Language").unwrap(), "eng");
    assert!(a.has_metadata("Title"));
    assert!(!a.has_metadata("NonExistent"));
    let _ = std::fs::remove_file(p);
}

#[test]
fn main_item_and_main_path_follow_redirect() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let main = a.main_item().unwrap();
    assert_eq!(main.path(), "apple");
    assert_eq!(main.text().unwrap(), "An apple.");
    assert_eq!(a.main_path().unwrap(), "apple");
    let _ = std::fs::remove_file(p);
}

#[test]
fn namespace_range_covers_expected_indices() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    // URL-pointer order established in build_test_zim — C entries first,
    // then M, then W. C has 6 items (apple, banana, fig, fruit, images/header, images/logo).
    let c_range = a.namespace_range(b'C').unwrap();
    assert_eq!(c_range.len(), 6);
    let m_range = a.namespace_range(b'M').unwrap();
    assert_eq!(m_range.len(), 2);
    let w_range = a.namespace_range(b'W').unwrap();
    assert_eq!(w_range.len(), 1);
    let x_range = a.namespace_range(b'X').unwrap();
    assert_eq!(x_range.len(), 0);
    let _ = std::fs::remove_file(p);
}

#[test]
fn articles_and_redirects_and_content_entries() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let arts: Vec<String> = a
        .articles()
        .map(|r| r.unwrap().path().to_string())
        .collect();
    // articles() includes M/W articles too (just skips redirects).
    assert!(arts.contains(&"apple".to_string()));
    assert!(arts.contains(&"Language".to_string()));
    assert!(!arts.iter().any(|p| p == "fruit"));

    let reds: Vec<String> = a
        .redirects()
        .map(|r| r.unwrap().path().to_string())
        .collect();
    assert!(reds.contains(&"fruit".to_string()));
    assert!(reds.contains(&"mainPage".to_string()));

    let content: Vec<String> = a
        .content_entries()
        .map(|r| r.unwrap().path().to_string())
        .collect();
    // content_entries only covers C-namespace (includes redirects in C).
    assert!(content.iter().all(|p| !p.contains("Language"))); // no M entries
    assert!(content.contains(&"apple".to_string()));
    let _ = std::fs::remove_file(p);
}

#[test]
fn by_prefix_binary_searches_for_start() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let imgs: Vec<String> = a
        .by_prefix(b'C', "images/")
        .map(|r| r.unwrap().path().to_string())
        .collect();
    assert_eq!(imgs, vec!["images/header.png", "images/logo.png"]);

    // Prefix that matches nothing.
    let empty: Vec<_> = a.by_prefix(b'C', "zzzz").collect();
    assert!(empty.is_empty());
    let _ = std::fs::remove_file(p);
}

#[test]
fn par_iter_by_path_yields_every_entry() {
    use rayon::prelude::*;
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let n = a
        .par_iter_by_path()
        .map(|r| r.unwrap().path().to_string())
        .count();
    assert_eq!(n as u32, a.entry_count());
    let _ = std::fs::remove_file(p);
}

#[test]
fn par_clusters_visits_each_cluster_once() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let sizes: Vec<usize> = a
        .par_clusters(|_idx, c| {
            (0..c.blob_count())
                .filter_map(|i| c.blob(i).ok())
                .map(|b| b.len())
                .sum::<usize>()
        })
        .unwrap();
    assert_eq!(sizes.len() as u32, a.cluster_count());
    let _ = std::fs::remove_file(p);
}

#[test]
fn summary_struct_matches_header() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let s = a.summary();
    assert_eq!(s.entry_count, 9);
    assert_eq!(s.content_entry_count, 6);
    assert_eq!(s.cluster_count, 1);
    assert!(s.uses_new_namespaces);
    assert!(s.has_main_entry);
    assert!(s.has_checksum);
    assert_eq!(s.main_path.as_deref(), Some("apple"));
    // Display impl doesn't crash.
    let _ = format!("{s}");
    let _ = std::fs::remove_file(p);
}

#[test]
fn item_text_bytes_is_html() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let item = a.get_item("apple").unwrap();
    assert_eq!(item.bytes().unwrap(), b"An apple.");
    assert_eq!(item.text().unwrap(), "An apple.");
    assert!(item.is_html()); // mime index 0 = "text/html" in this fixture
    assert!(item.is_text());
    assert!(!item.is_image());

    let logo = a.get_item("images/logo.png").unwrap();
    assert!(logo.is_image());
    assert!(!logo.is_html());
    assert!(!logo.is_text());
    let _ = std::fs::remove_file(p);
}

#[test]
fn blob_as_str_and_reader() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let item = a.get_item("apple").unwrap();
    let blob = item.get_data().unwrap();
    assert_eq!(blob.as_str().unwrap(), "An apple.");
    let mut buf = String::new();
    blob.reader().read_to_string(&mut buf).unwrap();
    assert_eq!(buf, "An apple.");
    // Deref<Target=[u8]>
    assert_eq!(&*blob, b"An apple.");
    let _ = std::fs::remove_file(p);
}

#[test]
fn entry_item_and_resolve() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let fruit = a.get_entry_by_path("fruit").unwrap();
    assert!(fruit.is_redirect());
    let resolved = fruit.resolve().unwrap();
    assert_eq!(resolved.path(), "apple");
    let item = fruit.item().unwrap(); // follows redirect
    assert_eq!(item.path(), "apple");
    let _ = std::fs::remove_file(p);
}

#[test]
fn entry_display_format() {
    let z = build_test_zim();
    let p = write_tmp(&z);
    let a = Archive::open(&p).unwrap();

    let apple = a.get_entry_by_path("apple").unwrap();
    assert_eq!(format!("{apple}"), "C/apple \"Apple\"");
    let fruit = a.get_entry_by_path("fruit").unwrap();
    assert_eq!(format!("{fruit}"), "↪ C/fruit \"Fruit\"");
    let _ = std::fs::remove_file(p);
}
