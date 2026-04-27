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
    c.write_to(&out).expect("write");

    let arc = Archive::open(&out).expect("open");
    // Expected title order using the url-fallback for empty-title
    // items: "Article 1", "Test Site", "icon.png", "style.css".
    // Auto-emitted "Counter" (M ns) and "listing/titleOrdered/v1"
    // (X ns) trail the user-content C-namespace entries — namespace
    // sorts after C bytewise. Filter them out for the equality
    // check; what matters here is the relative ordering of the
    // user items.
    let titles: Vec<String> = arc
        .iter_by_title()
        .map(|r| r.unwrap().title().to_string())
        .filter(|t| t != "Counter" && t != "listing/titleOrdered/v1")
        .collect();
    assert_eq!(
        titles,
        vec![
            "Article 1".to_string(),
            "Test Site".to_string(),
            "icon.png".to_string(),
            "style.css".to_string(),
        ],
        "title order should sort by effective title (url-fallback for empty)"
    );

    // Independent validation: real zimcheck -I must report Pass on the
    // produced ZIM. The previous bug surfaced as "Title index is not
    // properly sorted" — that's the symptom we're guarding against.
    if let Ok(out_bin) = Command::new("zimcheck")
        .args(["-I"])
        .arg(&out)
        .output()
    {
        let stdout = String::from_utf8_lossy(&out_bin.stdout);
        assert!(
            stdout.contains("Status: Pass"),
            "real zimcheck -I should Pass on the empty-title round-trip:\n{stdout}"
        );
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
