//! `zimbench` — CLI parity with kiwix `zim-tools`' `zimbench`.
//!
//! Mirrors upstream's behavior:
//!   1. Phase 1: collect URLs by scanning the archive sequentially. Prints
//!      `check url <url>\t<found_so_far> found` for each one until -n urls
//!      are gathered.
//!   2. Phase 2: linear access — touches the first -n articles in path order
//!      and reports total time.
//!   3. Phase 3: random access — picks -d distinct random articles and reads
//!      each up to -r times.
//!
//! Flags:
//!   -n N    number of linear accessed articles (default 1000)
//!   -r N    number of random accessed reads      (default = -n)
//!   -d N    number of distinct random articles    (default = -r)
//!   -v      print version and exit

use std::process::ExitCode;
use std::time::Instant;

use zimru::{Archive, Entry};

const VERSION: &str = "zimbench (zimru) 0.1.0";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut n: usize = 1000;
    let mut r: Option<usize> = None;
    let mut d: Option<usize> = None;
    let mut file: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-v" | "--version" => {
                println!("{VERSION}");
                return ExitCode::SUCCESS;
            }
            "-n" => {
                i += 1;
                n = args.get(i).and_then(|x| x.parse().ok()).unwrap_or(n);
            }
            "-r" => {
                i += 1;
                r = args.get(i).and_then(|x| x.parse().ok());
            }
            "-d" => {
                i += 1;
                d = args.get(i).and_then(|x| x.parse().ok());
            }
            other if other.starts_with('-') => {
                eprintln!("zimbench: unrecognized option '{other}'");
                eprintln!("Unknown option `{other}'");
                return ExitCode::from(2);
            }
            _ => {
                file = Some(a.clone());
            }
        }
        i += 1;
    }
    let r = r.unwrap_or(n);
    let d = d.unwrap_or(r);
    let file = match file {
        Some(f) => f,
        None => {
            eprintln!("usage: zimbench [options] zimfile");
            return ExitCode::from(2);
        }
    };
    match run(&file, n, r, d) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zimbench: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(file: &str, n: usize, r: usize, d: usize) -> Result<(), zimru::Error> {
    let arc = Archive::open(file)?;

    // ---- Phase 1: collect URLs (matching upstream verbose output) ----
    let mut urls: Vec<(u8, String)> = Vec::with_capacity(n);
    let mut found = 0usize;
    for entry in arc.iter_by_path() {
        let e = entry?;
        if e.is_redirect() {
            continue;
        }
        if e.namespace() != b'C' {
            continue;
        }
        urls.push((e.namespace(), e.path().to_string()));
        found += 1;
        println!("check url {}\t{} found", e.path(), found);
        if urls.len() >= n {
            break;
        }
    }
    println!("{} urls collected", urls.len());

    if urls.is_empty() {
        println!("Cannot find entry");
        return Ok(());
    }

    println!("collect random urls");
    let actual_d = d.min(urls.len());
    let mut rng = SplitMix64::new(0xDEADBEEFCAFEF00D);
    // Partial Fisher–Yates so exactly `actual_d` DISTINCT urls are
    // chosen — sampling with replacement would over-read hot articles
    // and skew the random-access numbers.
    let mut indices: Vec<usize> = (0..urls.len()).collect();
    for i in 0..actual_d {
        let j = i + (rng.next_u64() as usize) % (indices.len() - i);
        indices.swap(i, j);
    }
    let random_urls: Vec<(u8, String)> = indices[..actual_d]
        .iter()
        .map(|&i| urls[i].clone())
        .collect();

    // ---- Phase 2: linear access ----
    let start = Instant::now();
    let mut bytes: u64 = 0;
    for (ns, url) in &urls {
        let e = arc.entry_by_ns_path(*ns, url)?;
        let item = follow(&e)?;
        bytes += item.get_data()?.size() as u64;
    }
    let lin = start.elapsed();
    println!(
        "linear access: {} reads, {} bytes, {:.3}s, {:.2} MB/s",
        urls.len(),
        bytes,
        lin.as_secs_f64(),
        bytes as f64 / 1_048_576.0 / lin.as_secs_f64().max(1e-9)
    );

    // ---- Phase 3: random access ----
    if random_urls.is_empty() {
        // `-d 0` leaves nothing to sample; skip rather than panic on
        // the modulo below.
        println!("random access: skipped (no random urls selected)");
        return Ok(());
    }
    let start = Instant::now();
    let mut bytes: u64 = 0;
    for _ in 0..r {
        let idx = (rng.next_u64() as usize) % random_urls.len();
        let (ns, url) = &random_urls[idx];
        let e = arc.entry_by_ns_path(*ns, url)?;
        let item = follow(&e)?;
        bytes += item.get_data()?.size() as u64;
    }
    let rnd = start.elapsed();
    println!(
        "random access: {} reads, {} bytes, {:.3}s, {:.2} MB/s",
        r,
        bytes,
        rnd.as_secs_f64(),
        bytes as f64 / 1_048_576.0 / rnd.as_secs_f64().max(1e-9)
    );

    Ok(())
}

fn follow(e: &Entry) -> Result<zimru::Item, zimru::Error> {
    e.get_item(true)
}

/// Tiny deterministic PRNG so benchmark runs are reproducible without an extra dep.
struct SplitMix64 {
    s: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { s: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.s = self.s.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}
