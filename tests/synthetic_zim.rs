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
fn item_warmup_succeeds_on_direct_and_compressed_items() {
    // Two clusters in one ZIM:
    //   cluster 0 — uncompressed → items are direct-access, warmup
    //               should madvise + page-touch the region.
    //   cluster 1 — zstd → items are NOT direct-access, warmup must
    //               silently no-op (return Ok) without touching anything.

    let direct_payload = b"direct-access body".to_vec();
    let compressed_payload = vec![b'z'; 1024];

    let direct_cluster = build_uncompressed_cluster(&[&direct_payload]);
    let compressed_cluster = build_zstd_cluster(&[&compressed_payload]);

    let dirents = vec![
        Dir::Art(Article {
            namespace: b'C',
            url: "direct",
            title: "Direct",
            mime: 0,
            cluster: 0,
            blob: 0,
        }),
        Dir::Art(Article {
            namespace: b'C',
            url: "zipped",
            title: "Zipped",
            mime: 0,
            cluster: 1,
            blob: 0,
        }),
    ];
    let zim = build_zim(
        true,
        None,
        &dirents,
        &[0u32, 1],
        &[direct_cluster, compressed_cluster],
        &["text/plain"],
        true,
    );
    let path = write_temp("warmup", &zim);
    let arc = Archive::open(&path).unwrap();

    // Direct-access item: warmup should succeed.
    let direct_item = arc
        .get_entry_by_path("direct")
        .unwrap()
        .get_item(false)
        .unwrap();
    direct_item.warmup().expect("warmup direct");

    // Verify the data is still readable after warmup (we should not
    // have corrupted the mmap by touching pages).
    assert_eq!(direct_item.bytes().unwrap(), direct_payload);

    // Compressed item: warmup must no-op cleanly (compressed clusters
    // can't be page-warmed at the on-disk-bytes level — Xapian DBs
    // are uncompressed by spec, so the only valid use case for
    // warmup is direct-access items).
    let compressed_item = arc
        .get_entry_by_path("zipped")
        .unwrap()
        .get_item(false)
        .unwrap();
    compressed_item.warmup().expect("warmup compressed no-op");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn cluster_cache_stats_track_hits_and_misses() {
    // Two articles in one cluster: the first lookup is a cache miss,
    // the second hits. Over many lookups the hit/miss counters should
    // separate cleanly.

    let cluster = build_uncompressed_cluster(&[b"first body", b"second body"]);
    let dirents = vec![
        Dir::Art(Article {
            namespace: b'C',
            url: "first",
            title: "First",
            mime: 0,
            cluster: 0,
            blob: 0,
        }),
        Dir::Art(Article {
            namespace: b'C',
            url: "second",
            title: "Second",
            mime: 0,
            cluster: 0,
            blob: 1,
        }),
    ];
    let zim = build_zim(
        true,
        None,
        &dirents,
        &[0u32, 1],
        &[cluster],
        &["text/plain"],
        true,
    );
    let path = write_temp("cache_stats", &zim);
    let arc = Archive::open(&path).unwrap();

    // No accesses yet — clean baseline.
    let s0 = arc.cluster_cache_stats();
    assert_eq!(s0.hits, 0);
    assert_eq!(s0.misses, 0);
    assert_eq!(s0.entries, 0);

    let _ = arc.get_item("first").unwrap().get_data().unwrap();
    let s1 = arc.cluster_cache_stats();
    assert_eq!(s1.misses, 1, "first access should miss");
    assert_eq!(s1.hits, 0);
    assert_eq!(s1.entries, 1);

    let _ = arc.get_item("second").unwrap().get_data().unwrap();
    let _ = arc.get_item("first").unwrap().get_data().unwrap();
    let s2 = arc.cluster_cache_stats();
    // Both later lookups are in the same cluster as the first, so they
    // must hit; misses stay at 1.
    assert_eq!(s2.misses, 1);
    assert_eq!(s2.hits, 2);
    assert_eq!(s2.entries, 1);
    assert_eq!(s2.evictions, 0);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn illustrations_enumerate_metadata_pattern() {
    // M/Illustration_48x48@1 + M/Illustration_96x96@2 + an unrelated
    // M/Title metadata key. The illustrations() helper must return
    // sorted (w, h, scale) tuples for the first two and ignore Title.

    let body_48 = b"\x89PNG48".to_vec();
    let body_96 = b"\x89PNG96".to_vec();
    let body_title = b"Test".to_vec();
    let cluster = build_uncompressed_cluster(&[&body_48, &body_96, &body_title]);

    // Dirents in (ns, url) order. All three are M/.
    let dirents = vec![
        Dir::Art(Article {
            namespace: b'M',
            url: "Illustration_48x48@1",
            title: "Illustration_48x48@1",
            mime: 0,
            cluster: 0,
            blob: 0,
        }),
        Dir::Art(Article {
            namespace: b'M',
            url: "Illustration_96x96@2",
            title: "Illustration_96x96@2",
            mime: 0,
            cluster: 0,
            blob: 1,
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
    let title_order = vec![0u32, 1, 2];
    let zim = build_zim(
        true,
        None,
        &dirents,
        &title_order,
        &[cluster],
        &["image/png", "text/plain"],
        true,
    );
    let path = write_temp("illustrations", &zim);
    let arc = Archive::open(&path).unwrap();
    assert_eq!(arc.illustrations(), vec![(48, 48, 1), (96, 96, 2)]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn title_prefix_range_brackets_matching_entries() {
    // Build a content namespace with eight titles: Alpha, Apple, Apricot,
    // Banana, Cherry, Coconut, Cranberry, Date. After title-order sort
    // they collate the same. A prefix scan for "Ap" should land on
    // [Apple, Apricot]; for "C" on [Cherry, Coconut, Cranberry];
    // "Z" on the empty range; "" on the full set.

    let bodies: Vec<&[u8]> = (0..8).map(|_| b"x" as &[u8]).collect();
    let cluster = build_uncompressed_cluster(&bodies);

    // Dirents ordered by (ns, url). Use url == lowercased title so
    // url-order matches title-order for this fixture.
    let titles_in_url_order = [
        ("alpha", "Alpha"),
        ("apple", "Apple"),
        ("apricot", "Apricot"),
        ("banana", "Banana"),
        ("cherry", "Cherry"),
        ("coconut", "Coconut"),
        ("cranberry", "Cranberry"),
        ("date", "Date"),
    ];
    let dirents: Vec<Dir> = titles_in_url_order
        .iter()
        .enumerate()
        .map(|(i, (url, title))| {
            Dir::Art(Article {
                namespace: b'C',
                url,
                title,
                mime: 0,
                cluster: 0,
                blob: i as u32,
            })
        })
        .collect();

    let title_order: Vec<u32> = (0..titles_in_url_order.len() as u32).collect();
    let zim = build_zim(
        true,
        None,
        &dirents,
        &title_order,
        &[cluster],
        &["text/plain"],
        true,
    );
    let path = write_temp("title_prefix_range", &zim);
    let arc = Archive::open(&path).unwrap();

    assert_eq!(arc.title_prefix_range(b'C', "Ap").unwrap(), 1u32..3);
    assert_eq!(arc.title_prefix_range(b'C', "C").unwrap(), 4u32..7);
    assert_eq!(arc.title_prefix_range(b'C', "Z").unwrap(), 8u32..8);
    assert_eq!(arc.title_prefix_range(b'C', "").unwrap(), 0u32..8);
    // Wrong namespace yields the empty range — modern archives' listing
    // covers C only.
    assert_eq!(arc.title_prefix_range(b'A', "A").unwrap(), 0u32..0);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn legacy_random_entry_walks_non_article_namespaces() {
    // OSM-style legacy archive: user content lives only under `I/`
    // (image/tile namespace). The previous random_content_entry impl
    // looked at `A/` only and would return EntryNotFound on this
    // shape — exactly the symptom the shim observed on
    // osm-west-asia-v3.zim. After G6 random must reach the I/J/-
    // namespaces (and skip M/X) so /random returns 302, not 404.

    let cluster = build_uncompressed_cluster(&[b"tile-a", b"tile-b", b"tile-c", b"meta", b"idx"]);
    let dirents = vec![
        Dir::Art(Article {
            namespace: b'I',
            url: "tile-a.pbf",
            title: "tile-a",
            mime: 0,
            cluster: 0,
            blob: 0,
        }),
        Dir::Art(Article {
            namespace: b'I',
            url: "tile-b.pbf",
            title: "tile-b",
            mime: 0,
            cluster: 0,
            blob: 1,
        }),
        Dir::Art(Article {
            namespace: b'I',
            url: "tile-c.pbf",
            title: "tile-c",
            mime: 0,
            cluster: 0,
            blob: 2,
        }),
        Dir::Art(Article {
            namespace: b'M',
            url: "Title",
            title: "Title",
            mime: 1,
            cluster: 0,
            blob: 3,
        }),
        Dir::Art(Article {
            namespace: b'X',
            url: "fulltextIndex/xapian",
            title: "fulltextIndex/xapian",
            mime: 1,
            cluster: 0,
            blob: 4,
        }),
    ];
    let title_order = vec![0u32, 1, 2, 3, 4];
    let zim = build_zim(
        false,
        None,
        &dirents,
        &title_order,
        &[cluster],
        &["application/x-protobuf", "text/plain"],
        true,
    );
    let path = write_temp("legacy_random", &zim);
    let arc = Archive::open(&path).unwrap();

    // Pick many times — each pick must land in `I/` (the only user
    // namespace), never in `M/` or `X/`. A handful of picks would be
    // enough; 50 keeps the failure mode obvious without slowing the
    // suite.
    for _ in 0..50 {
        let entry = arc.random_content_entry().unwrap();
        let ns = entry.namespace();
        assert!(
            ns != b'M' && ns != b'X',
            "random returned non-user namespace {}",
            ns as char
        );
        assert_eq!(ns, b'I');
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn legacy_archive_media_count_includes_non_article_namespaces() {
    // Regression for ZIMRU_GAPS.md G1: on legacy (uses_new_namespaces=false)
    // archives the article/media walk must visit user content outside the
    // `A/` namespace (`-/` for layout, `I/` for images, `J/` for image-text)
    // and only exclude `M/` (metadata) and `X/` (search indexes). Counting
    // just `A/` makes catalogs of legacy archives report `media_count=0`.

    let mime_html = 0u16; // text/html
    let mime_png = 1u16; // image/png
    let mime_css = 2u16; // text/css
    let mime_xapian = 3u16; // application/octet-stream
    let mime_text = 4u16; // text/plain

    let blob_css = b"body{}".to_vec();
    let blob_page1 = b"<h1>1</h1>".to_vec();
    let blob_page2 = b"<h1>2</h1>".to_vec();
    let blob_png = b"\x89PNGstub".to_vec();
    let blob_title = b"Test Legacy".to_vec();
    let blob_idx = b"xapian-bytes".to_vec();

    let cluster = build_uncompressed_cluster(&[
        &blob_css,
        &blob_page1,
        &blob_page2,
        &blob_png,
        &blob_title,
        &blob_idx,
    ]);

    // Dirents in (namespace, url) order — `-` (0x2D), `A`, `I`, `M`, `X`.
    let dirents = vec![
        Dir::Art(Article {
            namespace: b'-',
            url: "style.css",
            title: "Style",
            mime: mime_css,
            cluster: 0,
            blob: 0,
        }),
        Dir::Red(Redirect {
            namespace: b'A',
            url: "aredirect",
            title: "ARedir",
            target_url_index: 2,
        }),
        Dir::Art(Article {
            namespace: b'A',
            url: "page1",
            title: "One",
            mime: mime_html,
            cluster: 0,
            blob: 1,
        }),
        Dir::Art(Article {
            namespace: b'A',
            url: "page2",
            title: "Two",
            mime: mime_html,
            cluster: 0,
            blob: 2,
        }),
        Dir::Art(Article {
            namespace: b'I',
            url: "img.png",
            title: "Image",
            mime: mime_png,
            cluster: 0,
            blob: 3,
        }),
        Dir::Art(Article {
            namespace: b'M',
            url: "Title",
            title: "Title",
            mime: mime_text,
            cluster: 0,
            blob: 4,
        }),
        Dir::Art(Article {
            namespace: b'X',
            url: "idx",
            title: "Idx",
            mime: mime_xapian,
            cluster: 0,
            blob: 5,
        }),
    ];

    // Title order (url-pointer indices sorted by (ns, title)).
    let title_order = vec![0u32, 1, 2, 3, 4, 5, 6];

    let zim = build_zim(
        false,
        None,
        &dirents,
        &title_order,
        &[cluster],
        &[
            "text/html",
            "image/png",
            "text/css",
            "application/octet-stream",
            "text/plain",
        ],
        true,
    );
    let path = write_temp("legacy_media_count", &zim);
    let arc = Archive::open(&path).unwrap();

    assert!(
        !arc.header().uses_new_namespaces(),
        "legacy archive expected"
    );
    // Two text/html articles in `A/` (not counting the redirect).
    assert_eq!(arc.article_count().unwrap(), 2);
    // Two media items: `-/style.css` and `I/img.png`. The `M/Title` and
    // `X/idx` entries must be excluded, the redirect must be skipped.
    assert_eq!(arc.media_count().unwrap(), 2);

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

#[test]
fn version_60_retains_legacy_namespaces_and_layout_paths() {
    let dirents = [
        Dir::Art(Article {
            namespace: b'-',
            url: "style.css",
            title: "Style",
            mime: 1,
            cluster: 0,
            blob: 0,
        }),
        Dir::Art(Article {
            namespace: b'A',
            url: "home",
            title: "Home",
            mime: 0,
            cluster: 0,
            blob: 1,
        }),
        Dir::Art(Article {
            namespace: b'I',
            url: "image",
            title: "Image",
            mime: 1,
            cluster: 0,
            blob: 2,
        }),
    ];
    let mut zim = build_zim(
        false,
        Some(1),
        &dirents,
        &[0, 1, 2],
        &[build_uncompressed_cluster(&[
            b"body{}",
            b"<h1>Home</h1>",
            b"pixels",
        ])],
        &["text/html", "text/plain"],
        false,
    );
    zim[4..6].copy_from_slice(&6u16.to_le_bytes());
    let path = write_temp("legacy_60", &zim);
    let archive = Archive::open(&path).unwrap();
    assert!(!archive.header().uses_new_namespaces());
    assert_eq!(archive.get_bytes("-/style.css").unwrap(), b"body{}");
    assert_eq!(archive.get_bytes("A/home").unwrap(), b"<h1>Home</h1>");
    assert_eq!(archive.get_bytes("home").unwrap(), b"<h1>Home</h1>");
    assert_eq!(archive.get_bytes("I/image").unwrap(), b"pixels");
    assert_eq!(
        archive.get_entry_by_title("Home").unwrap().namespace(),
        b'A'
    );
    assert_eq!(archive.article_and_media_counts().unwrap(), (1, 2));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn checksum_offset_overflow_is_a_truncation_error() {
    let mut zim = build_zim(true, None, &[], &[], &[], &[], false);
    zim[72..80].copy_from_slice(&u64::MAX.to_le_bytes());
    let path = write_temp("checksum_max", &zim);
    let archive = Archive::open(&path).unwrap();
    assert!(matches!(
        archive.checksum(),
        Err(Error::Truncated(u64::MAX))
    ));
    std::fs::remove_file(path).unwrap();
}

fn coexistence_zim(header: &[u32], v0: &[u32], v1: &[u32]) -> Vec<u8> {
    let dirents = [
        Dir::Art(Article {
            namespace: b'C',
            url: "alpha",
            title: "",
            mime: 0,
            cluster: 0,
            blob: 0,
        }),
        Dir::Art(Article {
            namespace: b'C',
            url: "beta",
            title: "Beta",
            mime: 0,
            cluster: 0,
            blob: 1,
        }),
        Dir::Art(Article {
            namespace: b'C',
            url: "image",
            title: "Icon",
            mime: 1,
            cluster: 0,
            blob: 2,
        }),
        Dir::Art(Article {
            namespace: b'M',
            url: "Title",
            title: "",
            mime: 1,
            cluster: 0,
            blob: 3,
        }),
        Dir::Art(Article {
            namespace: b'X',
            url: "listing/titleOrdered/v0",
            title: "",
            mime: 1,
            cluster: 0,
            blob: 4,
        }),
        Dir::Art(Article {
            namespace: b'X',
            url: "listing/titleOrdered/v1",
            title: "",
            mime: 1,
            cluster: 0,
            blob: 5,
        }),
    ];
    let v0: Vec<u8> = v0.iter().flat_map(|i| i.to_le_bytes()).collect();
    let v1: Vec<u8> = v1.iter().flat_map(|i| i.to_le_bytes()).collect();
    build_zim(
        true,
        Some(0),
        &dirents,
        header,
        &[build_uncompressed_cluster(&[
            b"alpha body",
            b"beta body",
            b"pixels",
            b"Catalog",
            &v0,
            &v1,
        ])],
        &["text/html", "application/octet-stream"],
        false,
    )
}

#[test]
fn modern_article_listing_wins_over_coexisting_full_tables() {
    let full = [1, 2, 0, 3, 4, 5];
    let zim = coexistence_zim(&full, &full, &[1, 0]);
    let path = write_temp("titles_coexist", &zim);
    let archive = Archive::open(&path).unwrap();
    assert_eq!(archive.title_listing().unwrap().as_ref(), &[1, 0]);
    assert_eq!(archive.title_count().unwrap(), 2);
    let titles: Vec<_> = archive
        .iter_by_title()
        .map(|e| e.unwrap().title().to_owned())
        .collect();
    assert_eq!(titles, ["Beta", "alpha"]);
    assert_eq!(archive.title_prefix_range(b'C', "a").unwrap(), 1..2);
    // The article subset does not make ordinary title/path lookup lose media.
    assert_eq!(archive.get_entry_by_title("Icon").unwrap().path(), "image");
    assert_eq!(
        archive.entry_by_ns_title(b'M', "Title").unwrap().path(),
        "Title"
    );
    assert_eq!(archive.get_bytes("image").unwrap(), b"pixels");
    assert!(archive.check_title_index().unwrap());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn title_integrity_checks_every_table_not_just_the_selected_subset() {
    let full = [1, 2, 0, 3, 4, 5];
    let duplicate = [1, 2, 0, 3, 4, 4];
    for (name, header, v0, v1) in [
        (
            "bad_header",
            duplicate.as_slice(),
            full.as_slice(),
            &[1, 0][..],
        ),
        ("bad_v0", full.as_slice(), duplicate.as_slice(), &[1, 0][..]),
        ("short_v0", full.as_slice(), &[1, 0][..], &[1, 0][..]),
        ("unsorted_v1", full.as_slice(), full.as_slice(), &[0, 1][..]),
        ("metadata_v1", full.as_slice(), full.as_slice(), &[1, 3][..]),
        (
            "duplicate_v1",
            full.as_slice(),
            full.as_slice(),
            &[1, 1][..],
        ),
        (
            "out_of_range_v1",
            full.as_slice(),
            full.as_slice(),
            &[6][..],
        ),
    ] {
        let path = write_temp(name, &coexistence_zim(header, v0, v1));
        let archive = Archive::open(&path).unwrap();
        assert!(!archive.check_title_index().unwrap(), "{name}");
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn empty_modern_article_subset_does_not_fall_back_to_full_table() {
    let full = [1, 2, 0, 3, 4, 5];
    let path = write_temp("titles_empty_subset", &coexistence_zim(&full, &full, &[]));
    let archive = Archive::open(&path).unwrap();
    assert_eq!(archive.title_count().unwrap(), 0);
    assert_eq!(archive.iter_by_title().count(), 0);
    assert_eq!(archive.get_entry_by_title("Beta").unwrap().path(), "beta");
    assert!(archive.check_title_index().unwrap());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn malformed_pointer_tables_fail_before_declared_count_allocations() {
    let mut zim = build_zim(true, None, &[], &[], &[], &[], false);
    zim[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    let path = write_temp("huge_title_count", &zim);
    let archive = Archive::open(&path).unwrap();
    assert!(matches!(archive.title_listing(), Err(Error::Truncated(_))));
    assert!(matches!(
        archive.entry_by_url_index(0),
        Err(Error::Truncated(_))
    ));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn malformed_cluster_offsets_reject_unaligned_and_header_overlapping_ranges() {
    for (name, words, blob) in [
        ("unaligned", vec![9u32, 12, 12], 0),
        ("inside_table", vec![12u32, 8, 12], 1),
        ("past_payload", vec![8u32, u32::MAX, 12], 0),
    ] {
        let mut cluster = vec![1];
        cluster.extend(words.into_iter().flat_map(u32::to_le_bytes));
        let dirents = [Dir::Art(Article {
            namespace: b'C',
            url: "bad",
            title: "",
            mime: 0,
            cluster: 0,
            blob,
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
        let path = write_temp(name, &zim);
        let archive = Archive::open(&path).unwrap();
        assert!(archive.validate_cluster(0).is_err(), "{name}");
        assert!(archive.blob_direct_access(0, blob).is_err(), "{name}");
        assert!(archive.get_bytes("bad").is_err(), "{name}");
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn cluster_reference_validation_checks_compressed_and_direct_blob_indices() {
    for (name, cluster) in [
        ("refs_direct", build_uncompressed_cluster(&[b"body"])),
        ("refs_compressed", build_zstd_cluster(&[b"body"])),
    ] {
        let dirents = [Dir::Art(Article {
            namespace: b'C',
            url: "home",
            title: "",
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
        let path = write_temp(name, &zim);
        let archive = Archive::open(&path).unwrap();
        archive.validate_cluster_references(0, &[0]).unwrap();
        assert!(matches!(
            archive.validate_cluster_references(0, &[1]),
            Err(Error::BadBlobIndex {
                blob: 1,
                count: 1,
                ..
            })
        ));
        assert_eq!(archive.get_bytes("home").unwrap(), b"body");
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn modern_v0_only_archive_keeps_its_full_title_listing() {
    let full = [1, 2, 0, 3, 4, 5];
    let mut zim = coexistence_zim(&full, &full, &[]);
    zim[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
    // Rename v1 to an ordinary index entry, leaving only v0 available.
    let name = b"listing/titleOrdered/v1";
    let start = zim.windows(name.len()).position(|s| s == name).unwrap();
    zim[start + name.len() - 1] = b'2';
    let path = write_temp("titles_v0_only", &zim);
    let archive = Archive::open(&path).unwrap();
    assert_eq!(archive.title_count().unwrap(), 6);
    let paths: Vec<_> = archive
        .iter_by_title()
        .map(|e| e.unwrap().path().to_owned())
        .collect();
    assert_eq!(
        paths,
        [
            "beta",
            "image",
            "alpha",
            "Title",
            "listing/titleOrdered/v0",
            "listing/titleOrdered/v2"
        ]
    );
    assert!(archive.check_title_index().unwrap());
    std::fs::remove_file(path).unwrap();
}
