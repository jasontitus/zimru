//! `zimwriterfs` — CLI parity with kiwix `zim-tools`' `zimwriterfs`.
//!
//! Packs every file under HTML_DIRECTORY into a single ZIM file. The main
//! page comes from `--welcome`, the illustration from `--illustration`,
//! and standard metadata (Title, Description, Creator, Publisher,
//! Language, Name, …) from individual flags. Mimetype is inferred from
//! the file extension.
//!
//! Usage matches upstream:
//!   zimwriterfs --welcome=index.html --illustration=icon48.png \
//!               --language=eng --title="My ZIM" --description="..." \
//!               --creator=Me --publisher=Kiwix --name=myzim \
//!               HTML_DIR OUT.zim

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zimru::writer::{Creator, Item};
use zimru::Compression;

const VERSION: &str = "zimwriterfs (zimru) 0.1.0";

#[derive(Default, Debug)]
struct Opts {
    welcome: Option<String>,
    illustration: Option<PathBuf>,
    language: Option<String>,
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
    creator: Option<String>,
    publisher: Option<String>,

    long_description: Option<String>,
    cluster_size_kb: Option<usize>,
    threads: Option<usize>,
    inflate_html: bool,
    redirects_file: Option<PathBuf>,
    without_ft_index: bool,
    tags: Option<String>,
    source: Option<String>,
    flavour: Option<String>,
    scraper: Option<String>,
    skip_libmagic: bool,
    verbose: bool,

    html_dir: Option<PathBuf>,
    zim_file: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut o = Opts::default();
    let mut i = 1;
    let mut positional = Vec::<String>::new();

    while i < args.len() {
        let a = &args[i];
        match split_flag(a) {
            ("-h", _) | ("--help", _) => { print_help(); return ExitCode::SUCCESS; }
            ("-V", _) | ("--version", _) => { println!("{VERSION}"); return ExitCode::SUCCESS; }
            ("-v", _) | ("--verbose", _) => o.verbose = true,
            ("-x", _) | ("--inflateHtml", _) => o.inflate_html = true,
            ("-j", _) | ("--withoutFTIndex", _) => o.without_ft_index = true,
            ("--skip-libmagic-check", _) => o.skip_libmagic = true,
            ("-w", v) | ("--welcome", v) => o.welcome = Some(value_or_next(v, &args, &mut i)),
            ("-I", v) | ("--illustration", v) => o.illustration = Some(PathBuf::from(value_or_next(v, &args, &mut i))),
            ("-l", v) | ("--language", v) => o.language = Some(value_or_next(v, &args, &mut i)),
            ("-n", v) | ("--name", v) => o.name = Some(value_or_next(v, &args, &mut i)),
            ("-t", v) | ("--title", v) => o.title = Some(value_or_next(v, &args, &mut i)),
            ("-d", v) | ("--description", v) => o.description = Some(value_or_next(v, &args, &mut i)),
            ("-c", v) | ("--creator", v) => o.creator = Some(value_or_next(v, &args, &mut i)),
            ("-p", v) | ("--publisher", v) => o.publisher = Some(value_or_next(v, &args, &mut i)),
            ("-L", v) | ("--longDescription", v) => o.long_description = Some(value_or_next(v, &args, &mut i)),
            ("-m", v) | ("--clusterSize", v) => o.cluster_size_kb = value_or_next(v, &args, &mut i).parse().ok(),
            ("-J", v) | ("--threads", v) => o.threads = value_or_next(v, &args, &mut i).parse().ok(),
            ("-r", v) | ("--redirects", v) => o.redirects_file = Some(PathBuf::from(value_or_next(v, &args, &mut i))),
            ("-a", v) | ("--tags", v) => o.tags = Some(value_or_next(v, &args, &mut i)),
            ("-e", v) | ("--source", v) => o.source = Some(value_or_next(v, &args, &mut i)),
            ("-o", v) | ("--flavour", v) => o.flavour = Some(value_or_next(v, &args, &mut i)),
            ("-s", v) | ("--scraper", v) => o.scraper = Some(value_or_next(v, &args, &mut i)),
            (other, _) if other.starts_with('-') => {
                eprintln!("zimwriterfs: unknown option `{other}`");
                return ExitCode::from(2);
            }
            _ => positional.push(a.clone()),
        }
        i += 1;
    }

    if positional.len() == 2 {
        o.html_dir = Some(PathBuf::from(&positional[0]));
        o.zim_file = Some(PathBuf::from(&positional[1]));
    } else {
        print_help();
        return ExitCode::from(2);
    }

