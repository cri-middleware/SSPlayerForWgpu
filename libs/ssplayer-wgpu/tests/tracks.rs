//! The track stack, reached through `SSPlayer`.
//!
//! Almost none of this needs a GPU: tracks are runtime state and `SSPlayer`
//! holds no wgpu types, so most of it is a pass-through check — that the player
//! forwards to the right context, that track 0 stays the structure track, and
//! that fades are advanced by the same `update` the playhead uses. Only the
//! tests that have to advance time take a device, because stepping the tree is
//! `SSScene::update`'s job.
//!
//! What the runtime *computes* is the SDK's own business and is tested there;
//! what this file protects is the seam.

use ssplayer_wgpu::{MaskBackend, PlaybackDirection, PlaybackStyle, SSPlayer, SSRenderer, SSScene, SceneConfig, TrackBlend, TrackSync};

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

/// A crossfade on track 0 hands the player over to the incoming animation from
/// the call on, so the animation it left behind can be selected again. Recorded
/// only when the fade completed, `set_animation` took the outgoing animation
/// for the one still selected and did nothing.
#[test]
fn a_crossfade_on_track_zero_selects_the_incoming_animation_at_once() {
    let Some((mut player, names)) = ringo() else { return };
    player.crossfade(0, &names[1], 400.0).unwrap();
    assert_eq!(player.current_animation(), Some(names[1].as_str()));

    player.set_animation(&names[1]).unwrap();
    assert!(player.track_fade_progress(0).is_some(), "the animation already selected: the fade goes on");

    player.set_animation(&names[0]).unwrap();
    assert_eq!(player.current_animation(), Some(names[0].as_str()));
    assert!(player.track_fade_progress(0).is_none(), "selecting the outgoing animation again rebuilt the stack");
}

/// Only track 0 decides the current animation, whichever call lays one on it.
#[test]
fn only_track_zero_s_animation_is_the_current_one() {
    let Some((mut player, names)) = ringo() else { return };
    player.crossfade(1, &names[1], 400.0).unwrap();
    player.set_track_animation(1, &names[2]).unwrap();
    assert_eq!(player.current_animation(), Some(names[0].as_str()));

    player.set_track_animation(0, &names[2]).unwrap();
    assert_eq!(player.current_animation(), Some(names[2].as_str()));
}

/// The host's transport holds across a switch of track 0's animation by any
/// call, as it does across `set_animation`: the loop count, which the runtime
/// restates per animation, is put back by the player.
#[test]
fn track_zero_s_transport_holds_across_a_crossfade() {
    let Some((mut player, names)) = ringo() else { return };
    player.set_speed_scale(2.0);
    player.set_loop_count(3);

    player.crossfade(0, &names[1], 400.0).unwrap();
    assert_eq!(player.speed_scale(), 2.0);
    assert_eq!(player.loop_count(), 3);

    player.set_track_animation(0, &names[2]).unwrap();
    assert_eq!(player.speed_scale(), 2.0);
    assert_eq!(player.loop_count(), 3);
}

