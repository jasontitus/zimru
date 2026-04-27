//! ZIM file writer.
//!
//! Mirrors the shape of libzim's `Creator`:
//!
//! ```no_run
//! use zimru::writer::{Creator, Item};
//! let mut c = Creator::new();
//! c.set_main_path("home");
//! c.add_item(Item::html("home", "Home", "<h1>Hi</h1>"));
//! c.add_metadata("Title", "Demo");
//! c.add_metadata("Language", "eng");
//! c.write_to("demo.zim")?;
//! # Ok::<(), zimru::Error>(())
//! ```
//!
//! Output: version `5.1` (new namespaces: `C` for content, `M` metadata,
//! `W` well-known, `X` indexes), in-header URL + title pointer lists, MD5
//! trailer. Readable by libzim 8+ and by `zimru` itself. Cross-validated
//! with upstream `zimcheck -A`.
//!
//! Bin-packing strategy: items are grouped greedily until the running
//! payload reaches `cluster_size_target` bytes (default 2 MiB, matching
//! upstream `zimwriterfs`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read as _, Seek, SeekFrom, Write as _};
use std::path::Path;

use md5::{Digest, Md5};
use rayon::prelude::*;

use crate::cluster::Compression;
use crate::error::{Error, Result};
use crate::header::{HEADER_SIZE, MAGIC};

/// Default target size for one cluster's worth of decompressed payload
/// (2 MiB — same as libzim's zimwriterfs default).
pub const DEFAULT_CLUSTER_SIZE_TARGET: usize = 2 * 1024 * 1024;

/// Default mimetype recorded for [`Creator::add_metadata`] entries when
/// the caller does not specify one. Most ZIM metadata keys are short
/// UTF-8 strings (Title, Language, Description, …); the explicit-mime
/// variant [`Creator::add_metadata_with_mimetype`] is for the handful
/// that aren't (e.g. the per-favicon PNG metadata used by old archives).
pub const DEFAULT_METADATA_MIMETYPE: &str = "text/plain;charset=utf-8";

/// A content item to add to the archive.
#[derive(Debug, Clone)]
pub struct Item {
    pub path: String,
    pub title: String,
    pub mimetype: String,
    pub content: Vec<u8>,
}

impl Item {
    pub fn new(
        path: impl Into<String>,
        title: impl Into<String>,
        mimetype: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            path: path.into(),
            title: title.into(),
            mimetype: mimetype.into(),
            content: content.into(),
        }
    }
    pub fn html(
        path: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self::new(path, title, "text/html", content)
    }
    pub fn text(
        path: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self::new(path, title, "text/plain", content)
    }
    pub fn png(
        path: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        Self::new(path, title, "image/png", content)
    }
}

/// A redirect alias. Resolved to a URL-pointer index at finalize time.
#[derive(Debug, Clone)]
pub struct Redirection {
    pub path: String,
    pub title: String,
    pub target_path: String,
}

/// One metadata entry buffered by [`Creator`]. Carries a per-entry mimetype
/// so non-text metadata (PNG favicons on legacy archives, etc.) round-trips
/// faithfully — the historical default is [`DEFAULT_METADATA_MIMETYPE`].
#[derive(Debug, Clone)]
pub struct MetadataEntry {
    pub name: String,
    pub mimetype: String,
    pub value: Vec<u8>,
}

/// High-level ZIM file builder.
pub struct Creator {
    items: Vec<Item>,
    redirections: Vec<Redirection>,
    metadata: Vec<MetadataEntry>,
    illustrations: Vec<(u32, Vec<u8>)>,
    main_path: Option<String>,
    compression: Compression,
    compression_level: Option<i32>,
    cluster_size_target: usize,
    uuid: [u8; 16],
}

impl Default for Creator {
    fn default() -> Self {
        Self::new()
    }
}

