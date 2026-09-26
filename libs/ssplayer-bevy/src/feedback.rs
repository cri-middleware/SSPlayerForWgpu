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

use crate::components::SsPlaybackState;

/// One player's playback state as of the render frame that produced it.
pub(crate) struct PlaybackFeedback {
    /// The main-world entity, already mapped back from its render-world twin.
    pub entity: Entity,
    pub frame_no: f32,
    pub is_finished: bool,
    /// The start *edge* — set only on the first tick after a `play()`.
    pub started_edge: bool,
    /// The completion *edge* — set only on the tick the animation finished.
    pub finished_edge: bool,
    /// The loop pulse — set only on the tick the timeline wrapped.
    pub looped: bool,
    pub events: Vec<SsEvent>,
}

/// The shared buffer, held by a clone in each world.
///
/// The render world replaces its contents each frame; the main world takes them.
/// An `Arc<Mutex<..>>` rather than a channel because it has to be a `Resource` in
/// both worlds, and a channel endpoint is not `Sync`.
#[derive(Resource, Clone, Default)]
pub(crate) struct FeedbackChannel(Arc<Mutex<Vec<PlaybackFeedback>>>);

impl FeedbackChannel {
    /// Render side: replace this frame's feedback wholesale. Replacing rather
    /// than appending keeps a main world that skipped a frame from replaying a
    /// stale batch.
    pub(crate) fn store(&self, feedback: Vec<PlaybackFeedback>) {
        if let Ok(mut buffer) = self.0.lock() {
            *buffer = feedback;
        }
    }

    /// Main side: take what the last render frame left.
    pub(crate) fn take(&self) -> Vec<PlaybackFeedback> {
        self.0.lock().map(|mut buffer| std::mem::take(&mut *buffer)).unwrap_or_default()
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
/// has not started yet, or a bumped `restart`. Turning `playing` back on after a
/// pause resumes, and is not a start.
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
/// [`SsPlaybackState`] and raise the reactive messages.
///
/// Runs in `PreUpdate` so a gameplay system reading the messages in `Update`
/// sees this frame's batch. A message for an entity that has since despawned is
/// harmless: nothing reads it against a live component.
pub(crate) fn dispatch_feedback(
    channel: Res<FeedbackChannel>,
    mut states: Query<&mut SsPlaybackState>,
    mut events: MessageWriter<SsEventMessage>,
    mut started: MessageWriter<SsStarted>,
    mut finished: MessageWriter<SsFinished>,
    mut looped: MessageWriter<SsLooped>,
) {
    for feedback in channel.take() {
        if let Ok(mut state) = states.get_mut(feedback.entity) {
            state.frame_no = feedback.frame_no;
            state.finished = feedback.is_finished;
        }
        if feedback.started_edge {
            started.write(SsStarted { entity: feedback.entity });
        }
        for event in feedback.events {
            events.write(SsEventMessage { entity: feedback.entity, event });
        }
        if feedback.finished_edge {
            finished.write(SsFinished { entity: feedback.entity });
        }
        if feedback.looped {
            looped.write(SsLooped { entity: feedback.entity });
        }
    }
}
