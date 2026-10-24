//! Transport controls on `SSPlayer`.
//!
//! Almost none of this needs a GPU: `SSPlayer` holds no wgpu types
//! and `load` borrows no `Device`, so seeking, sectioning and the
//! playback shape are all reachable without an adapter. Only the one test that
//! has to *advance* time takes a device, because stepping the tree is
//! `SSScene::update`'s job.
//!
//! The wire-value test is the load-bearing one. The runtime discriminates
//! `PlaybackDirection` as `Forward = 1 / Backward = -1` because it multiplies by
//! them, while the C API and every sibling Player use `0 / 1`. Re-exporting the
//! runtime's enum would have put `-1` on the `ffi` and `js` faces and silently
//! disagreed with all of them.

use ssplayer_wgpu::{MaskBackend, PlaybackDirection, PlaybackStyle, SSPlayer, SSRenderer, SSRuntimeError, SSScene, SceneConfig};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// A player with `Basic.ssab` loaded and its first animation selected.
fn loaded() -> SSPlayer {
    let mut player = SSPlayer::new();
    player.load(format!("{}/overall/Basic.ssab", asset_root())).unwrap();
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();
    player
}

#[test]
fn wire_values_are_zero_and_one_not_the_runtime_discriminants() {
    assert_eq!(PlaybackDirection::Forward.as_wire(), 0);
    assert_eq!(PlaybackDirection::Backward.as_wire(), 1);
    assert_eq!(PlaybackStyle::Normal.as_wire(), 0);
    assert_eq!(PlaybackStyle::PingPong.as_wire(), 1);

    for d in [PlaybackDirection::Forward, PlaybackDirection::Backward] {
        assert_eq!(PlaybackDirection::from_wire(d.as_wire()), d);
    }
    for s in [PlaybackStyle::Normal, PlaybackStyle::PingPong] {
        assert_eq!(PlaybackStyle::from_wire(s.as_wire()), s);
    }

    // Unknown input reads as the default rather than panicking, matching the C
    // API's own `direction == 0` test.
    assert_eq!(PlaybackDirection::from_wire(-1), PlaybackDirection::Forward);
    assert_eq!(PlaybackDirection::from_wire(99), PlaybackDirection::Forward);
    assert_eq!(PlaybackStyle::from_wire(-1), PlaybackStyle::Normal);
}

#[test]
fn an_empty_player_answers_instead_of_panicking() {
    let player = SSPlayer::new();

    assert_eq!(player.frame_no(), 0.0);
    assert_eq!(player.start_frame(), 0);
    assert_eq!(player.end_frame(), 0);
    assert_eq!(player.total_frames(), 0);
    assert_eq!(player.frame_rate(), 0);
    assert_eq!(player.playback_direction(), PlaybackDirection::Forward);
    assert_eq!(player.playback_style(), PlaybackStyle::Normal);
    assert!(player.frame_skip_enabled());
    assert!(!player.is_pausing());
    assert!(!player.is_finished());
    assert!(!player.just_looped());
}

#[test]
fn a_loaded_animation_reports_its_range() {
    let player = loaded();

    assert!(player.frame_rate() > 0, "fps comes from the pack");
    assert!(player.end_frame() >= player.start_frame());
    // Inclusive of both ends, matching every sibling Player's total-frames.
    assert_eq!(player.total_frames(), player.end_frame() - player.start_frame() + 1);
    assert!(player.total_frames() > 1);
}

#[test]
fn seek_moves_the_playhead_without_a_device() {
    let mut player = loaded();

    player.seek(3.5);
    assert_eq!(player.frame_no(), 3.5, "fractional seeks are honoured");

    player.seek(0.0);
    assert_eq!(player.frame_no(), 0.0);
}

#[test]
fn a_section_narrows_the_played_range() {
    let mut player = loaded();

    let full = player.total_frames();
    assert!(full > 4, "need room to carve a section");

    let start = player.start_frame();
    player.set_animation_section(start, start + 3).unwrap();

    assert_eq!(player.start_frame(), start);
    assert_eq!(player.end_frame(), start + 3);
    assert_eq!(player.total_frames(), 4);
    assert!(player.total_frames() < full);
}

#[test]
fn a_section_starting_on_the_last_frame_holds_it() {
    let mut player = loaded();
    let last = player.end_frame();
    assert!(last > player.start_frame(), "need more than one frame");

    player.set_animation_section(last, last).unwrap();
    assert_eq!((player.start_frame(), player.end_frame(), player.total_frames()), (last, last, 1));
    assert_eq!(player.frame_no(), last as f32, "the playhead moves to the section's start");

    // An end out of range is the animation's edge, which is that same frame.
    player.set_animation_section(last, -1).unwrap();
    assert_eq!((player.start_frame(), player.end_frame()), (last, last));
}