impl Creator {
    pub fn new() -> Self {
        Creator {
            items: Vec::new(),
            redirections: Vec::new(),
            metadata: Vec::new(),
            illustrations: Vec::new(),
            main_path: None,
            compression: Compression::Zstd,
            compression_level: None,
            cluster_size_target: DEFAULT_CLUSTER_SIZE_TARGET,
            uuid: default_uuid(),
        }
    }

    pub fn add_item(&mut self, item: Item) -> &mut Self {
        self.items.push(item);
        self
    }

    pub fn add_redirection(
        &mut self,
        path: impl Into<String>,
        title: impl Into<String>,
        target: impl Into<String>,
    ) -> &mut Self {
        self.redirections.push(Redirection {
            path: path.into(),
            title: title.into(),
            target_path: target.into(),
        });
        self
    }

    /// Metadata entry, stored in the `M` namespace. The recorded mimetype
    /// is [`DEFAULT_METADATA_MIMETYPE`] (`text/plain;charset=utf-8`) — for
    /// non-text metadata (PNG favicons on legacy archives, etc.) call
    /// [`Creator::add_metadata_with_mimetype`] instead.
    pub fn add_metadata(
        &mut self,
        name: impl Into<String>,
        value: impl Into<Vec<u8>>,
    ) -> &mut Self {
        self.add_metadata_with_mimetype(name, DEFAULT_METADATA_MIMETYPE, value)
    }

    /// Metadata entry with an explicit mimetype. The mimetype is stored
    /// verbatim in the dirent's mimetype index — callers are responsible
    /// for choosing a value that downstream readers will recognise.
    pub fn add_metadata_with_mimetype(
        &mut self,
        name: impl Into<String>,
        mimetype: impl Into<String>,
        value: impl Into<Vec<u8>>,
    ) -> &mut Self {
        self.metadata.push(MetadataEntry {
            name: name.into(),
            mimetype: mimetype.into(),
            value: value.into(),
        });
        self
    }

    /// ZIM illustration (PNG) of the given side length. Stored at
    /// `M/Illustration_NxN@1`.
    pub fn add_illustration(&mut self, side: u32, png: impl Into<Vec<u8>>) -> &mut Self {
        self.illustrations.push((side, png.into()));
        self
    }

    /// Declare the archive's main page. A `W/mainPage` redirect to this
    /// path is written at finalize time, matching how upstream tools
    /// expect to find the main page.
    pub fn set_main_path(&mut self, path: impl Into<String>) -> &mut Self {
        self.main_path = Some(path.into());
        self
    }

    pub fn set_compression(&mut self, c: Compression) -> &mut Self {
        self.compression = c;
        self
    }

    /// Set the compression level for the active algorithm.
    ///
    /// Range and meaning depend on the algorithm:
    /// * `Compression::Zstd` — accepts `1..=22` (libzstd's normal range, plus
    ///   negative "fast" levels). The default is `3`.
    /// * `Compression::Xz`   — accepts `0..=9`. The default is `3`.
    /// * `Compression::None` — ignored.
    ///
    /// Pass `None` (or never call this) to use the default for the algorithm.
    pub fn set_compression_level(&mut self, level: i32) -> &mut Self {
        self.compression_level = Some(level);
        self
    }

    pub fn set_cluster_size_target(&mut self, bytes: usize) -> &mut Self {
        self.cluster_size_target = bytes.max(1);
        self
    }

    pub fn set_uuid(&mut self, uuid: impl Into<crate::Uuid>) -> &mut Self {
        self.uuid = uuid.into().into_bytes();
        self
    }

    /// Materialize the archive to `path`. Consumes the builder.
    pub fn write_to(self, path: impl AsRef<Path>) -> Result<()> {
        // Open read+write so the streaming writer can seek-back to
        // overwrite cluster_ptrs and the header's late-known fields,
        // and re-read the finished file at the end to compute the
        // MD5 trailer (avoids buffering every cluster in RAM just
        // so we know its length up front).
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path.as_ref())?;
        finalize(self, file)
    }
}

// ---------- internals ----------

