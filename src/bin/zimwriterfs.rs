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

#[path = "_index_helper.rs"]
mod index_helper;
use index_helper::IndexHelper;

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
    compression_level: Option<i32>,
    threads: Option<usize>,
    inflate_html: bool,
    redirects_file: Option<PathBuf>,
    without_ft_index: bool,
    /// Optional explicit path to the xapianbuilder helper binary. If
    /// unset we look at $XAPIANBUILDER, then $PATH. Absence is
    /// non-fatal: the ZIM is built without search indexes.
    xapianbuilder_path: Option<PathBuf>,
    tags: Option<String>,
    source: Option<String>,
    flavour: Option<String>,
    scraper: Option<String>,
    skip_libmagic: bool,
    verbose: bool,
    /// Cluster routing strategy: "single" (default), "mime",
    /// "extension", or "path".
    cluster_by: Option<String>,
    /// Soft cap on parallel-batch in-flight bytes (MiB). Zero (the
    /// default) means "saturate cores", trading higher peak RSS
    /// for higher CPU utilisation. Non-zero forces an early drain
    /// so the parallel-batch buffer stays under the cap.
    max_memory_mb: Option<usize>,

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
            ("-h", _) | ("--help", _) => {
                print_help();
                return ExitCode::SUCCESS;
            }
            ("-V", _) | ("--version", _) => {
                println!("{VERSION}");
                return ExitCode::SUCCESS;
            }
            ("-v", _) | ("--verbose", _) => o.verbose = true,
            ("-x", _) | ("--inflateHtml", _) => o.inflate_html = true,
            ("-j", _) | ("--withoutFTIndex", _) => o.without_ft_index = true,
            ("--skip-libmagic-check", _) => o.skip_libmagic = true,
            ("-w", v) | ("--welcome", v) => o.welcome = Some(value_or_next(v, &args, &mut i)),
            ("-I", v) | ("--illustration", v) => {
                o.illustration = Some(PathBuf::from(value_or_next(v, &args, &mut i)))
            }
            ("-l", v) | ("--language", v) => o.language = Some(value_or_next(v, &args, &mut i)),
            ("-n", v) | ("--name", v) => o.name = Some(value_or_next(v, &args, &mut i)),
            ("-t", v) | ("--title", v) => o.title = Some(value_or_next(v, &args, &mut i)),
            ("-d", v) | ("--description", v) => {
                o.description = Some(value_or_next(v, &args, &mut i))
            }
            ("-c", v) | ("--creator", v) => o.creator = Some(value_or_next(v, &args, &mut i)),
            ("-p", v) | ("--publisher", v) => o.publisher = Some(value_or_next(v, &args, &mut i)),
            ("-L", v) | ("--longDescription", v) => {
                o.long_description = Some(value_or_next(v, &args, &mut i))
            }
            ("-m", v) | ("--clusterSize", v) => {
                o.cluster_size_kb = value_or_next(v, &args, &mut i).parse().ok()
            }
            ("--compression-level", v) => {
                o.compression_level = value_or_next(v, &args, &mut i).parse().ok()
            }
            ("-J", v) | ("--threads", v) => {
                o.threads = value_or_next(v, &args, &mut i).parse().ok()
            }
            ("-r", v) | ("--redirects", v) => {
                o.redirects_file = Some(PathBuf::from(value_or_next(v, &args, &mut i)))
            }
            ("-a", v) | ("--tags", v) => o.tags = Some(value_or_next(v, &args, &mut i)),
            ("-e", v) | ("--source", v) => o.source = Some(value_or_next(v, &args, &mut i)),
            ("-o", v) | ("--flavour", v) => o.flavour = Some(value_or_next(v, &args, &mut i)),
            ("-s", v) | ("--scraper", v) => o.scraper = Some(value_or_next(v, &args, &mut i)),
            ("--cluster-by", v) => o.cluster_by = Some(value_or_next(v, &args, &mut i)),
            ("--max-memory", v) => o.max_memory_mb = value_or_next(v, &args, &mut i).parse().ok(),
            ("--xapianbuilder-path", v) => {
                o.xapianbuilder_path = Some(PathBuf::from(value_or_next(v, &args, &mut i)))
            }
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

    // Configure rayon's global thread pool from -J/--threads if the
    // user specified it. Default (no flag) leaves rayon to size by
    // num_cpus(), which is the right thing on most hosts. We
    // intentionally don't error if the global pool was already
    // initialised — that just means the user invoked us in a
    // context where rayon was already set up.
    if let Some(n) = o.threads {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global();
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
        "Usage: zimwriterfs [mandatory arguments] [optional arguments] HTML_DIR ZIM_FILE\n\nMandatory:\n  -w/--welcome PATH      main HTML page (relative to HTML_DIR)\n  -I/--illustration PATH 48×48 PNG illustration (relative)\n  -l/--language LANG     ISO639-3 language code (e.g. eng)\n  -n/--name NAME         version-independent identifier\n  -t/--title TITLE       ZIM title\n  -d/--description TEXT  short description\n  -c/--creator AUTHOR    content creator\n  -p/--publisher PUB     ZIM creator/publisher\n\nOptional:\n  -L/--longDescription TEXT\n  -m/--clusterSize KB    cluster size in KiB (default 2048)\n  --compression-level N  compression level (zstd: 1..=22, xz: 0..=9)\n  -J/--threads N         rayon thread-pool size (default num_cpus)\n  -x/--inflateHtml       gunzip *.html files before packing\n  -j/--withoutFTIndex    don't build fulltext / title indexes\n  --xapianbuilder-path P override $PATH lookup of xapianbuilder helper\n  -r/--redirects PATH    TSV file: url\\ttitle\\ttarget_url\n  -a/--tags TAGS         semicolon-separated tags\n  -e/--source URL        source URL\n  -o/--flavour NAME      content flavour\n  -s/--scraper NAME      scraper tool name+version\n  --skip-libmagic-check  ignore libmagic; use file-extension mime detection (default in zimru)\n  -v/--verbose           print processing details\n  -V/--version           print version\n"
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
    if let Some(level) = o.compression_level {
        creator.set_compression_level(level);
    }
    if let Some(kb) = o.cluster_size_kb {
        creator.set_cluster_size_target(kb * 1024);
    }
    if let Some(mb) = o.max_memory_mb {
        creator.set_max_in_flight_bytes(mb * 1024 * 1024);
    }
    if let Some(strategy) = o.cluster_by.as_deref() {
        let s = match strategy {
            "single" | "" => zimru::writer::ClusterStrategy::Single,
            "mime" => zimru::writer::ClusterStrategy::ByMime,
            "extension" | "ext" => zimru::writer::ClusterStrategy::ByExtension,
            "path" | "path-segment" => zimru::writer::ClusterStrategy::ByFirstPathSegment,
            other => {
                eprintln!(
                    "zimwriterfs: --cluster-by must be one of single|mime|extension|path; got `{other}`"
                );
                return Err(zimru::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("invalid --cluster-by value: {other}"),
                )));
            }
        };
        creator.set_cluster_strategy(s);
    }
    creator.set_main_path(o.welcome.as_deref().unwrap());

    // Switch to streaming mode now so each subsequent add_item /
    // add_metadata / add_illustration call bin-packs into the
    // in-flight cluster and stream-encodes-and-writes when the
    // cluster overflows. Peak RSS becomes O(cluster_size_target ×
    // thread_count) instead of O(total input size).
    creator.start_writing(zim_file)?;

    // Spin up xapianbuilder helpers (one each for fulltext + title)
    // unless the user opted out. The helpers run in parallel with the
    // ZIM writer; we feed each entry via stdin as we add it. On
    // finish() we stream the output files back in bounded chunks and
    // add them as X/* items before the creator finalises.
    let language = o.language.clone().unwrap_or_default();
    let index_tmp = make_index_tmp_dir(zim_file)?;
    // Removes the temp dir on every exit path, including early `?`
    // returns while walking the input tree.
    let _index_tmp_cleanup = index_helper::TmpDirCleanup(index_tmp.clone());
    let mut indexer = if o.without_ft_index {
        IndexHelper::disabled()
    } else {
        IndexHelper::spawn(
            &language,
            &index_tmp,
            o.xapianbuilder_path.as_deref(),
            o.verbose,
        )
    };

    // Mandatory metadata.
    creator.add_metadata("Title", o.title.clone().unwrap());
    creator.add_metadata("Description", o.description.clone().unwrap());
    creator.add_metadata("Language", o.language.clone().unwrap());
    creator.add_metadata("Creator", o.creator.clone().unwrap());
    creator.add_metadata("Publisher", o.publisher.clone().unwrap());
    creator.add_metadata("Name", o.name.clone().unwrap());
    creator.add_metadata("Date", chrono_today_iso());

    // Optional metadata.
    if let Some(s) = &o.long_description {
        creator.add_metadata("LongDescription", s.clone());
    }
    if let Some(s) = &o.tags {
        creator.add_metadata("Tags", s.clone());
    }
    if let Some(s) = &o.source {
        creator.add_metadata("Source", s.clone());
    }
    if let Some(s) = &o.flavour {
        creator.add_metadata("Flavour", s.clone());
    }
    // Always emit M/Scraper. Real libzim's zimwriterfs writes its
    // own version string here ("zimwriterfs-3.x.x"); we emit the
    // user-provided value if any, otherwise fall back to our own
    // identifier so downstream tooling has something to read.
    let scraper = o.scraper.clone().unwrap_or_else(|| VERSION.to_string());
    creator.add_metadata("Scraper", scraper);

    // Illustration (must exist; mandatory upstream).
    let illu_path = html_dir.join(o.illustration.as_ref().unwrap());
    let illu_bytes = fs::read(&illu_path).map_err(|e| {
        zimru::Error::Io(std::io::Error::new(
            e.kind(),
            format!("--illustration {}: {e}", illu_path.display()),
        ))
    })?;
    creator.add_illustration(48, illu_bytes);

    // Walk HTML_DIR and ingest every file. The illustration is also kept
    // as a regular C entry so links from inside the archive that reference
    // it directly (e.g. `<link rel="icon" href="icon48.png">`) still
    // resolve — matches upstream zimwriterfs behavior.
    //
    // Two-tier processing:
    // - Small files (< STREAMING_THRESHOLD_BIN): read in parallel
    //   batches via rayon, push to creator in walk-order. Overlaps
    //   disk-wait across cores; the writer's per-item bookkeeping
    //   stays serial.
    // - Large files (>= threshold): processed one at a time via the
    //   chunked-streaming path. Don't bother batching — they
    //   already engage zstdmt internally.
    use rayon::prelude::*;
    use std::io::Read as _;
    const STREAMING_THRESHOLD_BIN: u64 = 4 * 1024 * 1024;
    // Bumped from 64 to 256: more files queued in flight per
    // rayon batch keeps the disk queue deeper, capturing some of
    // io_uring's submission-batching win on spinning rust + NVMe
    // alike. The peak in-batch memory is `READ_BATCH × avg_size`,
    // a few tens of MB on typical inputs — well under the
    // parallel-batch encode buffer.
    const READ_BATCH: usize = 256;
    let mut count = 0usize;

    // Build the (path, relpath, size, is_html) list once. metadata()
    // is one stat call per file — comparable to the old walk_dir's
    // `entry.file_type()` call, so no extra cost.
    struct PrepEntry {
        path: PathBuf,
        rel_str: String,
        size: u64,
        is_html: bool,
    }
    let (entries, symlinks) = walk_dir(html_dir)?;
    let prepped: Vec<PrepEntry> = entries
        .into_iter()
        .map(|p| {
            let rel = p.strip_prefix(html_dir).unwrap();
            let rel_str = if cfg!(windows) {
                rel.to_string_lossy().replace('\\', "/")
            } else {
                rel.to_string_lossy().into_owned()
            };
            let size = p.metadata().map(|m| m.len()).unwrap_or(0);
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_html = ext == "html" || ext == "htm";
            PrepEntry {
                path: p,
                rel_str,
                size,
                is_html,
            }
        })
        .collect();

    let needs_full_read = |e: &PrepEntry| e.is_html || o.inflate_html;

    // Processed payload after parallel read.
    struct ReadyItem {
        rel_str: String,
        title: String,
        mime: String,
        content: Vec<u8>,
    }

    let mut idx = 0usize;
    while idx < prepped.len() {
        let e = &prepped[idx];

        // Big non-HTML file → chunked-streaming path one-at-a-time.
        if !needs_full_read(e) && e.size >= STREAMING_THRESHOLD_BIN {
            let mut f = fs::File::open(&e.path)?;
            // Extension first, content sniff for extensionless files
            // (rewinding afterwards so the streaming loop sees the
            // whole body).
            let mime = match e.path.extension() {
                Some(_) => mime_for_path(&e.path),
                None => {
                    use std::io::Seek as _;
                    let mut head = [0u8; 1024];
                    let n = f.read(&mut head)?;
                    f.seek(std::io::SeekFrom::Start(0))?;
                    sniff_mime(&head[..n])
                        .unwrap_or("application/octet-stream")
                        .to_string()
                }
            };
            let compress_hint = if should_compress(&mime) {
                None
            } else {
                Some(false)
            };
            // Hint the kernel: we're about to read this whole
            // file sequentially. On Linux this turns on aggressive
            // readahead and drops pages behind us; on macOS it
            // enables F_RDAHEAD. Either way the next read() call
            // tends to find its bytes already warm in cache.
            zimru::io_hints::hint_sequential(&f);
            creator.begin_chunked_item(
                None,
                e.rel_str.clone(),
                e.rel_str.clone(),
                mime,
                Some(e.size),
                compress_hint,
            )?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                creator.chunked_item_chunk(&buf[..n])?;
            }
            creator.end_chunked_item()?;
            // Title indexer sees every C-namespace entry regardless
            // of mimetype. Big non-HTML files (images, audio, …) get
            // a title-only entry — same behaviour as libzim.
            indexer.feed_title(&e.rel_str, &e.rel_str, "");
            count += 1;
            idx += 1;
            if o.verbose && count.is_multiple_of(100) {
                eprintln!("[zimwriterfs] {count} items");
            }
            continue;
        }

        // Walk forward collecting a batch of small files (stop at
        // the next big file or end-of-list). Keep the batch all
        // either small-or-HTML so they share the parallel-read fate.
        let batch_start = idx;
        while idx < prepped.len()
            && (idx - batch_start) < READ_BATCH
            && (needs_full_read(&prepped[idx]) || prepped[idx].size < STREAMING_THRESHOLD_BIN)
        {
            idx += 1;
        }
        let batch = &prepped[batch_start..idx];

        // Parallel-read the batch into per-item contents. Errors
        // surface with the path so they're actionable.
        let items: Vec<ReadyItem> = batch
            .par_iter()
            .map(|e| -> Result<ReadyItem, zimru::Error> {
                let mut content = fs::read(&e.path)?;
                if o.inflate_html && e.is_html && content.starts_with(&[0x1f, 0x8b]) {
                    // Inflate so the packed item, title derivation, and
                    // fulltext indexing all see real HTML — the user
                    // asked for -x, so a corrupt .gz is a hard error,
                    // not a silent gzip-bytes-as-text/html item.
                    content = inflate_gzip(&content).map_err(|err| {
                        zimru::Error::Io(std::io::Error::other(format!(
                            "--inflateHtml {}: {err}",
                            e.path.display()
                        )))
                    })?;
                }
                // Resolve the mimetype: extension first, then content
                // sniff for extensionless files (dumped wiki articles).
                let mime = match e.path.extension() {
                    Some(_) => mime_for_path(&e.path),
                    None => sniff_mime(&content)
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                };
                // HTML gets its title from the <title> tag whether the
                // file was recognised by extension or by sniffing.
                let title = if e.is_html || mime.starts_with("text/html") {
                    derive_title_from_bytes(&content).unwrap_or_else(|| e.rel_str.clone())
                } else {
                    e.rel_str.clone()
                };
                Ok(ReadyItem {
                    rel_str: e.rel_str.clone(),
                    title,
                    mime,
                    content,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Push to creator in walk-order so bin-packing stays
        // deterministic.
        for it in items {
            // Feed both indexers BEFORE the content moves into the
            // creator (after which we no longer borrow it). The
            // title DB sees every entry; the fulltext DB only HTML
            // (the helper filters by mimetype internally). The
            // body is treated as UTF-8 lossily — non-UTF-8
            // payloads are not valid HTML to libzim's parser
            // anyway, so dropping ill-formed bytes here is
            // strictly safer than feeding them through.
            indexer.feed_title(&it.rel_str, &it.title, "");
            if it.mime.starts_with("text/html") {
                let body = std::str::from_utf8(&it.content)
                    .map(std::borrow::Cow::Borrowed)
                    .unwrap_or_else(|_| String::from_utf8_lossy(&it.content));
                indexer.feed_fulltext(&it.rel_str, &it.title, &it.mime, &body, &language);
            }
            let mut item = Item::new(it.rel_str, it.title, it.mime, it.content);
            if !should_compress(&item.mimetype) {
                item = item.with_compress(false);
            }
            creator.add_item(item);
            count += 1;
            if o.verbose && count.is_multiple_of(100) {
                eprintln!("[zimwriterfs] {count} items");
            }
        }
    }

    // Symlinks become redirect entries, matching upstream
    // zimwriterfs (and round-tripping `zimdump dump --redirect`
    // output). Each link's target is resolved against the link's own
    // directory, normalised, and re-expressed relative to
    // HTML_DIRECTORY; links that point outside the tree or at
    // nothing we walked are skipped with a warning.
    if !symlinks.is_empty() {
        use std::collections::HashSet;
        let rel_of = |p: &Path| -> String {
            let rel = p.strip_prefix(html_dir).unwrap_or(p);
            if cfg!(windows) {
                rel.to_string_lossy().replace('\\', "/")
            } else {
                rel.to_string_lossy().into_owned()
            }
        };
        let mut known: HashSet<String> = prepped.iter().map(|e| e.rel_str.clone()).collect();
        for link in &symlinks {
            known.insert(rel_of(link));
        }
        for link in &symlinks {
            let rel = rel_of(link);
            let raw = match fs::read_link(link) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("zimwriterfs: skipping symlink {rel}: {e}");
                    continue;
                }
            };
            let joined = link.parent().unwrap_or(html_dir).join(&raw);
            let target_rel = match normalize_within(html_dir, &joined) {
                Some(t) if known.contains(&t) => t,
                _ => {
                    eprintln!(
                        "zimwriterfs: skipping symlink {rel}: target {} not inside the html directory",
                        raw.display()
                    );
                    continue;
                }
            };
            creator.add_redirection(rel.clone(), rel.clone(), target_rel.clone());
            // Redirects also go in the title index, with their
            // target stored in value slot 1.
            indexer.feed_title(&rel, &rel, &target_rel);
            count += 1;
        }
    }

    // Drop the prepped list now that we're done with it.
    drop(prepped);

    // Optional redirects file (TSV: url \t title \t target_url).
    if let Some(rfile) = &o.redirects_file {
        let raw = fs::read_to_string(rfile)?;
        for (lineno, line) in raw.lines().enumerate() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                eprintln!("zimwriterfs: redirect file line {} malformed (need 3 tab-separated fields): {line}", lineno + 1);
                continue;
            }
            creator.add_redirection(parts[0], parts[1], parts[2]);
            // Redirects also go in the title index, with their
            // target stored in value slot 1.
            indexer.feed_title(parts[0], parts[1], parts[2]);
        }
    }

    // Drain xapianbuilder children, ingest their output blobs, and
    // attach them to the ZIM as `X/fulltext/xapian` and
    // `X/title/xapian`. The X namespace is uncompressed by kiwix
    // convention so libzim/kiwix-serve can mmap and Database(int fd)
    // directly into the index.
    let blobs = indexer.finish(o.verbose);
    for blob in blobs {
        if let Err(e) = blob.add_to(&mut creator) {
            eprintln!("[zimwriterfs] attaching index {} failed: {e}", blob.url);
        }
    }

    if o.verbose {
        eprintln!("[zimwriterfs] {count} items collected; finalising…");
    }
    creator.finish_writing()?;
    if o.verbose {
        eprintln!("[zimwriterfs] wrote {}", zim_file.display());
    }
    Ok(())
}

