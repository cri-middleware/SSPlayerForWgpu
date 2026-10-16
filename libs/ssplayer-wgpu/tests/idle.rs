//! A scene whose players have nothing to move keeps the frame it built last.
//!
//! What moves a player is the runtime's to say — its clock, and whether its
//! track stack still moves the pose — and every call that changes what a player
//! draws costs exactly one rebuild. A kept frame uploads nothing, which is what
//! these tests read: [`SceneStats::uploaded_bytes`] is zero on an update that
//! kept the last frame, and not on one that built a new one.
//!
//! [`SceneStats::uploaded_bytes`]: ssplayer_wgpu::SceneStats::uploaded_bytes

use ssplayer_wgpu::{CrowdMember, MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

/// Ringo, set up on `animation` and not started.
fn ringo(animation: &str) -> Option<SSPlayer> {
    let mut player = SSPlayer::new();
    player.load(format!("{}/Ringo/Ringo.ssab", asset_root())).ok()?;
    player.set_animation(animation).ok()?;
    Some(player)
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// One renderer and scene, ticked at 60 Hz. `None` without an adapter.
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

    /// One update; whether it built a new frame.
    fn tick(&mut self, players: &mut [&mut SSPlayer]) -> bool {
        self.scene.update(&mut self.renderer, &self.device, &self.queue, 1.0 / 60.0, players);
        self.scene.stats().uploaded_bytes > 0
    }

    /// How many of `n` updates built a new frame.
    fn builds(&mut self, n: usize, players: &mut [&mut SSPlayer]) -> usize {
        (0..n).filter(|_| self.tick(players)).count()
    }

    /// Tick until `player` has finished, failing past `limit` ticks.
    fn until_finished(&mut self, player: &mut SSPlayer, limit: usize) {
        for _ in 0..limit {
            self.tick(&mut [player]);
            if player.is_finished() {
                return;
            }
        }
        panic!("track 0 never finished in {limit} ticks");
    }
}

#[test]
fn a_stopped_player_is_built_once() {
    let Some(mut clock) = Clock::new() else { return };
    let Some(mut player) = ringo("wait1") else { return };
    assert!(clock.tick(&mut [&mut player]), "the first frame has to be built");
    assert!(clock.scene.stats().draw_calls > 0, "fixture sanity: Ringo draws");
    let draws = clock.scene.stats().draw_calls;
    assert_eq!(clock.builds(10, &mut [&mut player]), 0, "a stopped player was rebuilt");
    assert_eq!(clock.scene.stats().draw_calls, draws, "the kept frame still draws what it drew");
}

#[test]
fn a_playing_player_is_built_every_tick_and_a_paused_one_is_not() {
    let Some(mut clock) = Clock::new() else { return };
    let Some(mut player) = ringo("wait1") else { return };
    player.play();
    assert_eq!(clock.builds(10, &mut [&mut player]), 10);

    player.pause();
    let frame = player.frame_no();
    assert_eq!(clock.builds(10, &mut [&mut player]), 0, "a held player was rebuilt");
    assert_eq!(player.frame_no(), frame);

    player.resume();
    assert_eq!(clock.builds(5, &mut [&mut player]), 5);
}

#[test]
fn a_track_call_rebuilds_a_settled_player_once() {
    let Some(mut clock) = Clock::new() else { return };
    let Some(mut player) = ringo("wait1") else { return };
    player.set_track_animation(1, "attack1").unwrap();
    clock.tick(&mut [&mut player]);

    player.set_track_alpha(1, 0.5).unwrap();
    assert!(clock.tick(&mut [&mut player]), "the track call was not drawn");
    assert_eq!(clock.builds(10, &mut [&mut player]), 0, "the track call kept the player rebuilding");

    // Held rather than stopped: the same.
    player.play();
    clock.tick(&mut [&mut player]);
    player.pause();
    clock.tick(&mut [&mut player]);
    player.set_track_alpha(1, 1.0).unwrap();
    assert!(clock.tick(&mut [&mut player]), "the track call was not drawn");
    assert_eq!(clock.builds(10, &mut [&mut player]), 0, "the track call kept the held player rebuilding");
}

#[test]
fn a_finished_player_settles_and_a_cleared_overlay_does_not_keep_it_moving() {
    let Some(mut clock) = Clock::new() else { return };
    let Some(mut player) = ringo("attack1") else { return };
    player.set_loop_count(1);
    player.play();
    player.set_track_animation(1, "wait1").unwrap();
    clock.until_finished(&mut player, 600);
    assert_eq!(clock.builds(5, &mut [&mut player]), 5, "an overlay still playing after track 0 finished stopped moving");

    player.clear_track(1).unwrap();
    clock.tick(&mut [&mut player]);
    assert_eq!(clock.builds(10, &mut [&mut player]), 0, "a cleared overlay kept the finished player rebuilding");
}

#[test]
fn a_source_beside_track_zero_keeps_moving_after_it_finishes() {
    let Some(mut clock) = Clock::new() else { return };
    let Some(mut player) = ringo("attack1") else { return };
    player.set_loop_count(1);
    player.play();
    let source = player.add_track_source(0, "walk", 1.0).unwrap();
    clock.until_finished(&mut player, 600);

    let frame = player.source_frame(source).unwrap();
    assert_eq!(clock.builds(10, &mut [&mut player]), 10, "the added source stopped being drawn");
    assert_ne!(player.source_frame(source).unwrap(), frame, "the added source stopped moving");

    // Held, nothing moves.
    player.pause();
    clock.tick(&mut [&mut player]);
    assert_eq!(clock.builds(5, &mut [&mut player]), 0);
}

#[test]
fn a_crossfade_is_built_to_its_end_and_then_settles() {
    let Some(mut clock) = Clock::new() else { return };
    let Some(mut player) = ringo("attack1") else { return };
    player.set_loop_count(1);
    player.play();
    clock.until_finished(&mut player, 600);
    assert_eq!(clock.builds(3, &mut [&mut player]), 0, "fixture sanity: the finished player settled");

    player.set_track_animation(1, "wait1").unwrap();
    player.set_track_speed(1, 0.0).unwrap();
    player.fade_track(1, 0.0, 200.0).unwrap();
    let mut built = 0;
    while player.track_fade_progress(1).is_some() {
        assert!(clock.tick(&mut [&mut player]), "a tick of the fade was not built");
        built += 1;
        assert!(built < 60, "the fade never ended");
    }
    assert!(built > 1, "the fade was over in one tick");
    assert_eq!(clock.builds(10, &mut [&mut player]), 0, "the finished fade kept the player rebuilding");
}

#[test]
fn placement_rebuilds_only_when_it_changes() {
    let Some(mut clock) = Clock::new() else { return };
    let Some(mut player) = ringo("wait1") else { return };
    let crowd = [CrowdMember { x: 10.0, ..CrowdMember::default() }, CrowdMember { x: -10.0, ..CrowdMember::default() }];
    let place = |player: &mut SSPlayer, x: f32| {
        player.set_transform(x, 0.0, 1.0, 1.0, 0.0);
        player.set_alpha(1.0);
        player.set_tint(0xFFFFFF);
        player.set_visible(true);
        player.set_culled(false);
        player.set_order(0);
        player.set_crowd(&crowd);
        player.set_sub_frame_enabled(true);
    };
    place(&mut player, 0.0);
    clock.tick(&mut [&mut player]);

    // What an engine adapter does: the same values, every frame.
    let unchanged = (0..10)
        .filter(|_| {
            place(&mut player, 0.0);
            clock.tick(&mut [&mut player])
        })
        .count();
    assert_eq!(unchanged, 0, "writing the same placement rebuilt the frame");

    place(&mut player, 5.0);
    assert!(clock.tick(&mut [&mut player]), "a moved player was not rebuilt");
    assert!(!clock.tick(&mut [&mut player]));
}

#[test]
fn one_moving_player_rebuilds_the_frame_and_a_change_of_cast_does_too() {
    let Some(mut clock) = Clock::new() else { return };
    let (Some(mut still), Some(mut moving)) = (ringo("wait1"), ringo("walk")) else { return };
    clock.tick(&mut [&mut still, &mut moving]);
    assert_eq!(clock.builds(5, &mut [&mut still, &mut moving]), 0);

    moving.play();
    assert_eq!(clock.builds(5, &mut [&mut still, &mut moving]), 5, "the player that moves shares the frame");
    moving.stop();
    clock.tick(&mut [&mut still, &mut moving]);
    assert_eq!(clock.builds(5, &mut [&mut still, &mut moving]), 0);

    // Both settled, but the frame holds the pair.
    assert!(clock.tick(&mut [&mut still]), "a player leaving the scene was not rebuilt out of it");
    assert!(clock.tick(&mut [&mut still, &mut moving]), "a player joining the scene was not rebuilt into it");
    assert!(clock.tick(&mut [&mut moving, &mut still]), "two players swapping places were not rebuilt");
    moving.set_order(-1);
    still.set_order(1);
    assert!(clock.tick(&mut [&mut still, &mut moving]), "a new draw order was not rebuilt");
    assert!(!clock.tick(&mut [&mut still, &mut moving]));
}

#[test]
fn a_kept_frame_reports_no_tick() {
    let Some(mut clock) = Clock::new() else { return };
    let Some(mut player) = ringo("attack1") else { return };
    player.set_loop_count(1);
    player.play();
    clock.until_finished(&mut player, 600);
    assert!(player.just_finished(), "the tick that finished reported it");
    assert!(!clock.tick(&mut [&mut player]));
    assert!(!player.just_finished() && !player.just_looped() && player.events().is_empty(), "a kept frame reported the last tick again");
}
