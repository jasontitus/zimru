//! `zimsplit` — CLI parity with kiwix `zim-tools`' `zimsplit`.
//!
//! Splits a ZIM file into byte-aligned parts named `<prefix>.zimaa`,
//! `.zimab`, … so the original file equals the byte-wise concatenation of all
//! parts. Used for distributing very large ZIMs across filesystems with size
//! limits (FAT32, …).
//!
//! Usage (mirrors upstream):
//!   zimsplit [--prefix=PREFIX] [--force] [--size=N] <file>
//!   zimsplit --version
//!
//! `--size=N` accepts a number of bytes (default 2_000_000_000 ≈ 2 GB, same
//! as upstream). The default prefix is the input file path itself, so
//! `wikipedia.zim` → `wikipedia.zim.zimaa`, `wikipedia.zim.zimab`, ….

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::process::ExitCode;

use zimru::Error;

const VERSION: &str = "zimsplit (zimru) 0.1.0";
const DEFAULT_SIZE: u64 = 2_000_000_000;
const READ_BUF: usize = 4 * 1024 * 1024;

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
            "--force" => {
                force = true;
            }
            _ if a.starts_with("--prefix=") => {
                prefix = Some(a[9..].to_string());
            }
            "--prefix" => {
                i += 1;
                prefix = args.get(i).cloned();
            }
            _ if a.starts_with("--size=") => {
                size = parse_size(&a[7..]).unwrap_or(size);
            }
            "--size" => {
                i += 1;
                size = args.get(i).and_then(|s| parse_size(s)).unwrap_or(size);
            }
            _ if a.starts_with('-') => {
                eprintln!("zimsplit: unknown option `{a}`");
                return ExitCode::from(2);
            }
            _ => {
                file = Some(a.clone());
            }
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
        "\n    zimsplit splits smartly a ZIM file in smaller parts.\n\nUsage:\n    zimsplit [--prefix=PREFIX] [--force] [--size=N] <file>\n    zimsplit --version\n"
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
    num_str.parse::<u64>().ok().map(|n| n * mult)
}

fn run(file: &str, prefix: &str, size: u64, force: bool) -> Result<(), Error> {
    let total = std::fs::metadata(file)?.len();
    if size == 0 {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--size must be > 0",
        )));
    }
    if !force && total > size {
        // Upstream tries to split on cluster boundaries to keep clusters whole;
        // we don't have that smarts yet, so without --force only allow when the
        // requested split size is large enough to fit the cluster region in
        // a single part. Without writer-side rewrites we can only do byte-wise
        // splits, so warn and require --force for safety.
        eprintln!(
            "zimsplit: byte-aligned split may break cluster boundaries; pass --force to proceed."
        );
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "byte-aligned split would split a cluster",
        )));
    }
    let mut input = File::open(file)?;
    let mut buf = vec![0u8; READ_BUF];
    let mut written = 0u64;
    let mut suffix = [b'a', b'a'];
    let mut part_path = format!("{prefix}{}{}", suffix[0] as char, suffix[1] as char);
    eprintln!("opening new file {part_path}");
    let mut part = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&part_path)?;
    let mut total_written: u64 = 0;
    let mut part_count: u32 = 1;
    loop {
        let want = std::cmp::min(buf.len() as u64, size - written) as usize;
        if want == 0 {
            // rotate
            part.flush()?;
            advance_suffix(&mut suffix);
            part_path = format!("{prefix}{}{}", suffix[0] as char, suffix[1] as char);
            eprintln!("opening new file {part_path}");
            part = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&part_path)?;
            part_count += 1;
            written = 0;
            continue;
        }
        let n = input.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        part.write_all(&buf[..n])?;
        written += n as u64;
        total_written += n as u64;
    }
    part.flush()?;
    let _ = (total_written, total, part_count);
    Ok(())
}

fn advance_suffix(s: &mut [u8; 2]) {
    if s[1] < b'z' {
        s[1] += 1;
    } else {
        s[1] = b'a';
        s[0] += 1;
    }
}
