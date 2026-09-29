//! What a Bevy application spawns to put a character on screen.
//!
//! These live in the **main world**. They are the whole authoring surface: the
//! playback tree itself never appears here, because it is owned by the render
//! world (see [`crate::render`]) and reached only through these components.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use ssplayer_wgpu::{PlaybackDirection, PlaybackStyle};

use crate::asset::SsPack;

/// A SpriteStudio character.
///
/// Changing `pack` or `animation` rebuilds the playback tree on the next frame,
/// and so does the pack's file changing under a hot-reloading app; everything
/// else is allocation-free transport, which is why placement and playback sit in
/// their own components.
#[derive(Component, Clone, Debug, Default)]
#[require(SsTransform, SsPlayback, SsPlaybackState)]
pub struct SsPlayer {
    /// The `.ssab` this character plays from: `asset_server.load("Ringo/Ringo.ssab")`.
    /// The character appears once the pack and its atlases have loaded.
    pub pack: Handle<SsPack>,
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
    /// Whether the timeline advances. Toggling it pauses and resumes in place; it
    /// never rewinds, so it cannot replay a finished animation — that is
    /// [`SsPlayback::restart`]'s job.
    pub playing: bool,
    /// A multiple of the authored rate. `0` and negatives stop the playhead.
    pub speed_scale: f32,
    /// Draw order within the render target; lower draws first. Ties keep the
    /// order the entities were extracted in.
    pub order: i32,
    /// Player-level fade, folded into every vertex.
    pub alpha: f32,
    /// False skips the timeline advance as well as the draw.
    pub visible: bool,
    /// Bump this (any change in value) to replay the animation from its head,
    /// re-arming the loop count. A generation counter rather than a flag so a host
    /// can re-trigger a one-shot at will without having to clear anything, and so
    /// two restarts in a row are two restarts.
    pub restart: u32,
    /// Which way the playhead walks, as
    /// [`ssplayer_wgpu::SSPlayer::set_playback_direction`] takes it. The way it
    /// is actually travelling is [`SsPlaybackState::playing_forward`]: a
    /// ping-pong's return leg runs backward under `Forward`.
    pub direction: PlaybackDirection,
    /// What the playhead does at the far end: `Normal` wraps, `PingPong` turns
    /// back.
    pub style: PlaybackStyle,
    /// Cycles to play before stopping; `-1` (or `0`) loops for ever, which is
    /// also what an animation does unless told otherwise. A change re-arms the
    /// count, as a restart does.
    pub loop_count: i32,
    /// Replaces the authored frame rate; `0` keeps it. Prefer `speed_scale`,
    /// which scales the authored timing rather than replacing it.
    pub frame_rate: i32,
    /// On (the default) keeps playback on the wall clock by dropping frames
    /// under load; off advances exactly one frame per tick.
    pub frame_skip_enabled: bool,
    /// On (the default) draws between authored frames; off floors the frame
    /// that is drawn, pinning playback to whole ones.
    pub sub_frame_enabled: bool,
    /// Restrict the playhead to `start..=end` of the current animation, so
    /// looping and ping-pong turn around inside it; `None` is the whole
    /// animation. Setting or changing it puts the playhead at its start, and it
    /// applies to whichever animation is set up, so change it with `animation`.
    pub animation_section: Option<(i32, i32)>,
}

impl Default for SsPlayback {
    fn default() -> Self {
        Self {
            playing: true,
            speed_scale: 1.0,
            order: 0,
            alpha: 1.0,
            visible: true,
            restart: 0,
            direction: PlaybackDirection::Forward,
            style: PlaybackStyle::Normal,
            loop_count: -1,
            frame_rate: 0,
            frame_skip_enabled: true,
            sub_frame_enabled: true,
            animation_section: None,
        }
    }
}

/// Move a character's playhead to `frame`, as
/// [`ssplayer_wgpu::SSPlayer::seek`] does: a fractional frame is honoured, and
/// the playhead stays inside the section.
///
/// A message rather than a field of [`SsPlayback`], because a seek happens once
/// rather than being a state the character is in: write it with a
/// `MessageWriter<SsSeek>`. One sent before the character's pack has loaded is
/// kept and applied once its animation is set up; of several in one frame, the
/// last wins.
#[derive(Message, Clone, Copy, Debug)]
pub struct SsSeek {
    pub entity: Entity,
    pub frame: f32,
}

/// Runtime texture replacement: swap the texture a cellmap binds for a Bevy
/// image, live, from gameplay code
/// (costume changes, dynamic expressions).
///
/// Keyed by cellmap name, or by a TextureChange target's file name — whatever the
/// character's pack lists. Insert an entry to override, remove it to restore the
/// original atlas; the render world reconciles the map against what it has applied
/// each frame. An image that is not yet loaded is applied the frame it becomes
/// ready, so binding a freshly-spawned `Handle<Image>` is fine.
///
/// The replacement should match the original atlas's size and cell layout — the
/// runtime's UVs stay in the original cellmap's pixel space — and be loaded with
/// `is_srgb: false` in its `ImageLoaderSettings`: the player takes atlas bytes as
/// authored, and an sRGB image draws too dark. The override is this
/// character's alone: another entity playing the same pack is unaffected, and an
/// instance child (its own pack) does not inherit it.
///
/// Optional — only characters that swap textures need spawn it.
#[derive(Component, Clone, Debug, Default)]
pub struct SsCellmapTextures {
    /// Cellmap (or TextureChange-target) name -> the image to bind for it.
    pub overrides: HashMap<String, Handle<Image>>,
}

/// Playback state read back from the render world, refreshed every frame.
///
/// This is the pollable half of the round-trip; the reactive half is the
/// messages in [`crate::feedback`]. Both are **one frame behind** — what you
/// read this frame is what playback produced last frame — because playback runs
/// in the render world. Required by [`SsPlayer`], so it is always present.
#[derive(Component, Clone, Copy, Debug)]
pub struct SsPlaybackState {
    /// The frame the timeline last landed on.
    pub frame_no: f32,
    /// Whether the animation has reached its end.
    pub finished: bool,
    /// Which way the playhead is actually travelling: false on a ping-pong's
    /// return leg as well as under a backward direction. Gate an
    /// [`SsEvent::Audio`](ssplayer_wgpu::SsEvent::Audio) on it — SpriteStudio
    /// has no reverse audio, so a key crossed going backward is not meant to
    /// sound.
    pub playing_forward: bool,
}

impl Default for SsPlaybackState {
    /// Forward at rest, as the player reports it before anything has played.
    fn default() -> Self {
        Self { frame_no: 0.0, finished: false, playing_forward: true }
    }
}
