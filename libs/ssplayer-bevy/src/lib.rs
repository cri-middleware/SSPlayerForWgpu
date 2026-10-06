//! Bevy adapter for [`ssplayer_wgpu`] — SpriteStudio characters inside a Bevy
//! app, drawn on the device Bevy already owns.
//!
//! # Shape
//!
//! Bevy renders in two worlds, and this crate is split along that seam:
//!
//! * **Main world** — [`SsPlayer`], [`SsTransform`] and [`SsPlayback`]. Spawn
//!   them, change them, and that is the whole authoring surface. The `.ssab` a
//!   character plays is an [`SsPack`] asset, loaded through the `AssetServer`.
//! * **Render world** — the playback trees, one device-scoped `SSRenderer` and
//!   one target-scoped `SSScene`. Nothing here is reachable from gameplay code.
//!
//! Authoring state crosses to the render world through `ExtractSchedule`;
//! playback state crosses back through a shared buffer — the timeline events
//! ([`SsEventMessage`]), the start, loop and completion edges ([`SsStarted`],
//! [`SsLooped`], [`SsFinished`]), and the frame landed on ([`SsPlaybackState`]). The
//! return trip is one frame late, because playback runs in the render world.
//!
//! The player never owns Bevy's `Device` / `Queue` / render target; the draw
//! system lends them for the frame, which is the same contract the winit viewer
//! and the C++ hosts honour.
//!
//! # Supported Bevy
//!
//! Bevy **0.20.x**, and no other line. Bevy and this player must resolve to one
//! `wgpu` — 0.20 wants `^30` and the player is pinned to 30 — because two
//! majors of `wgpu` are two unrelated sets of types, and no amount of glue
//! bridges them. npm would express this as a peer dependency; cargo has no such
//! thing, so it is stated here and enforced by the workspace resolving one
//! `wgpu`.
//!
//! # Masks
//!
//! Bevy's 2d depth texture is `Depth32Float` and carries no stencil, so this
//! adapter selects the player's coverage-bitmap mask backend
//! (`MaskBackend::Coverage`) rather than its stencil one. Nothing is asked of
//! Bevy's attachments; the player rasterises its own bitmap into the encoder the
//! draw pass is opened on, which is what makes the clip this frame's rather than
//! the last one's. It costs one extra render pass per masking character.
//!
//! # Limits of this version
//!
//! * **One view.** The adapter holds one `SSScene` with every character in it,
//!   and `SSScene::update` fuses the timeline advance with the geometry build,
//!   so a second view would advance every animation twice per frame. Later views
//!   are skipped, with a warning.
//! * **MSAA must be off.** The player's pipelines are single-sampled.

mod asset;
mod components;
mod extract;
mod feedback;
mod render;

pub use asset::{SsPack, SsPackLoadError, SsPackLoader};
pub use components::{SsCellmapTextures, SsPlay, SsPlayback, SsPlaybackState, SsPlayer, SsSeek, SsStop, SsTransform};
pub use feedback::{SsEventMessage, SsFinished, SsLooped, SsStarted};
// The event payloads a host matches on, re-exported so it need not also depend
// on `ssplayer-wgpu` just to name them.
pub use ssplayer_wgpu::{AudioEvent, PlaybackDirection, PlaybackStyle, SignalEvent, SignalParam, SignalValue, SsEvent, UserDataEvent};

use bevy::app::{App, Plugin, PreUpdate};
use bevy::asset::AssetApp;
use bevy::core_pipeline::core_2d::main_transparent_pass_2d;
use bevy::core_pipeline::{Core2d, Core2dSystems};
use bevy::log::error;
use bevy::prelude::IntoScheduleConfigs;
use bevy::render::render_asset::RenderAssetPlugin;
use bevy::render::{ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};

/// Adds SpriteStudio playback and drawing to a Bevy app.
///
/// Add it after `DefaultPlugins`; it needs the render app to exist.
pub struct SsPlayerPlugin;

impl Plugin for SsPlayerPlugin {
    fn build(&self, app: &mut App) {
        // The buffer playback feedback crosses on. Both worlds hold a clone of
        // the one `Arc`, so what the render world writes the main world reads.
        let channel = feedback::FeedbackChannel::default();

        app.init_asset::<SsPack>()
            .init_asset_loader::<SsPackLoader>()
            // Moves each loaded pack into the render world, and again on a reload.
            .add_plugins(RenderAssetPlugin::<asset::RenderSsPack>::default())
            .add_message::<SsEventMessage>()
            .add_message::<SsStarted>()
            .add_message::<SsFinished>()
            .add_message::<SsLooped>()
            .add_message::<SsSeek>()
            .add_message::<SsPlay>()
            .add_message::<SsStop>()
            .insert_resource(channel.clone())
            // In `PreUpdate` so a gameplay system reading these in `Update` sees
            // the same frame's batch.
            .add_systems(PreUpdate, feedback::dispatch_feedback);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            error!("ssplayer-bevy: no render app — add this plugin after DefaultPlugins");
            return;
        };

        render_app
            .init_resource::<extract::ExtractedPlayers>()
            .insert_resource(channel)
            .add_systems(RenderStartup, render::init_state)
            .add_systems(ExtractSchedule, extract::extract_players)
            .add_systems(Render, render::sync_players.in_set(RenderSystems::Prepare))
            .add_systems(Core2d, render::draw_players.in_set(Core2dSystems::MainPass).after(main_transparent_pass_2d));
    }
}