#[test]
fn a_section_ending_before_it_starts_is_refused_and_changes_nothing() {
    let mut player = loaded();
    let (start, last) = (player.start_frame(), player.end_frame());
    assert!(player.total_frames() > 4, "need room to carve a section");

    player.set_animation_section(start + 1, start + 3).unwrap();
    player.seek(start as f32 + 2.5);
    let before = (player.start_frame(), player.end_frame(), player.frame_no());

    assert!(matches!(player.set_animation_section(start + 3, start + 1), Err(SSRuntimeError::InvalidParameter)));
    assert_eq!((player.start_frame(), player.end_frame(), player.frame_no()), before, "the section and the playhead stay as they were");

    // A refusal needs both ends in range: an end out of range is the
    // animation's edge on its side, whatever its number.
    player.set_animation_section(start + 3, -1).unwrap();
    assert_eq!((player.start_frame(), player.end_frame()), (start + 3, last));
}

#[test]
fn selecting_the_animation_already_set_up_does_nothing() {
    let mut player = loaded();

    player.set_animation_section(player.start_frame(), player.start_frame() + 3).unwrap();
    assert_eq!(player.total_frames(), 4);

    // A switch with nothing to switch to. Nothing moves — not the section, not
    // the playhead, not the loop budget. Restarting is `play`'s job.
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();
    assert_eq!(player.total_frames(), 4, "the section should still be in force");
}

#[test]
fn a_real_switch_drops_the_section_and_keeps_the_rest() {
    let mut player = loaded();
    let names = player.animation_names().unwrap();
    let Some(other) = names.iter().find(|n| Some(n.as_str()) != player.current_animation()).cloned() else {
        return; // single-animation pack: nothing to switch to
    };
    let full = player.total_frames();
    assert!(full > 4, "need room to carve a section");

    player.set_animation_section(player.start_frame(), player.start_frame() + 3).unwrap();
    player.set_speed_scale(2.0);
    player.set_frame_skip_enabled(false);
    player.set_playback_direction(PlaybackDirection::Backward, PlaybackStyle::PingPong);
    player.set_loop_count(3);

    player.set_animation(&other).unwrap();

    // The section goes: frame numbers mean nothing against a different clip, and
    // the runtime drops it for that reason.
    assert_eq!(player.total_frames(), player.end_frame() - player.start_frame() + 1);
    assert!(player.total_frames() > 4, "the section should not have followed");

    // Everything else stays: the runtime carries these across a setup on one
    // context.
    assert_eq!(player.speed_scale(), 2.0, "speed describes the playback, not the clip");
    assert!(!player.frame_skip_enabled(), "frame skip is a Player policy");
    assert_eq!(player.playback_direction(), PlaybackDirection::Backward);
    assert_eq!(player.playback_style(), PlaybackStyle::PingPong);
    // The loop count is the one the runtime drops, because the transition states
    // it — so the Player puts its own back, as every sibling Player does.
    assert_eq!(player.loop_count(), 3, "the host's loop count survives the switch");
}

/// The settings are the player's, not the tree's. This Player builds a new
/// context for its first animation and for a new pack, where the siblings keep
/// one context and rebind it, so without holding them itself the four would go
/// back to the runtime's defaults at exactly those two points.
#[test]
fn transport_set_before_the_first_animation_holds_across_a_new_pack() {
    let mut player = SSPlayer::new();
    player.set_speed_scale(0.5);
    player.set_frame_rate(12);
    player.set_frame_skip_enabled(false);
    player.set_playback_direction(PlaybackDirection::Backward, PlaybackStyle::PingPong);

    let check = |player: &SSPlayer, when: &str| {
        assert_eq!(player.speed_scale(), 0.5, "speed {when}");
        assert_eq!(player.frame_rate(), 12, "frame-rate override {when}");
        assert!(!player.frame_skip_enabled(), "frame skip {when}");
        assert_eq!(player.playback_direction(), PlaybackDirection::Backward, "direction {when}");
        assert_eq!(player.playback_style(), PlaybackStyle::PingPong, "style {when}");
    };
    check(&player, "read back before any tree exists");

    for pack in ["overall/Basic.ssab", "Ringo/Ringo.ssab"] {
        player.load(format!("{}/{pack}", asset_root())).unwrap();
        check(&player, &format!("after loading {pack}, before an animation"));
        let anim = player.animation_names().unwrap().first().cloned().unwrap();
        player.set_animation(&anim).unwrap();
        check(&player, &format!("once {pack}'s {anim} is set up"));
    }
}

/// A speed at or below zero is the stop the runtime turns it into, whether or
/// not a tree exists yet to hand it to.
#[test]
fn a_stop_set_before_the_first_animation_reads_back_as_a_stop() {
    let mut player = SSPlayer::new();
    player.set_speed_scale(-1.0);
    assert_eq!(player.speed_scale(), 0.0);

    player.load(format!("{}/overall/Basic.ssab", asset_root())).unwrap();
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();
    assert_eq!(player.speed_scale(), 0.0);
}