/// One to-be-written payload with all addressing metadata. Lives only
/// during `finalize`.
struct Payload {
    namespace: u8,
    url: String,
    title: String,
    mimetype: String,
    content: Vec<u8>,
}

#[derive(Debug, Clone)]
enum RawDirent {
    Article {
        namespace: u8,
        url: String,
        title: String,
        mime_idx: u16,
        cluster: u32,
        blob: u32,
    },
    Redirect {
        namespace: u8,
        url: String,
        title: String,
        target_ns: u8,
        target_url: String,
        resolved_index: Option<u32>,
    },
}

impl RawDirent {
    fn namespace(&self) -> u8 {
        match self {
            RawDirent::Article { namespace, .. } | RawDirent::Redirect { namespace, .. } => {
                *namespace
            }
        }
    }
    fn url(&self) -> &str {
        match self {
            RawDirent::Article { url, .. } | RawDirent::Redirect { url, .. } => url,
        }
    }
    fn title(&self) -> &str {
        match self {
            RawDirent::Article { title, .. } | RawDirent::Redirect { title, .. } => title,
        }
    }
}

fn intern_mime(m: &str, mimes: &mut Vec<String>, index: &mut BTreeMap<String, u16>) -> u16 {
    if let Some(&i) = index.get(m) {
        return i;
    }
    let i = mimes.len() as u16;
    mimes.push(m.to_string());
    index.insert(m.to_string(), i);
    i
}