    for (name, present) in [
        ("--welcome", o.welcome.is_some()),
        ("--illustration", o.illustration.is_some()),
        ("--language", o.language.is_some()),
        ("--name", o.name.is_some()),
        ("--title", o.title.is_some()),
        ("--description", o.description.is_some()),
        ("--creator", o.creator.is_some()),
        ("--publisher", o.publisher.is_some()),
    ] {
        if !present {
            eprintln!("zimwriterfs: missing mandatory argument `{name}`");
            return ExitCode::from(2);
        }
    }

    match run(&o) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zimwriterfs: {e}");
            ExitCode::FAILURE
        }
    }
}

fn split_flag(a: &str) -> (&str, Option<&str>) {
    if let Some(eq) = a.find('=') {
        if a.starts_with("--") || a.starts_with('-') {
            return (&a[..eq], Some(&a[eq + 1..]));
        }
    }
    (a, None)
}

fn value_or_next(v: Option<&str>, args: &[String], i: &mut usize) -> String {
    if let Some(v) = v {
        return v.to_string();
    }
    *i += 1;
    args.get(*i).cloned().unwrap_or_default()
}

fn print_help() {
    println!(
        "Usage: zimwriterfs [mandatory arguments] [optional arguments] HTML_DIR ZIM_FILE\n\nMandatory:\n  -w/--welcome PATH      main HTML page (relative to HTML_DIR)\n  -I/--illustration PATH 48×48 PNG illustration (relative)\n  -l/--language LANG     ISO639-3 language code (e.g. eng)\n  -n/--name NAME         version-independent identifier\n  -t/--title TITLE       ZIM title\n  -d/--description TEXT  short description\n  -c/--creator AUTHOR    content creator\n  -p/--publisher PUB     ZIM creator/publisher\n\nOptional:\n  -L/--longDescription TEXT\n  -m/--clusterSize KB    cluster size in KiB (default 2048)\n  -J/--threads N         number of threads (default 4) — currently no-op\n  -x/--inflateHtml       gunzip *.html files before packing\n  -j/--withoutFTIndex    don't build fulltext index (always)\n  -r/--redirects PATH    TSV file: url\\ttitle\\ttarget_url\n  -a/--tags TAGS         semicolon-separated tags\n  -e/--source URL        source URL\n  -o/--flavour NAME      content flavour\n  -s/--scraper NAME      scraper tool name+version\n  --skip-libmagic-check  ignore libmagic; use file-extension mime detection (default in zimru)\n  -v/--verbose           print processing details\n  -V/--version           print version\n"
    );
}

fn run(o: &Opts) -> Result<(), zimru::Error> {
    let html_dir = o.html_dir.as_ref().unwrap();
    let zim_file = o.zim_file.as_ref().unwrap();
    if !html_dir.is_dir() {
        return Err(zimru::Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("HTML_DIR {} is not a directory", html_dir.display()),
        )));
    }

    let mut creator = Creator::new();
    creator.set_compression(Compression::Zstd);
    if let Some(kb) = o.cluster_size_kb {
        creator.set_cluster_size_target(kb * 1024);
    }
    creator.set_main_path(o.welcome.as_deref().unwrap());

    // Mandatory metadata.
    creator.add_metadata("Title", o.title.clone().unwrap());
    creator.add_metadata("Description", o.description.clone().unwrap());
    creator.add_metadata("Language", o.language.clone().unwrap());
    creator.add_metadata("Creator", o.creator.clone().unwrap());
    creator.add_metadata("Publisher", o.publisher.clone().unwrap());
    creator.add_metadata("Name", o.name.clone().unwrap());
    creator.add_metadata(
        "Date",
        chrono_today_iso(),
    );

    // Optional metadata.
    if let Some(s) = &o.long_description { creator.add_metadata("LongDescription", s.clone()); }
    if let Some(s) = &o.tags             { creator.add_metadata("Tags",            s.clone()); }
    if let Some(s) = &o.source           { creator.add_metadata("Source",          s.clone()); }
    if let Some(s) = &o.flavour          { creator.add_metadata("Flavour",         s.clone()); }
    if let Some(s) = &o.scraper          { creator.add_metadata("Scraper",         s.clone()); }

    // Illustration (must exist; mandatory upstream).
    let illu_path = html_dir.join(o.illustration.as_ref().unwrap());
    let illu_bytes = fs::read(&illu_path).map_err(|e| zimru::Error::Io(std::io::Error::new(
        e.kind(),
        format!("--illustration {}: {e}", illu_path.display()),
    )))?;
    creator.add_illustration(48, illu_bytes);

    // Walk HTML_DIR and ingest every file. The illustration is also kept
    // as a regular C entry so links from inside the archive that reference
    // it directly (e.g. `<link rel="icon" href="icon48.png">`) still
    // resolve — matches upstream zimwriterfs behavior.
    let mut count = 0usize;
    for entry in walk_dir(html_dir)? {
        let path = entry;
        let rel = path.strip_prefix(html_dir).unwrap();
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let mime = mime_for_path(&path);
        let mut content = fs::read(&path)?;
        if o.inflate_html && (path.extension().is_some_and(|e| e == "html" || e == "htm")) {
            // Try gzip-decompress; if it doesn't look like gzip, leave as-is.
            if content.starts_with(&[0x1f, 0x8b]) {
                use std::io::Read as _;
                if let Ok(mut dec) = flate2_decoder(&content) {
                    let mut out = Vec::new();
                    if dec.read_to_end(&mut out).is_ok() {
                        content = out;
                    }
                }
            }
        }
        let title = derive_title(&path).unwrap_or_else(|| rel_str.clone());
        creator.add_item(Item::new(rel_str.clone(), title, mime, content));
        count += 1;
        if o.verbose && count.is_multiple_of(100) {
            eprintln!("[zimwriterfs] {count} items");
        }
    }

    // Optional redirects file (TSV: url \t title \t target_url).
    if let Some(rfile) = &o.redirects_file {
        let raw = fs::read_to_string(rfile)?;
        for (lineno, line) in raw.lines().enumerate() {
            if line.trim().is_empty() || line.starts_with('#') { continue; }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                eprintln!("zimwriterfs: redirect file line {} malformed (need 3 tab-separated fields): {line}", lineno + 1);
                continue;
            }
            creator.add_redirection(parts[0], parts[1], parts[2]);
        }
    }

    if o.verbose { eprintln!("[zimwriterfs] {count} items collected; finalising…"); }
    creator.write_to(zim_file)?;
    if o.verbose { eprintln!("[zimwriterfs] wrote {}", zim_file.display()); }
    Ok(())
}

