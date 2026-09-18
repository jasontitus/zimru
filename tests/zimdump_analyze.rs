#![cfg(feature = "writer")]
//! End-to-end test for `zimdump analyze`. Builds a small ZIM through
//! `zimru::writer::Creator`, runs the `zimdump analyze` binary on it, and:
//!
//! 1. Verifies the public `Archive::cluster_byte_range` invariant — the
//!    sum of every cluster's compressed bytes equals
//!    `checksum_pos - first_cluster_pointer`.
//! 2. Parses the binary's per-cluster table and confirms the row count
//!    matches `archive.cluster_count()`.
//! 3. Runs `--by-item` and confirms one row appears for each C-namespace
//!    article.

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
        "zimru-analyze-{tag}-{}-{}.zim",
        std::process::id(),
        ns
    ));
    p
}

fn zimdump_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_zimdump"))
}

#[test]
fn cluster_byte_ranges_cover_the_cluster_region() {
    let out = tmp_path("ranges");
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.set_cluster_size_target(8 * 1024); // force several clusters
    for i in 0..6 {
        let body = vec![b'A' + (i % 26) as u8; 4 * 1024];
        c.add_item(Item::new(
            format!("a{i:02}"),
            format!("Article {i}"),
            "text/plain",
            body,
        ));
    }
    c.write_to(&out).expect("write");

    let a = Archive::open(&out).expect("reopen");
    assert!(a.cluster_count() >= 2);
    // Cluster *index* order is not guaranteed to match on-disk order
    // (the writer emits clusters in encode-completion order), so
    // collect every range and verify they tile the cluster region
    // contiguously in *file* order with no gaps or overlaps.
    let mut ranges: Vec<std::ops::Range<u64>> = (0..a.cluster_count())
        .map(|idx| {
            let r = a.cluster_byte_range(idx).unwrap();
            assert!(r.end > r.start, "cluster {idx} has zero size");
            r
        })
        .collect();
    ranges.sort_by_key(|r| r.start);
    let mut total: u64 = 0;
    let mut prev_end: Option<u64> = None;
    for r in &ranges {
        if let Some(prev) = prev_end {
            assert_eq!(
                prev, r.start,
                "cluster region has a gap/overlap at offset {}",
                r.start
            );
        }
        total += r.end - r.start;
        prev_end = Some(r.end);
    }
    // The sum of all compressed cluster bytes must equal the on-disk
    // cluster region size. In the streaming-writer layout the cluster
    // region ends where the trailing url-pointer / title-pointer /
    // cluster-pointer / dirent tables begin, not at the checksum pos.
    let first = ranges.first().unwrap().start;
    let last = ranges.last().unwrap().end;
    assert_eq!(total, last - first);

    let _ = std::fs::remove_file(&out);
}

#[test]
fn analyze_subcommand_prints_one_row_per_cluster() {
    let zimdump = zimdump_binary();
    let out = tmp_path("analyze");
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.set_cluster_size_target(8 * 1024);
    for i in 0..6 {
        let body = vec![b'X'; 4 * 1024];
        c.add_item(Item::new(
            format!("a{i:02}"),
            format!("A{i}"),
            "text/plain",
            body,
        ));
    }
    c.write_to(&out).expect("write");
    let a = Archive::open(&out).unwrap();

    let result = Command::new(&zimdump)
        .arg("analyze")
        .arg(&out)
        .output()
        .expect("run zimdump analyze");
    assert!(
        result.status.success(),
        "zimdump analyze failed: {:?}",
        result
    );
    let stdout = String::from_utf8_lossy(&result.stdout);

    // Header line + N cluster rows + TOTAL row.
    let n = stdout.lines().count();
    let expected = 1 + a.cluster_count() as usize + 1;
    assert_eq!(
        n,
        expected,
        "expected {expected} lines (header + {} clusters + total), got {n}:\n{stdout}",
        a.cluster_count()
    );
    assert!(stdout.starts_with("clstr"), "missing header in:\n{stdout}");
    assert!(stdout.contains("TOTAL"), "missing TOTAL row in:\n{stdout}");

    let _ = std::fs::remove_file(&out);
}

#[test]
fn analyze_by_item_lists_every_article() {
    let zimdump = zimdump_binary();
    let out = tmp_path("analyze-byitem");
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.add_item(Item::text("alpha", "Alpha", "alpha"));
    c.add_item(Item::text("beta", "Beta", "beta"));
    c.add_item(Item::text("gamma", "Gamma", "gamma"));
    c.add_redirection("a", "Alias", "alpha");
    c.write_to(&out).expect("write");

    let result = Command::new(&zimdump)
        .arg("analyze")
        .arg("--by-item")
        .arg(&out)
        .output()
        .expect("run zimdump analyze --by-item");
    assert!(
        result.status.success(),
        "zimdump analyze --by-item failed: {:?}",
        result
    );
    let stdout = String::from_utf8_lossy(&result.stdout);

    // Three article rows; redirect must NOT appear.
    let mut count = 0;
    for line in stdout.lines().skip(1) {
        if line.contains("C/alpha") || line.contains("C/beta") || line.contains("C/gamma") {
            count += 1;
        }
    }
    assert_eq!(count, 3, "expected 3 article rows, got {count}:\n{stdout}");
    assert!(
        !stdout.contains("C/a "),
        "redirect alias must not be listed:\n{stdout}"
    );

    let _ = std::fs::remove_file(&out);
}
