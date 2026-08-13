#![cfg(feature = "cffi")]
//! Drives `tests/cffi_writer_smoke.c` end-to-end.
//!
//! Companion to `tests/cffi_smoke.{c,rs}` — that pair exercises the
//! reader-side C ABI on a ZIM built with the Rust `Creator`. This pair
//! exercises the writer-side C ABI in isolation: it builds a ZIM via
//! `zimru_creator_*` calls only (no Rust Creator), then reads it back
//! via the reader C ABI to assert the on-disk shape matches what was
//! requested.
//!
//! Skipped silently if no C compiler is on PATH so CI without a
//! toolchain still passes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn target_dir() -> PathBuf {
    // The test binary lives at target/<profile>/deps/<name>-<hash>, so
    // the cdylib is two levels up — resolving it this way works for
    // both a plain `cargo test --features cffi` (debug) and
    // `cargo test --release`, instead of hard-coding target/release.
    let exe = std::env::current_exe().expect("current_exe");
    exe.parent()
        .and_then(std::path::Path::parent)
        .expect("test exe not under target/<profile>/deps")
        .to_path_buf()
}

fn first_compiler(candidates: &[&'static str]) -> Option<&'static str> {
    candidates
        .iter()
        .copied()
        .find(|c| Command::new(c).arg("--version").output().is_ok())
}

fn cc() -> Option<&'static str> {
    first_compiler(&["cc", "gcc", "clang"])
}

fn dylib_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libzimru.dylib"
    } else if cfg!(target_os = "windows") {
        "zimru.dll"
    } else {
        "libzimru.so"
    }
}

fn unique_tmp(prefix: &str, suffix: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{ns}{suffix}", std::process::id(),))
}

/// Compile `src` with `compiler`, link against `libzimru` from the
/// current profile's target dir, run the resulting binary with
/// `out_zim_path` as argv[1] (so the C program writes its test ZIM
/// there), assert exit 0.
fn build_and_run(compiler: &str, src: &Path, out_zim_path: &Path) {
    let lib_dir = target_dir();
    let header_dir = manifest_dir().join("include");
    assert!(
        header_dir.join("zimru.h").exists(),
        "include/zimru.h missing — did `cargo build --features cffi` run?"
    );
    let dylib_path = lib_dir.join(dylib_name());
    if !dylib_path.exists() {
        // `cargo test` alone doesn't emit the cdylib — skip (like the
        // missing-compiler case) instead of failing with a panic, and
        // say exactly which build produces it for this profile.
        eprintln!(
            "[cffi_writer_smoke] {} not found — run `cargo build --features cffi` (same profile) first; skipping",
            dylib_path.display()
        );
        return;
    }

    let bin = unique_tmp("zimru_cffi_writer_smoke", "");
    let rpath_arg = format!("-Wl,-rpath,{}", lib_dir.display());
    let status = Command::new(compiler)
        .arg("-std=c11")
        .arg("-D_GNU_SOURCE")
        .arg("-I")
        .arg(&header_dir)
        .arg("-L")
        .arg(&lib_dir)
        .arg(&rpath_arg)
        .arg(src)
        .arg("-lzimru")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("invoke C compiler");
    assert!(status.success(), "writer-smoke: compile failed");

    let out = Command::new(&bin)
        .arg(out_zim_path)
        .env("LD_LIBRARY_PATH", &lib_dir)
        .env("DYLD_LIBRARY_PATH", &lib_dir)
        .output()
        .expect("run smoke binary");
    eprintln!(
        "--- cffi_writer_smoke stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!(
        "--- cffi_writer_smoke stdout ---\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.status.success(),
        "writer smoke exited non-zero: {:?}",
        out.status
    );

    let _ = std::fs::remove_file(&bin);
}

#[test]
fn c_consumer_can_build_a_zim_via_the_writer_c_abi() {
    let Some(cc) = cc() else {
        eprintln!("skip: no C compiler on PATH");
        return;
    };
    let zim = unique_tmp("cffi-writer-smoke", ".zim");
    let src = manifest_dir().join("tests").join("cffi_writer_smoke.c");
    build_and_run(cc, &src, &zim);
    let _ = std::fs::remove_file(&zim);
}
