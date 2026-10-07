//! The track stack, reached through `SSPlayer`.
//!
//! None of this needs a GPU: tracks are runtime state and `SSPlayer` holds no
//! wgpu types, so everything here is a pass-through check — that the player
//! forwards to the right context, that track 0 stays the structure track, and
//! that fades are advanced by the same `update` the playhead uses.
//!
//! What the runtime *computes* is the SDK's own business and is tested there;
//! what this file protects is the seam.

use ssplayer_wgpu::{SSPlayer, TrackBlend, TrackSync};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

/// A player with Ringo loaded — 25 animations on one rig, so there is always
/// something to lay on a second track.
fn ringo() -> Option<(SSPlayer, Vec<String>)> {
    let mut player = SSPlayer::new();
    player.load(format!("{}/Ringo/Ringo.ssab", asset_root())).ok()?;
    let names = player.animation_names()?;
    if names.len() < 2 {
        return None;
    }
    player.set_animation(&names[0]).ok()?;
    Some((player, names))
}

#[test]
fn a_player_without_an_animation_reports_no_tracks() {
    let player = SSPlayer::new();
    assert_eq!(player.track_count(), 0);
    assert!(player.track_alpha(0).is_none());
}

#[test]
fn setting_an_animation_gives_one_full_track() {
    let Some((player, _)) = ringo() else { return };
    assert_eq!(player.track_count(), 1);
    assert_eq!(player.track_alpha(0), Some(1.0));
}

#[test]
fn an_overlay_track_can_be_laid_on_and_taken_off() {
    let Some((mut player, names)) = ringo() else { return };

    player.set_track_animation(1, &names[1]).unwrap();
    assert_eq!(player.track_count(), 2);
    assert_eq!(player.track_alpha(1), Some(1.0));

    player.set_track_alpha(1, 0.25).unwrap();
    assert_eq!(player.track_alpha(1), Some(0.25));

    player.set_track_blend(1, TrackBlend::Additive).unwrap();
    assert_eq!(player.track_blend(1), Some(TrackBlend::Additive));

    player.clear_track(1).unwrap();
    // The track survives as a silent slot; only its animation is gone.
    assert_eq!(player.track_count(), 2);
    assert!(player.track_frame(1).is_none());
}

/// The sync reaches the track it names and, like the rest of the stack, goes
/// with the next `set_animation`.
#[test]
fn track_sync_is_set_per_track_and_reset_by_set_animation() {
    let Some((mut player, names)) = ringo() else { return };
    assert_eq!(player.track_sync(0), Some(TrackSync::Free));

    player.set_track_sync(1, TrackSync::Start).unwrap();
    assert_eq!(player.track_sync(1), Some(TrackSync::Start));
    assert_eq!(player.track_sync(0), Some(TrackSync::Free));

    player.set_animation(&names[1]).unwrap();
    assert!(player.track_sync(1).is_none());
    assert_eq!(player.track_sync(0), Some(TrackSync::Free));
}

#[test]
fn track_sync_needs_an_animation() {
    let mut player = SSPlayer::new();
    assert!(player.set_track_sync(0, TrackSync::Start).is_err());
    assert!(player.track_sync(0).is_none());
}

/// A second animation on track 0, mixed by weight: the handle it comes back
/// with reaches its weight, playhead and event threshold, and the track's main
/// source stays the one `set_animation` put there.
#[test]
fn a_source_can_be_added_to_a_track_and_tuned() {
    let Some((mut player, names)) = ringo() else { return };
    let main = player.track_main_source(0).expect("set_animation leaves a main source on track 0");

    let added = player.add_track_source(0, &names[1], 0.3).unwrap();
    assert_ne!(added, main);
    assert_eq!(player.track_main_source(0), Some(main), "adding does not take the track over");
    assert_eq!(player.track_count(), 1, "it shares track 0 rather than opening one");

    assert_eq!(player.source_weight(added), Some(0.3));
    player.set_source_weight(added, 0.7).unwrap();
    assert_eq!(player.source_weight(added), Some(0.7));

    player.set_source_frame(added, 2.0).unwrap();
    assert_eq!(player.source_frame(added), Some(2.0));

    assert_eq!(player.source_event_threshold(added), Some(f32::INFINITY), "only track 0's main source fires by default");
    player.set_source_event_threshold(added, 0.5).unwrap();
    assert_eq!(player.source_event_threshold(added), Some(0.5));
}

#[test]
fn source_calls_reject_a_handle_that_names_nothing() {
    let Some((mut player, _)) = ringo() else { return };
    assert!(player.source_weight(999).is_none());
    assert!(player.source_frame(999).is_none());
    assert!(player.set_source_weight(999, 1.0).is_err());
    assert!(player.set_source_frame(999, 0.0).is_err());
    assert!(player.set_source_event_threshold(999, 0.5).is_err());
}

#[test]
fn source_calls_need_an_animation() {
    let mut player = SSPlayer::new();
    assert!(player.track_main_source(0).is_none());
    assert!(player.add_track_source(0, "anything", 1.0).is_err());
    assert!(player.source_weight(0).is_none());
}

/// Track 0 carries the structure, so it is refused rather than silently
/// emptying the instance.
#[test]
fn track_zero_cannot_be_cleared() {
    let Some((mut player, _)) = ringo() else { return };
    assert!(player.clear_track(0).is_err());
}

/// Selecting an animation rebuilds the instance tree, and the stack goes with it.
#[test]
fn set_animation_collapses_the_stack() {
    let Some((mut player, names)) = ringo() else { return };
    player.set_track_animation(1, &names[1]).unwrap();
    assert_eq!(player.track_count(), 2);

    player.set_animation(&names[2]).unwrap();
    assert_eq!(player.track_count(), 1);
}

/// A modulation axis is driven by parking a track and moving its playhead.
#[test]
fn an_overlay_track_can_be_scrubbed_instead_of_played() {
    let Some((mut player, names)) = ringo() else { return };
    player.set_track_animation(1, &names[1]).unwrap();
    player.set_track_speed(1, 0.0).unwrap();

    player.set_track_frame(1, 4.0).unwrap();
    assert_eq!(player.track_frame(1), Some(4.0));
    player.set_track_frame(1, 7.0).unwrap();
    assert_eq!(player.track_frame(1), Some(7.0));
}

#[test]
fn fades_report_progress_and_are_rejected_when_malformed() {
    let Some((mut player, names)) = ringo() else { return };

    assert!(player.track_fade_progress(0).is_none());
    player.crossfade(0, &names[1], 400.0).unwrap();
    assert_eq!(player.track_fade_progress(0), Some(0.0));

    player.cancel_track_fade(0).unwrap();
    assert!(player.track_fade_progress(0).is_none());

    player.fade_track(0, 0.0, 250.0).unwrap();
    assert_eq!(player.track_fade_progress(0), Some(0.0));

    assert!(player.crossfade(0, &names[1], 0.0).is_err());
    assert!(player.crossfade(0, "not_a_real_anim", 100.0).is_err());
    assert!(player.fade_track(9, 0.0, 100.0).is_err());
}

/// Every track call needs an animation first — there is no stack to address
/// before `set_animation` has built one.
#[test]
fn track_calls_need_an_animation() {
    let mut player = SSPlayer::new();
    assert!(player.set_track_animation(1, "walk").is_err());
    assert!(player.set_track_alpha(1, 0.5).is_err());
    assert!(player.crossfade(0, "walk", 100.0).is_err());
}
