#![cfg(feature = "writer")]
//! End-to-end test for the `zimwriterfs` CLI:
//!   1. Build a tiny HTML site under a temp dir.
//!   2. Run our `zimwriterfs` binary on it.
//!   3. Re-open the produced ZIM with `zimru::Archive` and verify content.
//!   4. (If upstream is installed) run upstream `zimcheck -A` and confirm
//!      the file passes.
//!
//! Cargo supplies the binary under test; optional installed tools are used only
//! for additional interoperability checks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zimru::Archive;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_zimwriterfs"))
}

fn upstream_zimcheck() -> Option<PathBuf> {
    let p = PathBuf::from("/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.6.0/zimcheck");
    p.exists().then_some(p)
}

fn upstream_zimwriterfs() -> Option<PathBuf> {
    let p = PathBuf::from("/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.6.0/zimwriterfs");
    p.exists().then_some(p)
}

fn minimal_png() -> Vec<u8> {
    let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&48u32.to_be_bytes());
    v.extend_from_slice(&48u32.to_be_bytes());
    v.extend_from_slice(&[8, 0, 0, 0, 0]);
    v.extend_from_slice(&[0, 0, 0, 0]);
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(b"IDAT");
    v.extend_from_slice(&[0, 0, 0, 0]);
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(b"IEND");
    v.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
    v
}

/// Build a minimal HTML site:
///   /
///     index.html        (main page, links to about.html and styles.css)
///     about.html
///     styles.css
///     icon48.png        (illustration)
///     images/photo.jpg  (placeholder bytes)
fn build_site(root: &Path) {
    fs::create_dir_all(root.join("images")).unwrap();
    fs::write(
        root.join("index.html"),
        b"<!DOCTYPE html><html><head><title>Welcome</title><link rel=stylesheet href=\"styles.css\"></head><body><h1>Hello</h1><a href=\"about.html\">About</a></body></html>",
    ).unwrap();
    fs::write(
        root.join("about.html"),
        b"<!DOCTYPE html><html><head><title>About</title></head><body><p>About this archive.</p></body></html>",
    ).unwrap();
    fs::write(
        root.join("styles.css"),
        b"body { font-family: sans-serif; }",
    )
    .unwrap();
    fs::write(root.join("icon48.png"), minimal_png()).unwrap();
    fs::write(
        root.join("images/photo.jpg"),
        b"\xff\xd8\xff\xe0FAKE-JPEG-DATA",
    )
    .unwrap();
}

fn tmp_dir(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "zimru-zwfs-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn zimwriterfs_packs_a_directory_into_a_valid_zim() {
    let bin = binary();
    if !bin.exists() {
        eprintln!(
            "skip: {} not built (run `cargo build --release`)",
            bin.display()
        );
        return;
    }
    let site = tmp_dir("site");
    build_site(&site);
    let zim = site.parent().unwrap().join(format!(
        "{}.zim",
        site.file_name().unwrap().to_string_lossy()
    ));

    let status = Command::new(&bin)
        .args([
            "--welcome=index.html",
            "--illustration=icon48.png",
            "--language=eng",
            "--name=zimru-zwfs-test",
            "--title=zimru zimwriterfs e2e test",
            "--description=Round-trip integration test",
            "--creator=zimru-tests",
            "--publisher=zimru",
        ])
        .arg(&site)
        .arg(&zim)
        .status()
        .expect("run zimwriterfs");
    assert!(status.success(), "zimwriterfs exited with {status:?}");

    let a = Archive::open(&zim).expect("reopen produced ZIM");
    assert!(a.has_main_entry(), "no main entry");
    assert_eq!(a.main_path().unwrap(), "index.html", "main path != welcome");

    // Every file we put in should be retrievable.
    let want = [
        ("index.html", "Hello"),
        ("about.html", "About this archive"),
        ("styles.css", "font-family"),
    ];
    for (path, needle) in want {
        let body = a.get_text(path).expect(path);
        assert!(
            body.contains(needle),
            "{path} missing `{needle}` (got: {body})"
        );
    }
    // Non-text file bytes survive.
    assert_eq!(
        a.get_bytes("images/photo.jpg").unwrap(),
        b"\xff\xd8\xff\xe0FAKE-JPEG-DATA",
    );

    // Mandatory metadata round-trips.
    assert_eq!(
        a.metadata_str("Title").unwrap(),
        "zimru zimwriterfs e2e test"
    );
    assert_eq!(a.metadata_str("Language").unwrap(), "eng");
    assert_eq!(a.metadata_str("Creator").unwrap(), "zimru-tests");
    assert!(a.has_metadata("Illustration_48x48@1"));

    // Our checksum and (if available) upstream zimcheck both pass.
    assert!(a.check().unwrap(), "trailing MD5 mismatch");
    if let Some(zc) = upstream_zimcheck() {
        let out = Command::new(&zc)
            .arg("-A")
            .arg(&zim)
            .output()
            .expect("zimcheck");
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(
            s.contains("Overall Test Status: Pass"),
            "upstream zimcheck rejected the file:\n{s}"
        );
    }

    // Cleanup.
    fs::remove_dir_all(&site).ok();
    fs::remove_file(&zim).ok();
}

