//! Generates the C header for the `ffi` face.
//!
//! With the `ffi` feature on, cbindgen reads `ssplayer-wgpu`'s `src/ffi.rs` and writes
//! `include/ssplayer_wgpu.h` next to this crate — the path CMake consumers point
//! at. Without the feature this build script does nothing.

fn main() {
    println!("cargo:rerun-if-changed=../ssplayer-wgpu/src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    generate_header();
}

#[cfg(feature = "ffi")]
fn generate_header() {
    use std::path::PathBuf;

    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let header = crate_dir.join("include").join("ssplayer_wgpu.h");
    std::fs::create_dir_all(header.parent().expect("include dir")).expect("create include/");

    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml")).expect("read cbindgen.toml");
    match cbindgen::Builder::new().with_crate(crate_dir.join("..").join("ssplayer-wgpu")).with_config(config).generate() {
        // `write_to_file` leaves the file untouched when the contents match, so a
        // rebuild does not churn its mtime.
        Ok(bindings) => {
            bindings.write_to_file(&header);
        }
        // A header that fails to regenerate must not silently ship stale: fail the build.
        Err(e) => panic!("cbindgen failed to generate {}: {e}", header.display()),
    }
}

#[cfg(not(feature = "ffi"))]
fn generate_header() {}
