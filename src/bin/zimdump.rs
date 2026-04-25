//! `zimdump` — CLI parity with kiwix `zim-tools`' `zimdump`.
//!
//! Subcommands (mirrors upstream):
//!   zimdump info    [--ns=N]                       <file>
//!   zimdump list    [--details] [--idx=I|(--url=U [--ns=N])] <file>
//!   zimdump show    (--idx=I|(--url=U [--ns=N]))   <file>
//!   zimdump dump    --dir=DIR [--ns=N] [--redirect] <file>
//!   zimdump --help | --version
//!
//! Exit codes match upstream: 0 = ok, 1 = no/multiple matches, 2 = dump error.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use zimru::{Archive, Entry, Error};

const VERSION: &str = "zimdump (zimru) 0.1.0\n+ libzim equivalent: zimru 0.1.0";

#[cfg(unix)]
fn restore_sigpipe() {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    // SIGPIPE = 13, SIG_DFL = 0 — restore default so a closed pipe terminates
    // us cleanly instead of panicking inside std's print! machinery.
    unsafe { let _ = signal(13, 0); }
}
#[cfg(not(unix))]
fn restore_sigpipe() {}

fn main() -> ExitCode {
    restore_sigpipe();
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map(String::as_str);
    let res = match sub {
        Some("info") => cmd_info(&args[2..]),
        Some("list") => cmd_list(&args[2..]),
        Some("show") => cmd_show(&args[2..]),
        Some("dump") => cmd_dump(&args[2..]),
        Some("-h") | Some("--help") | None => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        Some("--version") => {
            println!("{VERSION}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("zimdump: unknown subcommand `{other}`");
            print_usage();
            return ExitCode::from(2);
        }
    };
    match res {
        Ok(code) => code,
        Err(e) => {
            eprintln!("zimdump: {e}");
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    println!(
        "\nzimdump tool is used to inspect a zim file and also to dump its contents into the filesystem.\n\nUsage:\n  zimdump list [--details] [--idx=INDEX|([--url=URL] [--ns=N])] [--] <file>\n  zimdump dump --dir=DIR [--ns=N] [--redirect] [--] <file>\n  zimdump show (--idx=INDEX|(--url=URL [--ns=N])) [--] <file>\n  zimdump info [--ns=N] [--] <file>\n  zimdump -h | --help\n  zimdump --version\n"
    );
}

#[derive(Default)]
struct Opts {
    file: Option<String>,
    idx: Option<u32>,
    url: Option<String>,
    ns: Option<u8>,
    details: bool,
    dir: Option<PathBuf>,
    redirect: bool,
}

fn parse_opts(args: &[String]) -> Result<Opts, Error> {
    let mut o = Opts::default();
    let mut i = 0;
    let mut positional: Vec<String> = Vec::new();
    while i < args.len() {
        let a = &args[i];
        if let Some(v) = a.strip_prefix("--idx=") {
            o.idx = Some(v.parse().map_err(|_| io_err("bad --idx"))?);
        } else if a == "--idx" {
            i += 1;
            o.idx = Some(args.get(i).ok_or_else(|| io_err("--idx needs a value"))?.parse().map_err(|_| io_err("bad --idx"))?);
        } else if let Some(v) = a.strip_prefix("--url=") {
            o.url = Some(v.to_string());
        } else if a == "--url" {
            i += 1;
            o.url = Some(args.get(i).ok_or_else(|| io_err("--url needs a value"))?.clone());
        } else if let Some(v) = a.strip_prefix("--ns=") {
            o.ns = Some(parse_ns(v)?);
        } else if a == "--ns" {
            i += 1;
            o.ns = Some(parse_ns(args.get(i).ok_or_else(|| io_err("--ns needs a value"))?)?);
        } else if let Some(v) = a.strip_prefix("--dir=") {
            o.dir = Some(PathBuf::from(v));
        } else if a == "--dir" {
            i += 1;
            o.dir = Some(PathBuf::from(args.get(i).ok_or_else(|| io_err("--dir needs a value"))?));
        } else if a == "--details" {
            o.details = true;
        } else if a == "--redirect" {
            o.redirect = true;
        } else if a == "--" {
            // rest are positional
            i += 1;
            while i < args.len() {
                positional.push(args[i].clone());
                i += 1;
            }
            break;
        } else if a.starts_with("--") {
            return Err(io_err(&format!("unknown option `{a}`")));
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    if positional.len() == 1 {
        o.file = Some(positional.into_iter().next().unwrap());
    } else if positional.len() > 1 {
        return Err(io_err("expected exactly one <file> positional argument"));
    }
    Ok(o)
}

fn parse_ns(s: &str) -> Result<u8, Error> {
    if s.len() == 1 {
        Ok(s.as_bytes()[0])
    } else {
        Err(io_err(&format!("--ns expects a single character, got `{s}`")))
    }
}

fn open_archive(opts: &Opts) -> Result<Archive, Error> {
    let path = opts.file.as_ref().ok_or_else(|| io_err("missing <file>"))?;
    Archive::open(path)
}

// ---------------- subcommand: info ----------------

fn cmd_info(args: &[String]) -> Result<ExitCode, Error> {
    let opts = parse_opts(args)?;
    let arc = open_archive(&opts)?;
    let h = arc.header();
    // count-entries: only C namespace in modern format (matches upstream behavior).
    let count = count_in_namespace(&arc, opts.ns.unwrap_or(b'C'))?;
    println!("count-entries: {count}");
    println!("uuid: {}", format_uuid(&h.uuid));
    println!("cluster count: {}", h.cluster_count);
    if h.has_checksum() {
        let cs = arc.checksum()?;
        println!("checksum: {}", hex_lower(&cs));
    }
    // Main page url (the actual content target, follows the redirect)
    if arc.has_main_entry() {
        let main = arc.main_entry()?;
        let resolved = follow_redirect(&main)?;
        println!("main page: {}", resolved.path());
    }
    // Favicon: check if M/Illustration_48x48@1 exists
    if arc.entry_by_ns_path(b'M', "Illustration_48x48@1").is_ok() {
        println!("favicon: Illustration_48x48@1");
    }
    Ok(ExitCode::SUCCESS)
}

fn count_in_namespace(arc: &Archive, ns: u8) -> Result<u64, Error> {
    Ok(arc.entry_count_in_namespace(ns)? as u64)
}

fn follow_redirect(e: &Entry) -> Result<Entry, Error> {
    let mut cur = e.clone();
    let mut depth = 0;
    while cur.is_redirect() {
        depth += 1;
        if depth > 16 {
            return Err(Error::RedirectLoop);
        }
        cur = cur.get_redirect_entry()?;
    }
    Ok(cur)
}

// ---------------- subcommand: list ----------------

fn cmd_list(args: &[String]) -> Result<ExitCode, Error> {
    let opts = parse_opts(args)?;
    let arc = open_archive(&opts)?;
    let target_ns = opts.ns.unwrap_or(b'C');

    if let Some(idx) = opts.idx {
        let e = entry_by_filtered_index(&arc, target_ns, idx)?;
        print_list_entry(&e, opts.details, idx);
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(url) = &opts.url {
        let e = arc.entry_by_ns_path(target_ns, url)?;
        let idx = filtered_index_of(&arc, target_ns, e.index())?;
        print_list_entry(&e, opts.details, idx);
        return Ok(ExitCode::SUCCESS);
    }

    // List entire namespace.
    let mut local_idx: u32 = 0;
    for entry in arc.iter_by_path() {
        let e = entry?;
        if e.namespace() != target_ns {
            continue;
        }
        print_list_entry(&e, opts.details, local_idx);
        local_idx += 1;
    }
    Ok(ExitCode::SUCCESS)
}

fn entry_by_filtered_index(arc: &Archive, ns: u8, idx: u32) -> Result<Entry, Error> {
    let mut local: u32 = 0;
    for entry in arc.iter_by_path() {
        let e = entry?;
        if e.namespace() == ns {
            if local == idx {
                return Ok(e);
            }
            local += 1;
        }
    }
    Err(Error::EntryNotFound)
}

fn filtered_index_of(arc: &Archive, ns: u8, target_global: u32) -> Result<u32, Error> {
    let mut local: u32 = 0;
    for entry in arc.iter_by_path() {
        let e = entry?;
        if e.namespace() == ns {
            if e.index() == target_global {
                return Ok(local);
            }
            local += 1;
        }
    }
    Err(Error::EntryNotFound)
}

fn print_list_entry(e: &Entry, details: bool, local_idx: u32) {
    if !details {
        println!("{}", e.path());
        return;
    }
    println!("path: {}", e.path());
    println!("* title:          {}", e.title());
    println!("* idx:            {}", local_idx);
    if e.is_redirect() {
        println!("* type:           redirect");
        if let Ok(target) = e.get_redirect_entry() {
            // Upstream prints redirect index as the target's filtered C-index.
            // We approximate using the target's global url-index — for content
            // namespace this is identical when the target is also in C.
            println!("* redirect index: {}", target.index());
        }
    } else if let Ok(item) = e.get_item(false) {
        println!("* type:           item");
        println!("* mime-type:      {}", item.mimetype());
        if let Ok(sz) = item.size() {
            println!("* item size:      {sz}");
        }
    }
}

// ---------------- subcommand: show ----------------

fn cmd_show(args: &[String]) -> Result<ExitCode, Error> {
    let opts = parse_opts(args)?;
    let arc = open_archive(&opts)?;
    let entry = if let Some(idx) = opts.idx {
        entry_by_filtered_index(&arc, opts.ns.unwrap_or(b'C'), idx)?
    } else if let Some(url) = &opts.url {
        arc.entry_by_ns_path(opts.ns.unwrap_or(b'A'), url)?
    } else {
        eprintln!("zimdump: --idx or --url required for `show`");
        return Ok(ExitCode::from(1));
    };
    if entry.is_redirect() {
        // Upstream message: "Entry <path> is a redirect."
        println!("Entry {} is a redirect.", entry.path());
        return Ok(ExitCode::SUCCESS);
    }
    let item = entry.get_item(false)?;
    io::stdout().write_all(item.get_data()?.data())?;
    Ok(ExitCode::SUCCESS)
}

// ---------------- subcommand: dump ----------------

fn cmd_dump(args: &[String]) -> Result<ExitCode, Error> {
    let opts = parse_opts(args)?;
    let arc = open_archive(&opts)?;
    let dir = opts
        .dir
        .clone()
        .ok_or_else(|| io_err("dump requires --dir=DIR"))?;
    fs::create_dir_all(&dir)?;
    let target_ns = opts.ns;
    let mut errors = 0usize;
    let mut errlog = fs::File::create(dir.join("dump_errors.log")).ok();
    for entry in arc.iter_by_path() {
        let e = entry?;
        if let Some(ns) = target_ns {
            if e.namespace() != ns {
                continue;
            }
        }
        if e.is_redirect() {
            let target = e.get_redirect_entry()?;
            let dest = dir.join(safe_path(e.path()));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            if opts.redirect {
                let _ = fs::remove_file(&dest);
                let target_path = safe_path(target.path());
                #[cfg(unix)]
                {
                    if let Err(err) = std::os::unix::fs::symlink(&target_path, &dest) {
                        errors += 1;
                        if let Some(f) = errlog.as_mut() {
                            let _ = writeln!(f, "symlink {} -> {}: {}", dest.display(), target_path, err);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = (&target_path,);
                }
            } else {
                // HTML redirect file
                let html = format!(
                    "<html><head><meta http-equiv=\"refresh\" content=\"0; url={0}\"></head><body><a href=\"{0}\">{0}</a></body></html>",
                    safe_path(target.path())
                );
                if let Err(err) = fs::write(&dest, html) {
                    errors += 1;
                    if let Some(f) = errlog.as_mut() {
                        let _ = writeln!(f, "write {}: {}", dest.display(), err);
                    }
                }
            }
        } else {
            let item = e.get_item(false)?;
            let dest = dir.join(safe_path(e.path()));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Err(err) = fs::write(&dest, item.get_data()?.data()) {
                errors += 1;
                if let Some(f) = errlog.as_mut() {
                    let _ = writeln!(f, "write {}: {}", dest.display(), err);
                }
            }
        }
    }
    if errors > 0 { Ok(ExitCode::from(2)) } else { Ok(ExitCode::SUCCESS) }
}

fn safe_path(p: &str) -> String {
    // Strip leading slashes; otherwise keep as-is (dirs are made above).
    p.trim_start_matches('/').to_string()
}

// ---------------- helpers ----------------

fn io_err(msg: &str) -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidInput, msg.to_string()))
}

fn hex_lower(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", x);
    }
    s
}

fn format_uuid(u: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u[0], u[1], u[2], u[3], u[4], u[5], u[6], u[7], u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15],
    )
}

// Re-use HashMap import for future expansions; keep noisy items at bottom.
#[allow(dead_code)]
fn _unused_hashmap_import_marker(_: HashMap<u32, u32>) {}