fn finalize(builder: Creator, mut file: File) -> Result<()> {
    let Creator {
        items,
        redirections,
        metadata,
        illustrations,
        main_path,
        compression,
        compression_level,
        cluster_size_target,
        uuid,
    } = builder;

    // 1. Collect every payload. An empty title is normalised to the
    //    url here so the dirent's title field matches the on-disk
    //    convention "empty title bytes ⇒ title is the url" — without
    //    this, the writer's title-order sort sees `""` (sorts before
    //    every non-empty title) but the reader falls back to url on
    //    parse, leaving the title-pointer list out of sync with what
    //    `zimcheck -I` and downstream readers expect ("Title index is
    //    not properly sorted"). Most callers (libzim's `zimwriterfs`
    //    among them) pass an empty title for non-HTML items where
    //    they want the filename to surface.
    let mut payloads: Vec<Payload> = Vec::new();
    for it in items {
        let title = if it.title.is_empty() { it.path.clone() } else { it.title };
        payloads.push(Payload {
            namespace: b'C',
            url: it.path,
            title,
            mimetype: it.mimetype,
            content: it.content,
        });
    }
    for m in metadata {
        payloads.push(Payload {
            namespace: b'M',
            url: m.name.clone(),
            title: m.name,
            mimetype: m.mimetype,
            content: m.value,
        });
    }
    for (side, png) in illustrations {
        let url = format!("Illustration_{side}x{side}@1");
        payloads.push(Payload {
            namespace: b'M',
            url: url.clone(),
            title: url,
            mimetype: "image/png".to_string(),
            content: png,
        });
    }

    // Collect redirections (including the optional W/mainPage one).
    let mut pending_redirects: Vec<RawDirent> = Vec::new();
    for r in redirections {
        let title = if r.title.is_empty() { r.path.clone() } else { r.title };
        pending_redirects.push(RawDirent::Redirect {
            namespace: b'C',
            url: r.path,
            title,
            target_ns: b'C',
            target_url: r.target_path,
            resolved_index: None,
        });
    }
    if let Some(m) = &main_path {
        pending_redirects.push(RawDirent::Redirect {
            namespace: b'W',
            url: "mainPage".to_string(),
            title: "mainPage".to_string(),
            target_ns: b'C',
            target_url: m.clone(),
            resolved_index: None,
        });
    }

    // 2. Sort payloads by (namespace, url) — keeps URL-pointer order and
    //    cluster order consistent for a stable on-disk layout across runs.
    payloads.sort_by(|a, b| {
        a.namespace
            .cmp(&b.namespace)
            .then_with(|| a.url.cmp(&b.url))
    });

    // 3. Bin-pack payloads into clusters.
    let mut groups: Vec<Vec<Payload>> = Vec::new();
    {
        let mut cur: Vec<Payload> = Vec::new();
        let mut cur_bytes: usize = 0;
        for p in payloads {
            let plen = p.content.len();
            if !cur.is_empty() && cur_bytes + plen > cluster_size_target {
                groups.push(std::mem::take(&mut cur));
                cur_bytes = 0;
            }
            cur_bytes += plen;
            cur.push(p);
        }
        if !cur.is_empty() {
            groups.push(cur);
        }
    }

    // 4a. Pass 1: walk groups serially to build dirents + intern mime types.
    //     This pass is fast (just metadata bookkeeping). The expensive
    //     cluster-encoding work is split out to a parallel pass below.
    let mut mimes: Vec<String> = Vec::new();
    let mut mime_index: BTreeMap<String, u16> = BTreeMap::new();
    let mut dirents: Vec<RawDirent> = Vec::new();
    let mut blob_groups: Vec<Vec<Vec<u8>>> = Vec::with_capacity(groups.len());

    for (ci, group) in groups.into_iter().enumerate() {
        let ci = ci as u32;
        let mut blobs_for_cluster: Vec<Vec<u8>> = Vec::with_capacity(group.len());
        for (bi, p) in group.into_iter().enumerate() {
            let bi = bi as u32;
            let mime_idx = intern_mime(&p.mimetype, &mut mimes, &mut mime_index);
            dirents.push(RawDirent::Article {
                namespace: p.namespace,
                url: p.url,
                title: p.title,
                mime_idx,
                cluster: ci,
                blob: bi,
            });
            blobs_for_cluster.push(p.content);
        }
        blob_groups.push(blobs_for_cluster);
    }

    // 4b. (Cluster encoding deferred to streaming write step 13b below
    //     so we don't have to hold every encoded cluster's bytes
    //     resident before starting to write. blob_groups stays alive
    //     through the dirent-sort step; freed progressively as we
    //     stream-encode-and-write.)
    let cluster_count_total = blob_groups.len() as u32;

    // 5. Append pending redirects.
    dirents.extend(pending_redirects);

    // 6. Sort all dirents by (ns, url) → URL-pointer order.
    dirents.sort_by(|a, b| {
        a.namespace()
            .cmp(&b.namespace())
            .then_with(|| a.url().cmp(b.url()))
    });

    // 7. Resolve redirect targets to url-pointer indices.
    for i in 0..dirents.len() {
        // Extract the target, look it up, stash it back.
        if let RawDirent::Redirect {
            target_ns,
            target_url,
            ..
        } = &dirents[i]
        {
            let ns = *target_ns;
            let url = target_url.clone();
            let resolved = dirents
                .binary_search_by(|d| d.namespace().cmp(&ns).then_with(|| d.url().cmp(&url)))
                .ok()
                .map(|j| j as u32);
            match &mut dirents[i] {
                RawDirent::Redirect { resolved_index, .. } => *resolved_index = resolved,
                _ => unreachable!(),
            }
        }
    }
    // Any redirect whose target wasn't found is a user error.
    for d in &dirents {
        if let RawDirent::Redirect {
            resolved_index: None,
            url,
            target_ns,
            target_url,
            ..
        } = d
        {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "redirect {} -> {}/{} has no target",
                    url,
                    char::from(*target_ns),
                    target_url
                ),
            )));
        }
    }

    // 8. Title-pointer order: dirent indices sorted by (ns, title).
    let mut title_order: Vec<u32> = (0..dirents.len() as u32).collect();
    title_order.sort_by(|&a, &b| {
        let da = &dirents[a as usize];
        let db = &dirents[b as usize];
        da.namespace()
            .cmp(&db.namespace())
            .then_with(|| da.title().cmp(db.title()))
    });

    // 9. Compute layout offsets. cluster_ptr_pos is fixed (it's a
    //    function of mime_list/url_ptr/title_ptr sizes), but the
    //    *values* at cluster_ptr_pos and the file's checksum_pos are
    //    only known after we've stream-encoded every cluster — so we
    //    write placeholder zeros first and seek-back-and-fix later.
    let entry_count = dirents.len() as u32;
    let cluster_count = cluster_count_total;
    let mime_list_bytes = encode_mime_list(&mimes);

    let mime_list_pos = HEADER_SIZE as u64;
    let url_ptr_pos = mime_list_pos + mime_list_bytes.len() as u64;
    let title_ptr_pos = url_ptr_pos + (entry_count as u64) * 8;
    let cluster_ptr_pos = title_ptr_pos + (entry_count as u64) * 4;
    let dirents_pos = cluster_ptr_pos + (cluster_count as u64) * 8;

    // 10. Render dirents, compute their absolute offsets.
    let mut dirent_blobs: Vec<Vec<u8>> = Vec::with_capacity(dirents.len());
    let mut dirent_offsets: Vec<u64> = Vec::with_capacity(dirents.len());
    let mut cursor = dirents_pos;
    for d in &dirents {
        let bytes = encode_dirent(d);
        dirent_offsets.push(cursor);
        cursor += bytes.len() as u64;
        dirent_blobs.push(bytes);
    }
    let clusters_pos = cursor;

    // 11. Main page index (as stored in the header).
    let main_page_idx = if main_path.is_some() {
        dirents
            .binary_search_by(|d| {
                d.namespace()
                    .cmp(&b'W')
                    .then_with(|| d.url().cmp("mainPage"))
            })
            .ok()
            .map(|i| i as u32)
            .unwrap_or(u32::MAX)
    } else {
        u32::MAX
    };

    // 12. First-pass write: header + all metadata + dirents, with
    //     cluster_ptr region as zero placeholders + checksum_pos = 0.
    //     Hashing for MD5 happens during a second pass at the end
    //     because we'll seek-back-and-fix the cluster_ptrs and the
    //     header's `cluster_ptr_pos` / `checksum_pos` fields after
    //     the cluster bytes are streamed.
    let placeholder_header = encode_header(&HeaderFields {
        major_version: 5,
        minor_version: 1,
        uuid,
        entry_count,
        cluster_count,
        url_ptr_pos,
        title_ptr_pos,
        cluster_ptr_pos,
        mime_list_pos,
        main_page: main_page_idx,
        checksum_pos: 0,
    });
    file.write_all(&placeholder_header)?;
    file.write_all(&mime_list_bytes)?;
    for off in &dirent_offsets {
        file.write_all(&off.to_le_bytes())?;
    }
    for idx in &title_order {
        file.write_all(&idx.to_le_bytes())?;
    }
    let cluster_ptrs_zero = vec![0u64; cluster_count as usize];
    for off in &cluster_ptrs_zero {
        file.write_all(&off.to_le_bytes())?;
    }
    drop(cluster_ptrs_zero);
    for b in &dirent_blobs {
        file.write_all(b)?;
    }
    drop(dirent_blobs);
    drop(dirent_offsets);

    // 13. Stream-encode-and-write each cluster. Encode in
    //     parallel-bounded batches so we hold at most
    //     `chunk_size × cluster_size` of compressed bytes resident at
    //     once. Each batch's source `blob_groups` slot is also freed
    //     after encode (via `mem::take`) so the input-side memory
    //     drains progressively as we stream.
    let chunk_size = rayon::current_num_threads().max(1);
    let mut cluster_offsets: Vec<u64> = Vec::with_capacity(cluster_count as usize);
    let mut current_pos = clusters_pos;
    let mut idx = 0usize;
    while idx < blob_groups.len() {
        let end = (idx + chunk_size).min(blob_groups.len());
        // Take ownership of this chunk's blobs so we can free them
        // after encode (the slots in blob_groups become empty Vecs).
        let chunk: Vec<Vec<Vec<u8>>> = blob_groups[idx..end]
            .iter_mut()
            .map(std::mem::take)
            .collect();
        let encoded: Vec<Vec<u8>> = chunk
            .into_par_iter()
            .map(|blobs| encode_cluster(&blobs, compression, compression_level))
            .collect::<Result<Vec<_>>>()?;
        for bytes in encoded {
            cluster_offsets.push(current_pos);
            current_pos += bytes.len() as u64;
            file.write_all(&bytes)?;
        }
        idx = end;
    }
    drop(blob_groups);
    let checksum_pos = current_pos;

    // 14. Seek-back fixes for the cluster-ptr region and the header's
    //     two computed-late fields.
    file.seek(SeekFrom::Start(cluster_ptr_pos))?;
    for off in &cluster_offsets {
        file.write_all(&off.to_le_bytes())?;
    }
    drop(cluster_offsets);
    file.seek(SeekFrom::Start(0))?;
    let final_header = encode_header(&HeaderFields {
        major_version: 5,
        minor_version: 1,
        uuid,
        entry_count,
        cluster_count,
        url_ptr_pos,
        title_ptr_pos,
        cluster_ptr_pos,
        mime_list_pos,
        main_page: main_page_idx,
        checksum_pos,
    });
    file.write_all(&final_header)?;
    file.flush()?;

    // 15. Compute MD5 by streaming-reading the now-final file from
    //     start to checksum_pos. SSDs do this at ~1 GB/s so the
    //     overhead is small relative to the cluster-encoding time
    //     we just saved by not buffering everything in RAM first.
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut hashed: u64 = 0;
    while hashed < checksum_pos {
        let want = ((checksum_pos - hashed) as usize).min(buf.len());
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        hashed += n as u64;
    }
    let digest: [u8; 16] = hasher.finalize().into();
    file.seek(SeekFrom::Start(checksum_pos))?;
    file.write_all(&digest)?;
    file.flush()?;
    Ok(())
}