#[test]
fn playback_shape_round_trips_through_the_runtime() {
    let mut player = loaded();

    assert_eq!(player.playback_direction(), PlaybackDirection::Forward);
    assert_eq!(player.playback_style(), PlaybackStyle::Normal);

    player.set_playback_direction(PlaybackDirection::Backward, PlaybackStyle::PingPong);
    // Reading these back proves the conversion survives BOTH directions across
    // the runtime's differently-discriminated enum.
    assert_eq!(player.playback_direction(), PlaybackDirection::Backward);
    assert_eq!(player.playback_style(), PlaybackStyle::PingPong);

    player.set_playback_direction(PlaybackDirection::Forward, PlaybackStyle::Normal);
    assert_eq!(player.playback_direction(), PlaybackDirection::Forward);
    assert_eq!(player.playback_style(), PlaybackStyle::Normal);
}

#[test]
fn loop_count_and_skip_frames_round_trip() {
    let mut player = loaded();

    player.set_loop_count(3);
    assert_eq!(player.loop_count(), 3);

    assert!(player.frame_skip_enabled(), "the runtime default is on");
    player.set_frame_skip_enabled(false);
    assert!(!player.frame_skip_enabled());
    player.set_frame_skip_enabled(true);
    assert!(player.frame_skip_enabled());
}

#[test]
fn frame_rate_overrides_the_authored_rate() {
    let mut player = loaded();

    let authored = player.frame_rate();
    assert!(authored > 0);

    player.set_frame_rate(authored * 2);
    assert_eq!(player.frame_rate(), authored * 2);
}

#[test]
fn backward_playback_retreats() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(&queue, 256.0, 256.0, false);

    let mut player = loaded();
    player.set_loop_count(-1);
    player.set_playback_direction(PlaybackDirection::Backward, PlaybackStyle::Normal);
    player.play();
    player.seek(player.end_frame() as f32);

    let from = player.frame_no();
    scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);

    assert!(player.frame_no() < from, "the playhead should retreat: {from} -> {}", player.frame_no());
}

/// `is_playing_forward` is the *actual* heading, and under ping-pong that parts
/// company with the configured one: `playback_direction` still reads Forward on
/// the return leg. This is the distinction that makes it the audio gate rather
/// than `playback_direction` — SpriteStudio has no reverse audio, so a key
/// crossed on the way back is not meant to sound.
#[test]
fn is_playing_forward_tracks_the_leg_not_the_configured_direction() {
    let mut player = loaded();
    // No animation set up yet reads forward: forward is the resting state, and
    // reporting backward would gate a host's audio off on its first frame.
    assert!(SSPlayer::new().is_playing_forward());

    player.set_loop_count(-1);
    player.set_playback_direction(PlaybackDirection::Forward, PlaybackStyle::PingPong);
    player.play();
    assert!(player.is_playing_forward(), "the outward leg runs forward");

    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping the return leg");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(&queue, 256.0, 256.0, false);

    // Walk into the return leg. Stepping rather than seeking: the heading is
    // derived from how the playhead crossed the end, which a seek does not do.
    let mut turned = false;
    for _ in 0..600 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        if !player.is_playing_forward() {
            turned = true;
            break;
        }
    }
    assert!(turned, "ping-pong must eventually turn around");
    assert_eq!(player.playback_direction(), PlaybackDirection::Forward, "the configured heading is untouched by the turn — that is the whole point");
}

/// The four transport verbs.
/// `pause` holds and nothing else; `resume` is what lifts the hold; `play`
/// heads out and so cannot serve as a resume; `stop` leaves the playhead alone.
#[test]
fn pause_holds_and_only_resume_lifts_it() {
    let mut player = loaded();

    // Nothing is playing yet: neither flag is set, and pause is ignored.
    assert!(!player.is_playing());
    assert!(!player.is_pausing());
    player.pause();
    assert!(!player.is_pausing(), "pausing a never-played animation does nothing");

    player.play();
    assert!(player.is_playing());
    assert!(!player.is_pausing());

    player.pause();
    assert!(player.is_pausing());
    assert!(player.is_playing(), "a hold is not a stop");

    player.pause();
    assert!(player.is_pausing(), "pause is idempotent — it never resumes");

    player.resume();
    assert!(!player.is_pausing());
    player.resume();
    assert!(!player.is_pausing(), "resume is idempotent too");

    // play() clears a hold, but by heading out rather than carrying on.
    player.pause();
    player.play();
    assert!(!player.is_pausing());

    // stop() clears it as well, and resume does not restart from there.
    player.pause();
    player.stop();
    assert!(!player.is_playing());
    assert!(!player.is_pausing(), "stopped is not held");
    player.resume();
    assert!(!player.is_playing(), "resume lifts a hold; it does not start playback");
}
