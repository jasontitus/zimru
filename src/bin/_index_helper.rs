//! Tiny subprocess wrapper used by `zimwriterfs` and `zimrecreate` to
//! drive the (separately-licensed) `xapianbuilder` binary.
//!
//! ## Why a separate binary?
//!
//! `xapianbuilder` is GPL-3-or-later (it links GPL `xapian-core` and
//! vendors GPL libzim parser code); zimru is MIT. Spawning the helper
//! as a child process and communicating only via JSONL on stdin and a
//! file on disk keeps the GPL boundary at the OS process edge — the
//! standard FSF-blessed pattern, same as `gcc` calling `as`/`ld`. We
//! deliberately avoid any in-process linkage. See xapianbuilder's
//! README for the contract.
//!
//! ## Behaviour
//!
//! - Locates the binary via `$XAPIANBUILDER` env var, then `$PATH`.
//! - If absent or unrunnable, returns `IndexHelper::Disabled` and the
//!   caller proceeds without an index (a valid ZIM without `X/*`).
//! - Spawns one process per index (fulltext, title) at construction
//!   time and holds open stdin pipes. Callers feed each entry exactly
//!   once via `feed_*`. Output is written to a temp file passed on the
//!   CLI; on `finish()` we close stdin, wait, and read the file back.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

/// Output of one `xapianbuilder` run. The built index stays in its
/// temp file — a fulltext DB is multi-GB on large archives, so it is
/// streamed into the writer in bounded chunks (see [`IndexBlob::add_to`])
/// instead of being slurped into one `Vec<u8>`.
pub struct IndexBlob {
    /// ZIM URL (without namespace), e.g. "fulltext/xapian".
    pub url: &'static str,
    pub mimetype: &'static str,
    /// Temp file holding the built index. Owned by the caller's temp
    /// dir; consumed by [`IndexBlob::add_to`].
    pub path: PathBuf,
    /// Byte size of `path`.
    pub size: u64,
}

impl IndexBlob {
    /// Stream this blob's temp file into the writer as an uncompressed
    /// `X`-namespace item, in bounded chunks.
    pub fn add_to(&self, creator: &mut zimru::writer::Creator) -> Result<(), zimru::Error> {
        let mut f = std::fs::File::open(&self.path)?;
        zimru::io_hints::hint_sequential(&f);
        creator.begin_chunked_item(
            Some(b'X'),
            self.url.to_string(),
            String::new(),
            self.mimetype.to_string(),
            Some(self.size),
            Some(false),
        )?;
        let mut buf = vec![0u8; 4 * 1024 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            creator.chunked_item_chunk(&buf[..n])?;
        }
        creator.end_chunked_item()
    }
}

/// Removes a temp directory (recursively) when dropped, so early error
/// returns in the callers don't strand `.{stem}.xapianbuilder.{pid}`
/// directories on disk.
pub struct TmpDirCleanup(pub PathBuf);

impl Drop for TmpDirCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One running `xapianbuilder` child + its stdin handle + the temp
/// path it'll write to.
struct Job {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Drains the child's piped stderr concurrently with feeding, so a
    /// chatty child can never fill the ~64 KB pipe buffer, stop reading
    /// stdin, and deadlock our `write_all`.
    stderr_thread: Option<std::thread::JoinHandle<Vec<u8>>>,
    /// Counts writes between `try_wait` liveness probes — see
    /// [`write_raw`].
    writes_since_liveness_check: u32,
    out_path: PathBuf,
    url: &'static str,
}

impl Job {
    fn take_stderr(&mut self) -> Vec<u8> {
        self.stderr_thread
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or_default()
    }
}

/// Public face of the helper. Internally either holds two live
/// children (`State::Active`) or is a no-op (`State::Disabled`); we
/// hide the enum so the private `Job` type doesn't leak into the
/// public API.
pub struct IndexHelper {
    state: State,
}

