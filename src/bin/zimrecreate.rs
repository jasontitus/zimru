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
//!   -j, --withoutFTIndex       don't build fulltext/title indexes (when
//!                              omitted, indexes are built via the external
//!                              `xapianbuilder` helper if it is on $PATH)
//!   -J, --threads <number>     encode worker pool size (default: one per CPU)
//!   --compression none|zstd|xz choose cluster compression (default zstd)
//!   --compression-level N      compression level (zstd: 1..=22, xz: 0..=9)
//!   --cluster-size BYTES       cluster size target (default 2MiB)
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use zimru::writer::{ClusterStrategy, Creator, Item};
use zimru::{Archive, Compression, Dirent};

#[path = "_index_helper.rs"]
mod index_helper;
use index_helper::IndexHelper;

const VERSION: &str = "zimrecreate (zimru) 0.1.0";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut src: Option<String> = None;
    let mut dst: Option<String> = None;
    let mut compression = Compression::Zstd;
    let mut compression_level: Option<i32> = None;
    let mut cluster_target: Option<usize> = None;
    let mut cluster_strategy: ClusterStrategy = ClusterStrategy::Single;
    let mut without_ft_index = false;
    let mut xapianbuilder_path: Option<PathBuf> = None;

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
            "-j" | "--withoutFTIndex" => without_ft_index = true,
            "--xapianbuilder-path" => {
                i += 1;
                xapianbuilder_path = args.get(i).map(PathBuf::from);
            }
            "-J" | "--threads" => {
                i += 1;
                if let Some(n) = args.get(i).and_then(|s| s.parse::<usize>().ok()) {
                    // Sizes the writer's encode worker pool (and the
                    // cluster-grouped source reads). Ignore failure —
                    // it just means a pool was already initialised.
                    let _ = rayon::ThreadPoolBuilder::new()
                        .num_threads(n)
                        .build_global();
                }
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
                    Some("path") | Some("path-segment") => ClusterStrategy::ByFirstPathSegment,
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
        without_ft_index,
        xapianbuilder_path.as_deref(),
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
        "\nzimrecreate recreates a ZIM file from an existing ZIM.\n\nUsage: zimrecreate ORIGIN_FILE OUTPUT_FILE [Options]\nOptions:\n\t-v, --version              print software version\n\t-j, --withoutFTIndex       don't create a fulltext index (always)\n\t-J, --threads <number>     encode worker pool size (default: one per CPU)\n\t--compression C            one of: none | zstd | xz  (default zstd)\n\t--compression-level N      compression level (zstd: 1..=22, xz: 0..=9)\n\t--cluster-size BYTES       cluster size target (default 2097152)\n"
    );
}

