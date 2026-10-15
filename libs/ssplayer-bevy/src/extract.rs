//! The one-way main world -> render world hand-off.
//!
//! The playback tree lives in the render world, so everything the main world
//! wants to say about a character has to cross here. Nothing crosses back yet:
//! reading events and completion edges is its own phase (see the crate docs).

use bevy::asset::RecursiveDependencyLoadState;
use bevy::ecs::message::MessageCursor;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::Extract;
use bevy::render::sync_world::MainEntity;

use crate::asset::SsPack;
use crate::components::{SsCellmapTextures, SsPlay, SsPlayback, SsPlayer, SsSeek, SsStop, SsTrackCommand, SsTransform, TrackCommand};

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
    pub transition_ms: f32,
    pub transform: SsTransform,
    pub playback: SsPlayback,
    /// The character's runtime texture overrides, cellmap name -> image. Empty
    /// when it spawned no [`SsCellmapTextures`]. The render world reconciles this
    /// against what it has bound.
    pub overrides: Vec<(String, Handle<Image>)>,
}

/// A slot for `extract_players` to fill: what a cast that grew is padded with.
impl Default for ExtractedPlayer {
    fn default() -> Self {
        Self { entity: MainEntity::from(Entity::PLACEHOLDER), pack: AssetId::default(), settled: false, animation: String::new(), transition_ms: 0.0, transform: SsTransform::default(), playback: SsPlayback::default(), overrides: Vec::new() }
    }
}

/// This frame's hand-off, written over last frame's each extract.
///
/// The order of `players` is the order the render world keeps them in, which is
/// what breaks ties between equal `SsPlayback::order` values — so it must not
/// depend on hashing.
#[derive(Resource, Default)]
pub(crate) struct ExtractedPlayers {
    pub delta_secs: f32,
    pub players: Vec<ExtractedPlayer>,
    /// The transport commands written this frame, each kind in the order its
    /// messages were written.
    pub seeks: Vec<(MainEntity, f32)>,
    pub plays: Vec<(MainEntity, Option<f32>)>,
    pub stops: Vec<MainEntity>,
    /// The track-stack commands written this frame, in the order they were written.
    pub track_commands: Vec<(MainEntity, TrackCommand)>,
    /// How far each kind has been read, as a `MessageReader` keeps it. When the
    /// main world swaps its message buffers is not tied to when extract runs, so
    /// reading only the newer buffer misses a message written just before a swap.
    seek_cursor: MessageCursor<SsSeek>,
    play_cursor: MessageCursor<SsPlay>,
    stop_cursor: MessageCursor<SsStop>,
    track_cursor: MessageCursor<SsTrackCommand>,
}

// A five-part query with an optional is exactly the shape `type_complexity`
// fires on, and the shape Bevy queries take; naming it would obscure, not clarify.
// A system takes what it reads as parameters, one message buffer per kind, so
// the count grows with the kinds; a `SystemParam` bundle would only move the list.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn extract_players(
    mut extracted: ResMut<ExtractedPlayers>,
    time: Extract<Res<Time>>,
    asset_server: Extract<Res<AssetServer>>,
    seeks: Extract<Res<Messages<SsSeek>>>,
    plays: Extract<Res<Messages<SsPlay>>>,
    stops: Extract<Res<Messages<SsStop>>>,
    track_commands: Extract<Res<Messages<SsTrackCommand>>>,
    query: Extract<Query<(Entity, &SsPlayer, &SsTransform, &SsPlayback, Option<&SsCellmapTextures>)>>,
) {
    extracted.delta_secs = time.delta_secs();
    let ExtractedPlayers { seeks: seek_list, plays: play_list, stops: stop_list, track_commands: track_list, seek_cursor, play_cursor, stop_cursor, track_cursor, .. } = &mut *extracted;
    seek_list.clear();
    seek_list.extend(seek_cursor.read(&seeks).map(|seek| (MainEntity::from(seek.entity), seek.frame)));
    play_list.clear();
    play_list.extend(play_cursor.read(&plays).map(|play| (MainEntity::from(play.entity), play.start_frame)));
    stop_list.clear();
    stop_list.extend(stop_cursor.read(&stops).map(|stop| MainEntity::from(stop.entity)));
    track_list.clear();
    track_list.extend(track_cursor.read(&track_commands).map(|command| (MainEntity::from(command.entity), command.command.clone())));
    // Written over last frame's players rather than rebuilt: each keeps its
    // strings, so a cast that holds steady allocates nothing here.
    let count = query.iter().len();
    extracted.players.truncate(count);
    extracted.players.resize_with(count, ExtractedPlayer::default);
    for ((entity, player, transform, playback, textures), out) in query.iter().zip(&mut extracted.players) {
        out.entity = MainEntity::from(entity);
        out.pack = player.pack.id();
        out.settled = matches!(asset_server.get_recursive_dependency_load_state(&player.pack), Some(RecursiveDependencyLoadState::Loaded | RecursiveDependencyLoadState::Failed(_)));
        out.animation.clone_from(&player.animation);
        out.transition_ms = player.transition_ms;
        out.transform = *transform;
        out.playback = *playback;
        copy_overrides(&mut out.overrides, textures.map(|t| &t.overrides));
    }
}

/// Make `into` a copy of `from`, reusing the names `into` already holds.
fn copy_overrides(into: &mut Vec<(String, Handle<Image>)>, from: Option<&HashMap<String, Handle<Image>>>) {
    let Some(from) = from else {
        into.clear();
        return;
    };
    into.truncate(from.len());
    let mut from = from.iter();
    for ((name, handle), (from_name, from_handle)) in into.iter_mut().zip(&mut from) {
        name.clone_from(from_name);
        handle.clone_from(from_handle);
    }
    into.extend(from.map(|(name, handle)| (name.clone(), handle.clone())));
}
