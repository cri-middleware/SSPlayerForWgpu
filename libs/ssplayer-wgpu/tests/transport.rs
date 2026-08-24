//! Transport controls on `SSPlayer` (SPEC §9.3).
//!
//! Almost none of this needs a GPU: `SSPlayer` holds no wgpu types (SPEC §1.2)
//! and `load` borrows no `Device` (§8.1), so seeking, sectioning and the
//! playback shape are all reachable without an adapter. Only the one test that
//! has to *advance* time takes a device, because stepping the tree is
//! `SSScene::update`'s job.
//!
//! The wire-value test is the load-bearing one. The runtime discriminates
//! `PlaybackDirection` as `Forward = 1 / Backward = -1` because it multiplies by
//! them, while the C API and every sibling Player use `0 / 1`. Re-exporting the
//! runtime's enum would have put `-1` on the `ffi` and `js` faces and silently
//! disagreed with all of them.

use ssplayer_wgpu::{MaskBackend, PlaybackDirection, PlaybackStyle, SSPlayer, SSRenderer, SSScene, SceneConfig};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })).ok()?;
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
    assert!(player.skip_frames());
    assert!(!player.is_paused());
    assert!(!player.is_finished());
    assert!(!player.is_looped());
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
    player.set_animation_section(start, start + 3);

    assert_eq!(player.start_frame(), start);
    assert_eq!(player.end_frame(), start + 3);
    assert_eq!(player.total_frames(), 4);
    assert!(player.total_frames() < full);
}

#[test]
fn set_animation_rebuilds_the_tree_and_takes_the_section_with_it() {
    let mut player = loaded();
    let full = player.total_frames();

    player.set_animation_section(player.start_frame(), player.start_frame() + 3);
    assert_eq!(player.total_frames(), 4);

    // Documented, not incidental: `set_animation` builds a fresh context, so
    // everything set here has to be re-applied after it.
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();
    assert_eq!(player.total_frames(), full, "the full range should be back");
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

    assert!(player.skip_frames(), "the runtime default is on");
    player.set_skip_frames(false);
    assert!(!player.skip_frames());
    player.set_skip_frames(true);
    assert!(player.skip_frames());
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

/// The four transport verbs, as the SDK's Player Porting Guide specifies them.
/// `pause` holds and nothing else; `resume` is what lifts the hold; `play`
/// heads out and so cannot serve as a resume; `stop` leaves the playhead alone.
#[test]
fn pause_holds_and_only_resume_lifts_it() {
    let mut player = loaded();

    // Nothing is playing yet: neither flag is set, and pause is ignored.
    assert!(!player.is_playing());
    assert!(!player.is_paused());
    player.pause();
    assert!(!player.is_paused(), "pausing a never-played animation does nothing");

    player.play();
    assert!(player.is_playing());
    assert!(!player.is_paused());

    player.pause();
    assert!(player.is_paused());
    assert!(player.is_playing(), "a hold is not a stop");

    player.pause();
    assert!(player.is_paused(), "pause is idempotent — it never resumes");

    player.resume();
    assert!(!player.is_paused());
    player.resume();
    assert!(!player.is_paused(), "resume is idempotent too");

    // play() clears a hold, but by heading out rather than carrying on.
    player.pause();
    player.play();
    assert!(!player.is_paused());

    // stop() clears it as well, and resume does not restart from there.
    player.pause();
    player.stop();
    assert!(!player.is_playing());
    assert!(!player.is_paused(), "stopped is not held");
    player.resume();
    assert!(!player.is_playing(), "resume lifts a hold; it does not start playback");
}