struct HeaderFields {
    major_version: u16,
    minor_version: u16,
    uuid: [u8; 16],
    entry_count: u32,
    cluster_count: u32,
    url_ptr_pos: u64,
    title_ptr_pos: u64,
    cluster_ptr_pos: u64,
    mime_list_pos: u64,
    main_page: u32,
    checksum_pos: u64,
}

fn encode_header(h: &HeaderFields) -> Vec<u8> {
    let mut buf = vec![0u8; HEADER_SIZE];
    buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    buf[4..6].copy_from_slice(&h.major_version.to_le_bytes());
    buf[6..8].copy_from_slice(&h.minor_version.to_le_bytes());
    buf[8..24].copy_from_slice(&h.uuid);
    buf[24..28].copy_from_slice(&h.entry_count.to_le_bytes());
    buf[28..32].copy_from_slice(&h.cluster_count.to_le_bytes());
    buf[32..40].copy_from_slice(&h.url_ptr_pos.to_le_bytes());
    buf[40..48].copy_from_slice(&h.title_ptr_pos.to_le_bytes());
    buf[48..56].copy_from_slice(&h.cluster_ptr_pos.to_le_bytes());
    buf[56..64].copy_from_slice(&h.mime_list_pos.to_le_bytes());
    buf[64..68].copy_from_slice(&h.main_page.to_le_bytes());
    buf[68..72].copy_from_slice(&u32::MAX.to_le_bytes()); // layout_page (deprecated)
    buf[72..80].copy_from_slice(&h.checksum_pos.to_le_bytes());
    buf
}