// One IndexHelper exists per build, so the size gap between the two
// variants (a couple hundred bytes of Job state vs nothing) is
// irrelevant — not worth the indirection of boxing the jobs.
#[allow(clippy::large_enum_variant)]
enum State {
    /// `fulltext` is `None` under upstream's `-j/--withoutFTIndex`, which
    /// suppresses only the fulltext index — the title index is still built.
    Active {
        fulltext: Option<Job>,
        title: Option<Job>,
    },
    Disabled,
}

// JSONL serialisation is hand-rolled to avoid pulling a serde
// dependency into zimru just for the helper protocol. The format is
// trivial: 4-5 string fields, control chars + double-quote +
// backslash escaped per RFC 8259.
fn write_json_string(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for &b in s.as_bytes() {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\x08' => out.extend_from_slice(b"\\b"),
            b'\x0c' => out.extend_from_slice(b"\\f"),
            0x00..=0x1f => {
                out.extend_from_slice(b"\\u00");
                let hi = b >> 4;
                let lo = b & 0x0f;
                out.push(if hi < 10 { b'0' + hi } else { b'a' + hi - 10 });
                out.push(if lo < 10 { b'0' + lo } else { b'a' + lo - 10 });
            }
            _ => out.push(b),
        }
    }
    out.push(b'"');
}

fn encode_title_doc(path: &str, title: &str, target_path: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(path.len() + title.len() + 64);
    buf.push(b'{');
    buf.extend_from_slice(b"\"path\":");
    write_json_string(&mut buf, path);
    buf.extend_from_slice(b",\"title\":");
    write_json_string(&mut buf, title);
    if !target_path.is_empty() {
        buf.extend_from_slice(b",\"target_path\":");
        write_json_string(&mut buf, target_path);
    }
    buf.extend_from_slice(b"}\n");
    buf
}

fn encode_fulltext_doc(
    path: &str,
    title: &str,
    mimetype: &str,
    body: &str,
    language: &str,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(body.len() + path.len() + 128);
    buf.push(b'{');
    buf.extend_from_slice(b"\"path\":");
    write_json_string(&mut buf, path);
    buf.extend_from_slice(b",\"title\":");
    write_json_string(&mut buf, title);
    buf.extend_from_slice(b",\"mimetype\":");
    write_json_string(&mut buf, mimetype);
    buf.extend_from_slice(b",\"language\":");
    write_json_string(&mut buf, language);
    buf.extend_from_slice(b",\"body\":");
    write_json_string(&mut buf, body);
    buf.extend_from_slice(b"}\n");
    buf
}

impl IndexHelper {
    /// Try to spawn both index builders. `language` is the
    /// ISO-639-3 code that goes straight to `--language`. `tmp_dir`
    /// is where the output files are written; the caller cleans it
    /// up.
    pub fn disabled() -> Self {
        IndexHelper {
            state: State::Disabled,
        }
    }

    /// `want_fulltext` / `want_title` select which indexes to build.
    /// Upstream's `-j` clears only the first: `zimrecreate -j` and
    /// `zimwriterfs -j` still emit `X/title/xapian`, which is what the
    /// suggestion box in kiwix-serve reads. Treating `-j` as "no indexes at
    /// all" produces an archive that is missing an entry upstream's would
    /// have, and quietly flatters any benchmark run in that mode.
    pub fn spawn(
        language: &str,
        tmp_dir: &std::path::Path,
        explicit_path: Option<&std::path::Path>,
        verbose: bool,
        want_fulltext: bool,
        want_title: bool,
    ) -> Self {
        if !want_fulltext && !want_title {
            return Self::disabled();
        }
        let bin = match resolve_binary(explicit_path) {
            Some(p) => p,
            None => {
                if verbose {
                    eprintln!(
                        "[zimwriterfs] xapianbuilder not found on $PATH; \
                         building ZIM without search indexes. Set \
                         $XAPIANBUILDER or pass --xapianbuilder-path to enable."
                    );
                }
                return Self::disabled();
            }
        };

        let fulltext_out = tmp_dir.join("fulltext.xapian");
        let title_out = tmp_dir.join("title.xapian");

        let fulltext = if want_fulltext {
            match spawn_one(&bin, "fulltext", &fulltext_out, language) {
                Ok(j) => Some(j),
                Err(e) => {
                    eprintln!(
                        "[zimwriterfs] xapianbuilder fulltext spawn failed: {e}; skipping indexes"
                    );
                    return Self::disabled();
                }
            }
        } else {
            None
        };
        let title = if want_title {
            match spawn_one(&bin, "title", &title_out, language) {
                Ok(j) => Some(j),
                Err(e) => {
                    eprintln!(
                        "[zimwriterfs] xapianbuilder title spawn failed: {e}; skipping indexes"
                    );
                    if let Some(mut fulltext) = fulltext {
                        fulltext.stdin.take();
                        let stderr_thread = fulltext.stderr_thread.take();
                        let _ = fulltext.child.wait_with_killing();
                        if let Some(t) = stderr_thread {
                            let _ = t.join();
                        }
                    }
                    return Self::disabled();
                }
            }
        } else {
            None
        };

        IndexHelper {
            state: State::Active { fulltext, title },
        }
    }