/// On track 0 `set_track_loop_count` is `set_loop_count`: the count it writes
/// is the one put back on the next animation, not the one set before it.
#[test]
fn track_zero_s_loop_count_by_either_call_holds_across_a_switch() {
    let Some((mut player, names)) = ringo() else { return };
    player.set_track_loop_count(0, 2).unwrap();
    player.crossfade(0, &names[1], 400.0).unwrap();
    assert_eq!(player.track_loop_count(0), Some(2), "a count never set with set_loop_count");

    player.set_loop_count(3);
    player.set_track_loop_count(0, 1).unwrap();
    player.set_track_animation(0, &names[2]).unwrap();
    assert_eq!(player.loop_count(), 1, "the count set before it came back");
    player.set_animation(&names[0]).unwrap();
    assert_eq!(player.loop_count(), 1);
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

/// A track's playback settings reach the track they name, and read back.
#[test]
fn a_track_s_playback_settings_reach_it() {
    let Some((mut player, names)) = ringo() else { return };
    player.set_track_animation(1, &names[1]).unwrap();

    player.set_track_speed(1, 0.5).unwrap();
    assert_eq!(player.track_speed(1), Some(0.5));
    player.set_track_frame_rate(1, 24).unwrap();
    assert_eq!(player.track_frame_rate(1), Some(24));
    player.set_track_frame_skip_enabled(1, false).unwrap();
    assert_eq!(player.track_frame_skip_enabled(1), Some(false));
    player.set_track_playback_direction(1, PlaybackDirection::Backward, PlaybackStyle::PingPong).unwrap();
    assert_eq!(player.track_playback_direction(1), Some(PlaybackDirection::Backward));
    assert_eq!(player.track_playback_style(1), Some(PlaybackStyle::PingPong));
    assert_eq!(player.track_outgoing_event_threshold(1), Some(f32::INFINITY), "silent by default");
    player.set_track_outgoing_event_threshold(1, 0.5).unwrap();
    assert_eq!(player.track_outgoing_event_threshold(1), Some(0.5));

    assert_eq!(player.track_loop_count(1), Some(-1), "laid looping forever");
    player.set_track_loop_count(1, 2).unwrap();
    assert_eq!(player.track_loop_count(1), Some(2));
    assert_eq!(player.track_speed(0), Some(1.0), "track 0 is untouched");

    player.set_track_animation(1, &names[2]).unwrap();
    assert_eq!(player.track_speed(1), Some(0.5), "the settings are the track's");
    assert_eq!(player.track_loop_count(1), Some(-1), "the loop count is the animation's");
}

#[test]
fn track_playback_settings_need_an_animation() {
    let mut player = SSPlayer::new();
    assert!(player.set_track_speed(0, 0.5).is_err());
    assert!(player.set_track_loop_count(0, 1).is_err());
    assert!(player.set_track_outgoing_event_threshold(0, 0.5).is_err());
    assert!(player.track_speed(0).is_none());
    assert!(!player.is_track_finished(0) && !player.track_just_looped(0));
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// An overlay reports its own loop and, given a count, its own end, while track
/// 0 plays on; a culled tick reports no loop, as it reports nothing else.
#[test]
fn an_overlay_track_reports_its_loop_and_its_end() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(&queue, 256.0, 256.0, false);
    let Some((mut player, _)) = ringo() else { return };
    player.set_animation("wait1").unwrap();
    player.play();
    player.set_track_animation(1, "attack1").unwrap();
    let mut tick = |player: &mut SSPlayer| scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [player]);

    let looped = (0..120).any(|_| {
        tick(&mut player);
        player.track_just_looped(1)
    });
    assert!(looped, "attack1 never wrapped on track 1");
    player.set_culled(true);
    tick(&mut player);
    assert!(!player.track_just_looped(1), "a culled tick reported the loop of the last one that ran");
    player.set_culled(false);

    player.set_track_animation(1, "attack1").unwrap();
    player.set_track_loop_count(1, 1).unwrap();
    let finished = (0..120).any(|_| {
        tick(&mut player);
        player.is_track_finished(1)
    });
    assert!(finished, "attack1 never finished on track 1");
    assert!(player.is_playing() && !player.is_finished(), "track 0 plays on");
}

/// What the canvas reports follows track 0 from the moment a crossfade on it
/// begins. Ringo's `dead` is drawn on a wider canvas than its other animations.
#[test]
fn the_canvas_follows_a_crossfade_on_track_zero() {
    let Some((mut player, _)) = ringo() else { return };
    player.set_animation("wait1").unwrap();
    assert_eq!(player.canvas_size(), Some((800.0, 600.0)), "fixture sanity");
    player.crossfade(0, "dead", 400.0).unwrap();
    assert_eq!(player.canvas_size(), Some((1300.0, 600.0)));
}

/// What advances a player in these tests: one renderer and scene, ticked at
/// 60 Hz. `None` without an adapter.
struct Clock {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: SSRenderer,
    scene: SSScene,
}

