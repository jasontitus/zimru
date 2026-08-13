#![cfg(all(feature = "cffi", feature = "writer"))]
//! Drives `tests/cffi_smoke.{c,cpp}` end-to-end. For each language:
//!
//! 1. Build a tiny ZIM via `zimru::writer::Creator` (one HTML article
//!    "home" containing the marker "ZIMRU-OK").
//! 2. Compile the source against `include/zimru.h` and link against
//!    the just-built `libzimru.so` / `libzimru.dylib`.
//! 3. Run the resulting binary with the ZIM path as argv[1].
//! 4. Assert exit 0.
//!
//! Each variant skips silently if its compiler isn't on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use zimru::writer::{Creator, Item};
use zimru::Compression;

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
        .and_then(Path::parent)
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

fn cxx() -> Option<&'static str> {
    first_compiler(&["c++", "g++", "clang++"])
}

fn build_test_zim(out: &Path) {
    let mut c = Creator::new();
    c.set_compression(Compression::Zstd);
    c.set_main_path("home");
    c.add_item(Item::html(
        "home",
        "Home Page",
        "<!doctype html><html><body>ZIMRU-OK marker for the C smoke test.</body></html>",
    ));
    c.add_metadata("Title", "cffi smoke test");
    c.add_metadata("Language", "eng");
    c.write_to(out).expect("write test zim");
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

/// Compile `src` with `compiler` (extra args injected in front), link
/// against `libzimru` from `lib_dir`, run the resulting binary on the
/// ZIM at `zim_path`, and assert exit 0. Tagged label for log output.
fn build_and_run(label: &str, compiler: &str, extra: &[&str], src: &Path, zim_path: &Path) {
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
            "[cffi_smoke] {} not found — run `cargo build --features cffi` (same profile) first; skipping {label}",
            dylib_path.display()
        );
        return;
    }

    let bin = unique_tmp(&format!("zimru_{label}"), "");
    let rpath_arg = format!("-Wl,-rpath,{}", lib_dir.display());
    let mut cmd = Command::new(compiler);
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.arg("-I")
        .arg(&header_dir)
        .arg("-L")
        .arg(&lib_dir)
        .arg(&rpath_arg)
        .arg(src)
        .arg("-lzimru")
        .arg("-o")
        .arg(&bin);
    let status = cmd.status().expect("invoke compiler");
    assert!(status.success(), "{label}: compile failed");

    let out = Command::new(&bin)
        .arg(zim_path)
        .env("LD_LIBRARY_PATH", &lib_dir)
        .env("DYLD_LIBRARY_PATH", &lib_dir)
        .output()
        .expect("run smoke binary");
    eprintln!(
        "--- {label} stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!(
        "--- {label} stdout ---\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.status.success(),
        "{label} exited non-zero: {:?}",
        out.status
    );

    let _ = std::fs::remove_file(&bin);
}

#[test]
fn c_consumer_can_read_a_zim_via_the_c_abi() {
    let Some(cc) = cc() else {
        eprintln!("skip: no C compiler on PATH");
        return;
    };
    let zim = unique_tmp("cffi-smoke-c", ".zim");
    build_test_zim(&zim);
    let src = manifest_dir().join("tests").join("cffi_smoke.c");
    build_and_run(
        "cffi_smoke_c",
        cc,
        &["-std=c11", "-D_GNU_SOURCE"],
        &src,
        &zim,
    );
    let _ = std::fs::remove_file(&zim);
}

#[test]
fn cpp_consumer_can_read_a_zim_via_the_c_abi() {
    let Some(cxx) = cxx() else {
        eprintln!("skip: no C++ compiler on PATH");
        return;
    };
    let zim = unique_tmp("cffi-smoke-cpp", ".zim");
    build_test_zim(&zim);
    let src = manifest_dir().join("tests").join("cffi_smoke.cpp");
    build_and_run("cffi_smoke_cpp", cxx, &["-std=c++17"], &src, &zim);
    let _ = std::fs::remove_file(&zim);
}
