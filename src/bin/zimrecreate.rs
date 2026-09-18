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

use std::path::{Path, PathBuf};
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
    // zimru extension: upstream has no flag for "no indexes at all", since
    // its -j keeps the title index.
    let mut without_indexes = false;
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
            "--without-indexes" => without_indexes = true,
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
        without_indexes,
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
        "\nzimrecreate recreates a ZIM file from an existing ZIM.\n\nUsage: zimrecreate ORIGIN_FILE OUTPUT_FILE [Options]\nOptions:\n\t-v, --version              print software version\n\t-j, --withoutFTIndex       skip the fulltext index (title index still built)\n\t--without-indexes          skip both the fulltext and title indexes\n\t-J, --threads <number>     encode worker pool size (default: one per CPU)\n\t--compression C            one of: none | zstd | xz  (default zstd)\n\t--compression-level N      compression level (zstd: 1..=22, xz: 0..=9)\n\t--cluster-size BYTES       cluster size target (default 2097152)\n"
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
    without_indexes: bool,
    xapianbuilder_path: Option<&std::path::Path>,
) -> Result<(), zimru::Error> {
    ensure_distinct_output(Path::new(src), Path::new(dst))?;
    let source = Archive::open(src)?;
    let legacy = !source.header().uses_new_namespaces();
    // Keep all intermediate files beside the destination, but inside a
    // directory exclusively created by this invocation. Only a completed
    // archive is published; failures never truncate the source or destination.
    let output_tmp = make_output_tmp_dir(Path::new(dst))?;
    let _output_cleanup = index_helper::TmpDirCleanup(output_tmp.clone());
    let output_path = output_tmp.join("archive.zim");
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
        let main = source.main_entry()?.resolve()?;
        let path = content_path(legacy, main.namespace(), main.path()).ok_or_else(|| {
            zimru::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "main entry is outside the content namespace",
            ))
        })?;
        creator.try_set_main_path(&path)?;
    }

    // Switch to streaming mode — bodies bin-pack into clusters and
    // stream-encode-write as we iterate the source. Peak RSS becomes
    // O(cluster_size_target × bucket_count) instead of O(total content).
    creator.start_writing(&output_path)?;

    // Pull the source's M/Language metadata so we can pass it to
    // xapianbuilder. Empty (the helper will skip stemming if so).
    let language = read_metadata_string(&source, "Language").unwrap_or_default();
    let index_tmp = output_tmp.join("indexes");
    std::fs::create_dir(&index_tmp)?;
    // `-j` matches upstream: it drops the fulltext index only. The title
    // index still gets built, because that is what upstream's `-j` output
    // contains and what kiwix's suggestion box reads.
    let mut indexer = if without_indexes {
        IndexHelper::disabled()
    } else {
        IndexHelper::spawn(
            &language,
            &index_tmp,
            xapianbuilder_path,
            false,
            !without_ft_index,
            true,
        )
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

    // A legacy URL includes its namespace. Retaining that prefix inside
    // modern C preserves relative links (A/home -> ../I/icon), and avoids
    // collisions between A/foo, I/foo and auxiliary B/U/V/W entries.

    // Pass 1 — dirents only (no cluster decompression): redirects and
    // small M-namespace entries are handled immediately; content
    // articles are collected as compact (cluster, blob, url_index)
    // triples for the cluster-grouped content pass below. Keeping the
    // path/title/mime Strings here instead would hold ~150–200 B per
    // entry for the whole archive (gigabytes on a 10M-entry Wikipedia),
    // undermining the streaming writer's bounded-RSS design; pass 2
    // re-resolves them from the mmap with one cheap dirent parse each.
    struct PendingContent {
        cluster: u32,
        blob: u32,
        url_index: u32,
    }
    let mut pending: Vec<PendingContent> = Vec::new();
    for entry in source.iter_by_path() {
        let entry = entry?;
        let ns = entry.namespace();
        let content = is_content_namespace(legacy, ns);
        let path = entry.path();
        let title = entry.title();

        if !legacy && ns == b'W' && path == "mainPage" {
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
                if content {
                    let path = content_path(legacy, ns, path).expect("content namespace");
                    // Look up the target's path so we can add it via the
                    // public redirection API.
                    let target = source.entry_by_url_index(r.redirect_index)?;
                    // `add_redirection` targets the content namespace —
                    // a redirect whose target lives elsewhere would be
                    // rewritten as C/<path>, never resolve, and fail the
                    // build at finish_writing. Reject rather than lose it.
                    if let Some(target_path) =
                        content_path(legacy, target.namespace(), target.path())
                    {
                        creator.try_add_redirection(
                            path.clone(),
                            title.to_string(),
                            target_path.clone(),
                        )?;
                        // Redirects belong in the title index only when their
                        // target is a front article (text/html), mirroring the
                        // content gate above and libzim's FRONT_ARTICLE rule.
                        // Without this, redirects pointing at assets (tiles,
                        // fonts, vector chunks) pollute the suggestion index.
                        let target_is_front = match target.resolve()?.dirent() {
                            Dirent::Article(a) => ml
                                .get(a.mimetype)
                                .is_some_and(|m| m.starts_with("text/html")),
                            Dirent::Redirect(_) => false,
                        };
                        if target_is_front {
                            indexer.feed_title(&path, title, &target_path);
                        }
                    } else {
                        return Err(zimru::Error::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "redirect {path} targets an entry outside the content namespace"
                            ),
                        )));
                    }
                }
                // Redirects in W/M/X namespaces are rebuilt implicitly by
                // re-adding the underlying entries.
            }
            Dirent::Article(a) => {
                if content {
                    pending.push(PendingContent {
                        cluster: a.cluster,
                        blob: a.blob,
                        url_index: entry.index(),
                    });
                } else if ns == b'M' {
                    // M-namespace entries are few and tiny; fetch them
                    // directly. Strip the special illustration path back
                    // into add_illustration where possible so the output
                    // is canonically shaped; everything else keeps its
                    // original mimetype (illustrations and favicons are
                    // binary — recording them as text/plain would corrupt
                    // them for downstream readers).
                    let mime = ml
                        .get(a.mimetype)
                        .unwrap_or("application/octet-stream")
                        .to_string();
                    let data = entry.get_item(false)?.bytes()?;
                    if let Some(side) = parse_illustration_path(path) {
                        creator.try_add_illustration(side, data)?;
                    } else if seen_metadata.insert(path.to_string()) {
                        creator.try_add_metadata_with_mimetype(path, mime, data)?;
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
    creator.try_add_metadata("Scraper", scraper)?;

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
            let p = &pending[i];
            // Re-resolve path/title/mime from the source dirent — one
            // cheap mmap-backed parse per item, in exchange for not
            // holding those Strings for the whole archive during pass 1.
            let entry = source.entry_by_url_index(p.url_index)?;
            let Dirent::Article(a) = entry.dirent() else {
                i += 1;
                continue;
            };
            let path = content_path(legacy, entry.namespace(), entry.path())
                .expect("pending entries belong to the content namespace");
            let title = entry.title();
            let mime = ml.get(a.mimetype).unwrap_or("application/octet-stream");
            // The blob copy here is a known extra memcpy: the bytes
            // already live in the decoded cluster payload, but `Item`
            // owns its body, so threading a borrowed slice through the
            // writer would be a lifetime refactor of Item/Bucket/encode.
            let data = cluster.blob(p.blob)?.to_vec();
            // Only front articles (text/html) belong in the title /
            // suggestion index — same gate as fulltext below. libzim
            // populates its title index solely from FRONT_ARTICLE
            // entries, judged exactly by this mime test. Feeding every
            // C-namespace item (map tiles, fonts, sprites, .pbf vector
            // chunks) instead bloated the title index by ~1000× (e.g.
            // 822k docs / 130 MB on a Hawaii OSM archive vs libzim's
            // handful of real place pages).
            if mime.starts_with("text/html") {
                indexer.feed_title(&path, title, "");
                let body = std::str::from_utf8(&data)
                    .map(std::borrow::Cow::Borrowed)
                    .unwrap_or_else(|_| String::from_utf8_lossy(&data));
                indexer.feed_fulltext(&path, title, mime, &body, &language);
            }
            let mut item = Item::new(path, title.to_string(), mime.to_string(), data);
            if keep_raw {
                item = item.with_compress(false);
            }
            creator.try_add_item(item)?;
            i += 1;
        }
    }
    drop(pending);

    // Drain the helper, stream output blobs in as X/* items (the
    // fulltext DB can be multi-GB — never buffer it whole). A failure
    // mid-stream leaves the writer with an in-flight chunked item, so
    // it must abort the build — continuing would finalize a corrupt
    // archive while exiting 0.
    let blobs = indexer.finish(false);
    for blob in blobs {
        blob.add_to(&mut creator)?;
    }

    creator.finish_writing()?;
    std::fs::rename(&output_path, dst)?;
    Ok(())
}

