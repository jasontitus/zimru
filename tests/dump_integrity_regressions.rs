//! Original fixtures built directly from the public ZIM format layout.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use md5::{Digest, Md5};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "zimru-dump-integrity-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    bytes: Vec<u8>,
    dirents: Vec<usize>,
    url_table: usize,
    title_table: usize,
    cluster_table: usize,
    cluster: usize,
}

impl Fixture {
    fn new(articles: &[&str], redirects: &[(&str, &str)]) -> Self {
        let mut entries: Vec<(&str, Option<&str>)> = articles.iter().map(|p| (*p, None)).collect();
        entries.extend(redirects.iter().map(|(p, target)| (*p, Some(*target))));
        entries.sort_by_key(|(path, _)| *path);
        let mut bytes = vec![0u8; 80];
        bytes.extend_from_slice(b"text/html\0\0");
        let url_table = bytes.len();
        bytes.resize(bytes.len() + entries.len() * 8, 0);
        let title_table = bytes.len();
        for index in 0..entries.len() {
            bytes.extend_from_slice(&(index as u32).to_le_bytes());
        }
        let cluster_table = bytes.len();
        bytes.extend_from_slice(&[0; 8]);
        let mut dirents = Vec::new();
        let mut blobs = Vec::new();
        for (index, (path, target)) in entries.iter().enumerate() {
            dirents.push(bytes.len());
            let pointer = (bytes.len() as u64).to_le_bytes();
            bytes[url_table + index * 8..url_table + index * 8 + 8].copy_from_slice(&pointer);
            let mime = if target.is_some() { u16::MAX } else { 0u16 };
            bytes.extend_from_slice(&mime.to_le_bytes());
            bytes.extend_from_slice(&[0, b'C']);
            bytes.extend_from_slice(&0u32.to_le_bytes());
            if let Some(target) = target {
                let target = entries.iter().position(|(path, _)| path == target).unwrap() as u32;
                bytes.extend_from_slice(&target.to_le_bytes());
            } else {
                bytes.extend_from_slice(&0u32.to_le_bytes());
                bytes.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
                blobs.push(format!("payload:{path}").into_bytes());
            }
            bytes.extend_from_slice(path.as_bytes());
            bytes.extend_from_slice(&[0, 0]); // empty title falls back to URL
        }
        let cluster = bytes.len();
        bytes[cluster_table..cluster_table + 8].copy_from_slice(&(cluster as u64).to_le_bytes());
        bytes.push(1); // uncompressed, 32-bit offsets
        let mut offset = ((blobs.len() + 1) * 4) as u32;
        for blob in &blobs {
            bytes.extend_from_slice(&offset.to_le_bytes());
            offset += blob.len() as u32;
        }
        bytes.extend_from_slice(&offset.to_le_bytes());
        for blob in blobs {
            bytes.extend_from_slice(&blob);
        }
        let checksum = bytes.len();
        bytes[0..4].copy_from_slice(&0x044d495au32.to_le_bytes());
        bytes[4..6].copy_from_slice(&6u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&1u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        bytes[28..32].copy_from_slice(&1u32.to_le_bytes());
        bytes[32..40].copy_from_slice(&(url_table as u64).to_le_bytes());
        bytes[40..48].copy_from_slice(&(title_table as u64).to_le_bytes());
        bytes[48..56].copy_from_slice(&(cluster_table as u64).to_le_bytes());
        bytes[56..64].copy_from_slice(&80u64.to_le_bytes());
        bytes[64..72].fill(0xff);
        bytes[72..80].copy_from_slice(&(checksum as u64).to_le_bytes());
        bytes.resize(checksum + 16, 0);
        Self {
            bytes,
            dirents,
            url_table,
            title_table,
            cluster_table,
            cluster,
        }
    }

    fn save(&mut self, path: &Path) {
        let checksum = self.bytes.len() - 16;
        let digest = Md5::digest(&self.bytes[..checksum]);
        self.bytes[checksum..].copy_from_slice(&digest);
        fs::write(path, &self.bytes).unwrap();
    }

    fn set_u32(&mut self, offset: usize, value: u32) {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

fn check(path: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zimcheck"))
        .args(flags)
        .arg(path)
        .output()
        .unwrap()
}

fn dump(path: &Path, root: &Path, redirect: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zimdump"));
    command
        .arg("dump")
        .arg(format!("--dir={}", root.display()))
        .arg("--ns=C");
    if redirect {
        command.arg("--redirect");
    }
    command.arg(path).output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn extraction_never_follows_existing_parent_final_or_log_links() {
    use std::os::unix::fs::symlink;
    let work = Workspace::new();
    let root = work.path("root");
    let outside = work.path("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"unchanged").unwrap();
    symlink(&outside, root.join("parent")).unwrap();
    symlink(outside.join("sentinel"), root.join("final")).unwrap();
    symlink(outside.join("sentinel"), root.join("dump_errors.log")).unwrap();
    fs::hard_link(outside.join("sentinel"), root.join("hardlink")).unwrap();
    let archive = work.path("input.zim");
    Fixture::new(&["parent/page", "final", "hardlink"], &[]).save(&archive);
    let output = dump(&archive, &root, false);
    assert_success(&output);
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"unchanged");
    assert!(!outside.join("page").exists());
    assert_eq!(fs::read(root.join("final")).unwrap(), b"payload:final");
    assert_eq!(
        fs::read(root.join("hardlink")).unwrap(),
        b"payload:hardlink"
    );
    assert_eq!(
        fs::read(root.join("_exceptions/parent%2fpage")).unwrap(),
        b"payload:parent/page"
    );
    assert!(!fs::symlink_metadata(root.join("dump_errors.log"))
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn exception_directory_symlink_fails_without_writing_outside_root() {
    use std::os::unix::fs::symlink;
    let work = Workspace::new();
    let root = work.path("root");
    let outside = work.path("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, root.join("_exceptions")).unwrap();
    let archive = work.path("input.zim");
    Fixture::new(&["block", "block/page"], &[]).save(&archive);
    let output = dump(&archive, &root, false);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert!(fs::read_to_string(root.join("dump_errors.log"))
        .unwrap()
        .contains("block/page"));
}

#[cfg(unix)]
#[test]
fn exception_final_symlink_is_replaced_not_followed() {
    use std::os::unix::fs::symlink;
    let work = Workspace::new();
    let root = work.path("root");
    fs::create_dir_all(root.join("_exceptions")).unwrap();
    let sentinel = work.path("sentinel");
    fs::write(&sentinel, b"unchanged").unwrap();
    symlink(&sentinel, root.join("_exceptions/block%2fpage")).unwrap();
    let archive = work.path("input.zim");
    Fixture::new(&["block", "block/page"], &[]).save(&archive);
    assert_success(&dump(&archive, &root, false));
    assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
    assert_eq!(
        fs::read(root.join("_exceptions/block%2fpage")).unwrap(),
        b"payload:block/page"
    );
}

#[cfg(unix)]
#[test]
fn html_redirects_are_relative_to_nested_and_exception_destinations_and_escaped() {
    let work = Workspace::new();
    let archive = work.path("input.zim");
    let root = work.path("root");
    let target = "dest/quo\"te<&'?#%.html";
    Fixture::new(
        &["block", target],
        &[("nested/deep/alias", target), ("block/deep/alias", target)],
    )
    .save(&archive);
    assert_success(&dump(&archive, &root, false));
    for (file, prefix) in [
        ("nested/deep/alias", "../../"),
        ("_exceptions/block%2fdeep%2falias", "../"),
    ] {
        let html = fs::read_to_string(root.join(file)).unwrap();
        let url = format!("{prefix}dest/quo%22te%3C%26%27%3F%23%25.html");
        assert!(html.contains(&format!("href=\"{url}\"")), "{html}");
        assert!(
            html.contains(&format!("content=\"0; url={url}\"")),
            "{html}"
        );
        assert!(
            html.contains("quo&quot;te&lt;&amp;&#39;?#%.html</a>"),
            "{html}"
        );
    }
}

#[cfg(unix)]
#[test]
fn symbolic_redirects_keep_single_step_targets_and_fallback_depth() {
    let work = Workspace::new();
    let archive = work.path("input.zim");
    let root = work.path("root");
    Fixture::new(
        &["block", "block/page", "target"],
        &[
            ("alias", "nested/alias"),
            ("nested/alias", "target"),
            ("block/deep/alias", "target"),
            ("relocated", "block/page"),
        ],
    )
    .save(&archive);
    assert_success(&dump(&archive, &root, true));
    assert_eq!(
        fs::read_link(root.join("alias")).unwrap(),
        PathBuf::from("./nested/alias")
    );
    assert_eq!(
        fs::read_link(root.join("nested/alias")).unwrap(),
        PathBuf::from("../target")
    );
    assert_eq!(
        fs::read_link(root.join("_exceptions/block%2fdeep%2falias")).unwrap(),
        PathBuf::from("../target")
    );
    assert_eq!(fs::read(root.join("alias")).unwrap(), b"payload:target");
    assert_eq!(
        fs::read(root.join("relocated")).unwrap(),
        b"payload:block/page"
    );
}

#[cfg(not(unix))]
#[test]
fn extraction_is_explicitly_unsupported_without_confined_filesystem_operations() {
    let work = Workspace::new();
    let archive = work.path("input.zim");
    let root = work.path("root");
    Fixture::new(&["target"], &[("alias", "target")]).save(&archive);
    let output = dump(&archive, &root, true);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not supported"));
    assert!(!root.exists());
}

#[test]
fn integrity_rejects_bad_references_and_order_with_valid_checksums() {
    let work = Workspace::new();
    let archive = work.path("input.zim");
    for mutation in [
        "cluster",
        "blob",
        "mime",
        "redirect",
        "main",
        "layout",
        "title-range",
        "title-duplicate",
        "title-order",
        "url-order",
        "cluster-pointer",
    ] {
        let mut fixture = Fixture::new(&["a", "b"], &[("c", "a")]);
        match mutation {
            "cluster" => fixture.set_u32(fixture.dirents[0] + 8, 1),
            "blob" => fixture.set_u32(fixture.dirents[0] + 12, 2),
            "mime" => fixture.bytes[fixture.dirents[0]..fixture.dirents[0] + 2]
                .copy_from_slice(&1u16.to_le_bytes()),
            "redirect" => fixture.set_u32(fixture.dirents[2] + 8, 3),
            "main" => fixture.set_u32(64, 3),
            "layout" => fixture.set_u32(68, 3),
            "title-range" => fixture.set_u32(fixture.title_table, 3),
            "title-duplicate" => fixture.set_u32(fixture.title_table + 4, 0),
            "title-order" => {
                fixture.set_u32(fixture.title_table, 1);
                fixture.set_u32(fixture.title_table + 4, 0);
            }
            "url-order" => {
                fixture.bytes[fixture.url_table..fixture.url_table + 8]
                    .copy_from_slice(&(fixture.dirents[1] as u64).to_le_bytes());
                fixture.bytes[fixture.url_table + 8..fixture.url_table + 16]
                    .copy_from_slice(&(fixture.dirents[0] as u64).to_le_bytes());
            }
            "cluster-pointer" => fixture.bytes[fixture.cluster_table..fixture.cluster_table + 8]
                .copy_from_slice(&1u64.to_le_bytes()),
            _ => unreachable!(),
        }
        fixture.save(&archive);
        assert_success(&check(&archive, &["-C"]));
        let output = check(&archive, &["-I", "-J"]);
        assert!(!output.status.success(), "accepted {mutation}");
        let json = String::from_utf8_lossy(&output.stdout);
        assert!(json.contains("\"integrity\""), "{mutation}: {json}");
        assert!(json.contains("\"ERROR\""), "{mutation}: {json}");
    }
}

#[test]
fn integrity_parses_later_dirents_that_sampling_would_miss() {
    let work = Workspace::new();
    let archive = work.path("input.zim");
    let paths: Vec<String> = (0..1024).map(|i| format!("entry{i:04}")).collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    let mut fixture = Fixture::new(&paths, &[]);
    fixture.bytes[fixture.dirents[1023] + 16] = 0xff;
    fixture.save(&archive);
    assert_success(&check(&archive, &["-C"]));
    let output = check(&archive, &["-I"]);
    assert!(!output.status.success(), "accepted malformed final dirent");
}

#[test]
fn content_checks_report_cluster_and_blob_read_failures_without_integrity_enabled() {
    let work = Workspace::new();
    let archive = work.path("input.zim");
    for failure in ["missing-cluster", "missing-blob", "undecodable-cluster"] {
        let mut fixture = Fixture::new(&["article"], &[]);
        match failure {
            "missing-cluster" => fixture.set_u32(fixture.dirents[0] + 8, 1),
            "missing-blob" => fixture.set_u32(fixture.dirents[0] + 12, 1),
            "undecodable-cluster" => fixture.bytes[fixture.cluster] = 5,
            _ => unreachable!(),
        }
        fixture.save(&archive);
        for flags in [&["-0", "-J"][..], &["-A"][..]] {
            let output = check(&archive, flags);
            assert!(!output.status.success(), "accepted {failure}");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains("Cannot read article article:"),
                "{failure}: {stdout}"
            );
        }
    }
}

#[test]
fn valid_fixture_passes_full_structural_integrity() {
    let work = Workspace::new();
    let archive = work.path("input.zim");
    Fixture::new(&["article", "nested/page"], &[("alias", "article")]).save(&archive);
    assert_success(&check(&archive, &["-I"]));
}

#[test]
fn integrity_accepts_long_acyclic_redirect_chains_but_rejects_cycles() {
    let work = Workspace::new();
    let archive = work.path("input.zim");
    let names: Vec<String> = (0..40).map(|i| format!("r{i:02}")).collect();
    let redirects: Vec<(&str, &str)> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            (
                name.as_str(),
                names.get(i + 1).map(String::as_str).unwrap_or("target"),
            )
        })
        .collect();
    let mut fixture = Fixture::new(&["target"], &redirects);
    fixture.save(&archive);
    assert_success(&check(&archive, &["-I"]));
    fixture.set_u32(fixture.dirents[39] + 8, 0);
    fixture.save(&archive);
    assert!(!check(&archive, &["-I"]).status.success());
}
