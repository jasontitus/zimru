//! End-to-end test for the `zimwriterfs`/`zimrecreate` ↔ xapianbuilder
//! integration. Skipped (with a notice) when xapianbuilder isn't on
//! PATH so this test doesn't fail on minimal CI without the helper.

use std::path::PathBuf;
use std::process::Command;

fn xapianbuilder_on_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("XAPIANBUILDER") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("xapianbuilder");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn zimwriterfs_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_zimwriterfs"))
}

fn zimdump_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_zimdump"))
}

fn workdir(label: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("zimru-xb-{}-{}", label, std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write_minimal_corpus(dir: &PathBuf) {
    std::fs::write(
        dir.join("index.html"),
        b"<!DOCTYPE html><html><head><title>Welcome</title></head><body><h1>Welcome</h1></body></html>",
    ).unwrap();
    std::fs::write(
        dir.join("Mollusca.html"),
        b"<!DOCTYPE html><html><head><title>Mollusca</title></head><body><h1>Mollusca</h1><p>Molluscs are invertebrate animals.</p></body></html>",
    ).unwrap();
    // Minimal valid 1x1 PNG.
    let png: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a,
        0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R',
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
        0x00, 0x00, 0x00, 0x0d, b'I', b'D', b'A', b'T',
        0x78, 0x9c, 0x63, 0xfc, 0xff, 0xff, 0x3f, 0x03,
        0x00, 0x06, 0x00, 0x02, 0xfe, 0xa6, 0x88, 0x73, 0x88,
        0x00, 0x00, 0x00, 0x00, b'I', b'E', b'N', b'D',
        0xae, 0x42, 0x60, 0x82,
    ];
    std::fs::write(dir.join("icon48.png"), png).unwrap();
}

fn build_zim(html_dir: &PathBuf, zim_path: &PathBuf, xb: Option<&PathBuf>) {
    let mut cmd = Command::new(zimwriterfs_bin());
    cmd.args([
        "--welcome=index.html",
        "--illustration=icon48.png",
        "--language=eng",
        "--name=demo",
        "--title=Demo",
        "--description=demo",
        "--creator=test",
        "--publisher=test",
    ]);
    if let Some(xb) = xb {
        cmd.arg("--xapianbuilder-path").arg(xb);
    }
    cmd.arg(html_dir).arg(zim_path);
    let out = cmd.output().expect("zimwriterfs spawn");
    if !out.status.success() {
        panic!(
            "zimwriterfs failed: {}\nstdout: {}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}

fn list_x_namespace(zim: &PathBuf) -> Vec<String> {
    let out = Command::new(zimdump_bin())
        .args(["list", "--ns=X"])
        .arg(zim)
        .output()
        .expect("zimdump spawn");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn builds_indexes_when_xapianbuilder_available() {
    let Some(xb) = xapianbuilder_on_path() else {
        eprintln!("skip: xapianbuilder not on PATH");
        return;
    };
    let dir = workdir("with_xb");
    let html_dir = dir.join("html");
    std::fs::create_dir_all(&html_dir).unwrap();
    write_minimal_corpus(&html_dir);
    let zim_path = dir.join("out.zim");

    build_zim(&html_dir, &zim_path, Some(&xb));

    let entries = list_x_namespace(&zim_path);
    assert!(entries.contains(&"fulltext/xapian".to_string()),
            "missing X/fulltext/xapian; got {entries:?}");
    assert!(entries.contains(&"title/xapian".to_string()),
            "missing X/title/xapian; got {entries:?}");
}

#[test]
fn builds_zim_without_indexes_when_helper_missing() {
    let dir = workdir("no_xb");
    let html_dir = dir.join("html");
    std::fs::create_dir_all(&html_dir).unwrap();
    write_minimal_corpus(&html_dir);
    let zim_path = dir.join("out.zim");

    // Point at a path that definitely doesn't exist; the writer
    // should warn and proceed.
    let bogus = PathBuf::from("/nonexistent/xapianbuilder");
    build_zim(&html_dir, &zim_path, Some(&bogus));

    let entries = list_x_namespace(&zim_path);
    // Listing index is auto-emitted by the writer, so it's fine if
    // X/listing/* is present — just assert the fulltext/title pair
    // isn't.
    assert!(!entries.contains(&"fulltext/xapian".to_string()),
            "fulltext/xapian unexpectedly present: {entries:?}");
    assert!(!entries.contains(&"title/xapian".to_string()),
            "title/xapian unexpectedly present: {entries:?}");
}

#[test]
fn skips_indexing_with_without_ft_index_flag() {
    let Some(xb) = xapianbuilder_on_path() else {
        eprintln!("skip: xapianbuilder not on PATH");
        return;
    };
    let dir = workdir("without_ft");
    let html_dir = dir.join("html");
    std::fs::create_dir_all(&html_dir).unwrap();
    write_minimal_corpus(&html_dir);
    let zim_path = dir.join("out.zim");

    let out = Command::new(zimwriterfs_bin())
        .args([
            "--welcome=index.html", "--illustration=icon48.png",
            "--language=eng", "--name=demo", "--title=Demo",
            "--description=demo", "--creator=test", "--publisher=test",
            "-j", // --withoutFTIndex
        ])
        .arg("--xapianbuilder-path").arg(&xb)
        .arg(&html_dir).arg(&zim_path)
        .output()
        .expect("zimwriterfs spawn");
    assert!(out.status.success());

    let entries = list_x_namespace(&zim_path);
    assert!(!entries.contains(&"fulltext/xapian".to_string()));
    assert!(!entries.contains(&"title/xapian".to_string()));
}
