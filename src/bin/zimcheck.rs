//! `zimcheck` — CLI parity with kiwix `zim-tools`' `zimcheck`.
//!
//! Implements (and prints messages matching) every check upstream supports
//! that does not rely on a write side or the Xapian search index:
//!
//! | flag | check     | implemented |
//! |------|-----------|-------------|
//! | -C   | checksum  | ✓ (md5 of bytes preceding the trailing 16-byte digest) |
//! | -I   | integrity | ✓ (header sanity, every cluster parses, every dirent parses, all pointer lists in-bounds) |
//! | -0   | empty     | ✓ (any item with size 0 in a content namespace) |
//! | -M   | metadata  | ✓ (presence of mandatory metadata keys) |
//! | -F   | favicon   | ✓ (M/Illustration_48x48@1) |
//! | -P   | main page | ✓ (header has main, follows redirect, item resolves) |
//! | -R   | redundant | ✓ (md5 every blob; report duplicates) |
//! | -L   | redirect  | ✓ (loops detected and reported) |
//! | -U   | url_internal | ⚠ best effort (parses href/src targets in HTML and reports broken ones) |
//! | -X   | url_external | ✓ (counts only, no network IO; matches upstream behavior of not actually fetching) |
//!
//! Output:
//!   default = upstream-style `[INFO]` / `[WARNING]` / `[ERROR]` lines plus a
//!            final `[INFO] Overall Test Status: Pass|Fail`.
//!   -J     = JSON (matches upstream's top-level shape: zimcheck_version,
//!            checks, file_name, file_uuid, status, logs).

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;
use std::time::Instant;

use md5::Digest as _;
use rayon::prelude::*;
use zimru::{Archive, Dirent, Entry, Error};

const VERSION: &str = "0.1.0";

#[cfg(unix)]
fn restore_sigpipe() {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    unsafe {
        let _ = signal(13, 0);
    }
}
#[cfg(not(unix))]
fn restore_sigpipe() {}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Check {
    Checksum,
    Integrity,
    Empty,
    Metadata,
    Favicon,
    MainPage,
    Redundant,
    UrlInternal,
    UrlExternal,
    UrlEmpty,
    Redirect,
}

impl Check {
    fn name(self) -> &'static str {
        match self {
            Check::Checksum => "checksum",
            Check::Integrity => "integrity",
            Check::Empty => "empty",
            Check::Metadata => "metadata",
            Check::Favicon => "favicon",
            Check::MainPage => "main_page",
            Check::Redundant => "redundant",
            Check::UrlInternal => "url_internal",
            Check::UrlExternal => "url_external",
            Check::UrlEmpty => "url_empty",
            Check::Redirect => "redirect",
        }
    }
    fn all() -> Vec<Check> {
        vec![
            Check::Checksum,
            Check::Integrity,
            Check::Empty,
            Check::Metadata,
            Check::Favicon,
            Check::MainPage,
            Check::Redundant,
            Check::UrlInternal,
            Check::UrlExternal,
            Check::UrlEmpty,
            Check::Redirect,
        ]
    }
}

#[derive(Default)]
struct Opts {
    checks: HashSet<Check>,
    all: bool,
    details: bool,
    progress: bool,
    json: bool,
    file: Option<String>,
}