impl Clock {
    fn new() -> Option<Self> {
        let Some((device, queue)) = gpu() else {
            eprintln!("no adapter — skipping");
            return None;
        };
        let renderer = SSRenderer::new(&device);
        let mut scene = SSScene::new(&device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
        scene.set_viewport(&queue, 256.0, 256.0, false);
        Some(Self { device, queue, renderer, scene })
    }

    fn tick(&mut self, players: &mut [&mut SSPlayer]) {
        self.scene.update(&mut self.renderer, &self.device, &self.queue, 1.0 / 60.0, players);
    }
}

/// A crossfade is advanced by the same update as the playhead, completes, and
/// releases the animation it faded out; the one it faded to stays selected, and
/// the one it left can be selected again.
#[test]
fn a_crossfade_advances_on_update_and_completes() {
    let Some(mut clock) = Clock::new() else { return };
    let Some((mut player, _)) = ringo() else { return };
    player.set_animation("wait1").unwrap();
    player.play();
    let outgoing = player.track_main_source(0).unwrap();
    player.crossfade(0, "walk", 200.0).unwrap();
    let incoming = player.track_main_source(0).unwrap();

    let mut last = 0.0;
    for _ in 0..30 {
        clock.tick(&mut [&mut player]);
        match player.track_fade_progress(0) {
            Some(progress) => {
                assert!(progress > last, "the fade did not advance: {last} -> {progress}");
                last = progress;
            }
            None => break,
        }
    }
    assert!(player.track_fade_progress(0).is_none(), "a 200 ms fade outlasted half a second");
    assert_eq!(player.track_main_source(0), Some(incoming));
    assert!(player.source_weight(outgoing).is_none(), "the outgoing animation was not released");
    assert_eq!(player.current_animation(), Some("walk"));

    player.set_animation("wait1").unwrap();
    assert_eq!(player.current_animation(), Some("wait1"), "the animation the fade left could not be selected again");
}

/// An overlay plays on the player's update, and the player's pause holds it.
#[test]
fn an_overlay_plays_on_update_and_holds_on_pause() {
    let Some(mut clock) = Clock::new() else { return };
    let Some((mut player, _)) = ringo() else { return };
    player.set_animation("wait1").unwrap();
    player.play();
    player.set_track_animation(1, "dance").unwrap();

    let start = player.track_frame(1).unwrap();
    for _ in 0..5 {
        clock.tick(&mut [&mut player]);
    }
    let moved = player.track_frame(1).unwrap();
    assert!(moved > start, "the overlay did not advance: {start} -> {moved}");

    player.pause();
    clock.tick(&mut [&mut player]);
    assert_eq!(player.track_frame(1), Some(moved), "the pause did not hold the overlay");

    player.resume();
    clock.tick(&mut [&mut player]);
    assert!(player.track_frame(1).unwrap() > moved, "the resume did not release the overlay");
}

/// A crossfade on track 0 plays the incoming animation at the speed the host
/// set, as `set_animation` would. Laid at the defaults, it played at 1×.
#[test]
fn a_crossfade_plays_the_incoming_animation_at_the_host_s_speed() {
    let Some(mut clock) = Clock::new() else { return };
    let Some((mut normal, _)) = ringo() else { return };
    let Some((mut fast, _)) = ringo() else { return };
    for (player, speed) in [(&mut normal, 1.0), (&mut fast, 2.0)] {
        player.set_animation("wait1").unwrap();
        player.set_speed_scale(speed);
        player.play();
        player.crossfade(0, "walk", 1000.0).unwrap();
    }
    for _ in 0..10 {
        clock.tick(&mut [&mut normal, &mut fast]);
    }
    let (normal_frame, fast_frame) = (normal.track_frame(0).unwrap(), fast.track_frame(0).unwrap());
    assert!(normal_frame > 0.0, "fixture sanity: walk advanced");
    assert!((fast_frame - 2.0 * normal_frame).abs() < 1.0, "walk at 2× reached {fast_frame}, at 1× {normal_frame}");
}

/// The loop count the host set holds for the animation a crossfade hands
/// track 0 to, so its end is reported. Laid looping forever, it never ended.
#[test]
fn the_incoming_animation_ends_on_the_host_s_loop_count() {
    let Some(mut clock) = Clock::new() else { return };
    let Some((mut player, _)) = ringo() else { return };
    player.set_loop_count(1);
    player.set_animation("wait1").unwrap();
    player.play();
    player.crossfade(0, "attack1", 100.0).unwrap();

    // attack1 is 20 frames at 60 fps: a third of a second.
    let ends = (0..60)
        .filter(|_| {
            clock.tick(&mut [&mut player]);
            player.just_finished()
        })
        .count();
    assert_eq!(ends, 1, "attack1 ended {ends} times in a second");
    assert!(player.is_finished());
}

/// The animation a crossfade leaves running out does not finish the player:
/// track 0 changed hands when the fade began, and the new animation has not.
#[test]
fn an_outgoing_animation_running_out_does_not_finish_the_player() {
    let Some(mut clock) = Clock::new() else { return };
    let Some((mut player, _)) = ringo() else { return };
    player.set_loop_count(1);
    player.set_animation("attack1").unwrap();
    player.play();
    player.seek(18.0);
    player.crossfade(0, "wait1", 1000.0).unwrap();

    // Past attack1's last frame, well short of wait1's 80.
    for _ in 0..20 {
        clock.tick(&mut [&mut player]);
        assert!(!player.just_finished() && !player.is_finished(), "the outgoing animation finished the player");
    }
    assert!(player.track_fade_progress(0).is_some(), "fixture sanity: still fading");
}

/// A crossfade restarted mid-fade takes the fade in flight over as it stands:
/// neither animation of the first fade jumps when the second begins. Settled
/// first, the incoming animation of the first leapt to full weight and the
/// outgoing one vanished.
#[test]
fn a_crossfade_restarted_mid_fade_carries_the_pose_over() {
    let Some(mut clock) = Clock::new() else { return };
    let Some((mut player, _)) = ringo() else { return };
    player.set_animation("wait1").unwrap();
    player.play();
    let first = player.track_main_source(0).unwrap();
    player.crossfade(0, "walk", 1000.0).unwrap();
    let second = player.track_main_source(0).unwrap();
    for _ in 0..15 {
        clock.tick(&mut [&mut player]);
    }
    let (first_weight, second_weight) = (player.source_weight(first).unwrap(), player.source_weight(second).unwrap());
    assert!(second_weight > 0.0 && first_weight > 0.0, "fixture sanity: mid-fade, {first_weight} / {second_weight}");

    player.crossfade(0, "run", 1000.0).unwrap();
    let third = player.track_main_source(0).unwrap();
    let close = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-4);
    assert!(close(player.source_weight(first), first_weight), "the first animation jumped: {first_weight} -> {:?}", player.source_weight(first));
    assert!(close(player.source_weight(second), second_weight), "the second animation jumped: {second_weight} -> {:?}", player.source_weight(second));
    assert_eq!(player.source_weight(third), Some(0.0), "the new animation starts from nothing");
}