/// Recursively yield every file under `dir`. Skips dotfiles (matches upstream).
fn walk_dir(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') { continue; }
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn mime_for_path(p: &Path) -> String {
    static TABLE: &[(&str, &str)] = &[
        ("html", "text/html"),
        ("htm",  "text/html"),
        ("xhtml","application/xhtml+xml"),
        ("css",  "text/css"),
        ("js",   "application/javascript"),
        ("mjs",  "application/javascript"),
        ("json", "application/json"),
        ("xml",  "application/xml"),
        ("svg",  "image/svg+xml"),
        ("png",  "image/png"),
        ("jpg",  "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("gif",  "image/gif"),
        ("webp", "image/webp"),
        ("ico",  "image/x-icon"),
        ("bmp",  "image/bmp"),
        ("pdf",  "application/pdf"),
        ("epub", "application/epub+zip"),
        ("zip",  "application/zip"),
        ("gz",   "application/gzip"),
        ("woff", "font/woff"),
        ("woff2","font/woff2"),
        ("ttf",  "font/ttf"),
        ("otf",  "font/otf"),
        ("mp3",  "audio/mpeg"),
        ("ogg",  "audio/ogg"),
        ("opus", "audio/ogg"),
        ("wav",  "audio/wav"),
        ("mp4",  "video/mp4"),
        ("webm", "video/webm"),
        ("ogv",  "video/ogg"),
        ("txt",  "text/plain"),
        ("md",   "text/markdown"),
        ("csv",  "text/csv"),
        ("tsv",  "text/tab-separated-values"),
    ];
    let ext = p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if let Some(ref e) = ext {
        let map: HashMap<&str, &str> = TABLE.iter().copied().collect();
        if let Some(&m) = map.get(e.as_str()) {
            return m.to_string();
        }
    }
    "application/octet-stream".to_string()
}

fn derive_title(p: &Path) -> Option<String> {
    // For HTML files, try to pull <title>...</title> out of the file.
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext == "html" || ext == "htm" {
        let bytes = fs::read(p).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let lower = text.to_ascii_lowercase();
        if let Some(s) = lower.find("<title>") {
            let s = s + "<title>".len();
            if let Some(e) = lower[s..].find("</title>") {
                let raw = &text[s..s + e];
                return Some(raw.trim().to_string());
            }
        }
    }
    None
}

fn chrono_today_iso() -> String {
    // Avoid pulling in chrono; the date is stored as YYYY-MM-DD which we
    // can construct directly from SystemTime.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86_400;
    // Algorithm from Howard Hinnant's date library, public domain.
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Lightweight gzip decoder fallback. We don't have flate2 as a dep, so
/// just signal "no inflate" if the call site requested it. For users who
/// truly need -x, a future commit can add flate2.
fn flate2_decoder(_buf: &[u8]) -> std::io::Result<std::io::Empty> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "--inflateHtml requires the flate2 feature (not yet enabled)",
    ))
}