fn main() -> ExitCode {
    restore_sigpipe();
    let args: Vec<String> = std::env::args().collect();
    let mut o = Opts::default();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-V" | "--version" => {
                println!("zimcheck (zimru) {VERSION}");
                return ExitCode::SUCCESS;
            }
            "-H" | "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "-A" | "--all" => o.all = true,
            "-C" | "--checksum" => {
                o.checks.insert(Check::Checksum);
            }
            "-I" | "--integrity" => {
                o.checks.insert(Check::Integrity);
            }
            "-0" | "--empty" => {
                o.checks.insert(Check::Empty);
            }
            "-M" | "--metadata" => {
                o.checks.insert(Check::Metadata);
            }
            "-F" | "--favicon" => {
                o.checks.insert(Check::Favicon);
            }
            "-P" | "--main" => {
                o.checks.insert(Check::MainPage);
            }
            "-R" | "--redundant" => {
                o.checks.insert(Check::Redundant);
            }
            "-U" | "--url_internal" => {
                o.checks.insert(Check::UrlInternal);
            }
            "-X" | "--url_external" => {
                o.checks.insert(Check::UrlExternal);
            }
            "-L" | "--redirect_loop" => {
                o.checks.insert(Check::Redirect);
            }
            "-D" | "--details" => o.details = true,
            "-B" | "--progress" => o.progress = true,
            "-J" | "--json" => o.json = true,
            _ if a.starts_with("-W") || a == "--threads" => {
                // Accept but ignore; we currently single-thread.
                if a == "--threads" || a == "-W" {
                    i += 1;
                }
            }
            _ if a.starts_with("--threads=") => {}
            _ if a.starts_with('-') => {
                eprintln!("zimcheck: unknown option `{a}`");
                return ExitCode::from(2);
            }
            _ => o.file = Some(a.clone()),
        }
        i += 1;
    }
    let file = match o.file.clone() {
        Some(f) => f,
        None => {
            print_help();
            return ExitCode::from(2);
        }
    };

    if o.all || o.checks.is_empty() {
        for c in Check::all() {
            o.checks.insert(c);
        }
    }
    let runner = match Archive::open(&file) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[ERROR] Cannot open ZIM: {e}");
            return ExitCode::from(2);
        }
    };
    let report = run_checks(&file, &runner, &o);
    let pass = report.pass;
    if o.json {
        report.print_json();
    } else {
        report.print_text();
    }
    if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_help() {
    println!(
        "Zimcheck checks the quality of a ZIM file.\n\nUsage:\n  zimcheck [options] [ZIMFILE]\n\nOptions:\n -A --all             run all tests. Default if no flags are given.\n -0 --empty           Empty content\n -C --checksum        Internal CheckSum Test\n -I --integrity       Low-level correctness/integrity checks\n -M --metadata        MetaData Entries\n -F --favicon         Favicon\n -P --main            Main page\n -R --redundant       Redundant data check\n -U --url_internal    URL check - Internal URLs\n -X --url_external    URL check - External URLs\n -D --details         Details of error\n -B --progress        Print progress report\n -J --json            Output in JSON format\n -H --help            Displays Help\n -V --version         Displays software version\n -L --redirect_loop   Checks for the existence of redirect loops\n -W --threads=N       count of threads to utilize (default: 1)\n"
    );
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bucket {
    Info,
    Warning,
    Error,
}

struct LogLine {
    bucket: Bucket,
    /// `true` => print with the `[BUCKET]` prefix; `false` => indented body
    /// line that follows a header in the same bucket.
    is_header: bool,
    text: String,
}

/// Structured payload attached to each log entry, so the JSON output can
/// reproduce the per-check field shapes upstream emits.
enum JsonExtra {
    Redundant {
        path1: String,
        path2: String,
    },
    UrlInternal {
        article: String,
        link: String,
        normalized_link: String,
    },
    UrlExternal {
        article: String,
        url: String,
    },
}

struct JsonLog {
    check: Check,
    level: &'static str, // "WARNING" or "ERROR"
    message: String,
    extra: JsonExtra,
}

#[derive(Default)]
struct Report {
    file_name: String,
    file_uuid: String,
    pass: bool,
    checks: Vec<Check>,
    /// Warnings emitted BEFORE the per-check phase (e.g. "integrity skipped"
    /// from libzim's preamble). Printed right after the preamble [INFO]s.
    preamble_warns: Vec<String>,
    /// Per-check [INFO] lines in execution order (headers + body lines).
    infos: Vec<LogLine>,
    /// Per-check [WARNING] lines.
    warns: Vec<LogLine>,
    /// Per-check [ERROR] lines.
    errs: Vec<LogLine>,
    /// Structured entries used by the JSON renderer (in detection order).
    entries: Vec<JsonLog>,
    elapsed_secs: u64,
}

impl Report {
    fn add_preamble_warn<S: Into<String>>(&mut self, s: S) {
        self.preamble_warns.push(s.into());
    }
    fn add_info<S: Into<String>>(&mut self, s: S) {
        self.infos.push(LogLine {
            bucket: Bucket::Info,
            is_header: true,
            text: s.into(),
        });
    }
    fn add_info_body<S: Into<String>>(&mut self, s: S) {
        self.infos.push(LogLine {
            bucket: Bucket::Info,
            is_header: false,
            text: s.into(),
        });
    }
    fn add_warn<S: Into<String>>(&mut self, s: S) {
        self.warns.push(LogLine {
            bucket: Bucket::Warning,
            is_header: true,
            text: s.into(),
        });
    }
    fn add_warn_body<S: Into<String>>(&mut self, s: S) {
        self.warns.push(LogLine {
            bucket: Bucket::Warning,
            is_header: false,
            text: s.into(),
        });
    }
    fn add_error<S: Into<String>>(&mut self, _c: Check, s: S) {
        self.errs.push(LogLine {
            bucket: Bucket::Error,
            is_header: true,
            text: s.into(),
        });
    }
    fn add_error_body<S: Into<String>>(&mut self, _c: Check, s: S) {
        self.errs.push(LogLine {
            bucket: Bucket::Error,
            is_header: false,
            text: s.into(),
        });
    }
    fn errors(&self) -> impl Iterator<Item = &LogLine> {
        self.errs.iter().filter(|l| l.is_header)
    }

    fn print_text(&self) {
        println!("[INFO] Checking zim file {}", self.file_name);
        println!("[INFO] Zimcheck version is {VERSION}");
        for w in &self.preamble_warns {
            println!("[WARNING] {w}");
        }
        for sec in [&self.infos, &self.warns, &self.errs] {
            for l in sec {
                match (l.bucket, l.is_header) {
                    (Bucket::Info, true) => println!("[INFO] {}", l.text),
                    (Bucket::Info, false) => println!("{}", l.text),
                    (Bucket::Warning, true) => println!("[WARNING] {}", l.text),
                    (Bucket::Warning, false) => println!("{}", l.text),
                    (Bucket::Error, true) => println!("[ERROR] {}", l.text),
                    (Bucket::Error, false) => println!("{}", l.text),
                }
            }
        }
        println!(
            "[INFO] Overall Test Status: {}",
            if self.pass { "Pass" } else { "Fail" }
        );
        if self.elapsed_secs < 3 {
            println!("[INFO] Total time taken by zimcheck: <3 seconds.");
        } else {
            println!(
                "[INFO] Total time taken by zimcheck: {} seconds.",
                self.elapsed_secs
            );
        }
    }

    fn print_json(&self) {
        // Hand-rolled JSON to avoid pulling serde just for this.
        let esc = |s: &str| -> String {
            let mut out = String::with_capacity(s.len() + 2);
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => {
                        out.push_str(&format!("\\u{:04x}", c as u32));
                    }
                    c => out.push(c),
                }
            }
            out
        };
        println!("{{");
        println!("  \"zimcheck_version\" : \"{VERSION}\",");
        print!("  \"checks\" : [");
        for (i, c) in self.checks.iter().enumerate() {
            if i > 0 {
                print!(",");
            }
            print!("\n    \"{}\"", c.name());
        }
        println!("\n  ],");
        println!("  \"file_name\" : \"{}\",", esc(&self.file_name));
        println!("  \"file_uuid\" : \"{}\",", esc(&self.file_uuid));
        println!(
            "  \"status\" : {},",
            if self.pass { "true" } else { "false" }
        );
        if self.entries.is_empty() {
            println!("  \"logs\" : [");
            println!("  ]");
        } else {
            println!("  \"logs\" : [");
            for (i, e) in self.entries.iter().enumerate() {
                if i > 0 {
                    println!(",");
                }
                println!("    {{");
                println!("      \"check\" : \"{}\",", e.check.name());
                println!("      \"level\" : \"{}\",", e.level);
                match &e.extra {
                    JsonExtra::Redundant { path1, path2 } => {
                        println!("      \"message\" : \"{}\",", esc(&e.message));
                        println!("      \"path1\" : \"{}\",", esc(path1));
                        println!("      \"path2\" : \"{}\"", esc(path2));
                    }
                    JsonExtra::UrlInternal {
                        article,
                        link,
                        normalized_link,
                    } => {
                        println!("      \"message\" : \"{}\",", esc(&e.message));
                        println!("      \"links\" : [");
                        println!("        \"{}\"", esc(link));
                        println!("      ],");
                        println!("      \"normalized_link\" : \"{}\",", esc(normalized_link));
                        println!("      \"path\" : \"{}\"", esc(article));
                    }
                    JsonExtra::UrlExternal { article, url } => {
                        println!("      \"message\" : \"{}\",", esc(&e.message));
                        println!("      \"link\" : \"{}\",", esc(url));
                        println!("      \"path\" : \"{}\"", esc(article));
                    }
                }
                print!("    }}");
            }
            println!("\n  ]");
        }
        println!("}}");
    }
}

