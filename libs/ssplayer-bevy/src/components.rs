//! What a Bevy application spawns to put a character on screen.
//!
//! These live in the **main world**. They are the whole authoring surface: the
//! playback tree itself never appears here, because it is owned by the render
//! world (see [`crate::render`]) and reached only through these components.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use ssplayer_wgpu::{PlaybackDirection, PlaybackStyle, TrackBlend, TrackSync};

use crate::asset::SsPack;

/// A SpriteStudio character.
///
/// Changing `pack` or `animation` rebuilds the playback tree on the next frame,
/// and so does the pack's file changing under a hot-reloading app; everything
/// else is allocation-free transport, which is why placement and playback sit in
/// their own components. The one exception is a change of `animation` with a
/// `transition_ms` above zero: that crossfades instead, on the tree in place.
#[derive(Component, Clone, Debug, Default)]
#[require(SsTransform, SsPlayback, SsPlaybackState, SsTrackState)]
pub struct SsPlayer {
    /// The `.ssab` this character plays from: `asset_server.load("Ringo/Ringo.ssab")`.
    /// The character appears once the pack and its atlases have loaded.
    pub pack: Handle<SsPack>,
    /// Animation to select. Empty means "do not select one yet".
    ///
    /// This is the one way to change track 0's animation: [`TrackCommand`]s
    /// that would lay one on track 0 are refused, so what this names and what
    /// track 0 plays never part. Only a change of value acts, so a system may
    /// write it every frame.
    pub animation: String,
    /// How a change of `animation` is made: `0` (the default) switches at once,
    /// rebuilding the tree; above zero crossfades to the new animation over that
    /// many milliseconds, as [`TrackCommand::Crossfade`] does on other tracks.
    /// The first animation a character selects, and the first after a change of
    /// `pack`, is always switched to at once: there is nothing to fade from.
    pub transition_ms: f32,
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
    /// never rewinds, so it cannot replay a finished animation, nor start one an
    /// [`SsStop`] ended — that is [`SsPlayback::restart`]'s job, or an
    /// [`SsPlay`]'s.
    pub playing: bool,
    /// A multiple of the authored rate. `0` and negatives stop the playhead.
    pub speed_scale: f32,
    /// Draw order within the render target; lower draws first. Ties keep the
    /// order the entities were extracted in.
    pub order: i32,
    /// Player-level fade, folded into every vertex.
    pub alpha: f32,
    /// Player-level tint, `0xRRGGBB`, multiplied into every part after its
    /// PartsColor; white (the default) leaves colours as authored. Like `alpha`,
    /// it leaves mask coverage alone.
    pub tint: u32,
    /// False draws nothing while the timeline keeps running, so hiding a
    /// character for a moment does not slow it down.
    pub visible: bool,
    /// True skips the timeline advance as well as the draw — for a character
    /// that cannot be seen at all. The time spent culled is dropped.
    pub culled: bool,
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
            tint: 0xFF_FFFF,
            visible: true,
            culled: false,
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

/// Start a character's animation from the head of its section, or from
/// `start_frame`, as [`ssplayer_wgpu::SSPlayer::play`] and
/// [`ssplayer_wgpu::SSPlayer::play_from`] do: it rewinds, re-arms the loop count
/// and starts even a finished or stopped animation. `start_frame: None` is what
/// bumping [`SsPlayback::restart`] does.
///
/// [`SsPlayback::playing`] still governs afterwards, as it does after a
/// restart: a character held with `playing: false` starts and holds on its
/// first frame. One sent before the pack has loaded is applied once the
/// animation is set up; of several in one frame, the last wins.
#[derive(Message, Clone, Copy, Debug)]
pub struct SsPlay {
    pub entity: Entity,
    pub start_frame: Option<f32>,
}

/// End a character's playback where it stands, as
/// [`ssplayer_wgpu::SSPlayer::stop`] does. The playhead stays, so the frame it
/// stopped on is still drawn, and it does not count as finished.
///
/// A stop is not a pause: turning [`SsPlayback::playing`] off and on again
/// does not start a stopped animation, because resuming only carries on what
/// is running. Start it again with [`SsPlay`] or [`SsPlayback::restart`]. One
/// sent before the pack has loaded is applied once the animation is set up.
#[derive(Message, Clone, Copy, Debug)]
pub struct SsStop {
    pub entity: Entity,
}

/// One operation on a character's track stack — motion blending, as
/// [`ssplayer_wgpu::SSPlayer`]'s track API performs it.
///
/// Tracks fold onto the setup pose in ascending order; track 0 is what
/// [`SsPlayer::animation`] sets up, and only it: `SetAnimation` and `Crossfade`
/// on track 0 are refused with a warning.
/// Crossfade track 0 by changing `animation` with [`SsPlayer::transition_ms`]
/// set. A message rather than a component because
/// these are operations, a crossfade above all, rather than a state: write
/// them with a `MessageWriter<SsTrackCommand>`. Several in one frame apply in
/// the order they were written. Ones sent before the character's pack has
/// loaded are kept and applied once its animation is set up.
///
/// **A new `animation` resets the stack** to track 0 alone, as
/// [`ssplayer_wgpu::SSPlayer::set_animation`] does: send the overlays and the
/// track settings again after changing it. What the stack holds comes back in
/// [`SsTrackState`], a frame late.
#[derive(Message, Clone, Debug)]
pub struct SsTrackCommand {
    pub entity: Entity,
    pub command: TrackCommand,
}

/// What an [`SsTrackCommand`] does. Each variant is the
/// [`ssplayer_wgpu::SSPlayer`] call of the same name.
///
/// A track's playback settings (`SetSpeed` to `SetPlaybackDirection`) are the
/// track's: they reach every animation on it, and every one laid on it later.
/// Track 0's come from [`SsPlayback`], as its loop count does, so these
/// commands are refused on track 0.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackCommand {
    /// Lay `animation` on `track`, replacing what it played. Not on track 0,
    /// which [`SsPlayer::animation`] sets.
    SetAnimation { track: u32, animation: String },
    /// Fade `track` over to `animation` in `duration_ms`. Not on track 0: change
    /// [`SsPlayer::animation`] with [`SsPlayer::transition_ms`] set.
    Crossfade { track: u32, animation: String, duration_ms: f32 },
    /// Drive `track`'s alpha to `target` in `duration_ms`.
    FadeTrack { track: u32, target: f32, duration_ms: f32 },
    /// Stop the fade on `track` where it stands.
    CancelFade { track: u32 },
    /// Silence `track`. Track 0 is refused.
    Clear { track: u32 },
    /// `track`'s strength.
    SetAlpha { track: u32, alpha: f32 },
    /// How `track` layers onto what is below.
    SetBlend { track: u32, blend: TrackBlend },
    /// Where the animations laid on `track` from now on start. Send it before
    /// the animation it is for.
    SetSync { track: u32, sync: TrackSync },
    /// Seek `track` — how a continuous-control axis is driven.
    SetFrame { track: u32, frame: f32 },
    /// `track`'s own speed; `0` parks it.
    SetSpeed { track: u32, speed: f32 },
    /// The frame rate `track` plays at instead of the authored one; `0` clears it.
    SetFrameRate { track: u32, fps: i32 },
    /// Whether `track` drops frames to keep to the wall clock.
    SetFrameSkipEnabled { track: u32, enabled: bool },
    /// Which way `track` plays, and what it does at the far end.
    SetPlaybackDirection { track: u32, direction: PlaybackDirection, style: PlaybackStyle },
    /// How many times `track`'s animation plays before it holds its last
    /// frame; `-1` (or `0`) loops forever. It belongs to the animation: send it
    /// after the one it is for, since each is laid looping forever. Not on
    /// track 0, whose count is [`SsPlayback::loop_count`].
    SetLoopCount { track: u32, count: i32 },
    /// How far into a crossfade on `track` the outgoing animation keeps firing
    /// events: while its share of the track, `1 - progress`, is at least this.
    /// `f32::INFINITY` (the default) silences it from the start of the fade.
    SetOutgoingEventThreshold { track: u32, threshold: f32 },
    /// Lay `animation` on `track` beside what it carries, mixed by `weight`. Its
    /// handle comes back in [`SsTrackState::sources`].
    AddSource { track: u32, animation: String, weight: f32 },
    /// A source's share within its track.
    SetSourceWeight { source: u32, weight: f32 },
    /// Seek one source.
    SetSourceFrame { source: u32, frame: f32 },
    /// The weight at or above which a source fires its events; only track 0's
    /// main source fires by default.
    SetSourceEventThreshold { source: u32, threshold: f32 },
}

