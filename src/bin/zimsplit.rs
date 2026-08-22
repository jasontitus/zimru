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
    // checked_mul: a huge suffixed value like `20000000000G` must be
    // rejected, not silently wrapped into a tiny (or bogus) split size.
    num_str
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mult))
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
        return Err(Error::Io(std::io::Error::other(
            "byte-aligned split would split a cluster",
        )));
    }
    let mut input = File::open(file)?;
    let mut buf = vec![0u8; READ_BUF];
    let mut written = 0u64;
    let mut suffix: Vec<u8> = b"aa".to_vec();
    let mut part_path = format!("{prefix}{}", String::from_utf8_lossy(&suffix));
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
            part_path = format!("{prefix}{}", String::from_utf8_lossy(&suffix));
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

/// Advance an alphabetic part suffix: `aa` → `ab` → … → `az` → `ba` → …
/// `zz` → `aaa` → …
///
/// The suffix grows rather than wrapping. The previous fixed `[u8; 2]`
/// incremented the leading byte unconditionally, so the 27th part was named
/// `{a` (`z` + 1) and every part past `zz` walked further out of the
/// alphabet — filenames no concatenating tool recognises, produced silently.
/// A 2 GB `--size` on a 60 GB archive needs 30 parts, which is well inside
/// what this is asked to do.
fn advance_suffix(s: &mut Vec<u8>) {
    for i in (0..s.len()).rev() {
        if s[i] < b'z' {
            s[i] += 1;
            return;
        }
        s[i] = b'a';
    }
    // Every position rolled over: widen (`zz` → `aaa`).
    s.insert(0, b'a');
}

#[cfg(test)]
mod suffix_tests {
    use super::advance_suffix;

    fn seq(n: usize) -> Vec<String> {
        let mut s: Vec<u8> = b"aa".to_vec();
        let mut out = vec![String::from_utf8(s.clone()).unwrap()];
        for _ in 1..n {
            advance_suffix(&mut s);
            out.push(String::from_utf8(s.clone()).unwrap());
        }
        out
    }

    #[test]
    fn suffix_walks_the_alphabet_and_widens() {
        let v = seq(28);
        assert_eq!(&v[..3], ["aa", "ab", "ac"]);
        // The old fixed-width version produced "{a" here.
        assert_eq!(v[25], "az");
        assert_eq!(v[26], "ba");
        assert_eq!(v[27], "bb");
        // Every part name stays alphabetic, however many there are.
        assert!(v.iter().all(|p| p.bytes().all(|b| b.is_ascii_lowercase())));
    }

    #[test]
    fn suffix_widens_past_zz() {
        let v = seq(26 * 26 + 2);
        assert_eq!(v[26 * 26 - 1], "zz");
        assert_eq!(v[26 * 26], "aaa");
        assert_eq!(v[26 * 26 + 1], "aab");
    }
}
