#![cfg(feature = "writer")]
//! Original public-format fixtures exercising CLI file safety and legacy URLs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use md5::{Digest, Md5};
use zimru::Archive;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "zimru-cli-preservation-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

enum Content {
    Blob(Vec<u8>),
    Redirect(u8, String),
}

struct Record {
    namespace: u8,
    path: String,
    mime: u16,
    content: Content,
}

fn article(namespace: u8, path: &str, mime: u16, body: &[u8]) -> Record {
    Record {
        namespace,
        path: path.into(),
        mime,
        content: Content::Blob(body.to_vec()),
    }
}

fn redirect(namespace: u8, path: &str, target_namespace: u8, target: &str) -> Record {
    Record {
        namespace,
        path: path.into(),
        mime: 0,
        content: Content::Redirect(target_namespace, target.into()),
    }
}

/// Public ZIM format: tables and dirents precede raw clusters. Cluster IDs
/// deliberately run backwards relative to physical positions, which is legal.
fn fixture(
    major: u16,
    minor: u16,
    mut records: Vec<Record>,
    main: Option<(u8, &str)>,
) -> (Vec<u8>, Vec<u64>) {
    records.sort_by(|a, b| (a.namespace, &a.path).cmp(&(b.namespace, &b.path)));
    let mimes = b"text/html\0text/css\0application/octet-stream\0text/plain\0\0";
    let mut dirents = Vec::new();
    let mut clusters = Vec::new();
    for record in &records {
        let mut dirent = Vec::new();
        let mime = if matches!(&record.content, Content::Redirect(..)) {
            u16::MAX
        } else {
            record.mime
        };
        dirent.extend_from_slice(&mime.to_le_bytes());
        dirent.extend_from_slice(&[0, record.namespace]);
        dirent.extend_from_slice(&0u32.to_le_bytes());
        match &record.content {
            Content::Blob(body) => {
                dirent.extend_from_slice(&(clusters.len() as u32).to_le_bytes());
                dirent.extend_from_slice(&0u32.to_le_bytes());
                let mut cluster = vec![1];
                cluster.extend_from_slice(&8u32.to_le_bytes());
                cluster.extend_from_slice(&(8 + body.len() as u32).to_le_bytes());
                cluster.extend_from_slice(body);
                clusters.push(cluster);
            }
            Content::Redirect(namespace, path) => {
                let index = records
                    .iter()
                    .position(|r| r.namespace == *namespace && r.path == *path)
                    .unwrap() as u32;
                dirent.extend_from_slice(&index.to_le_bytes());
            }
        }
        dirent.extend_from_slice(record.path.as_bytes());
        dirent.push(0);
        // Explicit title equals the original path, including for legacy entries.
        dirent.extend_from_slice(record.path.as_bytes());
        dirent.push(0);
        dirents.push(dirent);
    }
    let url_pos = 80 + mimes.len() as u64;
    let title_pos = url_pos + 8 * records.len() as u64;
    let cluster_pos = title_pos + 4 * records.len() as u64;
    let mut cursor = cluster_pos + 8 * clusters.len() as u64;
    let mut dirent_offsets = Vec::new();
    for dirent in &dirents {
        dirent_offsets.push(cursor);
        cursor += dirent.len() as u64;
    }
    let mut cluster_offsets = vec![0u64; clusters.len()];
    for (index, cluster) in clusters.iter().enumerate().rev() {
        cluster_offsets[index] = cursor;
        cursor += cluster.len() as u64;
    }
    let main_index = main
        .map(|(namespace, path)| {
            records
                .iter()
                .position(|r| r.namespace == namespace && r.path == path)
                .unwrap() as u32
        })
        .unwrap_or(u32::MAX);
    let mut out = Vec::new();
    out.extend_from_slice(&0x044D495Au32.to_le_bytes());
    out.extend_from_slice(&major.to_le_bytes());
    out.extend_from_slice(&minor.to_le_bytes());
    out.extend_from_slice(&[0x37; 16]);
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    out.extend_from_slice(&(clusters.len() as u32).to_le_bytes());
    out.extend_from_slice(&url_pos.to_le_bytes());
    out.extend_from_slice(&title_pos.to_le_bytes());
    out.extend_from_slice(&cluster_pos.to_le_bytes());
    out.extend_from_slice(&80u64.to_le_bytes());
    out.extend_from_slice(&main_index.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&cursor.to_le_bytes());
    out.extend_from_slice(mimes);
    for offset in dirent_offsets {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    for index in 0..records.len() as u32 {
        out.extend_from_slice(&index.to_le_bytes());
    }
    for offset in &cluster_offsets {
        out.extend_from_slice(&offset.to_le_bytes());
    }
    for dirent in dirents {
        out.extend_from_slice(&dirent);
    }
    for cluster in clusters.iter().rev() {
        out.extend_from_slice(cluster);
    }
    assert_eq!(out.len() as u64, cursor);
    let digest = Md5::digest(&out);
    out.extend_from_slice(&digest);
    (out, cluster_offsets)
}

fn modern_fixture(count: usize, blob_size: usize) -> (Vec<u8>, Vec<u64>) {
    fixture(
        6,
        1,
        (0..count)
            .map(|index| {
                article(
                    b'C',
                    &format!("item-{index:04}"),
                    2,
                    &vec![index as u8; blob_size],
                )
            })
            .collect(),
        None,
    )
}

fn recreate(source: &Path, destination: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_zimrecreate"))
        .arg(source)
        .arg(destination)
        .args(["--without-indexes", "--compression", "none"])
        .output()
        .unwrap()
}

fn split(source: &Path, prefix: &Path, size: u64, force: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zimsplit"));
    command
        .arg("--prefix")
        .arg(prefix)
        .arg("--size")
        .arg(size.to_string());
    if force {
        command.arg("--force");
    }
    command.arg(source).output().unwrap()
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn part_path(prefix: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!(
        "{}{}{}",
        prefix.display(),
        char::from(b'a' + (index / 26) as u8),
        char::from(b'a' + (index % 26) as u8)
    ))
}

#[test]
fn recreate_rejects_same_source_path_without_changing_bytes() {
    let dir = TempDir::new();
    let source = dir.path("source.zim");
    let (bytes, _) = modern_fixture(1, 100);
    fs::write(&source, &bytes).unwrap();
    assert!(!recreate(&source, &source).status.success());
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn recreate_rejects_hardlink_and_symlink_aliases() {
    let dir = TempDir::new();
    let source = dir.path("source.zim");
    let (bytes, _) = modern_fixture(1, 100);
    fs::write(&source, &bytes).unwrap();
    let hardlink = dir.path("hardlink.zim");
    let symlink = dir.path("symlink.zim");
    fs::hard_link(&source, &hardlink).unwrap();
    std::os::unix::fs::symlink(&source, &symlink).unwrap();
    for alias in [&hardlink, &symlink] {
        assert!(!recreate(&source, alias).status.success());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert_eq!(fs::read(alias).unwrap(), bytes);
    }
    assert!(fs::symlink_metadata(symlink)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn recreate_keeps_source_and_existing_destination_on_late_read_failure() {
    let dir = TempDir::new();
    let source = dir.path("broken.zim");
    let destination = dir.path("existing.zim");
    let (mut bytes, offsets) = modern_fixture(1, 100);
    // Header/tables are readable; the cluster's first blob offset is invalid.
    let first_offset = offsets[0] as usize + 1;
    bytes[first_offset..first_offset + 4].copy_from_slice(&3u32.to_le_bytes());
    fs::write(&source, &bytes).unwrap();
    fs::write(&destination, b"existing destination must survive").unwrap();
    assert!(!recreate(&source, &destination).status.success());
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"existing destination must survive"
    );
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
}

#[test]
fn recreate_preserves_legacy_namespace_links_main_and_redirects() {
    let html = b"<link href='../-/style.css'><img src='../I/icon'><a href='icon'>article</a><a href='start'>start</a>";
    let css = b"body { background: url('../I/icon') }";
    for major in [5, 6] {
        let dir = TempDir::new();
        let source = dir.path("legacy.zim");
        let destination = dir.path("modern.zim");
        let (bytes, _) = fixture(
            major,
            0,
            vec![
                article(b'-', "style.css", 1, css),
                article(b'A', "home", 0, html),
                article(b'A', "icon", 0, b"an article, not the image"),
                redirect(b'A', "start", b'A', "home"),
                redirect(b'A', "image", b'I', "icon"),
                redirect(b'A', "chain", b'A', "start"),
                article(b'I', "icon", 2, b"image bytes"),
                article(b'B', "home", 3, b"article metadata"),
                article(b'U', "home", 3, b"category text"),
                article(b'V', "home", 3, b"category articles"),
                article(b'W', "home", 3, b"article categories"),
            ],
            Some((b'A', "start")),
        );
        fs::write(&source, &bytes).unwrap();
        assert_success(recreate(&source, &destination));
        let archive = Archive::open(&destination).unwrap();
        assert!(archive.header().uses_new_namespaces());
        assert_eq!(archive.main_path().unwrap(), "A/home");
        assert_eq!(archive.get_bytes("A/home").unwrap(), html);
        assert_eq!(archive.get_bytes("-/style.css").unwrap(), css);
        assert_eq!(archive.get_bytes("I/icon").unwrap(), b"image bytes");
        assert_eq!(
            archive.get_bytes("A/icon").unwrap(),
            b"an article, not the image"
        );
        assert_eq!(archive.get_bytes("A/start").unwrap(), html);
        assert_eq!(archive.get_bytes("A/chain").unwrap(), html);
        assert_eq!(archive.get_bytes("A/image").unwrap(), b"image bytes");
        let redirect = archive
            .get_entry_by_path("A/image")
            .unwrap()
            .get_redirect_entry()
            .unwrap();
        assert_eq!(redirect.path(), "I/icon");
        assert_eq!(redirect.namespace(), b'C');
        for (path, body) in [
            ("B/home", "article metadata"),
            ("U/home", "category text"),
            ("V/home", "category articles"),
            ("W/home", "article categories"),
        ] {
            assert_eq!(archive.get_text(path).unwrap(), body);
        }
        assert!(archive.get_entry_by_path("home").is_err());
        assert!(archive.check().unwrap());
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }
}

#[test]
fn split_roundtrip_uses_physical_cluster_boundaries_and_normalizes_prefix() {
    let dir = TempDir::new();
    let source = dir.path("source.zim");
    let prefix = dir.path("parts.zim");
    let (bytes, offsets) = modern_fixture(8, 700);
    fs::write(&source, &bytes).unwrap();
    // Fits the table region and at least one cluster, but not two clusters.
    let size = 1200;
    assert_success(split(&source, &dir.path("parts"), size, false));
    let mut joined = Vec::new();
    let mut count = 0;
    while part_path(&prefix, count).exists() {
        let part = fs::read(part_path(&prefix, count)).unwrap();
        assert!(!part.is_empty());
        assert!(part.len() as u64 <= size);
        joined.extend_from_slice(&part);
        if joined.len() < bytes.len() {
            assert!(
                offsets.contains(&(joined.len() as u64)),
                "cut is not a cluster boundary"
            );
        }
        count += 1;
    }
    assert!(count > 1);
    assert_eq!(joined, bytes);
    let joined_path = dir.path("joined.zim");
    fs::write(&joined_path, joined).unwrap();
    let archive = Archive::open(joined_path).unwrap();
    assert!(archive.check().unwrap());
    assert_eq!(archive.get_bytes("item-0003").unwrap(), vec![3; 700]);
}

#[test]
fn split_exact_size_has_no_trailing_empty_part() {
    let dir = TempDir::new();
    let source = dir.path("source.zim");
    let prefix = dir.path("parts.zim");
    let (bytes, _) = modern_fixture(1, 100);
    fs::write(&source, &bytes).unwrap();
    assert_success(split(&source, &prefix, bytes.len() as u64, false));
    assert_eq!(fs::read(part_path(&prefix, 0)).unwrap(), bytes);
    assert!(!part_path(&prefix, 1).exists());
}

#[cfg(unix)]
#[test]
fn split_checks_first_and_later_hardlink_symlink_and_same_path_aliases() {
    let (bytes, offsets) = modern_fixture(4, 700);
    let size = *offsets.iter().min().unwrap();
    for index in [0, 1] {
        for alias_kind in ["hardlink", "symlink", "same-path"] {
            let dir = TempDir::new();
            let prefix = dir.path("parts.zim");
            let alias = part_path(&prefix, index);
            let source = if alias_kind == "same-path" {
                alias.clone()
            } else {
                dir.path("source.zim")
            };
            fs::write(&source, &bytes).unwrap();
            match alias_kind {
                "hardlink" => fs::hard_link(&source, &alias).unwrap(),
                "symlink" => std::os::unix::fs::symlink(&source, &alias).unwrap(),
                _ => {}
            }
            assert!(!split(&source, &prefix, size, true).status.success());
            assert_eq!(fs::read(&source).unwrap(), bytes);
            assert_eq!(fs::read(&alias).unwrap(), bytes);
            if index == 1 {
                assert!(
                    !part_path(&prefix, 0).exists(),
                    "preflight must precede every output creation"
                );
            }
        }
    }
}

#[test]
fn split_refuses_existing_later_output_and_oversized_regions_without_writing() {
    let dir = TempDir::new();
    let source = dir.path("source.zim");
    let prefix = dir.path("parts.zim");
    let (bytes, _) = modern_fixture(3, 700);
    fs::write(&source, &bytes).unwrap();
    fs::write(part_path(&prefix, 1), b"keep this output").unwrap();
    assert!(!split(&source, &prefix, 900, true).status.success());
    assert!(!part_path(&prefix, 0).exists());
    assert_eq!(
        fs::read(part_path(&prefix, 1)).unwrap(),
        b"keep this output"
    );
    fs::remove_file(part_path(&prefix, 1)).unwrap();
    assert!(!split(&source, &prefix, 100, false).status.success());
    assert!(!part_path(&prefix, 0).exists());
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn split_stops_at_supported_zz_suffix_and_rejects_more_parts_before_writing() {
    let dir = TempDir::new();
    let source = dir.path("source.zim");
    let prefix = dir.path("parts.zim");
    // One prefix region, 674 ten-byte clusters, and the last cluster+checksum.
    let (bytes, offsets) = modern_fixture(675, 1);
    fs::write(&source, &bytes).unwrap();
    assert_success(split(&source, &prefix, 10, true));
    let mut joined = Vec::new();
    for index in 0..676 {
        let part = fs::read(part_path(&prefix, index)).unwrap();
        assert!(!part.is_empty());
        joined.extend_from_slice(&part);
        if index < 675 {
            assert!(offsets.contains(&(joined.len() as u64)));
        }
    }
    assert_eq!(joined, bytes);
    assert!(!dir.path("parts.zimaaa").exists());
    let (too_many, _) = modern_fixture(676, 1);
    fs::write(&source, too_many).unwrap();
    let other_prefix = dir.path("overflow.zim");
    assert!(!split(&source, &other_prefix, 10, true).status.success());
    assert!(!part_path(&other_prefix, 0).exists());
    assert!(!dir.path("overflow.zimaaa").exists());
}