    /// Feed one entry to the title index. Always called for content
    /// entries (title DB indexes everything in namespace C, regardless
    /// of mimetype).
    pub fn feed_title(&mut self, path: &str, title: &str, target_path: &str) {
        let State::Active { title: Some(t), .. } = &mut self.state else {
            return;
        };
        let buf = encode_title_doc(path, title, target_path);
        write_raw(t, &buf);
    }

    /// Feed one entry to the fulltext index. Skip non-HTML entries:
    /// libzim's fulltext indexer only sees `text/html` (everything
    /// else has `hasIndexData() == false`).
    ///
    /// `title` must be the entry's RAW title, exactly as it will be
    /// stored in the dirent. Since xapianbuilder's 2026-06 writer
    /// improvements the helper does its own normalisation for
    /// fulltext mode — callers must not pre-strip/fold titles (doing
    /// so would make the indexed title diverge from the stored one).
    pub fn feed_fulltext(
        &mut self,
        path: &str,
        title: &str,
        mimetype: &str,
        body: &str,
        language: &str,
    ) {
        if !mimetype.starts_with("text/html") {
            return;
        }
        let State::Active {
            fulltext: Some(fulltext),
            ..
        } = &mut self.state
        else {
            return;
        };
        let buf = encode_fulltext_doc(path, title, mimetype, body, language);
        write_raw(fulltext, &buf);
    }

    /// Close stdin on both children, wait, and return the produced
    /// blobs (as temp-file references — see [`IndexBlob::add_to`]). On
    /// any error returns an empty Vec — the caller proceeds without
    /// indexes, matching libzim's "indexes are best-effort" posture.
    pub fn finish(mut self, verbose: bool) -> Vec<IndexBlob> {
        // Take the state out so the `Drop` impl (which kills children
        // on early-error paths) sees `Disabled` and no-ops.
        let State::Active { fulltext, title } = std::mem::replace(&mut self.state, State::Disabled)
        else {
            return Vec::new();
        };

        // Close stdin pipes so the children see EOF and finalise.
        let mut jobs: Vec<Job> = [fulltext, title].into_iter().flatten().collect();
        for j in &mut jobs {
            j.stdin.take();
        }

        let mut blobs = Vec::with_capacity(jobs.len());
        for mut job in jobs {
            let url = job.url;
            let out_path = job.out_path.clone();
            let status = job.child.wait();
            let stderr = job.take_stderr();
            match status {
                Ok(status) if status.success() => match std::fs::metadata(&out_path) {
                    Ok(m) if m.len() > 0 => {
                        if verbose {
                            eprintln!("[zimwriterfs] xapianbuilder {url}: {} bytes", m.len());
                        }
                        blobs.push(IndexBlob {
                            url,
                            mimetype: "application/octet-stream+xapian",
                            path: out_path,
                            size: m.len(),
                        });
                    }
                    Ok(_) => {
                        if verbose {
                            eprintln!("[zimwriterfs] xapianbuilder {url}: empty output, skipping");
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[zimwriterfs] xapianbuilder {url}: cannot stat {}: {e}",
                            out_path.display()
                        );
                    }
                },
                Ok(status) => {
                    eprintln!(
                        "[zimwriterfs] xapianbuilder {url} exited {}: {}",
                        status,
                        String::from_utf8_lossy(&stderr).trim_end()
                    );
                }
                Err(e) => {
                    eprintln!("[zimwriterfs] xapianbuilder {url} wait failed: {e}");
                }
            }
        }
        blobs
    }
}

