#![cfg(feature = "writer")]
//! Tests for the ZIM writer (`zimru::writer::Creator`).
//!
//! Strategy for each test:
//!   1. Build an archive in memory using `Creator`.
//!   2. Write it to a temp file.
//!   3. Reopen it with our own `Archive` and assert the structure matches.
//!   4. (Where present) shell out to upstream `zimcheck -A` for independent
//!      validation that the file conforms to the spec. Skipped when
//!      upstream is not installed so CI without it still passes.
//!
//! All temp files are deleted at the end of each test.

use std::path::PathBuf;
use std::process::Command;

use zimru::writer::{Creator, Item};
use zimru::{Archive, Compression};

fn tmp_path(tag: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut p = std::env::temp_dir();
    p.push(format!(
        "zimru-writer-{tag}-{}-{}.zim",
        std::process::id(),
        ns
    ));
    p
}

/// Minimal byte-sequence that starts with the PNG magic so upstream
/// `zimcheck` is willing to accept it as a valid illustration. Not a
/// renderable PNG — just the magic + a stub IHDR + IEND — but that's
/// all zimcheck actually checks (the regex is `^\x89PNG\x0d\x0a\x1a\x0a`).
fn minimal_png() -> Vec<u8> {
    // PNG signature.
    let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    // A 1×1 placeholder IHDR (not a valid CRC, but zimcheck doesn't look).
    v.extend_from_slice(&13u32.to_be_bytes()); // chunk length
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&1u32.to_be_bytes()); // width
    v.extend_from_slice(&1u32.to_be_bytes()); // height
    v.extend_from_slice(&[8, 0, 0, 0, 0]); // bit depth, color type, ...
    v.extend_from_slice(&[0, 0, 0, 0]); // stub CRC
                                        // Empty IDAT.
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(b"IDAT");
    v.extend_from_slice(&[0, 0, 0, 0]);
    // IEND.
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(b"IEND");
    v.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]); // real IEND CRC
    v
}

