//! `ssplayer-wgpu` — a wgpu-based player for SpriteStudio 7 animations.
//!
//! Three layers (SPEC): the runtime (`ssruntime`, computation) feeds
//! [`SSInternalPlayer`] (rendering), which is driven by [`SSPlayer`]
//! (operation) — the primary public API. The player borrows the host's wgpu
//! device/queue/target rather than owning them, so it can render into any wgpu
//! host (a standalone viewer, a browser canvas, or an engine like Bevy).

mod frame;
mod internal;
mod player;

pub use frame::FrameView;
pub use internal::{RendererConfig, SSInternalPlayer, Vertex};
pub use player::{SSPlayer, SSPlayerConfig};
