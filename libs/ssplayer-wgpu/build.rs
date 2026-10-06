//! Bakes in the SpriteStudio-SDK release this build links, for `SDK_VERSION`
//! and `ss_sdk_version()`.
//!
//! `scripts/build.sh` names it in `SSPLAYER_SDK_VERSION`, from the submodule's
//! tag. A build without it — a plain `cargo build` — links an SDK built from
//! source, and says `local`, the word the sibling players use for one.

fn main() {
    println!("cargo:rerun-if-env-changed=SSPLAYER_SDK_VERSION");
    let sdk = std::env::var("SSPLAYER_SDK_VERSION").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "local".to_owned());
    println!("cargo:rustc-env=SSPLAYER_SDK_VERSION={sdk}");
}