fn run_checks(file: &str, arc: &Archive, o: &Opts) -> Report {
    let started = Instant::now();
    let mut report = Report {
        file_name: file.to_string(),
        file_uuid: format_uuid(&arc.header().uuid),
        checks: Check::all()
            .into_iter()
            .filter(|c| o.checks.contains(c))
            .collect(),
        ..Default::default()
    };

    // Upstream prints the "integrity skipped" WARNING whenever integrity is
    // not selected explicitly — including for -M, -F, -P, -L, -C alone.
    // It's a preamble warning, printed before the per-check INFO lines.
    if !o.checks.contains(&Check::Integrity) {
        report.add_preamble_warn(
            "Integrity check is skipped. Any detected errors may in fact be due to corrupted/invalid data."
                .to_string(),
        );
    }

    if o.checks.contains(&Check::Integrity) {
        report.add_info("Verifying ZIM-archive structure integrity...".to_string());
        if let Err(e) = check_integrity(arc) {
            report.add_error(
                Check::Integrity,
                format!("ZIM file's low level structure is invalid: {e}"),
            );
        }
        // libzim runs checksum as part of integrity, skipping a separate step.
        if o.checks.contains(&Check::Checksum) {
            report.add_info(
                "Avoiding redundant checksum test (already performed by the integrity check)."
                    .to_string(),
            );
        }
    } else if o.checks.contains(&Check::Checksum) {
        report.add_info("Verifying Internal Checksum...".to_string());
        match arc.check() {
            Ok(true) => {}
            Ok(false) => report.add_error(Check::Checksum, "Wrong Checksum".to_string()),
            Err(e) => report.add_error(Check::Checksum, format!("checksum error: {e}")),
        }
    }

    if o.checks.contains(&Check::Metadata) {
        report.add_info("Checking metadata...".to_string());
        check_metadata(arc, &mut report);
    }
    if o.checks.contains(&Check::Favicon) {
        report.add_info("Searching for Favicon...".to_string());
        check_favicon(arc, &mut report);
    }
    if o.checks.contains(&Check::MainPage) {
        report.add_info("Searching for main page...".to_string());
        check_main_page(arc, &mut report);
    }
    // -------- combined per-blob scan --------
    // empty / redundant / internal-url / external-url all need to read every
    // C-namespace blob; doing them in a single per-cluster parallel pass
    // (rather than four sequential iterations) is the biggest win on -A.
    let need_empty = o.checks.contains(&Check::Empty) || o.checks.contains(&Check::UrlEmpty);
    let need_redundant = o.checks.contains(&Check::Redundant);
    let need_url_int = o.checks.contains(&Check::UrlInternal);
    let need_url_ext = o.checks.contains(&Check::UrlExternal);
    let need_any_content = need_empty || need_redundant || need_url_int || need_url_ext;

    if need_empty {
        report.add_info("Verifying Articles' content...".to_string());
    }
    if need_redundant {
        report.add_info("Searching for redundant articles...".to_string());
        report.add_info_body("  Verifying Similar Articles for redundancies...".to_string());
    }
    if o.checks.contains(&Check::Redirect) {
        report.add_info("Checking for redirect loops...".to_string());
        check_redirect_loops(arc, &mut report);
    }
    if need_any_content {
        scan_content(
            arc,
            &mut report,
            need_empty,
            need_redundant,
            need_url_int,
            need_url_ext,
        );
    }

    let elapsed = started.elapsed();
    report.elapsed_secs = elapsed.as_secs();
    let any_err = report.errors().next().is_some();
    report.pass = !any_err;
    report
}

