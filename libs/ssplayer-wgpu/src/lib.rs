//! `ssplayer-wgpu` — a wgpu-based player for SpriteStudio animations.
//!
//! The player borrows the host's wgpu device, queue and render target rather
//! than owning them, so it draws into any wgpu host — a standalone viewer, a
//! browser canvas, or an engine like Bevy.
//!
//! Three types, split by how long their resources stay useful:
//!
//! * [`SSRenderer`] — one per **device**: pipelines, bind group layouts, the
//!   sampler and the atlas textures. The host owns it and lends it.
//! * [`SSScene`] — one per **render target**: the projection and the frame's
//!   geometry.
//! * [`SSPlayer`] — one per character: the playback tree. Holds no GPU resource.

/// The filesystem behind [`SSPlayer::load`]. Needs the `embedded` feature (for
/// the decoder) and a target that has a filesystem.
#[cfg(all(feature = "embedded", not(target_arch = "wasm32")))]
mod assets;
/// Bulk atlas decoding for the loading path. Needs the `embedded` feature, which
/// is what carries the decoder.
#[cfg(feature = "embedded")]
mod decode;
mod events;
/// The C-ABI face (`ffi` feature): a thin translation layer whose `ss_player_*`
/// entry points are exported from the cdylib/staticlib and described by the
/// generated `ssplayer_wgpu.h`. The work it delegates to lives in [`host`].
#[cfg(feature = "ffi")]
pub mod ffi;
mod frame;
/// A window-owning wgpu host (`ffi` feature), used by [`ffi`] and available to
/// Rust callers that have a raw window handle rather than a winit `Window`.
#[cfg(feature = "ffi")]
pub mod host;
mod mask;
mod metrics;
mod pack;
mod player;
mod renderer;
/// Generation-tagged slot storage for the handle-based hosts.
#[cfg(any(feature = "ffi", feature = "web"))]
mod roster;
mod scene;
#[cfg(feature = "web")]
mod web;

pub use events::{AudioEvent, SignalEvent, SignalParam, SignalValue, SsEvent, UserDataEvent};
pub use frame::FrameView;
pub use mask::{GeomKind, MaskOp, MaskWriter, MaskWriters, PartMaskFlags, StencilTest, plan_writers};
pub use metrics::SceneStats;
pub use pack::{EmbeddedImage, EmbeddedTexture, SSPack, TextureRef};
pub use player::{AssetLoader, AtlasKeys, BlendOp, MissingAsset, MissingKind, OverridePriority, PlaybackDirection, PlaybackStyle, SSPlayer};
pub use renderer::{AlphaMode, Retention, SSRenderer};
pub use scene::{BatchInfo, FrameChunk, IDENTITY_TRANSFORM, MaskBackend, NodeSlot, SSScene, SceneConfig};
/// Re-exported from the runtime: how a track folds onto the tracks below it.
pub use ssruntime::TrackBlend;
pub use ssruntime::error::SSRuntimeError;
#[cfg(feature = "web")]
pub use web::{SsContextWeb, SsPlayerWeb, SsSurfaceWeb};