fn encode_mime_list(mimes: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in mimes {
        out.extend_from_slice(m.as_bytes());
        out.push(0);
    }
    out.push(0); // terminator
    out
}

fn encode_dirent(d: &RawDirent) -> Vec<u8> {
    let mut out = Vec::new();
    match d {
        RawDirent::Article {
            namespace,
            url,
            title,
            mime_idx,
            cluster,
            blob,
        } => {
            out.extend_from_slice(&mime_idx.to_le_bytes());
            out.push(0); // parameter_len
            out.push(*namespace);
            out.extend_from_slice(&0u32.to_le_bytes()); // revision
            out.extend_from_slice(&cluster.to_le_bytes());
            out.extend_from_slice(&blob.to_le_bytes());
            out.extend_from_slice(url.as_bytes());
            out.push(0);
            if title != url {
                out.extend_from_slice(title.as_bytes());
            }
            out.push(0);
        }
        RawDirent::Redirect {
            namespace,
            url,
            title,
            resolved_index,
            ..
        } => {
            let idx = resolved_index.unwrap_or(0);
            out.extend_from_slice(&0xFFFFu16.to_le_bytes());
            out.push(0);
            out.push(*namespace);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&idx.to_le_bytes());
            out.extend_from_slice(url.as_bytes());
            out.push(0);
            if title != url {
                out.extend_from_slice(title.as_bytes());
            }
            out.push(0);
        }
    }
    out
}