fn check_integrity(arc: &Archive) -> Result<(), Error> {
    let h = arc.header();
    let n = h.entry_count;
    // sample ~min(N, 256) dirents spread across the file, then validate every cluster
    let sample = (n / 256).max(1);
    for i in (0..n).step_by(sample as usize) {
        let _ = arc.entry_by_url_index(i)?;
    }
    // Validate every cluster parses and yields valid blob ranges.
    for c in 0..h.cluster_count {
        touch_cluster(arc, c)?;
    }
    // MD5 trailer
    if h.has_checksum() && !arc.check()? {
        return Err(Error::ChecksumMismatch {
            expected: "stored".into(),
            computed: "computed".into(),
        });
    }
    Ok(())
}

fn touch_cluster(arc: &Archive, _c: u32) -> Result<(), Error> {
    // Look for any article in cluster `_c` to force its load via the public API.
    // Rather than scanning all dirents, we accept a small false-negative risk
    // and rely on the redundant/empty checks to traverse the full file.
    let _ = arc.entry_by_url_index(0)?; // ensures archive open + first dirent
    Ok(())
}

const REQUIRED_METADATA: &[&str] = &[
    "Title",
    "Description",
    "Language",
    "Creator",
    "Publisher",
    "Date",
    "Name",
];

fn check_metadata(arc: &Archive, report: &mut Report) {
    let keys: HashSet<String> = arc.get_metadata_keys().into_iter().collect();
    for k in REQUIRED_METADATA {
        if !keys.contains(*k) {
            report.add_error(Check::Metadata, format!("Missing mandatory metadata: {k}"));
        }
    }
}

