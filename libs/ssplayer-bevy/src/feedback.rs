//! The render-world -> main-world hand-off: timeline events and lifecycle edges,
//! carried back so gameplay can react to them.
//!
//! Playback advances in the render world (see [`crate::render`]), so what a tick
//! produces — the events it crossed, whether it started, looped or finished, the
//! frame it landed on — has to cross back. It does so through a buffer the draw
//! system fills and a main-world system drains, which is why it lands **one frame late**:
//! the main world reads on frame N+1 what the render world produced on frame N.
//! For a sound cue or a state change that is imperceptible; a host that cannot
//! tolerate it would have to move playback into the main world, which this crate
//! deliberately does not do.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ssplayer_wgpu::SsEvent;

use crate::components::{SsPlaybackState, SsSourceInfo, SsTrackInfo, SsTrackState};

/// One player's playback state as of the render frame that produced it.
pub(crate) struct PlaybackFeedback {
    /// The main-world entity, already mapped back from its render-world twin.
    pub entity: Entity,
    pub frame_no: f32,
    pub is_finished: bool,
    pub playing_forward: bool,
    /// The start *edge* — set only on the first tick after a `play()`.
    pub started_edge: bool,
    /// The completion *edge* — set only on the tick the animation finished.
    pub finished_edge: bool,
    /// The loop pulse — set only on the tick the timeline wrapped.
    pub looped: bool,
    pub events: Vec<SsEvent>,
    /// The track stack after the advance, for [`SsTrackState`].
    pub tracks: Vec<SsTrackInfo>,
    pub sources: Vec<SsSourceInfo>,
}

/// An element for the render side to fill: what a batch for a cast that grew is
/// padded with.
impl Default for PlaybackFeedback {
    fn default() -> Self {
        Self { entity: Entity::PLACEHOLDER, frame_no: 0.0, is_finished: false, playing_forward: true, started_edge: false, finished_edge: false, looped: false, events: Vec::new(), tracks: Vec::new(), sources: Vec::new() }
    }
}

/// The two batches in flight.
#[derive(Default)]
struct Batches {
    /// What the last render frame produced, until the main world reads it.
    ready: Vec<PlaybackFeedback>,
    /// A batch already read, kept so the next render frame fills its elements,
    /// and their vectors, instead of allocating new ones.
    spare: Vec<PlaybackFeedback>,
}

impl Batches {
    /// Keep `batch` for reuse, unless the one already kept has more room.
    fn keep(&mut self, batch: Vec<PlaybackFeedback>) {
        if batch.capacity() > self.spare.capacity() {
            self.spare = batch;
        }
    }
}

/// The shared buffer, held by a clone in each world.
///
/// The render world publishes a batch each frame; the main world reads it once
/// and hands it back. An `Arc<Mutex<..>>` rather than a channel because it has
/// to be a `Resource` in both worlds, and a channel endpoint is not `Sync`.
#[derive(Resource, Clone, Default)]
pub(crate) struct FeedbackChannel(Arc<Mutex<Batches>>);

impl FeedbackChannel {
    /// Render side: a batch to fill, holding whatever it last held. It is
    /// empty only until two frames have gone through.
    pub(crate) fn spare(&self) -> Vec<PlaybackFeedback> {
        self.0.lock().map(|mut batches| std::mem::take(&mut batches.spare)).unwrap_or_default()
    }

    /// Render side: replace this frame's feedback wholesale. Replacing rather
    /// than appending keeps a main world that skipped a frame from replaying a
    /// stale batch.
    pub(crate) fn publish(&self, batch: Vec<PlaybackFeedback>) {
        if let Ok(mut batches) = self.0.lock() {
            let unread = std::mem::replace(&mut batches.ready, batch);
            batches.keep(unread);
        }
    }

    /// Main side: read what the last render frame left, if it has not been
    /// read already, and hand the batch back for the render side to refill.
    /// What `read` moves out of the elements is what it may keep.
    pub(crate) fn read(&self, read: impl FnOnce(&mut [PlaybackFeedback])) {
        let Ok(mut batch) = self.0.lock().map(|mut batches| std::mem::take(&mut batches.ready)) else { return };
        read(&mut batch);
        if let Ok(mut batches) = self.0.lock() {
            batches.keep(batch);
        }
    }
}

/// A timeline event raised on one character this frame.
///
/// Carries [`SsEvent`] verbatim — match on it for the user-data marker, the
/// signal, or the audio trigger. The player never plays sound itself, so an
/// [`SsEvent::Audio`] is the cue for the host's own audio stack, not a sound.
#[derive(Message, Clone, Debug)]
pub struct SsEventMessage {
    pub entity: Entity,
    pub event: SsEvent,
}

/// Raised once, on the first frame a character's animation ran after a start:
/// a new pack or animation on [`crate::SsPlayer`] while
/// [`crate::SsPlayback`]`::playing` is on, `playing` turning on for a tree that
/// has not started yet, an [`crate::SsPlay`], or a bumped `restart`. Turning
/// `playing` back on after a pause resumes, and is not a start; nor is a new
/// animation with [`crate::SsPlayer::transition_ms`] above zero, which
/// crossfades on the tree that is already playing.
#[derive(Message, Clone, Copy, Debug)]
pub struct SsStarted {
    pub entity: Entity,
}

/// Raised once, on the frame a character's animation reached its end.
#[derive(Message, Clone, Copy, Debug)]
pub struct SsFinished {
    pub entity: Entity,
}

/// Raised once, on the frame a character's timeline wrapped back to the start.
#[derive(Message, Clone, Copy, Debug)]
pub struct SsLooped {
    pub entity: Entity,
}

/// Drain the render world's feedback into the main world: update the pollable
/// [`SsPlaybackState`] and [`SsTrackState`], and raise the reactive messages.
///
/// Runs in `PreUpdate` so a gameplay system reading the messages in `Update`
/// sees this frame's batch. A message for an entity that has since despawned is
/// harmless: nothing reads it against a live component.
pub(crate) fn dispatch_feedback(
    channel: Res<FeedbackChannel>,
    mut states: Query<(&mut SsPlaybackState, &mut SsTrackState)>,
    mut events: MessageWriter<SsEventMessage>,
    mut started: MessageWriter<SsStarted>,
    mut finished: MessageWriter<SsFinished>,
    mut looped: MessageWriter<SsLooped>,
) {
    channel.read(|batch| {
        for feedback in batch {
            if let Ok((mut state, mut stack)) = states.get_mut(feedback.entity) {
                state.frame_no = feedback.frame_no;
                state.finished = feedback.is_finished;
                state.playing_forward = feedback.playing_forward;
                // Swapped rather than copied: the batch goes back with the
                // component's previous vectors, which the render side refills.
                std::mem::swap(&mut stack.tracks, &mut feedback.tracks);
                std::mem::swap(&mut stack.sources, &mut feedback.sources);
            }
            if feedback.started_edge {
                started.write(SsStarted { entity: feedback.entity });
            }
            for event in feedback.events.drain(..) {
                events.write(SsEventMessage { entity: feedback.entity, event });
            }
            if feedback.finished_edge {
                finished.write(SsFinished { entity: feedback.entity });
            }
            if feedback.looped {
                looped.write(SsLooped { entity: feedback.entity });
            }
        }
    });
}
