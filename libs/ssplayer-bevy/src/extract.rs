//! The one-way main world -> render world hand-off.
//!
//! The playback tree lives in the render world, so everything the main world
//! wants to say about a character has to cross here. Nothing crosses back yet:
//! reading events and completion edges is its own phase (see the crate docs).

use bevy::prelude::*;
use bevy::render::Extract;
use bevy::render::sync_world::MainEntity;

use crate::components::{SsCellmapTextures, SsPlayback, SsPlayer, SsTransform};

/// One character's worth of main-world state, as of this frame.
pub(crate) struct ExtractedPlayer {
    pub entity: MainEntity,
    pub src: String,
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
}

// A five-part query with an optional is exactly the shape `type_complexity`
// fires on, and the shape Bevy queries take; naming it would obscure, not clarify.
#[allow(clippy::type_complexity)]
pub(crate) fn extract_players(mut extracted: ResMut<ExtractedPlayers>, time: Extract<Res<Time>>, query: Extract<Query<(Entity, &SsPlayer, &SsTransform, &SsPlayback, Option<&SsCellmapTextures>)>>) {
    extracted.delta_secs = time.delta_secs();
    extracted.players.clear();
    extracted.players.extend(query.iter().map(|(entity, player, transform, playback, textures)| ExtractedPlayer {
        entity: MainEntity::from(entity),
        src: player.src.clone(),
        animation: player.animation.clone(),
        transform: *transform,
        playback: *playback,
        overrides: textures.map(|t| t.overrides.iter().map(|(name, handle)| (name.clone(), handle.clone())).collect()).unwrap_or_default(),
    }));
}