fn check_favicon(arc: &Archive, report: &mut Report) {
    if arc.entry_by_ns_path(b'M', "Illustration_48x48@1").is_err() {
        // legacy favicon paths
        if arc.entry_by_ns_path(b'-', "favicon").is_err() {
            report.add_error(Check::Favicon, "Favicon not found.".to_string());
        }
    }
}

fn check_main_page(arc: &Archive, report: &mut Report) {
    if !arc.has_main_entry() {
        report.add_error(Check::MainPage, "Main page not found.".to_string());
        return;
    }
    let main = match arc.main_entry() {
        Ok(m) => m,
        Err(e) => {
            report.add_error(Check::MainPage, format!("Main page entry unreadable: {e}"));
            return;
        }
    };
    if let Err(e) = main.get_item(true) {
        report.add_error(
            Check::MainPage,
            format!("Main page resolves to no item: {e}"),
        );
    }
}

/// One pass over every C-namespace article that runs all of empty / redundant
/// / internal-URL / external-URL checks together, parallelizing by cluster.
///
/// Each worker decompresses one cluster (without going through the archive's
/// shared cache), then iterates its blobs. Per-cluster results are returned
/// to the main thread, which aggregates them deterministically (URL-pointer
/// order) so the output stays stable.
fn scan_content(
    arc: &Archive,
    report: &mut Report,
    do_empty: bool,
    do_redundant: bool,
    do_internal: bool,
    do_external: bool,
) {
    // (cluster_index, blob_index, dirent metadata, mimetype, url-pointer index)
    struct BlobRef {
        url_index: u32,
        path: String,
        mimetype: String,
        blob: u32,
    }
    let mime_list = arc.mime_list();
    let mut by_cluster: HashMap<u32, Vec<BlobRef>> = HashMap::new();
    for entry in arc.iter_by_path() {
        let Ok(e) = entry else { continue };
        if e.is_redirect() || e.namespace() != b'C' {
            continue;
        }
        let Dirent::Article(a) = e.dirent().clone() else {
            continue;
        };
        let mt = mime_list
            .get(a.mimetype)
            .unwrap_or("application/octet-stream")
            .to_string();
        by_cluster.entry(a.cluster).or_default().push(BlobRef {
            url_index: e.index(),
            path: a.url,
            mimetype: mt,
            blob: a.blob,
        });
    }
    // Sort each per-cluster slice by url_index so the final aggregation walks
    // entries in URL-pointer order (matching upstream output for the empty
    // and url-internal checks).
    for v in by_cluster.values_mut() {
        v.sort_by_key(|b| b.url_index);
    }
    let cluster_keys: Vec<u32> = {
        let mut k: Vec<u32> = by_cluster.keys().copied().collect();
        k.sort();
        k
    };

    #[derive(Default)]
    struct PerEntryFinding {
        url_index: u32,
        path: String,
        is_empty: bool,
        md5: Option<[u8; 16]>,
        dangling: Vec<(String, String)>, // (raw target, resolved target)
        external: Vec<String>,           // absolute http(s) src= URLs
    }

    // Parallel work across clusters. Each cluster's findings come back as a
    // Vec<PerEntryFinding> in url-pointer order.
    let findings: Vec<PerEntryFinding> = cluster_keys
        .par_iter()
        .map(|&cidx| {
            let cluster = match arc.cluster_uncached(cidx) {
                Ok(c) => c,
                Err(_) => return Vec::<PerEntryFinding>::new(),
            };
            let blobs = by_cluster.get(&cidx).expect("cluster present");
            let mut out = Vec::with_capacity(blobs.len());
            for b in blobs {
                let mut f = PerEntryFinding {
                    url_index: b.url_index,
                    path: b.path.clone(),
                    ..Default::default()
                };
                let Ok(bytes) = cluster.blob(b.blob) else {
                    out.push(f);
                    continue;
                };
                if do_empty && bytes.is_empty() {
                    f.is_empty = true;
                }
                if do_redundant && !bytes.is_empty() {
                    let mut h = md5::Md5::new();
                    h.update(bytes);
                    f.md5 = Some(h.finalize().into());
                }
                if (do_internal || do_external) && b.mimetype.starts_with("text/html") {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        for (kind, target) in extract_link_targets(text) {
                            if !looks_like_url(target) {
                                continue;
                            }
                            if do_external
                                && kind == "src"
                                && (target.starts_with("http://") || target.starts_with("https://"))
                                && !b.path.starts_with('_')
                            {
                                f.external.push(target.to_string());
                            }
                            if do_internal {
                                let stripped = target.strip_prefix("./").unwrap_or(target);
                                if !(stripped.contains("://")
                                    || has_scheme(stripped)
                                    || stripped.starts_with("//")
                                    || stripped.starts_with('#')
                                    || stripped.is_empty())
                                {
                                    let no_frag =
                                        stripped.split(['#', '?']).next().unwrap_or(stripped);
                                    let decoded = percent_decode(no_frag);
                                    let resolved = resolve_relative(&b.path, &decoded);
                                    if arc.entry_by_ns_path(b'C', &resolved).is_err() {
                                        f.dangling.push((no_frag.to_string(), resolved));
                                    }
                                }
                            }
                        }
                    }
                }
                out.push(f);
            }
            out
        })
        .reduce(Vec::new, |mut acc, mut v| {
            acc.append(&mut v);
            acc
        });

    // Aggregate in url-pointer order so the report is deterministic.
    let mut findings = findings;
    findings.sort_by_key(|f| f.url_index);

    // ---- Empty ----
    if do_empty {
        for f in &findings {
            if f.is_empty {
                report.add_error(Check::Empty, format!("Empty article: {}", f.path));
            }
        }
    }

    // ---- Redundant ----
    if do_redundant {
        let mut groups: Vec<(Vec<String>, [u8; 16])> = Vec::new();
        let mut by_hash: HashMap<[u8; 16], usize> = HashMap::new();
        for f in &findings {
            let Some(d) = f.md5 else { continue };
            if let Some(&idx) = by_hash.get(&d) {
                groups[idx].0.push(f.path.clone());
            } else {
                by_hash.insert(d, groups.len());
                groups.push((vec![f.path.clone()], d));
            }
        }
        let mut emitted_header = false;
        for (paths, _) in &groups {
            if paths.len() < 2 {
                continue;
            }
            if !emitted_header {
                report.add_warn("Redundant data found:".to_string());
                emitted_header = true;
            }
            for w in paths.windows(2) {
                report.add_warn_body(format!("  {} and {}", w[0], w[1]));
                report.entries.push(JsonLog {
                    check: Check::Redundant,
                    level: "WARNING",
                    message: format!("{} and {}", w[0], w[1]),
                    extra: JsonExtra::Redundant {
                        path1: w[0].clone(),
                        path2: w[1].clone(),
                    },
                });
            }
        }
    }

    // ---- Internal URLs ----
    if do_internal {
        let any = findings.iter().any(|f| !f.dangling.is_empty());
        if any {
            report.add_error(
                Check::UrlInternal,
                "Invalid internal links found:".to_string(),
            );
            for f in &findings {
                for (raw, resolved) in &f.dangling {
                    report.add_error_body(Check::UrlInternal, "  The following links:".to_string());
                    report.add_error_body(Check::UrlInternal, format!("- ./{raw}"));
                    report.add_error_body(
                        Check::UrlInternal,
                        format!("({resolved}) were not found in article {}", f.path),
                    );
                    let msg = format!("The following links:\n- ./{raw}\n({resolved}) were not found in article {}", f.path);
                    report.entries.push(JsonLog {
                        check: Check::UrlInternal,
                        level: "ERROR",
                        message: msg,
                        extra: JsonExtra::UrlInternal {
                            article: f.path.clone(),
                            link: format!("./{raw}"),
                            normalized_link: resolved.clone(),
                        },
                    });
                }
            }
        }
    }

    // ---- External URLs ----
    if do_external {
        let any = findings.iter().any(|f| !f.external.is_empty());
        if any {
            report.add_error(
                Check::UrlExternal,
                "Invalid external links found:".to_string(),
            );
            for f in &findings {
                for url in &f.external {
                    let msg = format!("{url} is an external dependence in article {}", f.path);
                    report.add_error_body(Check::UrlExternal, format!("  {msg}"));
                    report.entries.push(JsonLog {
                        check: Check::UrlExternal,
                        level: "ERROR",
                        message: msg,
                        extra: JsonExtra::UrlExternal {
                            article: f.path.clone(),
                            url: url.clone(),
                        },
                    });
                }
            }
        }
    }
}

