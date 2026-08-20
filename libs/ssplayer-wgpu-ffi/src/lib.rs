//! Binary faces of [`ssplayer_wgpu`]: the C ABI that CMake hosts link, and the
//! wasm-bindgen surface `wasm-pack` builds. Both are defined in `ssplayer-wgpu`
//! itself; this crate exists only to emit them as `staticlib` / `cdylib` so the
//! published `ssplayer-wgpu` can stay a plain `rlib`.
//!
//! The two faces re-export into modules of their own rather than into the crate
//! root: no build ever wants both, but `--all-features` turns them on together.

#[cfg(feature = "ffi")]
pub mod ffi {
    pub use ssplayer_wgpu::ffi::*;
}

#[cfg(feature = "web")]
pub mod web {
    pub use ssplayer_wgpu::{SsContextWeb, SsPlayerWeb, SsSurfaceWeb};
}