impl Drop for IndexHelper {
    /// Early-error cleanup: without this, a `?` return in the caller
    /// after `spawn` leaks two un-reaped `xapianbuilder` children (with
    /// nobody ever reading their output). The normal path goes through
    /// `finish`, which replaces the state with `Disabled` first.
    fn drop(&mut self) {
        if let State::Active { fulltext, title } = &mut self.state {
            for job in [fulltext, title].into_iter().flatten() {
                job.stdin.take();
                let _ = job.child.kill();
                let _ = job.child.wait();
                let _ = job.take_stderr();
            }
        }
    }
}

fn write_raw(job: &mut Job, buf: &[u8]) {
    if job.stdin.is_none() {
        return;
    }
    // A child that already exited will never drain the pipe again —
    // stop feeding instead of blocking in `write_all` once the 64 KB
    // pipe buffer fills. A dead direct child is normally caught by the
    // EPIPE error path below, so this probe only backstops the case
    // where another process still holds the pipe's read end (e.g. an
    // inherited fd in a grandchild); probing every N writes keeps the
    // waitpid syscall off the per-document hot path (feeds number in
    // the tens of millions on large archives).
    job.writes_since_liveness_check += 1;
    if job.writes_since_liveness_check >= 256 {
        job.writes_since_liveness_check = 0;
        if let Ok(Some(status)) = job.child.try_wait() {
            eprintln!(
                "[zimwriterfs] xapianbuilder {} exited early ({status}); disabling feed",
                job.url
            );
            job.stdin.take();
            return;
        }
    }
    let Some(stdin) = job.stdin.as_mut() else {
        return;
    };
    if let Err(e) = stdin.write_all(buf) {
        eprintln!(
            "[zimwriterfs] xapianbuilder {} stdin write failed: {e} (child likely exited)",
            job.url
        );
        // Drop the stdin handle — no point writing further.
        job.stdin.take();
    }
}

fn spawn_one(
    bin: &std::path::Path,
    subcommand: &'static str,
    out_path: &std::path::Path,
    language: &str,
) -> std::io::Result<Job> {
    let url: &'static str = match subcommand {
        "fulltext" => "fulltext/xapian",
        "title" => "title/xapian",
        _ => unreachable!(),
    };
    let mut cmd = Command::new(bin);
    cmd.arg(subcommand)
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(out_path)
        .arg("--quiet");
    if !language.is_empty() {
        cmd.arg("--language").arg(language);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    let stdin = child.stdin.take();
    // Drain stderr from a dedicated thread for the whole feeding phase
    // — leaving it in the pipe until `finish` would let a chatty child
    // fill the pipe buffer, block on its own stderr writes, stop
    // reading stdin, and deadlock the parent.
    let stderr_thread = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    Ok(Job {
        child,
        stdin,
        stderr_thread,
        writes_since_liveness_check: 0,
        out_path: out_path.to_path_buf(),
        url,
    })
}

fn resolve_binary(explicit: Option<&std::path::Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.exists().then(|| p.to_path_buf());
    }
    if let Ok(env_path) = std::env::var("XAPIANBUILDER") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }
    // Fall back to PATH lookup.
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("xapianbuilder");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

// `wait_with_killing` is a small extension trait for the Child handle
// we use in error paths to ensure we don't leak a half-spawned helper.
trait ChildExt {
    fn wait_with_killing(self) -> std::io::Result<std::process::ExitStatus>;
}
impl ChildExt for Child {
    fn wait_with_killing(mut self) -> std::io::Result<std::process::ExitStatus> {
        let _ = self.kill();
        self.wait()
    }
}