/// A character's track stack as the render world last left it — the readable
/// half of [`SsTrackCommand`]. **One frame behind**, like [`SsPlaybackState`].
/// Required by [`SsPlayer`], so it is always present; empty until the
/// character's animation is set up.
#[derive(Component, Clone, Debug, Default)]
pub struct SsTrackState {
    /// One entry per track, track 0 first.
    pub tracks: Vec<SsTrackInfo>,
    /// The sources laid with [`TrackCommand::AddSource`] that are still
    /// playing, with their handles. A source leaves the list when it is
    /// released: a crossfade on its track completing, `Clear`, or a new
    /// animation.
    pub sources: Vec<SsSourceInfo>,
}

/// One track of [`SsTrackState`].
#[derive(Clone, Debug, PartialEq)]
pub struct SsTrackInfo {
    /// The source carrying the track's animation; `None` on an empty track.
    pub main_source: Option<u32>,
    pub alpha: f32,
    pub blend: TrackBlend,
    pub sync: TrackSync,
    /// The track's playhead; `None` on an empty track.
    pub frame: Option<f32>,
    /// How far a fade on the track has got, `0..=1`; `None` when not fading.
    pub fade_progress: Option<f32>,
    /// The track's speed, frame rate (the override or the authored rate),
    /// frame skip and heading — what [`TrackCommand::SetSpeed`] and the
    /// commands after it set.
    pub speed: f32,
    pub frame_rate: i32,
    pub frame_skip_enabled: bool,
    pub direction: PlaybackDirection,
    pub style: PlaybackStyle,
    /// `-1` for a track that loops forever; `None` on an empty track.
    pub loop_count: Option<i32>,
    /// The track's animation has played its loop count out and holds its last
    /// frame.
    pub finished: bool,
    /// The last advance wrapped the track's animation back to its start.
    pub just_looped: bool,
    pub outgoing_event_threshold: f32,
}

/// A source added with [`TrackCommand::AddSource`], as it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct SsSourceInfo {
    /// The handle the `Source*` commands take.
    pub source: u32,
    pub track: u32,
    /// The animation it was laid with.
    pub animation: String,
    pub weight: f32,
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
/// messages ([`SsStarted`](crate::SsStarted), [`SsLooped`](crate::SsLooped),
/// [`SsFinished`](crate::SsFinished), [`SsEventMessage`](crate::SsEventMessage)).
/// Both are **one frame behind** — what you read this frame is what playback
/// produced last frame — because playback runs in the render world. Required by
/// [`SsPlayer`], so it is always present.
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