#[allow(clippy::too_many_arguments)]
fn run(
    src: &str,
    dst: &str,
    compression: Compression,
    compression_level: Option<i32>,
    cluster_target: Option<usize>,
    cluster_strategy: ClusterStrategy,
    without_ft_index: bool,
    xapianbuilder_path: Option<&std::path::Path>,
) -> Result<(), zimru::Error> {
    let source = Archive::open(src)?;
    // We're about to walk every entry + every cluster end-to-end,
    // so hint the kernel to prefetch sequentially.
    source.advise_sequential_scan();
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

    // Pull the source's M/Language metadata so we can pass it to
    // xapianbuilder. Empty (the helper will skip stemming if so).
    let language = read_metadata_string(&source, "Language").unwrap_or_default();
    let dst_path = std::path::Path::new(dst);
    let index_tmp = make_index_tmp_dir(dst_path)?;
    let mut indexer = if without_ft_index {
        IndexHelper::disabled()
    } else {
        IndexHelper::spawn(&language, &index_tmp, xapianbuilder_path, false)
    };

    // Iterate every entry, partition by kind. We skip:
    //   W/mainPage         — added automatically via set_main_path above
    //   X/*                — search indexes we don't rebuild (matches --withoutFTIndex)
    //   M/Counter          — auto-regenerated by zimru's writer from the
    //                        rebuilt mime histogram; the source's value
    //                        would be stale after re-encode.
    //   M/Scraper          — let zimru stamp its own scraper string
    //                        (the recreate isn't libzim's output).
    let ml = source.mime_list();
    let mut seen_metadata = std::collections::HashSet::new();

    // Pass 1 — dirents only (no cluster decompression): redirects and
    // small M-namespace entries are handled immediately; C-namespace
    // articles are collected as (cluster, blob, path, title, mime)
    // tuples for the cluster-grouped content pass below.
    struct PendingContent {
        cluster: u32,
        blob: u32,
        path: String,
        title: String,
        mime: String,
    }
    let mut pending: Vec<PendingContent> = Vec::new();
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
        if ns == b'M' && (path == "Counter" || path == "Scraper") {
            continue;
        }

        match entry.dirent() {
            Dirent::Redirect(r) => {
                if ns == b'C' {
                    // Look up the target's path so we can add it via the
                    // public redirection API.
                    let target = source.entry_by_url_index(r.redirect_index)?;
                    let target_path = target.path().to_string();
                    creator.add_redirection(
                        path.to_string(),
                        title.to_string(),
                        target_path.clone(),
                    );
                    // Redirects belong in the title index only when their
                    // target is a front article (text/html), mirroring the
                    // content gate above and libzim's FRONT_ARTICLE rule.
                    // Without this, redirects pointing at assets (tiles,
                    // fonts, vector chunks) pollute the suggestion index.
                    let target_is_front = match target.dirent() {
                        Dirent::Article(a) => ml
                            .get(a.mimetype)
                            .is_some_and(|m| m.starts_with("text/html")),
                        Dirent::Redirect(_) => false,
                    };
                    if target_is_front {
                        indexer.feed_title(path, title, &target_path);
                    }
                }
                // Redirects in W/M/X namespaces are rebuilt implicitly by
                // re-adding the underlying entries.
            }
            Dirent::Article(a) => {
                let mime = ml
                    .get(a.mimetype)
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if ns == b'C' {
                    pending.push(PendingContent {
                        cluster: a.cluster,
                        blob: a.blob,
                        path: path.to_string(),
                        title: title.to_string(),
                        mime,
                    });
                } else if ns == b'M' {
                    // M-namespace entries are few and tiny; fetch them
                    // directly. Strip the special illustration path back
                    // into add_illustration where possible so the output
                    // is canonically shaped.
                    let data = entry.get_item(false)?.bytes()?;
                    if let Some(side) = parse_illustration_path(path) {
                        creator.add_illustration(side, data);
                    } else if seen_metadata.insert(path.to_string()) {
                        creator.add_metadata(path, data);
                    }
                }
            }
        }
    }

    // Stamp M/Scraper. We skipped the source's value above because the
    // recreate isn't the original tool's output, but dropping it entirely
    // loses provenance and leaves the archive a metadata key short of both
    // the source and libzim's zimrecreate. Preserve the original value and
    // record that zimru repackaged it.
    let scraper = match read_metadata_string(&source, "Scraper") {
        Some(orig) if !orig.is_empty() => format!("{orig}; recreated by {VERSION}"),
        _ => VERSION.to_string(),
    };
    creator.add_metadata("Scraper", scraper);

    // Pass 2 — content, grouped by source cluster so each cluster is
    // decompressed exactly once. Iterating in URL order instead visits
    // clusters near-randomly (scrapers pack clusters in crawl order,
    // not URL order) and thrashes the cluster LRU into re-decoding the
    // same clusters dozens of times — on a 1.1 GB Wikipedia archive
    // that was >5 minutes of redundant zstd work. `cluster_uncached`
    // bypasses the shared cache, keeping memory at one decoded cluster.
    pending.sort_unstable_by_key(|p| (p.cluster, p.blob));
    let mut i = 0;
    while i < pending.len() {
        let cidx = pending[i].cluster;
        let cluster = source.cluster_uncached(cidx)?;
        // Preserve the source cluster's compression choice: items the
        // original writer left uncompressed (already-compressed media
        // — JPEG, WebM, fonts …) stay uncompressed. Recompressing them
        // costs the bulk of high-level zstd encode time for a ~2% size
        // gain, and upstream zimrecreate keeps them raw too.
        let keep_raw = cluster.compression() == Compression::None;
        while i < pending.len() && pending[i].cluster == cidx {
            let p = &mut pending[i];
            let data = cluster.blob(p.blob)?.to_vec();
            // Only front articles (text/html) belong in the title /
            // suggestion index — same gate as fulltext below. libzim
            // populates its title index solely from FRONT_ARTICLE
            // entries, judged exactly by this mime test. Feeding every
            // C-namespace item (map tiles, fonts, sprites, .pbf vector
            // chunks) instead bloated the title index by ~1000× (e.g.
            // 822k docs / 130 MB on a Hawaii OSM archive vs libzim's
            // handful of real place pages).
            if p.mime.starts_with("text/html") {
                indexer.feed_title(&p.path, &p.title, "");
                let body = std::str::from_utf8(&data)
                    .map(std::borrow::Cow::Borrowed)
                    .unwrap_or_else(|_| String::from_utf8_lossy(&data));
                indexer.feed_fulltext(&p.path, &p.title, &p.mime, &body, &language);
            }
            // Move the strings out of the pending slot instead of
            // cloning all three — `pending` is dropped right after
            // this loop, so nothing reads them again.
            let mut item = Item::new(
                std::mem::take(&mut p.path),
                std::mem::take(&mut p.title),
                std::mem::take(&mut p.mime),
                data,
            );
            if keep_raw {
                item = item.with_compress(false);
            }
            creator.add_item(item);
            i += 1;
        }
    }
    drop(pending);

    // Drain the helper, attach output blobs as X/* items.
    let blobs = indexer.finish(false);
    for blob in blobs {
        creator.add_item(
            Item::in_namespace(b'X', blob.url, "", blob.mimetype, blob.bytes).with_compress(false),
        );
    }
    let _ = std::fs::remove_dir_all(&index_tmp);

    creator.finish_writing()?;
    Ok(())
}

fn read_metadata_string(archive: &Archive, name: &str) -> Option<String> {
    let bytes = archive.get_metadata(name).ok()?;
    String::from_utf8(bytes).ok()
}

fn make_index_tmp_dir(zim_file: &std::path::Path) -> Result<PathBuf, zimru::Error> {
    let parent = zim_file.parent().unwrap_or(std::path::Path::new("."));
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