/// Recursively yield every file under `dir`. Skips dotfiles (matches upstream).
/// Recursively yield `(regular_files, symlinks)` under `dir`. Skips
/// dotfiles (matches upstream). Symlinks are reported separately so
/// the caller can turn them into redirect entries instead of either
/// silently dropping them or duplicating the target's content.
fn walk_dir(dir: &Path) -> std::io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut out = Vec::new();
    let mut links = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                links.push(path);
            } else if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    links.sort();
    Ok((out, links))
}

/// Normalise `p` (which may contain `.` / `..` components) and
/// re-express it relative to `root` with `/` separators. Returns
/// `None` if the path escapes `root` or normalises to nothing.
fn normalize_within(root: &Path, p: &Path) -> Option<String> {
    use std::path::Component;
    let rel = p.strip_prefix(root).ok()?;
    let mut stack: Vec<String> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(x) => stack.push(x.to_string_lossy().into_owned()),
            Component::ParentDir => {
                stack.pop()?;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    if stack.is_empty() {
        None
    } else {
        Some(stack.join("/"))
    }
}

/// Whether content of this mimetype belongs in a compressed cluster.
/// Mirrors upstream zimwriterfs/libzim behaviour: text-like formats
/// compress; already-compressed media (JPEG, PNG, WebP, WebM, Ogg,
/// fonts, archives …) goes into uncompressed clusters — recompressing
/// it costs most of the high-level zstd encode time on media-heavy
/// builds for a ~2% size gain.
fn should_compress(mime: &str) -> bool {
    let mime = mime.split(';').next().unwrap_or(mime).trim();
    if mime.starts_with("text/") {
        return true;
    }
    if mime.ends_with("+xml") || mime.ends_with("+json") {
        return true;
    }
    matches!(
        mime,
        "application/javascript"
            | "application/x-javascript"
            | "application/ecmascript"
            | "application/json"
            | "application/xml"
            | "application/wasm"
    )
}

fn mime_for_path(p: &Path) -> String {
    static TABLE: &[(&str, &str)] = &[
        ("html", "text/html"),
        ("htm", "text/html"),
        ("xhtml", "application/xhtml+xml"),
        ("css", "text/css"),
        ("js", "application/javascript"),
        ("mjs", "application/javascript"),
        ("json", "application/json"),
        ("xml", "application/xml"),
        ("svg", "image/svg+xml"),
        ("png", "image/png"),
        ("jpg", "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("gif", "image/gif"),
        ("webp", "image/webp"),
        ("ico", "image/x-icon"),
        ("bmp", "image/bmp"),
        ("pdf", "application/pdf"),
        ("epub", "application/epub+zip"),
        ("zip", "application/zip"),
        ("gz", "application/gzip"),
        ("woff", "font/woff"),
        ("woff2", "font/woff2"),
        ("ttf", "font/ttf"),
        ("otf", "font/otf"),
        ("mp3", "audio/mpeg"),
        ("ogg", "audio/ogg"),
        ("opus", "audio/ogg"),
        ("wav", "audio/wav"),
        ("mp4", "video/mp4"),
        ("webm", "video/webm"),
        ("ogv", "video/ogg"),
        ("txt", "text/plain"),
        ("md", "text/markdown"),
        ("csv", "text/csv"),
        ("tsv", "text/tab-separated-values"),
    ];
    // Built once — this runs per input file, so rebuilding the map on
    // every call was one HashMap allocation + ~38 hashes per file.
    static MAP: std::sync::OnceLock<HashMap<&'static str, &'static str>> =
        std::sync::OnceLock::new();
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if let Some(ref e) = ext {
        let map = MAP.get_or_init(|| TABLE.iter().copied().collect());
        if let Some(&m) = map.get(e.as_str()) {
            return m.to_string();
        }
    }
    "application/octet-stream".to_string()
}

/// Best-effort content sniff for files whose extension didn't resolve
/// a mimetype. The big real-world case is `zimdump dump` output, where
/// every wiki article is an extensionless HTML file — upstream
/// zimwriterfs resolves those through libmagic; without sniffing they
/// were mislabelled `application/octet-stream`, which broke fulltext
/// indexing, title extraction, link checking, and (with mime-driven
/// cluster compression) routed all article text into raw clusters.
fn sniff_mime(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if head.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if head.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if head.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    if head.starts_with(b"%PDF") {
        return Some("application/pdf");
    }
    if head.starts_with(b"\x1f\x8b") {
        return Some("application/gzip");
    }
    // Text-ish sniff over the first KB.
    let window = &head[..head.len().min(1024)];
    let text = String::from_utf8_lossy(window);
    let lower = text.to_ascii_lowercase();
    let trimmed = lower.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    if trimmed.starts_with("<!doctype html")
        || trimmed.starts_with("<html")
        || lower.contains("<head")
        || lower.contains("<body")
    {
        return Some("text/html");
    }
    if trimmed.starts_with("<svg") {
        return Some("image/svg+xml");
    }
    if trimmed.starts_with("<?xml") {
        // Could be SVG with an XML prolog.
        return if lower.contains("<svg") {
            Some("image/svg+xml")
        } else {
            Some("application/xml")
        };
    }
    let utf8_ok = match std::str::from_utf8(window) {
        Ok(_) => true,
        // A multibyte char split at the window edge is fine.
        Err(e) => e.error_len().is_none(),
    };
    if !window.is_empty() && !window.contains(&0) && utf8_ok {
        return Some("text/plain");
    }
    None
}

/// Pull `<title>...</title>` out of an HTML body the caller already
/// has in memory. Returns `None` on non-HTML or no title tag. This
/// is the in-memory variant — the per-walk loop reads each HTML
/// file's bytes once into a `Vec<u8>` for the body and reuses
/// those same bytes here, avoiding a second `fs::read` of the
/// same file (Wikipedia-shaped builds saved ~15 GB of redundant
/// disk reads on top-1000-articles when this was wired in).
fn derive_title_from_bytes(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    // Case-insensitive byte search — lowercasing the whole body just to
    // locate one tag allocated a full copy of every HTML file (tens of
    // GB of transient churn on a large build).
    let s = find_ascii_ci(text.as_bytes(), b"<title>")? + "<title>".len();
    let e = find_ascii_ci(&text.as_bytes()[s..], b"</title>")?;
    Some(text[s..s + e].trim().to_string())
}

/// Position of the ASCII-case-insensitive `needle` in `hay`; `needle`
/// must already be lowercase.
fn find_ascii_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len())
        .position(|w| w.iter().zip(needle).all(|(a, b)| a.eq_ignore_ascii_case(b)))
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

/// Inflate a gzipped buffer for `-x/--inflateHtml`. `MultiGzDecoder`
/// handles multi-member gzip streams (concatenated .gz files).
fn inflate_gzip(buf: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::with_capacity(buf.len().saturating_mul(4));
    let mut dec = flate2::read::MultiGzDecoder::new(buf);
    dec.read_to_end(&mut out)?;
    Ok(out)
}

/// Allocate a per-run temp directory next to the output ZIM for the
/// xapianbuilder children's output files. We avoid `std::env::temp_dir`
/// because Wikipedia-scale fulltext indexes can be tens of GB and many
/// systems put `/tmp` on a small ramdisk; co-locating with the ZIM
/// keeps bytes on the same filesystem the user already chose for the
/// big output.
fn make_index_tmp_dir(zim_file: &Path) -> Result<PathBuf, zimru::Error> {
    let parent = zim_file.parent().unwrap_or(Path::new("."));
    let stem = zim_file
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "zim".into());
    let pid = std::process::id();
    let dir = parent.join(format!(".{stem}.xapianbuilder.{pid}"));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
