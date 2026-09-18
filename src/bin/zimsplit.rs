//! `zimsplit` — split an archive at cluster boundaries.
//!
//! Parts are named `<prefix>aa`, `<prefix>ab`, …, `<prefix>zz`, where
//! the prefix ends in `.zim` (appended if needed). Concatenating the
//! parts reconstructs the original file without rewriting any bytes.
//!
//! `--size=N` is a maximum part size, defaulting to 2_000_000_000 bytes.
//! `--force` allows an indivisible region to exceed that limit; it never
//! cuts a cluster or overwrites an existing output.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use zimru::{Archive, Error};

const VERSION: &str = "zimsplit (zimru) 0.1.0";
const DEFAULT_SIZE: u64 = 2_000_000_000;
const READ_BUF: usize = 4 * 1024 * 1024;
const MAX_PARTS: usize = 26 * 26;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut prefix: Option<String> = None;
    let mut size: u64 = DEFAULT_SIZE;
    let mut force = false;
    let mut file: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--version" => {
                println!("{VERSION}");
                return ExitCode::SUCCESS;
            }
            "--force" => force = true,
            _ if a.starts_with("--prefix=") => prefix = Some(a[9..].to_string()),
            "--prefix" => {
                i += 1;
                prefix = args.get(i).cloned();
            }
            _ if a.starts_with("--size=") => {
                let Some(parsed) = parse_size(&a[7..]) else {
                    eprintln!("zimsplit: --size requires a positive byte count");
                    return ExitCode::from(2);
                };
                size = parsed;
            }
            "--size" => {
                i += 1;
                let Some(parsed) = args.get(i).and_then(|s| parse_size(s)) else {
                    eprintln!("zimsplit: --size requires a positive byte count");
                    return ExitCode::from(2);
                };
                size = parsed;
            }
            _ if a.starts_with('-') => {
                eprintln!("zimsplit: unknown option `{a}`");
                return ExitCode::from(2);
            }
            _ => file = Some(a.clone()),
        }
        i += 1;
    }
    let Some(file) = file else {
        print_help();
        return ExitCode::from(2);
    };
    let prefix = prefix.unwrap_or_else(|| file.clone());
    match run(&file, &prefix, size, force) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zimsplit: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "\n    zimsplit splits a ZIM archive at cluster boundaries.\n\nUsage:\n    zimsplit [--prefix=PREFIX] [--force] [--size=N] <file>\n    zimsplit --version\n\n    Cuts are made only at cluster starts, so a single cluster and the\n    region holding the header, pointer tables and directory entries are\n    each indivisible. --force permits oversized parts when such a region\n    cannot fit --size. Existing outputs are never overwritten. At most\n    676 parts (aa..zz) are supported.\n"
    );
}

fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num_str, mult) = if let Some(stem) = s.strip_suffix(['G', 'g']) {
        (stem, 1_000_000_000u64)
    } else if let Some(stem) = s.strip_suffix(['M', 'm']) {
        (stem, 1_000_000u64)
    } else if let Some(stem) = s.strip_suffix(['K', 'k']) {
        (stem, 1_000u64)
    } else {
        (s, 1u64)
    };
    num_str
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mult))
        .filter(|&n| n > 0)
}

fn invalid_input(message: impl Into<String>) -> Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into()).into()
}

/// Plan every output before creating anything. Physical order is independent
/// of cluster indices, so sort the cluster starts before choosing cuts.
fn part_ends(archive: &Archive, size: u64, force: bool) -> Result<Vec<u64>, Error> {
    let total = archive.file_len();
    let mut boundaries = Vec::with_capacity(archive.cluster_count() as usize + 1);
    for index in 0..archive.cluster_count() {
        let offset = archive.cluster_offset(index)?;
        if offset < archive.header().mime_list_pos || offset >= total {
            return Err(invalid_input(
                "cluster offset lies outside the archive data",
            ));
        }
        boundaries.push(offset);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries.push(total);

    let mut ends = Vec::new();
    let mut start = 0u64;
    let mut next = 0;
    while start < total {
        let limit = start.saturating_add(size);
        let mut after = next;
        while after < boundaries.len() && boundaries[after] <= limit {
            after += 1;
        }
        let end = if after > next {
            boundaries[after - 1]
        } else if force {
            let end = boundaries[next];
            eprintln!(
                "zimsplit: indivisible region requires a {}-byte part",
                end - start
            );
            after = next + 1;
            end
        } else {
            return Err(invalid_input(format!(
                "indivisible region at {start} requires {} bytes, exceeding --size={size}; increase --size or use --force for oversized parts",
                boundaries[next] - start
            )));
        };
        ends.push(end);
        if ends.len() > MAX_PARTS {
            return Err(invalid_input(
                "split requires more than 676 parts (aa through zz); increase --size",
            ));
        }
        start = end;
        next = after;
    }
    Ok(ends)
}

/// Remove only files created by this invocation when copying fails.
struct OutputCleanup {
    paths: Vec<PathBuf>,
    complete: bool,
}

impl Drop for OutputCleanup {
    fn drop(&mut self) {
        if !self.complete {
            for path in &self.paths {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn run(file: &str, prefix: &str, size: u64, force: bool) -> Result<(), Error> {
    if size == 0 {
        return Err(invalid_input("--size must be > 0"));
    }
    let archive = Archive::open(file)?;
    let ends = part_ends(&archive, size, force)?;
    let prefix = if prefix.ends_with(".zim") {
        prefix.to_owned()
    } else {
        format!("{prefix}.zim")
    };
    let paths: Vec<PathBuf> = (0..ends.len())
        .map(|index| {
            PathBuf::from(format!(
                "{prefix}{}{}",
                char::from(b'a' + (index / 26) as u8),
                char::from(b'a' + (index % 26) as u8)
            ))
        })
        .collect();

    // Check ALL destinations, including later parts, before writing part aa.
    // Refusing every existing directory entry also catches source hardlinks,
    // symlinks (including dangling ones), and aliases between output parts.
    for path in &paths {
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(invalid_input(format!(
                    "output already exists: {}",
                    path.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    let mut input = File::open(file)?;
    let mut buf = vec![0u8; READ_BUF];
    let mut cleanup = OutputCleanup {
        paths: Vec::new(),
        complete: false,
    };
    let mut start = 0;
    for (path, end) in paths.into_iter().zip(ends) {
        // create_new closes the race between preflight and creation and never
        // follows a newly introduced symlink to the source.
        let mut part = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        eprintln!("opening new file {}", path.display());
        cleanup.paths.push(path);
        let mut remaining = end - start;
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            input.read_exact(&mut buf[..want])?;
            part.write_all(&buf[..want])?;
            remaining -= want as u64;
        }
        part.flush()?;
        start = end;
    }
    cleanup.complete = true;
    Ok(())
}
