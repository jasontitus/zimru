//! Fault injection for the shell harnesses under `bench/`.
//!
//! Every harness must fail closed: a tool that crashes, truncates its
//! report, or prints nothing must never turn into PASS / MATCH /
//! IDENTICAL. The upstream zim-tools are replaced by shell stubs so the
//! tests need no installed libzim.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "zimru-bench-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("up")).unwrap();
        fs::create_dir_all(root.join("our")).unwrap();
        fs::create_dir_all(root.join("out")).unwrap();
        fs::create_dir_all(root.join("zim-cache")).unwrap();
        fs::write(root.join("zim-cache/a.zim"), b"not a real zim").unwrap();
        Sandbox { root }
    }

    fn stub(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.root.join(rel);
        fs::write(&path, format!("#!/usr/bin/env bash\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn run(&self, script: &str, args: &[&str]) -> Output {
        let out = Command::new("bash")
            .arg(repo().join("bench").join(script))
            .args(args)
            .current_dir(&self.root)
            .env("UPSTREAM_DIR", self.root.join("up"))
            .env("OUR_DIR", self.root.join("our"))
            .env("OUT", self.root.join("out"))
            .env("ZIMRU_BUILD", "0")
            .output()
            .expect("bash present");
        eprintln!(
            "--- {script} {args:?}\nstatus={:?}\nstdout:\n{}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A complete, clean zimcheck report as 3.8.0 prints it.
const CLEAN_CHECK: &str = r#"echo "[INFO] Checking zim file $2"
echo "[INFO] Overall Test Status: Pass"
echo "[INFO] Total time taken by zimcheck: 0 seconds."
exit 0"#;

#[test]
fn recreate_suite_rejects_empty_digest() {
    let sb = Sandbox::new("empty-digest");
    sb.stub("up/zimcheck", CLEAN_CHECK);
    sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    // --quiet with a digest request that prints nothing: the old harness
    // compared two empty strings and printed MATCH.
    sb.stub("our/zimru", "exit 0");
    let out = sb.run("recreate-suite.sh", &["zim-cache/*.zim"]);
    assert!(!out.status.success());
    let text = stdout(&out);
    assert!(text.contains("ERROR verifying source"), "{text}");
    assert!(!text.contains("MATCH"), "{text}");
}

#[test]
fn recreate_suite_detects_content_difference() {
    let sb = Sandbox::new("digest-diff");
    sb.stub("up/zimcheck", CLEAN_CHECK);
    sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    // Digest depends on the archive name, so source and recreated differ.
    sb.stub(
        "our/zimru",
        r#"echo "content_entries: 3"
case "$2" in
  *.recreated.zim) echo "content_md5: 00000000000000000000000000000000";;
  *) echo "content_md5: ffffffffffffffffffffffffffffffff";;
esac"#,
    );
    let out = sb.run("recreate-suite.sh", &["zim-cache/*.zim"]);
    assert!(!out.status.success());
    let text = stdout(&out);
    assert!(text.contains("CONTENT DIFF"), "{text}");
}

#[test]
fn recreate_suite_passes_when_everything_agrees() {
    let sb = Sandbox::new("digest-ok");
    sb.stub("up/zimcheck", CLEAN_CHECK);
    sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    sb.stub(
        "our/zimru",
        "echo 'content_entries: 3'; echo 'content_md5: 0123456789abcdef0123456789abcdef'",
    );
    let out = sb.run("recreate-suite.sh", &["zim-cache/*.zim"]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(stdout(&out).contains("CONTENT MATCH; checker PASS"));
}

#[test]
fn checker_report_without_completion_marker_is_an_error() {
    let sb = Sandbox::new("truncated-check");
    // "Pass" line present, but the report ends before the timing footer:
    // zimcheck was killed mid-run and must not count as PASS.
    sb.stub(
        "up/zimcheck",
        "echo '[INFO] Overall Test Status: Pass'; exit 0",
    );
    sb.stub("up/zimrecreate", "printf recreated > \"$2\"; exit 0");
    sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    let out = sb.run("recreate-bench.sh", &["zim-cache/*.zim"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("incomplete or unknown checker report"));
}

#[test]
fn checker_crash_is_distinguished_from_findings() {
    let sb = Sandbox::new("crash-check");
    sb.stub("up/zimcheck", "echo boom >&2; exit 139");
    sb.stub("up/zimrecreate", "printf recreated > \"$2\"; exit 0");
    sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    let out = sb.run("recreate-bench.sh", &["zim-cache/*.zim"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("checker execution failed (139)"));
}

#[test]
fn checker_findings_claiming_pass_are_unknown_report() {
    let sb = Sandbox::new("inconsistent-check");
    sb.stub(
        "up/zimcheck",
        r#"echo "[ERROR] Metadata: missing Title"
echo "[INFO] Overall Test Status: Pass"
echo "[INFO] Total time taken by zimcheck: 0 seconds."
exit 0"#,
    );
    sb.stub("up/zimrecreate", "printf recreated > \"$2\"; exit 0");
    sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    let out = sb.run("recreate-bench.sh", &["zim-cache/*.zim"]);
    assert!(!out.status.success());
}

#[test]
fn recreate_bench_reports_source_class_regressions() {
    let sb = Sandbox::new("regress-check");
    // The source is clean; every recreated archive fails a new check class.
    sb.stub(
        "up/zimcheck",
        r#"case "$2" in
  *.zim.zr.zim|*.zim.up.zim)
    echo "[ERROR] Metadata: missing Title"
    echo "[INFO] Overall Test Status: Fail"
    echo "[INFO] Total time taken by zimcheck: 0 seconds."
    exit 1;;
  *)
    echo "[INFO] Overall Test Status: Pass"
    echo "[INFO] Total time taken by zimcheck: 0 seconds."
    exit 0;;
esac"#,
    );
    sb.stub("up/zimrecreate", "printf recreated > \"$2\"; exit 0");
    sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    let out = sb.run("recreate-bench.sh", &["zim-cache/*.zim"]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("checker REGRESS"));
}

#[test]
fn content_verify_rejects_truncated_manifest() {
    let sb = Sandbox::new("manifest");
    let manifest = sb.stub(
        "zim-manifest",
        // Two rows for an archive that zimdump says holds three entries.
        "printf 'a\\tb\\tc\\nd\\te\\tf\\n'; exit 0",
    );
    let dump = sb.stub("our/zimdump", "echo 'count-entries: 3'");
    let recreate = sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    sb.stub("up/zimrecreate", "printf recreated > \"$2\"; exit 0");
    let out = Command::new("bash")
        .arg(repo().join("bench/content-verify.sh"))
        .arg("zim-cache/a.zim")
        .current_dir(&sb.root)
        .env("UPSTREAM_DIR", sb.root.join("up"))
        .env("OUT", sb.root.join("out"))
        .env("ZIM_MANIFEST", &manifest)
        .env("ZIMRU_DUMP", &dump)
        .env("ZIMRU_RECREATE", &recreate)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text = stdout(&out);
    assert!(!text.contains("IDENTICAL"), "{text}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("failed/incomplete source manifest"));
}

#[test]
fn content_verify_accepts_consistent_empty_manifest() {
    let sb = Sandbox::new("manifest-empty");
    let manifest = sb.stub("zim-manifest", "exit 0");
    let dump = sb.stub("our/zimdump", "echo 'count-entries: 0'");
    let recreate = sb.stub("our/zimrecreate", "printf recreated > \"$2\"; exit 0");
    sb.stub("up/zimrecreate", "printf recreated > \"$2\"; exit 0");
    let out = Command::new("bash")
        .arg(repo().join("bench/content-verify.sh"))
        .arg("zim-cache/a.zim")
        .current_dir(&sb.root)
        .env("UPSTREAM_DIR", sb.root.join("up"))
        .env("OUT", sb.root.join("out"))
        .env("ZIM_MANIFEST", &manifest)
        .env("ZIMRU_DUMP", &dump)
        .env("ZIMRU_RECREATE", &recreate)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(stdout(&out).contains("verified manifest: 0 entries"));
}

#[test]
fn parity_fails_on_exit_status_mismatch_with_identical_stdout() {
    let sb = Sandbox::new("parity-status");
    for tool in ["zimdump", "zimcheck"] {
        sb.stub(&format!("up/{tool}"), "echo same; exit 1");
        sb.stub(&format!("our/{tool}"), "echo same; exit 0");
    }
    let out = sb.run("parity.sh", &["zim-cache/a.zim"]);
    assert!(!out.status.success());
    let text = stdout(&out);
    assert!(!text.contains("PASS"), "{text}");
    assert!(text.contains("Summary: 0/12 cases pass"), "{text}");
}

#[test]
fn parity_fails_on_stderr_difference_and_passes_when_streams_agree() {
    let sb = Sandbox::new("parity-stderr");
    for tool in ["zimdump", "zimcheck"] {
        sb.stub(
            &format!("up/{tool}"),
            "echo same; echo 'warn: x' >&2; exit 0",
        );
        sb.stub(&format!("our/{tool}"), "echo same; exit 0");
    }
    let out = sb.run("parity.sh", &["zim-cache/a.zim"]);
    assert!(!out.status.success());
    assert!(!stdout(&out).contains("PASS"));

    for tool in ["zimdump", "zimcheck"] {
        sb.stub(
            &format!("our/{tool}"),
            "echo same; echo 'warn: x' >&2; exit 0",
        );
    }
    let out = sb.run("parity.sh", &["zim-cache/a.zim"]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(stdout(&out).contains("Summary: 12/12 cases pass"));
}

#[test]
fn parity_treats_crash_as_failure_even_if_both_crash_identically() {
    let sb = Sandbox::new("parity-crash");
    for tool in ["zimdump", "zimcheck"] {
        sb.stub(&format!("up/{tool}"), "echo same; exit 134");
        sb.stub(&format!("our/{tool}"), "echo same; exit 134");
    }
    let out = sb.run("parity.sh", &["zim-cache/a.zim"]);
    assert!(!out.status.success());
    assert!(!stdout(&out).contains("PASS"));
}

#[test]
fn parity_requires_upstream_executables() {
    let sb = Sandbox::new("parity-missing");
    for tool in ["zimdump", "zimcheck"] {
        sb.stub(&format!("our/{tool}"), "echo same; exit 0");
    }
    let out = sb.run("parity.sh", &["zim-cache/a.zim"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing executable"));
}

#[test]
fn toolset_bench_marks_crashes_and_exits_nonzero() {
    let sb = Sandbox::new("toolset-crash");
    for tool in ["zimcheck", "zimdump", "zimbench"] {
        sb.stub(&format!("up/{tool}"), "exit 0");
        sb.stub(&format!("our/{tool}"), "exit 0");
    }
    // Only our zimdump crashes; every other case is healthy.
    sb.stub("our/zimdump", "exit 139");
    let out = sb.run("toolset-bench.sh", &["zim-cache/a.zim"]);
    assert!(!out.status.success());
    let text = stdout(&out);
    assert!(text.contains("ABORT"), "{text}");
    assert!(text.contains("139/0"), "{text}");
    // zimcheck rows still carry real times.
    assert!(text.contains("zimcheck -C"), "{text}");
}
