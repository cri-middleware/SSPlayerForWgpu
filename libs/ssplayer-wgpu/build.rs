//! Bakes in the SpriteStudio-SDK version this build links, for `SDK_VERSION`
//! and `ss_sdk_version()`.
//!
//! `scripts/build.sh` names it in `SSPLAYER_SDK_VERSION`, from the submodule's
//! version. A build without it — a plain `cargo build` — says `local`.

fn main() {
    println!("cargo:rerun-if-env-changed=SSPLAYER_SDK_VERSION");
    let sdk = std::env::var("SSPLAYER_SDK_VERSION").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "local".to_owned());
    println!("cargo:rustc-env=SSPLAYER_SDK_VERSION={sdk}");
}
