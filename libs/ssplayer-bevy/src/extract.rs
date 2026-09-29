//! The one-way main world -> render world hand-off.
//!
//! The playback tree lives in the render world, so everything the main world
//! wants to say about a character has to cross here. Nothing crosses back yet:
//! reading events and completion edges is its own phase (see the crate docs).

use bevy::asset::RecursiveDependencyLoadState;
use bevy::prelude::*;
use bevy::render::Extract;
use bevy::render::sync_world::MainEntity;

use crate::asset::SsPack;
use crate::components::{SsCellmapTextures, SsPlayback, SsPlayer, SsSeek, SsTransform};

/// One character's worth of main-world state, as of this frame.
pub(crate) struct ExtractedPlayer {
    pub entity: MainEntity,
    pub pack: AssetId<SsPack>,
    /// Whether the pack and everything it depends on have finished loading —
    /// successfully or not. A character is built only once this holds, so it
    /// never shows a frame drawn before its atlases arrived; an atlas that failed
    /// draws white, as it does when the player loads for itself.
    pub settled: bool,
    pub animation: String,
    pub transform: SsTransform,
    pub playback: SsPlayback,
    /// The character's runtime texture overrides, cellmap name -> image. Empty
    /// when it spawned no [`SsCellmapTextures`]. The render world reconciles this
    /// against what it has bound.
    pub overrides: Vec<(String, Handle<Image>)>,
}

/// This frame's hand-off, rebuilt from scratch each extract.
///
/// The order of `players` is the order the render world keeps them in, which is
/// what breaks ties between equal `SsPlayback::order` values — so it must not
/// depend on hashing.
#[derive(Resource, Default)]
pub(crate) struct ExtractedPlayers {
    pub delta_secs: f32,
    pub players: Vec<ExtractedPlayer>,
    /// The seeks written this frame, in the order they were written.
    pub seeks: Vec<(MainEntity, f32)>,
}

// A five-part query with an optional is exactly the shape `type_complexity`
// fires on, and the shape Bevy queries take; naming it would obscure, not clarify.
#[allow(clippy::type_complexity)]
pub(crate) fn extract_players(
    mut extracted: ResMut<ExtractedPlayers>,
    time: Extract<Res<Time>>,
    asset_server: Extract<Res<AssetServer>>,
    seeks: Extract<Res<Messages<SsSeek>>>,
    query: Extract<Query<(Entity, &SsPlayer, &SsTransform, &SsPlayback, Option<&SsCellmapTextures>)>>,
) {
    extracted.delta_secs = time.delta_secs();
    // The messages written since the main world last swapped its buffers, which
    // is this frame's: extract runs once per main-world frame, after it.
    extracted.seeks.clear();
    extracted.seeks.extend(seeks.iter_current_update_messages().map(|seek| (MainEntity::from(seek.entity), seek.frame)));
    extracted.players.clear();
    extracted.players.extend(query.iter().map(|(entity, player, transform, playback, textures)| ExtractedPlayer {
        entity: MainEntity::from(entity),
        pack: player.pack.id(),
        settled: matches!(asset_server.get_recursive_dependency_load_state(&player.pack), Some(RecursiveDependencyLoadState::Loaded | RecursiveDependencyLoadState::Failed(_))),
        animation: player.animation.clone(),
        transform: *transform,
        playback: *playback,
        overrides: textures.map(|t| t.overrides.iter().map(|(name, handle)| (name.clone(), handle.clone())).collect()).unwrap_or_default(),
    }));
}
