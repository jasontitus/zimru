//! `zimru` CLI — small inspection / extraction tool for ZIM files.
//!
//! Subcommands:
//! ```text
//! zimru info <file>                 # header + counts + main entry + checksum status
//! zimru list <file> [--limit N]     # list entries in path order
//! zimru titles <file> [--limit N]   # list entries in title order
//! zimru meta <file> [<key>]         # list metadata keys, or dump one
//! zimru get <file> <path>           # write the named entry's bytes to stdout
//! zimru check <file>                # verify the trailing MD5 checksum
//! zimru mimes <file>                # print the mime-type list
//! zimru readall <file> [--md5]      # read every blob (benchmark / integrity)
//! ```

use std::io::{self, Write};
use std::process::ExitCode;

use md5::Digest as _;
use zimru::{Archive, Error};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let res = match args.get(1).map(String::as_str) {
        Some("info") => cmd_info(&args[2..]),
        Some("list") => cmd_list(&args[2..], false),
        Some("titles") => cmd_list(&args[2..], true),
        Some("meta") => cmd_meta(&args[2..]),
        Some("get") => cmd_get(&args[2..]),
        Some("check") => cmd_check(&args[2..]),
        Some("mimes") => cmd_mimes(&args[2..]),
        Some("readall") => cmd_readall(&args[2..]),
        Some("--help") | Some("-h") | None => {
            usage();
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("zimru: unknown subcommand `{other}`");
            usage();
            return ExitCode::from(2);
        }
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zimru: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "usage:\n  \
         zimru info <file>\n  \
         zimru list <file> [--limit N]\n  \
         zimru titles <file> [--limit N]\n  \
         zimru meta <file> [<key>]\n  \
         zimru get <file> <path>\n  \
         zimru check <file>\n  \
         zimru mimes <file>\n  \
         zimru readall <file> [--md5] [--quiet]"
    );
}

fn cmd_info(args: &[String]) -> Result<(), Error> {
    let path = args.first().ok_or_else(io_arg)?;
    let arc = Archive::open(path)?;
    let h = arc.header();
    println!("file:           {path}");
    println!("version:        {}.{}", h.major_version, h.minor_version);
    println!(
        "namespaces:     {}",
        if h.uses_new_namespaces() {
            "new (C/M/W/X)"
        } else {
            "legacy (A/I/M/...)"
        }
    );
    println!("uuid:           {}", hex_lower(&h.uuid));
    println!("entry_count:    {}", h.entry_count);
    println!("cluster_count:  {}", h.cluster_count);
    println!("url_ptr_pos:    {}", h.url_ptr_pos);
    println!("title_ptr_pos:  {}", h.title_ptr_pos);
    println!("cluster_ptr_pos:{}", h.cluster_ptr_pos);
    println!("mime_list_pos:  {}", h.mime_list_pos);
    println!("checksum_pos:   {}", h.checksum_pos);
    println!("mime_types:     {}", arc.mime_list().len());
    println!("has_main_page:  {}", arc.has_main_entry());
    if arc.has_main_entry() {
        if let Ok(e) = arc.main_entry() {
            println!("main_namespace: {}", char::from(e.namespace()));
            println!("main_path:      {}", e.path());
            println!("main_title:     {}", e.title());
            println!("main_redirect:  {}", e.is_redirect());
        }
    }
    println!("has_checksum:   {}", arc.has_checksum());
    if arc.has_checksum() {
        println!("checksum:       {}", hex_lower(&arc.checksum()?));
    }
    Ok(())
}

fn cmd_list(args: &[String], by_title: bool) -> Result<(), Error> {
    let path = args.first().ok_or_else(io_arg)?;
    let mut limit: Option<usize> = Some(50);
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--limit" {
            limit = args.get(i + 1).and_then(|x| x.parse().ok());
            i += 2;
        } else if args[i] == "--all" {
            limit = None;
            i += 1;
        } else {
            i += 1;
        }
    }
    let arc = Archive::open(path)?;
    let iter: Box<dyn Iterator<Item = _>> = if by_title {
        Box::new(arc.iter_by_title())
    } else {
        Box::new(arc.iter_by_path())
    };
    let mut count = 0usize;
    for ent in iter {
        let e = ent?;
        let kind = if e.is_redirect() { "R" } else { "A" };
        let ns = char::from(e.namespace());
        println!(
            "{:>6}  {kind} {ns}/{}    {}",
            e.index(),
            e.path(),
            e.title()
        );
        count += 1;
        if let Some(lim) = limit {
            if count >= lim {
                break;
            }
        }
    }
    Ok(())
}