#[test]
fn zimwriterfs_redirects_file_is_honoured() {
    let bin = binary();
    if !bin.exists() {
        return;
    }
    let site = tmp_dir("redir");
    build_site(&site);
    let redir = site.join("redirects.tsv");
    fs::write(&redir, "start\tStart\tindex.html\nhome\tHome\tindex.html\n").unwrap();
    let zim = site.parent().unwrap().join(format!(
        "{}.zim",
        site.file_name().unwrap().to_string_lossy()
    ));
    let status = Command::new(&bin)
        .args([
            "--welcome=index.html",
            "--illustration=icon48.png",
            "--language=eng",
            "--name=zimru-zwfs-redir",
            "--title=Redirect test",
            "--description=Test",
            "--creator=zimru",
            "--publisher=zimru",
        ])
        .arg("--redirects")
        .arg(&redir)
        .arg(&site)
        .arg(&zim)
        .status()
        .expect("run");
    assert!(status.success());

    let a = Archive::open(&zim).unwrap();
    let start = a.get_entry_by_path("start").unwrap();
    assert!(start.is_redirect());
    assert_eq!(start.resolve().unwrap().path(), "index.html");
    let home = a.get_entry_by_path("home").unwrap();
    assert!(home.is_redirect());
    assert_eq!(home.resolve().unwrap().path(), "index.html");

    fs::remove_dir_all(&site).ok();
    fs::remove_file(&zim).ok();
}

#[test]
fn zimwriterfs_matches_upstream_writerfs_shape() {
    // Side-by-side check: build the same site twice, once via upstream
    // zimwriterfs and once via ours. Both files should:
    //   * Open in zimru.
    //   * Pass `upstream zimcheck -A`.
    //   * Contain the same set of paths in the C namespace.
    let bin = binary();
    let Some(up_bin) = upstream_zimwriterfs() else {
        eprintln!("skip: upstream zimwriterfs not installed");
        return;
    };
    if !bin.exists() {
        return;
    }

    let site = tmp_dir("compare");
    build_site(&site);

    let our_zim = site.parent().unwrap().join("our.zim");
    let up_zim = site.parent().unwrap().join("up.zim");

    let common_args = [
        "--welcome=index.html",
        "--illustration=icon48.png",
        "--language=eng",
        "--name=zimru-compare",
        "--title=compare",
        "--description=compare",
        "--creator=zimru",
        "--publisher=zimru",
        "--withoutFTIndex",
        "--skip-libmagic-check", // upstream needs this when no system magic file is installed
    ];

    // Upstream errors out if the destination already exists.
    let _ = fs::remove_file(&our_zim);
    let _ = fs::remove_file(&up_zim);

    let our_status = Command::new(&bin)
        .args(common_args)
        .arg(&site)
        .arg(&our_zim)
        .status()
        .unwrap();
    assert!(
        our_status.success(),
        "zimru zimwriterfs failed: {our_status:?}"
    );
    let up_status = Command::new(&up_bin)
        .args(common_args)
        .arg(&site)
        .arg(&up_zim)
        .status()
        .unwrap();
    assert!(
        up_status.success(),
        "upstream zimwriterfs failed: {up_status:?}"
    );

    // Both should be valid per upstream zimcheck.
    if let Some(zc) = upstream_zimcheck() {
        for f in [&our_zim, &up_zim] {
            let out = Command::new(&zc).arg("-A").arg(f).output().unwrap();
            let s = String::from_utf8_lossy(&out.stdout);
            assert!(s.contains("Overall Test Status: Pass"), "{f:?}:\n{s}");
        }
    }

    // Same set of C-namespace paths in both archives.
    let our = Archive::open(&our_zim).unwrap();
    let up = Archive::open(&up_zim).unwrap();
    let mut our_paths: Vec<String> = our
        .content_entries()
        .filter_map(|e| e.ok().map(|e| e.path().to_string()))
        .collect();
    let mut up_paths: Vec<String> = up
        .content_entries()
        .filter_map(|e| e.ok().map(|e| e.path().to_string()))
        .collect();
    our_paths.sort();
    up_paths.sort();
    assert_eq!(
        our_paths, up_paths,
        "C-namespace paths differ between zimru and upstream zimwriterfs output"
    );

    fs::remove_dir_all(&site).ok();
    fs::remove_file(&our_zim).ok();
    fs::remove_file(&up_zim).ok();
}

