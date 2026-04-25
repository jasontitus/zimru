// Generates `include/zimru.h` from the `extern "C"` symbols in `src/cffi/`.
// Runs only when the `cffi` Cargo feature is enabled (see Cargo.toml).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    println!("cargo:rerun-if-changed=src/cffi");

    if std::env::var_os("CARGO_FEATURE_CFFI").is_none() {
        return;
    }

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out_path = std::path::Path::new(&crate_dir)
        .join("include")
        .join("zimru.h");
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("create include/ dir");

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_file("cbindgen.toml").unwrap_or_default())
        .generate()
        .expect("cbindgen failed to generate header")
        .write_to_file(&out_path);
}