fn encode_cluster(
    blobs: &[Vec<u8>],
    compression: Compression,
    level: Option<i32>,
) -> Result<Vec<u8>> {
    let n = blobs.len();
    let payload_bytes_sum: usize = blobs.iter().map(|b| b.len()).sum();
    // 4-byte offsets unless the total would overflow u32.
    let ptr_size_4 = (n + 1) * 4;
    let total_4 = ptr_size_4 + payload_bytes_sum;
    let extended = total_4 > u32::MAX as usize;
    let ptr_size = if extended { 8 } else { 4 };
    let header_len = (n + 1) * ptr_size;

    let mut payload = Vec::with_capacity(header_len + payload_bytes_sum);
    let mut cursor: u64 = header_len as u64;
    for b in blobs {
        push_offset(&mut payload, cursor, extended);
        cursor += b.len() as u64;
    }
    push_offset(&mut payload, cursor, extended);
    for b in blobs {
        payload.extend_from_slice(b);
    }

    // When the caller didn't pin a compression level, honour
    // `ZSTD_CLEVEL` / `XZ_DEFAULTS` env vars so cross-stack tooling
    // can configure both real libzim (which respects libzstd's env)
    // and zimru with the same one-liner. Falls back to a fast level
    // 3 default for zstd / 3 for xz when no env var is set.
    fn env_zstd_level() -> Option<i32> {
        std::env::var("ZSTD_CLEVEL").ok().and_then(|v| v.parse().ok())
    }
    let (compression_id, body) = match compression {
        Compression::None => (1u8, payload),
        Compression::Zstd => {
            let lvl = level.or_else(env_zstd_level).unwrap_or(3);
            (
                5u8,
                zstd::stream::encode_all(&payload[..], lvl)
                    .map_err(|e| Error::Decompression(format!("zstd encode: {e}")))?,
            )
        }
        Compression::Xz => {
            let lvl = level.unwrap_or(3).clamp(0, 9) as u32;
            (4u8, {
                let mut enc = xz2::write::XzEncoder::new(Vec::new(), lvl);
                enc.write_all(&payload)
                    .map_err(|e| Error::Decompression(format!("xz encode: {e}")))?;
                enc.finish()
                    .map_err(|e| Error::Decompression(format!("xz finish: {e}")))?
            })
        }
    };

    let info_byte = compression_id | (if extended { 0x10 } else { 0 });
    let mut bytes = Vec::with_capacity(1 + body.len());
    bytes.push(info_byte);
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

fn push_offset(v: &mut Vec<u8>, x: u64, extended: bool) {
    if extended {
        v.extend_from_slice(&x.to_le_bytes());
    } else {
        v.extend_from_slice(&(x as u32).to_le_bytes());
    }
}

fn default_uuid() -> [u8; 16] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let pid = std::process::id() as u64;
    let mut u = [0u8; 16];
    u[..8].copy_from_slice(&nanos.to_le_bytes());
    u[8..].copy_from_slice(&pid.wrapping_mul(0x9E3779B97F4A7C15).to_le_bytes());
    u
}