fn writer_command() -> Command {
    let mut cmd = Command::new(binary());
    cmd.args([
        "--welcome=index.html",
        "--illustration=icon48.png",
        "--language=eng",
        "--name=filesystem-regressions",
        "--title=Filesystem regressions",
        "--description=Original filesystem regression fixtures",
        "--creator=zimru-tests",
        "--publisher=zimru",
    ]);
    cmd
}

#[test]
fn output_inside_source_is_rejected_before_creation_or_truncation() {
    let root = tmp_dir("output-inside");
    let site = root.join("site");
    build_site(&site);
    let original = fs::read(site.join("index.html")).unwrap();
    for output in [site.join("archive.zim"), site.join("index.html")] {
        let result = writer_command()
            .arg("--without-indexes")
            .arg(&site)
            .arg(output)
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "output inside source was accepted"
        );
        assert_eq!(fs::read(site.join("index.html")).unwrap(), original);
        assert!(!site.join("archive.zim").exists());
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn output_symlink_aliases_cannot_bypass_source_protection() {
    use std::os::unix::fs::symlink;
    let root = tmp_dir("output-alias");
    let site = root.join("site");
    build_site(&site);
    let original = fs::read(site.join("index.html")).unwrap();
    let alias = root.join("source-alias");
    symlink(&site, &alias).unwrap();
    let output_alias = root.join("output.zim");
    symlink(site.join("index.html"), &output_alias).unwrap();
    for (input, output) in [
        (&site, alias.join("archive.zim")),
        (&alias, site.join("index.html")),
        (&site, output_alias.clone()),
    ] {
        let result = writer_command()
            .arg("--without-indexes")
            .arg(input)
            .arg(output)
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "aliased source output was accepted"
        );
        assert_eq!(fs::read(site.join("index.html")).unwrap(), original);
        assert!(!site.join("archive.zim").exists());
    }
    assert!(fs::symlink_metadata(output_alias).unwrap().is_symlink());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replacing_hardlinked_output_preserves_source_bytes() {
    let root = tmp_dir("output-hardlink");
    let site = root.join("site");
    build_site(&site);
    let original = fs::read(site.join("index.html")).unwrap();
    let output = root.join("archive.zim");
    fs::hard_link(site.join("index.html"), &output).unwrap();
    let result = writer_command()
        .arg("--without-indexes")
        .arg(&site)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(site.join("index.html")).unwrap(), original);
    let archive = Archive::open(&output).unwrap();
    assert_eq!(archive.get_bytes("index.html").unwrap(), original);
    assert!(archive.get_entry_by_path("archive.zim").is_err());
    drop(archive);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_chains_must_end_at_an_ingested_file() {
    use std::os::unix::fs::symlink;
    let root = tmp_dir("symlink-graph");
    let site = root.join("site");
    build_site(&site);
    fs::write(site.join(".hidden.html"), b"not ingested").unwrap();
    fs::write(root.join("outside.html"), b"outside").unwrap();
    for (link, target) in [
        ("a", "b"),
        ("b", "missing"),
        ("cycle-a", "cycle-b"),
        ("cycle-b", "cycle-a"),
        ("cycle-parent", "cycle-a"),
        ("self", "self"),
        ("hidden", ".hidden.html"),
        ("hidden-parent", "hidden"),
        ("outside", "../outside.html"),
        ("outside-parent", "outside"),
        ("valid-a", "valid-b"),
        ("valid-b", "index.html"),
    ] {
        symlink(target, site.join(link)).unwrap();
    }
    let output = root.join("archive.zim");
    let result = writer_command()
        .arg("--without-indexes")
        .arg(&site)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let archive = Archive::open(&output).unwrap();
    for missing in [
        "a",
        "b",
        "cycle-a",
        "cycle-b",
        "cycle-parent",
        "self",
        "hidden",
        "hidden-parent",
        "outside",
        "outside-parent",
    ] {
        assert!(archive.get_entry_by_path(missing).is_err(), "{missing}");
    }
    for valid in ["valid-a", "valid-b"] {
        let entry = archive.get_entry_by_path(valid).unwrap();
        assert!(entry.is_redirect());
        assert_eq!(entry.resolve().unwrap().path(), "index.html");
        assert_eq!(
            archive.get_bytes(valid).unwrap(),
            fs::read(site.join("index.html")).unwrap()
        );
    }
    assert!(archive.check().unwrap());
    drop(archive);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn large_extensionless_html_keeps_title_and_fulltext_body() {
    use std::os::unix::fs::PermissionsExt;
    let root = tmp_dir("large-sniffed-html");
    let site = root.join("site");
    build_site(&site);
    let mut html = String::from(
        "<!doctype html><html><head><title>Large sniffed article</title></head><body>",
    );
    html.push_str(&"x".repeat(4 * 1024 * 1024));
    html.push_str("END-OF-LARGE-ARTICLE</body></html>");
    fs::write(site.join("article"), &html).unwrap();
    let mut media = vec![0x55; 4 * 1024 * 1024];
    media[..3].copy_from_slice(b"\xff\xd8\xff");
    fs::write(site.join("media"), &media).unwrap();
    let helper = root.join("capture-helper");
    fs::write(
        &helper,
        "#!/bin/sh\ncat > \"$CAPTURE_DIR/$1.jsonl\"\nprintf 'captured index' > \"$5\"\n",
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    let output = root.join("archive.zim");
    let result = writer_command()
        .arg("--xapianbuilder-path")
        .arg(&helper)
        .env("CAPTURE_DIR", &root)
        .arg(&site)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let archive = Archive::open(&output).unwrap();
    assert_eq!(
        archive.get_entry_by_path("article").unwrap().title(),
        "Large sniffed article",
    );
    assert_eq!(archive.get_text("article").unwrap(), html);
    assert_eq!(archive.get_bytes("media").unwrap(), media);
    let title_docs = fs::read_to_string(root.join("title.jsonl")).unwrap();
    assert!(title_docs
        .lines()
        .any(|line| line == r#"{"path":"article","title":"Large sniffed article"}"#));
    let fulltext_docs = fs::read_to_string(root.join("fulltext.jsonl")).unwrap();
    let article_doc = fulltext_docs
        .lines()
        .find(|line| line.starts_with(r#"{"path":"article","#))
        .expect("sniffed HTML must be sent to the fulltext helper");
    assert_eq!(
        article_doc,
        format!(
            r#"{{"path":"article","title":"Large sniffed article","mimetype":"text/html","language":"eng","body":"{html}"}}"#
        ),
    );
    assert!(!fulltext_docs.contains(r#""path":"media""#));
    drop(archive);
    fs::remove_dir_all(root).unwrap();
}