fn upstream_zimcheck() -> Option<PathBuf> {
    let candidate = PathBuf::from("/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.6.0/zimcheck");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn upstream_zimdump() -> Option<PathBuf> {
    let candidate = PathBuf::from("/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.6.0/zimdump");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

/// Run `upstream zimcheck -A` on a file and assert it passes.
fn assert_upstream_ok(path: &std::path::Path) {
    let Some(zimcheck) = upstream_zimcheck() else {
        eprintln!("skip: upstream zimcheck not available at /opt/...");
        return;
    };
    let out = Command::new(&zimcheck)
        .arg("-A")
        .arg(path)
        .output()
        .expect("run zimcheck");
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("upstream zimcheck -A output:\n{stdout}");
    assert!(
        out.status.success() || stdout.contains("Overall Test Status: Pass"),
        "upstream zimcheck rejected the file:\n{stdout}"
    );
}

fn assert_upstream_checksum_ok(path: &std::path::Path) {
    let Some(zimcheck) = upstream_zimcheck() else {
        return;
    };
    let out = Command::new(&zimcheck)
        .arg("-C")
        .arg(path)
        .output()
        .expect("run zimcheck -C");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Overall Test Status: Pass"),
        "upstream zimcheck -C failed:\n{stdout}"
    );
}

// --------------------------------------------------------------------

#[test]
fn roundtrips_a_single_article() {
    let out = tmp_path("single");
    let mut c = Creator::new();
    c.set_compression(Compression::None);
    c.add_item(Item::html("home", "Home", "<h1>Hello</h1>"));
    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    // 1 user item + auto-emitted M/Counter + auto-emitted
    // X/listing/titleOrdered/v1 = 3 dirents.
    assert_eq!(a.entry_count(), 3);
    // 1 cluster for the user item, 1 cluster for the M/Counter
    // tail batch, 1 cluster for the listing entry — adjacent
    // pushes that don't share buckets.
    assert!(a.cluster_count() >= 1);
    assert_eq!(a.get_text("home").unwrap(), "<h1>Hello</h1>");
    assert!(a.check().unwrap(), "our checksum must verify");

    assert_upstream_checksum_ok(&out);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn roundtrips_items_metadata_and_main_page() {
    let out = tmp_path("full");
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.set_main_path("home");
    c.add_item(Item::html("home", "Home", "<h1>Welcome</h1>"));
    c.add_item(Item::html(
        "about",
        "About",
        "<p>An archive built by zimru.</p>",
    ));
    c.add_item(Item::text(
        "robots.txt",
        "robots",
        "User-agent: *\nDisallow:\n",
    ));
    c.add_item(Item::png(
        "favicon.png",
        "favicon",
        vec![0x89, 0x50, 0x4E, 0x47],
    ));
    c.add_metadata("Title", "Zimru Demo");
    c.add_metadata("Description", "Round-trip test");
    c.add_metadata("Language", "eng");
    c.add_metadata("Creator", "zimru tests");
    c.add_metadata("Publisher", "zimru");
    c.add_metadata("Date", "2026-04-25");
    c.add_metadata("Name", "zimru-roundtrip");
    c.add_illustration(48, minimal_png()); // real PNG magic, satisfies zimcheck regex
    c.add_redirection("start", "Start", "home");
    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    assert!(a.has_main_entry());
    assert_eq!(a.main_path().unwrap(), "home");
    assert_eq!(a.get_text("home").unwrap(), "<h1>Welcome</h1>");
    assert_eq!(
        a.get_text("about").unwrap(),
        "<p>An archive built by zimru.</p>"
    );
    assert_eq!(
        a.get_text("robots.txt").unwrap(),
        "User-agent: *\nDisallow:\n"
    );
    assert_eq!(
        a.get_bytes("favicon.png").unwrap(),
        vec![0x89, 0x50, 0x4E, 0x47]
    );

    // Redirect should resolve to the target.
    let start = a.get_entry_by_path("start").unwrap();
    assert!(start.is_redirect());
    assert_eq!(start.resolve().unwrap().path(), "home");

    // Metadata is UTF-8 addressable.
    assert_eq!(a.metadata_str("Title").unwrap(), "Zimru Demo");
    assert_eq!(a.metadata_str("Language").unwrap(), "eng");
    let keys = a.get_metadata_keys();
    for required in [
        "Title",
        "Description",
        "Language",
        "Creator",
        "Publisher",
        "Date",
        "Name",
    ] {
        assert!(
            keys.iter().any(|k| k == required),
            "missing metadata: {required}"
        );
    }

    // Illustration is visible.
    assert!(a.has_metadata("Illustration_48x48@1"));

    // Our own checksum verification + upstream's full sweep.
    assert!(a.check().unwrap());
    assert_upstream_ok(&out);

    let _ = std::fs::remove_file(&out);
}

#[test]
fn splits_into_multiple_clusters_when_content_exceeds_target() {
    let out = tmp_path("multi-cluster");
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    // 16 items × 128 KiB each = 2 MiB total; target 256 KiB forces ≥ 8 clusters.
    c.set_cluster_size_target(256 * 1024);
    for i in 0..16 {
        let body = vec![b'A' + (i % 26) as u8; 128 * 1024];
        c.add_item(Item::new(
            format!("a{i:02}"),
            format!("Article {i}"),
            "text/plain",
            body,
        ));
    }
    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    // 16 user items + auto-emitted M/Counter + X/listing/titleOrdered/v1 = 18.
    assert_eq!(a.entry_count(), 18);
    assert!(
        a.cluster_count() >= 8,
        "expected ≥8 clusters, got {}",
        a.cluster_count()
    );
    // Every article decompresses intact.
    for i in 0..16 {
        let want = vec![b'A' + (i % 26) as u8; 128 * 1024];
        assert_eq!(a.get_bytes(&format!("a{i:02}")).unwrap(), want);
    }
    assert!(a.check().unwrap());
    assert_upstream_checksum_ok(&out);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn written_file_passes_upstream_zimcheck_integrity() {
    if upstream_zimcheck().is_none() {
        eprintln!("skip: upstream zimcheck not installed");
        return;
    }
    let out = tmp_path("integrity");
    let mut c = Creator::new();
    c.set_main_path("home");
    c.add_item(Item::html(
        "home",
        "Home",
        "<html><body><h1>H</h1><a href=\"about\">About</a></body></html>",
    ));
    c.add_item(Item::html(
        "about",
        "About",
        "<html><body><p>A</p></body></html>",
    ));
    c.add_metadata("Title", "integrity test");
    c.add_metadata("Description", "checked by upstream");
    c.add_metadata("Language", "eng");
    c.add_metadata("Creator", "zimru");
    c.add_metadata("Publisher", "zimru");
    c.add_metadata("Date", "2026-04-25");
    c.add_metadata("Name", "zimru-integrity");
    c.add_illustration(48, minimal_png());
    c.write_to(&out).expect("write");

    // Upstream integrity and checksum must both pass.
    let zc = upstream_zimcheck().unwrap();
    for flag in ["-I", "-C", "-M", "-P"] {
        let out_ = Command::new(&zc).arg(flag).arg(&out).output().expect("run");
        let s = String::from_utf8_lossy(&out_.stdout);
        assert!(s.contains("Pass"), "upstream zimcheck {flag} failed:\n{s}");
    }

    let _ = std::fs::remove_file(&out);
}

#[test]
fn written_file_readable_by_upstream_zimdump() {
    let Some(zimdump) = upstream_zimdump() else {
        eprintln!("skip: upstream zimdump not installed");
        return;
    };
    let out = tmp_path("zimdump");
    let mut c = Creator::new();
    c.set_main_path("home");
    c.add_item(Item::html("home", "Home", "<h1>Hi</h1>"));
    c.add_metadata("Title", "dump test");
    c.add_metadata("Language", "eng");
    c.write_to(&out).expect("write");

    // zimdump info should print a sane summary.
    let info = Command::new(&zimdump)
        .arg("info")
        .arg(&out)
        .output()
        .expect("info");
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(stdout.contains("uuid:"), "info missing uuid:\n{stdout}");
    assert!(
        stdout.contains("cluster count:"),
        "info missing cluster count:\n{stdout}"
    );
    assert!(
        stdout.contains("main page:"),
        "info missing main page:\n{stdout}"
    );

    // zimdump list should enumerate the C-namespace entry.
    let list = Command::new(&zimdump)
        .arg("list")
        .arg(&out)
        .output()
        .expect("list");
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("home"), "list missing 'home':\n{stdout}");

    let _ = std::fs::remove_file(&out);
}

#[test]
fn zimru_reader_round_trips_every_compression() {
    for comp in [Compression::None, Compression::Zstd, Compression::Xz] {
        let out = tmp_path(&format!("comp-{comp:?}"));
        let mut c = Creator::new();
        c.set_compression(comp);
        c.add_item(Item::text("a", "A", "alpha"));
        c.add_item(Item::text("b", "B", "beta"));
        c.add_item(Item::text("c", "C", "gamma"));
        c.write_to(&out).unwrap();

        let a = Archive::open(&out).unwrap();
        assert_eq!(a.get_text("a").unwrap(), "alpha");
        assert_eq!(a.get_text("b").unwrap(), "beta");
        assert_eq!(a.get_text("c").unwrap(), "gamma");
        assert!(a.check().unwrap());

        let _ = std::fs::remove_file(&out);
    }
}

#[test]
fn compression_level_round_trips_at_extremes() {
    // For zstd and xz, build the same archive at the lowest and highest
    // levels. Both must round-trip identically; the high-level output must
    // not be larger than the low-level output (the payload is the highly
    // redundant "AAAA…" pattern, so a higher level can only do as well or
    // better).
    let payload = vec![b'A'; 64 * 1024];

    for (comp, lo, hi) in [(Compression::Zstd, 1, 19), (Compression::Xz, 0, 9)] {
        let make = |level: i32| -> std::path::PathBuf {
            let p = tmp_path(&format!("level-{comp:?}-{level}"));
            let mut c = Creator::new();
            c.set_compression(comp);
            c.set_compression_level(level);
            c.add_item(Item::text("blob", "Blob", payload.clone()));
            c.write_to(&p).expect("write");
            p
        };

        let lo_path = make(lo);
        let hi_path = make(hi);

        let lo_size = std::fs::metadata(&lo_path).unwrap().len();
        let hi_size = std::fs::metadata(&hi_path).unwrap().len();
        assert!(
            hi_size <= lo_size,
            "{comp:?}: level {hi} ({hi_size} bytes) larger than level {lo} ({lo_size} bytes)"
        );

        for p in [&lo_path, &hi_path] {
            let a = Archive::open(p).expect("reopen");
            assert_eq!(a.get_bytes("blob").unwrap(), payload);
            assert!(a.check().unwrap(), "checksum should verify ({comp:?})");
            let _ = std::fs::remove_file(p);
        }
    }
}

#[test]
fn empty_title_normalises_to_url_for_correct_title_sort() {
    // Regression for shim-zimwriterfs interaction: zim-tools'
    // `zimwriterfs` passes title="" for non-HTML items, expecting the
    // writer to derive "filename" from the path. The writer now
    // normalises empty titles to the url before sorting, so the
    // title-pointer list ends up correctly sorted (was previously
    // putting empty-title entries at index 0 since "" < everything,
    // failing zimcheck -I with "Title index is not properly sorted").
    //
    // Mix of explicit-title HTML items and empty-title binary items;
    // after open the title-order iteration must be in lexicographic
    // (ns, title) order using the *url* as the title for the
    // empty-title entries.

    let out = tmp_path("empty-title");
    let mut c = Creator::new();
    // Items in URL order, with mixed explicit / empty titles.
    c.add_item(Item::new(
        "article1.html".to_string(),
        "Article 1".to_string(),
        "text/html".to_string(),
        b"<h1>1</h1>".to_vec(),
    ));
    c.add_item(Item::new(
        "icon.png".to_string(),
        String::new(), // empty — should fall back to url at sort time
        "image/png".to_string(),
        b"\x89PNGstub".to_vec(),
    ));
    c.add_item(Item::new(
        "index.html".to_string(),
        "Test Site".to_string(),
        "text/html".to_string(),
        b"<h1>home</h1>".to_vec(),
    ));
    c.add_item(Item::new(
        "style.css".to_string(),
        String::new(), // empty
        "text/css".to_string(),
        b"body{}".to_vec(),
    ));
    c.add_item(Item::html("untitled.html", "", "<p>untitled</p>"));
    c.write_to(&out).expect("write");

    let arc = Archive::open(&out).expect("open");
    // Modern iteration uses the v1 article subset. Resources stay out,
    // while an HTML entry with an empty title sorts by its path.
    let titles: Vec<String> = arc
        .iter_by_title()
        .map(|r| r.unwrap().title().to_string())
        .collect();
    assert_eq!(
        titles,
        vec![
            "Article 1".to_string(),
            "Test Site".to_string(),
            "untitled.html".to_string(),
        ],
        "title order should sort by effective title (url-fallback for empty)"
    );
    assert!(
        arc.check_title_index().unwrap(),
        "the full v0 table remains valid"
    );

    // Independent validation: real zimcheck -I must report Pass on the
    // produced ZIM. The previous bug surfaced as "Title index is not
    // properly sorted" — that's the symptom we're guarding against.
    // Skip the assert if the binary launched but produced no output
    // at all (e.g. broken libzim/xapian ABI on the host) — without
    // a "Status:" line there's nothing meaningful to compare. A real
    // ZIM regression produces a "Status: Fail" line, which we'd still
    // catch.
    if let Ok(out_bin) = Command::new("zimcheck").args(["-I"]).arg(&out).output() {
        let stdout = String::from_utf8_lossy(&out_bin.stdout);
        if stdout.contains("Status:") {
            assert!(
                stdout.contains("Status: Pass"),
                "real zimcheck -I should Pass on the empty-title round-trip:\n{stdout}"
            );
        }
    }

    let _ = std::fs::remove_file(&out);
}

#[test]
fn rejects_dangling_redirect() {
    let out = tmp_path("bad-redirect");
    let mut c = Creator::new();
    c.add_item(Item::text("real", "Real", "present"));
    c.add_redirection("alias", "Alias", "nonexistent");
    if let Ok(()) = c.write_to(&out) {
        panic!("expected error for dangling redirect")
    }
    // File may exist but partial — don't try to read it.
    let _ = std::fs::remove_file(&out);
}

#[test]
fn caller_supplied_counter_suppresses_auto_generated_one() {
    // A caller-supplied M/Counter arrives in finalize step 1 and sits
    // in a pending bucket when the auto-Counter guard runs — the guard
    // must see it there (not only in committed dirents), or the output
    // carries two (M, Counter) dirents and violates the unique-URL
    // invariant.
    let out = tmp_path("caller-counter");
    let mut c = Creator::new();
    c.set_main_path("home");
    c.start_writing(&out).expect("start_writing");
    c.add_item(Item::html("home", "Home", "<html><body>hi</body></html>"));
    c.add_metadata("Title", "counter dedup");
    c.add_metadata("Language", "eng");
    c.add_metadata("Counter", "text/html=1");
    c.finish_writing().expect("finish_writing");

    let arc = Archive::open(&out).expect("reopen");
    let counters: Vec<_> = arc
        .iter_by_path()
        .map(|e| e.unwrap())
        .filter(|e| e.namespace() == b'M' && e.path() == "Counter")
        .collect();
    assert_eq!(
        counters.len(),
        1,
        "exactly one M/Counter dirent must exist (caller's copy wins)"
    );
    let bytes = counters[0].get_item(false).unwrap().bytes().unwrap();
    assert_eq!(
        bytes, b"text/html=1",
        "the caller-supplied Counter value must be preserved verbatim"
    );

    let _ = std::fs::remove_file(&out);
}

#[test]
fn auto_counter_counts_pending_content() {
    // When the auto M/Counter is generated (finalize step 2a), a small
    // archive's entire content is still sitting in un-flushed buckets —
    // the histogram must count those pending articles too (it used to
    // come out empty), and must count only C-namespace articles
    // (metadata / index entries excluded, matching libzim).
    let out = tmp_path("auto-counter");
    let mut c = Creator::new();
    c.set_main_path("home");
    c.add_item(Item::html("home", "Home", "<html><body>a</body></html>"));
    c.add_item(Item::html("about", "About", "<html><body>b</body></html>"));
    // Parameterised media types fold onto the bare type: zimcheck's Counter
    // grammar admits neither `;` nor spaces inside a key.
    c.add_item(Item::new(
        "legacy",
        "Legacy",
        "text/html; charset=iso-8859-1",
        b"<html><body>c</body></html>".to_vec(),
    ));
    c.add_item(Item::new(
        "logo.png",
        "Logo",
        "image/png",
        vec![1u8, 2, 3, 4],
    ));
    c.add_metadata("Title", "auto counter");
    c.add_metadata("Language", "eng");
    c.write_to(&out).expect("write");

    let arc = Archive::open(&out).expect("reopen");
    let counter = String::from_utf8(arc.get_metadata("Counter").expect("Counter present"))
        .expect("Counter is utf8");
    assert!(
        counter.contains("text/html=3"),
        "counter should fold the charset variant into text/html, got {counter:?}"
    );
    assert!(
        !counter.contains("charset"),
        "counter keys must not carry MIME parameters, got {counter:?}"
    );
    assert!(
        counter.contains("image/png=1"),
        "counter should count the png item, got {counter:?}"
    );
    assert!(
        !counter.contains("text/plain"),
        "M-namespace metadata must not be counted, got {counter:?}"
    );
    let _ = std::fs::remove_file(&out);
}

#[test]
fn v1_listing_contains_only_front_articles_at_final_url_indices() {
    let out = tmp_path("article-listing");
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd)
        .set_compression_level(1);
    c.add_item(Item::in_namespace(
        b'A',
        "legacy",
        "Legacy",
        "text/html",
        b"legacy",
    ));
    c.add_item(Item::html("home", "Home", "<p>home</p>"));
    c.add_item(Item::new(
        "parameterized",
        "Parameters",
        "text/html;charset=utf-8",
        b"<p>p</p>",
    ));
    c.add_item(Item::html("untitled.html", "", "<p>untitled</p>"));
    c.add_item(Item::png("icon.png", "Icon", vec![1, 2, 3]));
    c.add_item(Item::text("robots.txt", "Robots", "no"));
    c.add_redirection("alias", "First", "chain");
    c.add_redirection("chain", "Second", "home");
    c.add_redirection("image-alias", "Not an article", "icon.png");
    c.add_metadata_with_mimetype("HTML", "text/html", "<p>metadata</p>");
    c.set_main_path("home");
    for (namespace, path) in [(b'X', "aaa"), (b'X', "zzz"), (b'Z', "after-listing")] {
        c.add_item(Item::in_namespace(
            namespace,
            path,
            path,
            "text/plain",
            b"other",
        ));
    }
    c.write_to(&out).unwrap();

    let arc = Archive::open(&out).unwrap();
    let entries: Vec<_> = arc.iter_by_path().map(Result::unwrap).collect();
    let listing = entries
        .iter()
        .find(|e| e.namespace() == b'X' && e.path() == "listing/titleOrdered/v1")
        .unwrap()
        .get_item(false)
        .unwrap();
    assert_eq!(
        arc.cluster(listing.cluster_index()).unwrap().compression(),
        Compression::None
    );
    let bytes = listing.bytes().unwrap();
    let expected_paths = ["alias", "home", "parameterized", "chain", "untitled.html"];
    let expected_indices: Vec<u32> = expected_paths
        .iter()
        .map(|path| {
            entries
                .iter()
                .position(|e| e.namespace() == b'C' && e.path() == *path)
                .unwrap() as u32
        })
        .collect();
    let expected_bytes: Vec<u8> = expected_indices
        .iter()
        .flat_map(|idx| idx.to_le_bytes())
        .collect();
    assert_eq!(
        bytes, expected_bytes,
        "listing must reference the final URL table"
    );
    assert_eq!(arc.title_count().unwrap(), expected_paths.len() as u32);
    assert_eq!(
        arc.iter_by_title()
            .map(|e| e.unwrap().path().to_string())
            .collect::<Vec<_>>(),
        expected_paths
    );
    assert!(arc.check_title_index().unwrap());
    let _ = std::fs::remove_file(out);
}

#[test]
fn invalid_strings_are_rejected_without_reserving_keys_or_corrupting_output() {
    for streaming in [false, true] {
        let out = tmp_path("invalid-strings");
        let mut c = Creator::new();
        c.set_compression(Compression::None);
        if streaming {
            c.start_writing(&out).unwrap();
        }
        for item in [
            Item::html("bad\0path", "Title", "x"),
            Item::html("bad\npath", "Title", "x"),
            Item::html("reusable", "bad\u{0085}title", "x"),
            Item::in_namespace(0, "bad-ns", "Title", "text/plain", b"x"),
        ] {
            assert!(matches!(
                c.try_add_item(item),
                Err(zimru::Error::Io(e)) if e.kind() == std::io::ErrorKind::InvalidInput
            ));
        }
        for mime in [
            "",
            "plain",
            "/plain",
            "text/",
            "text/plain\0evil",
            "text/\nplain",
            "text/plain;missing-value",
            "text/plain;charset=\"unterminated",
        ] {
            assert!(
                c.try_add_item(Item::new("reusable", "Title", mime, b"x"))
                    .is_err(),
                "{mime:?}"
            );
        }
        assert!(c.try_add_metadata("bad\0name", b"x").is_err());
        assert!(c
            .try_add_metadata_with_mimetype("Title", "text/plain\r\nbad", b"x")
            .is_err());
        assert!(c
            .try_add_redirection("alias", "Alias", "bad\0target")
            .is_err());
        assert!(c.try_set_main_path("bad\0target").is_err());

        c.try_add_item(Item::html("reusable", "Reusable", "original"))
            .unwrap();
        c.try_add_item(Item::new(
            "café/日本語 page",
            "A \"quoted\" title",
            "text/plain; charset=\"utf-8\"",
            b"unicode path",
        ))
        .unwrap();
        c.try_add_metadata("Title", "valid title").unwrap();
        c.try_add_redirection("alias", "Alias", "reusable").unwrap();
        c.try_set_main_path("reusable").unwrap();
        c.write_to(&out).unwrap();
        let arc = Archive::open(&out).unwrap();
        assert_eq!(arc.get_text("reusable").unwrap(), "original");
        assert_eq!(arc.get_text("café/日本語 page").unwrap(), "unicode path");
        assert_eq!(arc.get_text("alias").unwrap(), "original");
        assert_eq!(arc.metadata_str("Title").unwrap(), "valid title");
        assert_eq!(arc.main_path().unwrap(), "reusable");
        assert!(arc.check().unwrap());
        let _ = std::fs::remove_file(out);
    }
}

#[test]
fn duplicate_keys_are_rejected_across_entry_creation_apis() {
    let out = tmp_path("duplicate-keys");
    let mut c = Creator::new();
    c.try_add_item(Item::html("home", "Home", "original"))
        .unwrap();
    assert!(c
        .try_add_item(Item::html("home", "Replacement", "wrong"))
        .is_err());
    assert!(c.try_add_redirection("home", "Alias", "home").is_err());
    c.try_add_metadata("Title", "original title").unwrap();
    assert!(c
        .try_add_item(Item::in_namespace(
            b'M',
            "Title",
            "",
            "text/plain",
            b"wrong"
        ))
        .is_err());
    c.try_add_illustration(48, b"original illustration")
        .unwrap();
    assert!(c
        .try_add_metadata("Illustration_48x48@1", b"wrong")
        .is_err());
    c.try_set_main_path("home").unwrap();
    assert!(c
        .try_add_item(Item::in_namespace(
            b'W',
            "mainPage",
            "",
            "text/plain",
            b"wrong"
        ))
        .is_err());
    c.start_writing(&out).unwrap();
    assert!(c
        .try_add_item(Item::html("home", "Replacement", "wrong"))
        .is_err());
    assert!(c.try_add_metadata("Title", "replacement").is_err());
    // Same path in a different namespace is a distinct key.
    c.try_add_metadata("home", "metadata home").unwrap();
    c.finish_writing().unwrap();
    let arc = Archive::open(&out).unwrap();
    assert_eq!(arc.get_text("home").unwrap(), "original");
    assert_eq!(arc.metadata_str("Title").unwrap(), "original title");
    assert_eq!(
        arc.get_metadata("Illustration_48x48@1").unwrap(),
        b"original illustration"
    );
    assert_eq!(arc.metadata_str("home").unwrap(), "metadata home");
    let _ = std::fs::remove_file(out);
}

#[test]
fn infallible_builders_panic_before_adding_invalid_entries() {
    let out = tmp_path("builder-validation");
    let mut c = Creator::new();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        c.add_item(Item::html("home", "invalid\0title", "wrong"));
    }))
    .is_err());
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        c.add_metadata("bad\nname", "wrong");
    }))
    .is_err());
    c.add_item(Item::html("home", "Home", "valid"));
    c.write_to(&out).unwrap();
    let arc = Archive::open(&out).unwrap();
    assert_eq!(arc.get_text("home").unwrap(), "valid");
    assert!(!arc.has_metadata("bad"));
    let _ = std::fs::remove_file(out);
}

#[test]
fn cyclic_redirects_return_an_error_instead_of_an_invalid_listing() {
    let out = tmp_path("redirect-cycle");
    let mut c = Creator::new();
    c.add_redirection("a", "A", "b");
    c.add_redirection("b", "B", "a");
    assert!(c.write_to(&out).is_err());
    let _ = std::fs::remove_file(out);
}