fn read_metadata_string(archive: &Archive, name: &str) -> Option<String> {
    let bytes = archive.get_metadata(name).ok()?;
    String::from_utf8(bytes).ok()
}

/// Legacy `Z` held the old Xapian fulltext index (now rebuilt under `X`);
/// copying it as content would re-pack a useless multi-hundred-MB database.
fn is_content_namespace(legacy: bool, namespace: u8) -> bool {
    if legacy {
        !matches!(namespace, b'M' | b'X' | b'Z')
    } else {
        namespace == b'C'
    }
}

fn content_path(legacy: bool, namespace: u8, path: &str) -> Option<String> {
    is_content_namespace(legacy, namespace).then(|| {
        if legacy {
            format!("{}/{path}", char::from(namespace))
        } else {
            path.to_owned()
        }
    })
}
fn ensure_distinct_output(source: &Path, destination: &Path) -> Result<(), zimru::Error> {
    let source_metadata = std::fs::metadata(source)?;
    let destination_metadata = match std::fs::metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    #[cfg(unix)]
    let aliases = {
        use std::os::unix::fs::MetadataExt;
        source_metadata.dev() == destination_metadata.dev()
            && source_metadata.ino() == destination_metadata.ino()
    };
    // Without a portable file-identity API, refuse existing destinations
    // conservatively rather than risk replacing an alias of the input.
    #[cfg(not(unix))]
    let aliases = {
        let _ = (source_metadata, destination_metadata);
        true
    };
    if aliases {
        return Err(zimru::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source and destination must be distinct files",
        )));
    }
    Ok(())
}

fn make_output_tmp_dir(destination: &Path) -> Result<PathBuf, zimru::Error> {
    let parent = destination.parent().unwrap_or(Path::new("."));
    let pid = std::process::id();
    for sequence in 0u64.. {
        let dir = parent.join(format!(".zimrecreate-{pid}-{sequence}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("temporary directory sequence exhausted")
}

fn parse_illustration_path(path: &str) -> Option<u32> {
    // Only "Illustration_NxN@1" maps onto `add_illustration` (whose key is
    // always `@1`); other scales are copied verbatim as metadata so they
    // neither collide with the @1 key nor get silently dropped.
    let rest = path.strip_prefix("Illustration_")?;
    let dims = rest.strip_suffix("@1")?;
    let (w, h) = dims.split_once('x')?;
    let w: u32 = w.parse().ok()?;
    let h: u32 = h.parse().ok()?;
    if w == h {
        Some(w)
    } else {
        None
    }
}