fn cmd_meta(args: &[String]) -> Result<(), Error> {
    let path = args.first().ok_or_else(io_arg)?;
    let arc = Archive::open(path)?;
    if let Some(key) = args.get(1) {
        let bytes = arc.get_metadata(key)?;
        io::stdout().write_all(&bytes)?;
    } else {
        for k in arc.get_metadata_keys() {
            println!("{k}");
        }
    }
    Ok(())
}

fn cmd_get(args: &[String]) -> Result<(), Error> {
    let path = args.first().ok_or_else(io_arg)?;
    let want = args.get(1).ok_or_else(io_arg)?;
    let arc = Archive::open(path)?;
    let entry = if let Some((ns, rest)) = ns_split(want) {
        arc.entry_by_ns_path(ns, rest)?
    } else {
        arc.get_entry_by_path(want)?
    };
    let item = entry.get_item(true)?;
    io::stdout().write_all(item.get_data()?.data())?;
    Ok(())
}

/// Split a "<letter>/<rest>" path into (namespace, rest). Used when the CLI
/// needs to address a non-content namespace explicitly (X, M, W).
fn ns_split(path: &str) -> Option<(u8, &str)> {
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b'/' && bytes[0].is_ascii_uppercase() {
        Some((bytes[0], &path[2..]))
    } else {
        None
    }
}

fn cmd_check(args: &[String]) -> Result<(), Error> {
    let path = args.first().ok_or_else(io_arg)?;
    let arc = Archive::open(path)?;
    if !arc.has_checksum() {
        println!("no checksum present");
        return Ok(());
    }
    let ok = arc.check()?;
    println!("checksum: {}", if ok { "OK" } else { "MISMATCH" });
    if !ok {
        std::process::exit(2);
    }
    Ok(())
}

fn cmd_readall(args: &[String]) -> Result<(), Error> {
    let path = args.first().ok_or_else(io_arg)?;
    let mut hash_md5 = false;
    let mut quiet = false;
    for a in &args[1..] {
        match a.as_str() {
            "--md5" => hash_md5 = true,
            "--quiet" | "-q" => quiet = true,
            _ => {}
        }
    }
    let arc = Archive::open(path)?;
    let n = arc.entry_count();
    let start = std::time::Instant::now();
    let mut articles = 0u64;
    let mut redirects = 0u64;
    let mut total_bytes: u64 = 0;
    let mut hasher = if hash_md5 {
        Some(md5::Md5::new())
    } else {
        None
    };
    for i in 0..n {
        let entry = arc.entry_by_url_index(i)?;
        if entry.is_redirect() {
            redirects += 1;
            continue;
        }
        articles += 1;
        let item = entry.get_item(false)?;
        let blob = item.get_data()?;
        total_bytes += blob.size() as u64;
        if let Some(h) = hasher.as_mut() {
            h.update(blob.data());
        }
    }
    let elapsed = start.elapsed();
    if !quiet {
        println!("entries:        {n}");
        println!("articles:       {articles}");
        println!("redirects:      {redirects}");
        println!("total_bytes:    {total_bytes}");
        println!("elapsed_ms:     {}", elapsed.as_millis());
        let mb = total_bytes as f64 / (1024.0 * 1024.0);
        let secs = elapsed.as_secs_f64();
        if secs > 0.0 {
            println!("throughput_MBs: {:.2}", mb / secs);
        }
        if let Some(h) = hasher {
            let d: [u8; 16] = h.finalize().into();
            println!("blob_md5:       {}", hex_lower(&d));
        }
    }
    Ok(())
}

fn cmd_mimes(args: &[String]) -> Result<(), Error> {
    let path = args.first().ok_or_else(io_arg)?;
    let arc = Archive::open(path)?;
    for (i, m) in arc.mime_list().iter().enumerate() {
        println!("{:>4}  {m}", i);
    }
    Ok(())
}

fn io_arg() -> Error {
    Error::Io(io::Error::new(
        io::ErrorKind::InvalidInput,
        "missing argument",
    ))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}