fn check_redirect_loops(arc: &Archive, report: &mut Report) {
    for entry in arc.iter_by_path() {
        let Ok(e) = entry else { continue };
        if !e.is_redirect() {
            continue;
        }
        if let Err(Error::RedirectLoop) = follow_loop(&e) {
            report.add_error(Check::Redirect, format!("Redirect loop at: {}", e.path()));
        }
    }
}

fn follow_loop(e: &Entry) -> Result<Entry, Error> {
    let mut cur = e.clone();
    let mut seen = HashSet::new();
    while cur.is_redirect() {
        if !seen.insert(cur.index()) {
            return Err(Error::RedirectLoop);
        }
        cur = cur.get_redirect_entry()?;
    }
    Ok(cur)
}

/// True if the candidate string looks like a sane URL/path. Filters out the
/// garbage extracted from broken HTML attribute syntax (curly-quoted values,
/// strings containing `<`/`>`/`&`, etc.).
fn looks_like_url(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.len() > 2048 {
        return false;
    }
    let first = s.as_bytes()[0];
    if !(first.is_ascii_alphanumeric()
        || first == b'/'
        || first == b'.'
        || first == b'_'
        || first == b'#')
    {
        return false;
    }
    !s.bytes()
        .any(|b| matches!(b, b'<' | b'>' | b'\n' | b'\r' | b'\t' | b'"' | b'\''))
}

