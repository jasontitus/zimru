//! `zimrecreate` — CLI parity with kiwix `zim-tools`' `zimrecreate`.
//!
//! Reads every entry (articles + redirects + metadata + illustrations
//! plus main page redirect) from the source archive and writes a new
//! archive using `zimru::writer::Creator`.
//!
//! ```text
//! Usage (matches upstream):
//!   zimrecreate ORIGIN_FILE OUTPUT_FILE [Options]
//!
//! Options:
//!   -v, --version              print software version
//!   -j, --withoutFTIndex       don't build a fulltext index (always on
//!                              in zimru — we don't have a fulltext indexer yet)
//!   -J, --threads <number>     ignored (single-threaded writer for now)
//!   --compression none|zstd|xz choose cluster compression (default zstd)
//!   --compression-level N      compression level (zstd: 1..=22, xz: 0..=9)
//!   --cluster-size BYTES       cluster size target (default 2MiB)
//! ```

use std::process::ExitCode;

use zimru::writer::{ClusterStrategy, Creator, Item};
use zimru::{Archive, Compression, Dirent};

const VERSION: &str = "zimrecreate (zimru) 0.1.0";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut src: Option<String> = None;
    let mut dst: Option<String> = None;
    let mut compression = Compression::Zstd;
    let mut compression_level: Option<i32> = None;
    let mut cluster_target: Option<usize> = None;
    let mut cluster_strategy: ClusterStrategy = ClusterStrategy::Single;

    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-v" | "--version" => {
                println!("{VERSION}");
                return ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "-j" | "--withoutFTIndex" => {}
            "-J" | "--threads" => {
                i += 1; // consume value, ignore
            }
            "--compression" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("none") => compression = Compression::None,
                    Some("zstd") => compression = Compression::Zstd,
                    Some("xz") => compression = Compression::Xz,
                    other => {
                        eprintln!("zimrecreate: unknown --compression `{other:?}`");
                        return ExitCode::from(2);
                    }
                }
            }
            "--compression-level" => {
                i += 1;
                match args.get(i).and_then(|s| s.parse::<i32>().ok()) {
                    Some(n) => compression_level = Some(n),
                    None => {
                        eprintln!("zimrecreate: --compression-level requires an integer");
                        return ExitCode::from(2);
                    }
                }
            }
            "--cluster-size" => {
                i += 1;
                cluster_target = args.get(i).and_then(|s| s.parse().ok());
            }
            "--cluster-by" => {
                i += 1;
                cluster_strategy = match args.get(i).map(String::as_str) {
                    Some("single") | Some("") => ClusterStrategy::Single,
                    Some("mime") => ClusterStrategy::ByMime,
                    Some("extension") | Some("ext") => ClusterStrategy::ByExtension,
                    Some("path") | Some("path-segment") => {
                        ClusterStrategy::ByFirstPathSegment
                    }
                    other => {
                        eprintln!(
                            "zimrecreate: --cluster-by must be one of single|mime|extension|path; got `{other:?}`"
                        );
                        return ExitCode::from(2);
                    }
                };
            }
            a if a.starts_with('-') => {
                eprintln!("zimrecreate: unknown option `{a}`");
                return ExitCode::from(2);
            }
            _ => {
                if src.is_none() {
                    src = Some(a.to_string());
                } else if dst.is_none() {
                    dst = Some(a.to_string());
                }
            }
        }
        i += 1;
    }

    let (src, dst) = match (src, dst) {
        (Some(s), Some(d)) => (s, d),
        _ => {
            print_help();
            return ExitCode::from(2);
        }
    };

    match run(
        &src,
        &dst,
        compression,
        compression_level,
        cluster_target,
        cluster_strategy,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zimrecreate: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "\nzimrecreate recreates a ZIM file from an existing ZIM.\n\nUsage: zimrecreate ORIGIN_FILE OUTPUT_FILE [Options]\nOptions:\n\t-v, --version              print software version\n\t-j, --withoutFTIndex       don't create a fulltext index (always)\n\t-J, --threads <number>     ignored\n\t--compression C            one of: none | zstd | xz  (default zstd)\n\t--compression-level N      compression level (zstd: 1..=22, xz: 0..=9)\n\t--cluster-size BYTES       cluster size target (default 2097152)\n"
    );
}

fn run(
    src: &str,
    dst: &str,
    compression: Compression,
    compression_level: Option<i32>,
    cluster_target: Option<usize>,
    cluster_strategy: ClusterStrategy,
) -> Result<(), zimru::Error> {
    let source = Archive::open(src)?;
    let mut creator = Creator::new();
    creator.set_compression(compression);
    if let Some(level) = compression_level {
        creator.set_compression_level(level);
    }
    creator.set_uuid(source.uuid()); // preserve UUID so tooling can spot the relationship
    if let Some(n) = cluster_target {
        creator.set_cluster_size_target(n);
    }
    creator.set_cluster_strategy(cluster_strategy);

    // Forward the main path (if any).
    if source.has_main_entry() {
        if let Ok(main) = source.main_path() {
            creator.set_main_path(&main);
        }
    }

    // Switch to streaming mode — bodies bin-pack into clusters and
    // stream-encode-write as we iterate the source. Peak RSS becomes
    // O(cluster_size_target × bucket_count) instead of O(total content).
    creator.start_writing(dst)?;

    // Iterate every entry, partition by kind. We skip:
    //   W/mainPage  (added automatically via set_main_path above)
    //   X/*         (search indexes we don't rebuild — matches --withoutFTIndex)
    let ml = source.mime_list();
    let mut seen_metadata = std::collections::HashSet::new();
    for entry in source.iter_by_path() {
        let entry = entry?;
        let ns = entry.namespace();
        let path = entry.path();
        let title = entry.title();

        if ns == b'W' && path == "mainPage" {
            continue;
        }
        if ns == b'X' {
            continue;
        }

        match entry.dirent() {
            Dirent::Redirect(r) => {
                if ns == b'C' {
                    // Look up the target's path so we can add it via the
                    // public redirection API.
                    let target = source.entry_by_url_index(r.redirect_index)?;
                    creator.add_redirection(
                        path.to_string(),
                        title.to_string(),
                        target.path().to_string(),
                    );
                }
                // Redirects in W/M/X namespaces are rebuilt implicitly by
                // re-adding the underlying entries.
            }
            Dirent::Article(_) => {
                let item = entry.get_item(false)?;
                let mime = ml
                    .get(match entry.dirent() {
                        Dirent::Article(a) => a.mimetype,
                        _ => 0,
                    })
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let data = item.bytes()?;
                if ns == b'C' {
                    creator.add_item(Item::new(path, title, mime, data));
                } else if ns == b'M' {
                    // Strip the special illustration path back into add_illustration
                    // where possible so the output is canonically shaped.
                    if let Some(side) = parse_illustration_path(path) {
                        creator.add_illustration(side, data);
                    } else if seen_metadata.insert(path.to_string()) {
                        creator.add_metadata(path, data);
                    }
                }
            }
        }
    }

    creator.finish_writing()?;
    Ok(())
}

fn parse_illustration_path(path: &str) -> Option<u32> {
    // "Illustration_NxN@1" -> Some(N)
    let rest = path.strip_prefix("Illustration_")?;
    let at_idx = rest.find('@')?;
    let dims = &rest[..at_idx];
    let (w, h) = dims.split_once('x')?;
    let w: u32 = w.parse().ok()?;
    let h: u32 = h.parse().ok()?;
    if w == h {
        Some(w)
    } else {
        None
    }
}
