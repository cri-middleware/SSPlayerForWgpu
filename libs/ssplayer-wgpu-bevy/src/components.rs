//! What a Bevy application spawns to put a character on screen.
//!
//! These live in the **main world**. They are the whole authoring surface: the
//! playback tree itself never appears here, because it is owned by the render
//! world (see [`crate::render`]) and reached only through these components.

use bevy::prelude::*;

/// A SpriteStudio character.
///
/// Changing `src` or `animation` rebuilds the playback tree on the next frame;
/// everything else is allocation-free transport, which is why placement and
/// playback sit in their own components.
#[derive(Component, Clone, Debug, Default)]
#[require(SsTransform, SsPlayback)]
pub struct SsPlayer {
    /// Path to the `.ssab` this character plays from, as
    /// [`ssplayer_wgpu::SSPlayer::load`] takes it.
    pub src: String,
    /// Animation to select. Empty means "do not select one yet".
    pub animation: String,
}

/// Where the character sits, in the same terms
/// [`ssplayer_wgpu::SSPlayer::set_transform`] takes.
///
/// Deliberately not Bevy's [`Transform`]: that is a 3D affine whose decomposition
/// into the player's five scalars is lossy, and a host that wants the two linked
/// can write the one system that copies across.
#[derive(Component, Clone, Copy, Debug)]
pub struct SsTransform {
    pub x: f32,
    pub y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    /// Radians.
    pub rotation: f32,
}

impl Default for SsTransform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale_x: 1.0, scale_y: 1.0, rotation: 0.0 }
    }
}

/// Transport and compositing state — everything that costs nothing to change.
#[derive(Component, Clone, Copy, Debug)]
pub struct SsPlayback {
    /// Whether the timeline advances.
    pub playing: bool,
    pub speed: f32,
    /// Draw order within the render target; lower draws first. Ties keep the
    /// order the entities were extracted in.
    pub order: i32,
    /// Player-level fade, folded into every vertex.
    pub alpha: f32,
    /// False skips the timeline advance as well as the draw.
    pub visible: bool,
}

impl Default for SsPlayback {
    fn default() -> Self {
        Self { playing: true, speed: 1.0, order: 0, alpha: 1.0, visible: true }
    }
}
