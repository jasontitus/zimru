//! End-to-end test that builds a fully spec-compliant synthetic ZIM file
//! in memory (header → mime list → URL/title/cluster pointer lists → dirents
//! → cluster → MD5 checksum) and verifies that the `zimru` public API can
//! navigate it correctly. No real ZIM files required.

use std::io::Write;

use md5::{Digest, Md5};
use zimru::{Archive, Error};

const MAGIC: u32 = 0x44D495A;

struct Article {
    namespace: u8,
    url: &'static str,
    title: &'static str,
    mime: u16,
    cluster: u32,
    blob: u32,
}

struct Redirect {
    namespace: u8,
    url: &'static str,
    title: &'static str,
    target_url_index: u32,
}

enum Dir {
    Art(Article),
    Red(Redirect),
}

fn write_dirent(out: &mut Vec<u8>, d: &Dir) {
    match d {
        Dir::Art(a) => {
            out.extend_from_slice(&a.mime.to_le_bytes());
            out.push(0); // parameter_len
            out.push(a.namespace);
            out.extend_from_slice(&0u32.to_le_bytes()); // revision
            out.extend_from_slice(&a.cluster.to_le_bytes());
            out.extend_from_slice(&a.blob.to_le_bytes());
            out.extend_from_slice(a.url.as_bytes());
            out.push(0);
            out.extend_from_slice(a.title.as_bytes());
            out.push(0);
        }
        Dir::Red(r) => {
            out.extend_from_slice(&0xFFFFu16.to_le_bytes());
            out.push(0);
            out.push(r.namespace);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&r.target_url_index.to_le_bytes());
            out.extend_from_slice(r.url.as_bytes());
            out.push(0);
            out.extend_from_slice(r.title.as_bytes());
            out.push(0);
        }
    }
}

fn build_uncompressed_cluster(blobs: &[&[u8]]) -> Vec<u8> {
    let n = blobs.len();
    let header_len = (n + 1) * 4;
    let mut out = Vec::new();
    out.push(0x01); // info: uncompressed, 4-byte offsets
    let mut cursor = header_len as u32;
    for blob in blobs {
        out.extend_from_slice(&cursor.to_le_bytes());
        cursor += blob.len() as u32;
    }
    out.extend_from_slice(&cursor.to_le_bytes());
    for blob in blobs {
        out.extend_from_slice(blob);
    }
    out
}

fn build_zstd_cluster(blobs: &[&[u8]]) -> Vec<u8> {
    let inner = build_uncompressed_cluster(blobs);
    let payload = &inner[1..];
    let compressed = zstd::stream::encode_all(payload, 3).unwrap();
    let mut out = Vec::with_capacity(1 + compressed.len());
    out.push(0x05);
    out.extend_from_slice(&compressed);
    out
}

fn build_zim(
    new_namespaces: bool,
    main_url_idx: Option<u32>,
    dirents: &[Dir],
    title_order: &[u32], // indices into url-pointer order, sorted by title
    clusters: &[Vec<u8>],
    mimes: &[&str],
    with_checksum: bool,
) -> Vec<u8> {
    // Layout:
    // [header 80 bytes][mime list][url ptr list][title ptr list][cluster ptr list][dirents...][clusters...][md5 if any]
    let header_size = 80usize;

    // Mime list bytes
    let mut mime_bytes: Vec<u8> = Vec::new();
    for m in mimes {
        mime_bytes.extend_from_slice(m.as_bytes());
        mime_bytes.push(0);
    }
    mime_bytes.push(0); // empty string terminator

    let n_entries = dirents.len() as u32;
    let n_clusters = clusters.len() as u32;

    let mime_list_pos = header_size as u64;
    let url_ptr_pos = mime_list_pos + mime_bytes.len() as u64;
    let title_ptr_pos = url_ptr_pos + (n_entries as u64) * 8;
    let cluster_ptr_pos = title_ptr_pos + (n_entries as u64) * 4;
    let dirents_pos = cluster_ptr_pos + (n_clusters as u64) * 8;

    // Render dirents and remember each one's offset.
    let mut dirent_blobs: Vec<Vec<u8>> = Vec::new();
    let mut dirent_offsets: Vec<u64> = Vec::new();
    let mut cursor = dirents_pos;
    for d in dirents {
        let mut buf = Vec::new();
        write_dirent(&mut buf, d);
        dirent_offsets.push(cursor);
        cursor += buf.len() as u64;
        dirent_blobs.push(buf);
    }
    let clusters_pos = cursor;

    let mut cluster_offsets: Vec<u64> = Vec::new();
    let mut cursor = clusters_pos;
    for c in clusters {
        cluster_offsets.push(cursor);
        cursor += c.len() as u64;
    }
    let after_clusters = cursor;

    let checksum_pos = if with_checksum { after_clusters } else { 0 };

    // Build the file in order.
    let mut out = Vec::new();

    // Header
    out.extend_from_slice(&MAGIC.to_le_bytes());
    let major = if new_namespaces { 6u16 } else { 5u16 };
    let minor = if new_namespaces { 1u16 } else { 0u16 };
    out.extend_from_slice(&major.to_le_bytes());
    out.extend_from_slice(&minor.to_le_bytes());
    let uuid: [u8; 16] = *b"zimru-test-uuid!";
    out.extend_from_slice(&uuid);
    out.extend_from_slice(&n_entries.to_le_bytes());
    out.extend_from_slice(&n_clusters.to_le_bytes());
    out.extend_from_slice(&url_ptr_pos.to_le_bytes());
    out.extend_from_slice(&title_ptr_pos.to_le_bytes());
    out.extend_from_slice(&cluster_ptr_pos.to_le_bytes());
    out.extend_from_slice(&mime_list_pos.to_le_bytes());
    out.extend_from_slice(&main_url_idx.unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes()); // layout_page (deprecated)
    out.extend_from_slice(&checksum_pos.to_le_bytes());
    debug_assert_eq!(out.len(), header_size);

    // Mime list
    out.extend_from_slice(&mime_bytes);

    // URL pointer list
    for off in &dirent_offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }

    // Title pointer list (u32 indices into url-pointer list, in title order)
    for idx in title_order {
        out.extend_from_slice(&idx.to_le_bytes());
    }

    // Cluster pointer list
    for off in &cluster_offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }

    // Dirents
    for b in &dirent_blobs {
        out.extend_from_slice(b);
    }

    // Clusters
    for c in clusters {
        out.extend_from_slice(c);
    }

    // Checksum (16 bytes md5 of everything before)
    if with_checksum {
        let mut h = Md5::new();
        h.update(&out);
        let digest: [u8; 16] = h.finalize().into();
        out.extend_from_slice(&digest);
    }

    out
}

fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("zimru-test-{name}-{}.zim", std::process::id()));
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(bytes).unwrap();
    p
}

// ---------- tests ----------

#[test]
fn round_trips_articles_and_redirects_uncompressed() {
    // C namespace contents:
    //   url=apple   title=Apple    body="An apple."
    //   url=banana  title=Banana   body="A banana."
    //   url=fruit   title=Fruit    redirect -> apple (url-index 0)
    // M namespace metadata:
    //   url=Title   title=Title    body="Test"
    //   url=Lang    title=Lang     body="eng"
    //
    // Dirents must be sorted by (namespace, url) — that order also gives URL
    // pointer order. Title-pointer order is by (namespace, title).

    let body_apple = b"An apple.";
    let body_banana = b"A banana.";
    let body_title = b"Test";
    let body_lang = b"eng";

    let cluster = build_uncompressed_cluster(&[body_apple, body_banana, body_title, body_lang]);

    // Place dirents in (ns, url) sort order:
    let dirents = vec![
        Dir::Art(Article {
            namespace: b'C',
            url: "apple",
            title: "Apple",
            mime: 0,
            cluster: 0,
            blob: 0,
        }),
        Dir::Art(Article {
            namespace: b'C',
            url: "banana",
            title: "Banana",
            mime: 0,
            cluster: 0,
            blob: 1,
        }),
        Dir::Red(Redirect {
            namespace: b'C',
            url: "fruit",
            title: "Fruit",
            target_url_index: 0,
        }),
        Dir::Art(Article {
            namespace: b'M',
            url: "Lang",
            title: "Lang",
            mime: 1,
            cluster: 0,
            blob: 3,
        }),
        Dir::Art(Article {
            namespace: b'M',
            url: "Title",
            title: "Title",
            mime: 1,
            cluster: 0,
            blob: 2,
        }),
    ];

    // Title order ((ns, title) sort): C/Apple(0), C/Banana(1), C/Fruit(2), M/Lang(3), M/Title(4)
    let title_order = vec![0u32, 1, 2, 3, 4];

    let zim = build_zim(
        true,
        Some(0), // main entry = apple
        &dirents,
        &title_order,
        &[cluster],
        &["text/html", "text/plain"],
        true,
    );

    let path = write_temp("uncompressed", &zim);
    let arc = Archive::open(&path).expect("open");

    assert_eq!(arc.entry_count(), 5);
    assert_eq!(arc.cluster_count(), 1);
    assert!(arc.has_main_entry());
    assert!(arc.has_checksum());
    assert!(arc.check().unwrap(), "checksum should verify");

    // Path lookup
    let apple = arc.get_entry_by_path("apple").unwrap();
    assert_eq!(apple.title(), "Apple");
    assert!(!apple.is_redirect());
    assert_eq!(apple.get_item(false).unwrap().mimetype(), "text/html");
    assert_eq!(
        apple.get_item(false).unwrap().get_data().unwrap().data(),
        body_apple
    );

    // Redirect resolution
    let fruit = arc.get_entry_by_path("fruit").unwrap();
    assert!(fruit.is_redirect());
    let resolved = fruit.get_redirect_entry().unwrap();
    assert_eq!(resolved.path(), "apple");
    let item = fruit.get_item(true).unwrap(); // follow redirect
    assert_eq!(item.path(), "apple");
    assert_eq!(item.get_data().unwrap().data(), body_apple);

    // get_item(false) on redirect should error
    matches!(fruit.get_item(false), Err(Error::NotAnItem));

    // Title lookup
    let banana = arc.get_entry_by_title("Banana").unwrap();
    assert_eq!(banana.path(), "banana");

    // Metadata
    assert_eq!(arc.get_metadata("Title").unwrap(), b"Test");
    assert_eq!(arc.get_metadata("Lang").unwrap(), b"eng");
    let mut keys = arc.get_metadata_keys();
    keys.sort();
    assert_eq!(keys, vec!["Lang".to_string(), "Title".to_string()]);

    // Iteration in path order
    let urls: Vec<String> = arc
        .iter_by_path()
        .map(|r| {
            let e = r.unwrap();
            format!("{}/{}", char::from(e.namespace()), e.path())
        })
        .collect();
    assert_eq!(
        urls,
        vec!["C/apple", "C/banana", "C/fruit", "M/Lang", "M/Title"]
    );

    // Iteration in title order
    let titles: Vec<String> = arc
        .iter_by_title()
        .map(|r| r.unwrap().title().to_string())
        .collect();
    assert_eq!(titles, vec!["Apple", "Banana", "Fruit", "Lang", "Title"]);

    // Main entry
    let main = arc.main_entry().unwrap();
    assert_eq!(main.path(), "apple");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn round_trips_zstd_compressed_cluster() {
    let body = vec![b'x'; 100_000];
    let cluster = build_zstd_cluster(&[&body, b"small"]);
    let dirents = vec![
        Dir::Art(Article {
            namespace: b'C',
            url: "big",
            title: "Big",
            mime: 0,
            cluster: 0,
            blob: 0,
        }),
        Dir::Art(Article {
            namespace: b'C',
            url: "small",
            title: "Small",
            mime: 0,
            cluster: 0,
            blob: 1,
        }),
    ];
    let title_order = vec![0u32, 1];
    let zim = build_zim(
        true,
        None,
        &dirents,
        &title_order,
        &[cluster],
        &["text/plain"],
        true,
    );
    let path = write_temp("zstd", &zim);
    let arc = Archive::open(&path).unwrap();
    assert!(arc.check().unwrap());
    let big = arc
        .get_entry_by_path("big")
        .unwrap()
        .get_item(false)
        .unwrap();
    assert_eq!(big.get_data().unwrap().data(), body.as_slice());
    let small = arc
        .get_entry_by_path("small")
        .unwrap()
        .get_item(false)
        .unwrap();
    assert_eq!(small.get_data().unwrap().data(), b"small");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn lookup_returns_not_found() {
    let cluster = build_uncompressed_cluster(&[b"hello"]);
    let dirents = vec![Dir::Art(Article {
        namespace: b'C',
        url: "only",
        title: "Only",
        mime: 0,
        cluster: 0,
        blob: 0,
    })];
    let zim = build_zim(
        true,
        None,
        &dirents,
        &[0],
        &[cluster],
        &["text/plain"],
        false,
    );
    let path = write_temp("notfound", &zim);
    let arc = Archive::open(&path).unwrap();
    assert!(matches!(
        arc.get_entry_by_path("missing"),
        Err(Error::EntryNotFound)
    ));
    assert!(matches!(
        arc.get_entry_by_title("Missing"),
        Err(Error::EntryNotFound)
    ));
    assert!(!arc.has_checksum());
    assert!(matches!(arc.check(), Err(Error::NoChecksum)));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn detects_corrupted_checksum() {
    let cluster = build_uncompressed_cluster(&[b"hi"]);
    let dirents = vec![Dir::Art(Article {
        namespace: b'C',
        url: "x",
        title: "X",
        mime: 0,
        cluster: 0,
        blob: 0,
    })];
    let mut zim = build_zim(
        true,
        None,
        &dirents,
        &[0],
        &[cluster],
        &["text/plain"],
        true,
    );
    // Flip the very first byte of the trailing checksum.
    let cs_pos = zim.len() - 16;
    zim[cs_pos] ^= 0xFF;
    let path = write_temp("badcrc", &zim);
    let arc = Archive::open(&path).unwrap();
    assert!(arc.has_checksum());
    assert!(!arc.check().unwrap());
    let _ = std::fs::remove_file(&path);
}