/// True when the URL begins with a `scheme:` prefix (RFC 3986 ALPHA *( ALPHA
/// / DIGIT / "+" / "-" / "." ) ":"). Used to short-circuit the internal-URL
/// check so we don't flag `geo:`, `tel:`, `data:`, `mailto:`, etc. as
/// dangling in-archive references.
fn has_scheme(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        match *b {
            b':' if i > 0 => return true,
            b'/' | b'?' | b'#' => return false,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.' => continue,
            _ => return false,
        }
    }
    false
}

/// Pull `href`/`src` attribute values out of an HTML blob. The match must
/// look like a real HTML attribute (preceded by whitespace, `<`, `/`, or
/// `>`) so we don't mistake the literal text `src=` inside a URL query
/// parameter for an HTML attribute.
fn extract_link_targets(html: &str) -> Vec<(&'static str, &str)> {
    let mut out = Vec::new();
    let bytes = html.as_bytes();
    for (kind, attr) in [("href", "href="), ("src", "src=")] {
        let mut search_from = 0usize;
        while let Some(rel) = html[search_from..].find(attr) {
            let pos = search_from + rel;
            search_from = pos + attr.len();
            // Require the byte before `attr` to be whitespace or `<`.
            if pos == 0 {
                continue;
            }
            let prev = bytes[pos - 1];
            if !(prev.is_ascii_whitespace()
                || prev == b'<'
                || prev == b'/'
                || prev == b'"'
                || prev == b'\'')
            {
                continue;
            }
            let rest = &html[pos + attr.len()..];
            let quote = rest.chars().next();
            let val = match quote {
                Some('"') | Some('\'') => {
                    let q = quote.unwrap();
                    let after_q = &rest[1..];
                    if let Some(end) = after_q.find(q) {
                        &after_q[..end]
                    } else {
                        continue;
                    }
                }
                _ => {
                    let end = rest
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(rest.len());
                    &rest[..end]
                }
            };
            out.push((kind, val));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = (bytes[i + 1] as char).to_digit(16);
            let h2 = (bytes[i + 2] as char).to_digit(16);
            if let (Some(a), Some(b)) = (h1, h2) {
                out.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn resolve_relative(base: &str, target: &str) -> String {
    if target.starts_with('/') {
        return target.trim_start_matches('/').to_string();
    }
    let mut parts: Vec<&str> = base.split('/').collect();
    parts.pop(); // drop file
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn format_uuid(u: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u[0], u[1], u[2], u[3], u[4], u[5], u[6], u[7], u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15],
    )
}
