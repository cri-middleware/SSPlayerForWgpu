//! `ssplayer-wgpu` — a wgpu-based player for SpriteStudio 7 animations.
//!
//! Three layers (SPEC): the runtime (`ssruntime`, computation) feeds
//! [`SSInternalPlayer`] (rendering), which is driven by [`SSPlayer`]
//! (operation) — the primary public API. The player borrows the host's wgpu
//! device/queue/target rather than owning them, so it can render into any wgpu
//! host (a standalone viewer, a browser canvas, or an engine like Bevy).

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
mod internal;
mod mask;
mod player;
#[cfg(feature = "web")]
mod web;

pub use frame::FrameView;
pub use internal::{BatchInfo, IDENTITY_TRANSFORM, RendererConfig, SSInternalPlayer, Vertex};
pub use mask::{GeomKind, MaskOp, MaskWriter, PartMaskFlags, StencilTest, plan_writers};
pub use player::{SSPlayer, SSPlayerConfig};
#[cfg(feature = "web")]
pub use web::SsPlayerWeb;
