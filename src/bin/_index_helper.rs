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

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

/// Buffer in front of each child's stdin pipe. One entry-doc per
/// `write(2)` syscall (the old shape) costs millions of pipe writes on
/// a Wikipedia-scale build; batching to 256 KiB amortises that to a
/// few writes per MB of index feed.
const STDIN_BUF_CAP: usize = 256 * 1024;

/// Output of one `xapianbuilder` run.
pub struct IndexBlob {
    /// ZIM URL (without namespace), e.g. "fulltext/xapian".
    pub url: &'static str,
    pub mimetype: &'static str,
    pub bytes: Vec<u8>,
}

/// One running `xapianbuilder` child + its stdin handle + the temp
/// path it'll write to.
struct Job {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    out_path: PathBuf,
    url: &'static str,
    /// Reusable per-job serialisation buffer — one allocation for the
    /// whole build instead of a fresh `Vec` per fed entry.
    scratch: Vec<u8>,
}

/// Public face of the helper. Internally either holds two live
/// children (`State::Active`) or is a no-op (`State::Disabled`); we
/// hide the enum so the private `Job` type doesn't leak into the
/// public API.
pub struct IndexHelper {
    state: State,
}

// One IndexHelper exists per process, so the size gap between the
// variants (Job grew a BufWriter + scratch Vec) is irrelevant.
#[allow(clippy::large_enum_variant)]
enum State {
    Active { fulltext: Job, title: Job },
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

fn encode_title_doc(buf: &mut Vec<u8>, path: &str, title: &str, target_path: &str) {
    buf.clear();
    buf.push(b'{');
    buf.extend_from_slice(b"\"path\":");
    write_json_string(buf, path);
    buf.extend_from_slice(b",\"title\":");
    write_json_string(buf, title);
    if !target_path.is_empty() {
        buf.extend_from_slice(b",\"target_path\":");
        write_json_string(buf, target_path);
    }
    buf.extend_from_slice(b"}\n");
}

fn encode_fulltext_doc(
    buf: &mut Vec<u8>,
    path: &str,
    title: &str,
    mimetype: &str,
    body: &str,
    language: &str,
) {
    buf.clear();
    buf.reserve(body.len() + path.len() + 128);
    buf.push(b'{');
    buf.extend_from_slice(b"\"path\":");
    write_json_string(buf, path);
    buf.extend_from_slice(b",\"title\":");
    write_json_string(buf, title);
    buf.extend_from_slice(b",\"mimetype\":");
    write_json_string(buf, mimetype);
    buf.extend_from_slice(b",\"language\":");
    write_json_string(buf, language);
    buf.extend_from_slice(b",\"body\":");
    write_json_string(buf, body);
    buf.extend_from_slice(b"}\n");
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

    pub fn spawn(
        language: &str,
        tmp_dir: &std::path::Path,
        explicit_path: Option<&std::path::Path>,
        verbose: bool,
    ) -> Self {
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

        let fulltext = match spawn_one(&bin, "fulltext", &fulltext_out, language) {
            Ok(j) => j,
            Err(e) => {
                eprintln!(
                    "[zimwriterfs] xapianbuilder fulltext spawn failed: {e}; skipping indexes"
                );
                return Self::disabled();
            }
        };
        let title = match spawn_one(&bin, "title", &title_out, language) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("[zimwriterfs] xapianbuilder title spawn failed: {e}; skipping indexes");
                let _ = fulltext.child.wait_with_killing();
                return Self::disabled();
            }
        };

        IndexHelper {
            state: State::Active { fulltext, title },
        }
    }

    /// Feed one entry to the title index. Always called for content
    /// entries (title DB indexes everything in namespace C, regardless
    /// of mimetype).
    pub fn feed_title(&mut self, path: &str, title: &str, target_path: &str) {
        let State::Active { title: t, .. } = &mut self.state else {
            return;
        };
        let mut buf = std::mem::take(&mut t.scratch);
        encode_title_doc(&mut buf, path, title, target_path);
        write_raw(t, &buf);
        t.scratch = buf;
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
        let State::Active { fulltext, .. } = &mut self.state else {
            return;
        };
        let mut buf = std::mem::take(&mut fulltext.scratch);
        encode_fulltext_doc(&mut buf, path, title, mimetype, body, language);
        write_raw(fulltext, &buf);
        fulltext.scratch = buf;
    }

    /// Close stdin on both children, wait, and return the produced
    /// blobs. On any error returns an empty Vec — the caller proceeds
    /// without indexes, matching libzim's "indexes are best-effort"
    /// posture.
    pub fn finish(self, verbose: bool) -> Vec<IndexBlob> {
        let State::Active {
            mut fulltext,
            mut title,
        } = self.state
        else {
            return Vec::new();
        };

        // Flush the write buffers, then close stdin pipes so the
        // children see EOF and finalise.
        for j in [&mut fulltext, &mut title] {
            if let Some(mut s) = j.stdin.take() {
                let _ = s.flush();
            }
        }

        let mut blobs = Vec::with_capacity(2);
        for job in [fulltext, title] {
            let url = job.url;
            let out_path = job.out_path.clone();
            match job.child.wait_with_output() {
                Ok(out) if out.status.success() => match std::fs::read(&out_path) {
                    Ok(bytes) if !bytes.is_empty() => {
                        if verbose {
                            eprintln!("[zimwriterfs] xapianbuilder {url}: {} bytes", bytes.len());
                        }
                        blobs.push(IndexBlob {
                            url,
                            mimetype: "application/octet-stream+xapian",
                            bytes,
                        });
                    }
                    Ok(_) => {
                        if verbose {
                            eprintln!("[zimwriterfs] xapianbuilder {url}: empty output, skipping");
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[zimwriterfs] xapianbuilder {url}: cannot read {}: {e}",
                            out_path.display()
                        );
                    }
                },
                Ok(out) => {
                    eprintln!(
                        "[zimwriterfs] xapianbuilder {url} exited {}: {}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr).trim_end()
                    );
                }
                Err(e) => {
                    eprintln!("[zimwriterfs] xapianbuilder {url} wait failed: {e}");
                }
            }
            // Output file is consumed; remove eagerly. Errors
            // ignored — the caller cleans up tmp_dir on its way out
            // anyway.
            let _ = std::fs::remove_file(&out_path);
        }
        blobs
    }
}

fn write_raw(job: &mut Job, buf: &[u8]) {
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
    let stdin = child
        .stdin
        .take()
        .map(|s| BufWriter::with_capacity(STDIN_BUF_CAP, s));
    Ok(Job {
        child,
        stdin,
        out_path: out_path.to_path_buf(),
        url,
        scratch: Vec::new(),
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
