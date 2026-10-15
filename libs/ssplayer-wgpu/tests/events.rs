//! Timeline events and the end-of-playback notifications.
//!
//! These pin two things, because
//! both are invisible until they are wrong and both have bitten sibling Players:
//!
//! 1. **Events on frames that frame skipping jumped over must still fire.** A
//!    slow tick advances several frames at once; the runtime records every frame
//!    it stepped through, and the Player has to read *that* rather than looking
//!    at the frame it landed on.
//! 2. **`finished` is an edge, not a state.** The runtime's flag is sticky until
//!    the next play, so reporting it raw fires every frame afterwards, and
//!    forgetting to re-arm the latch in `play` loses the *next* completion.
//!
//! The start edge sits beside them: raised by the first tick that runs after a
//! `play`, never by `resume`, and dropped when the `play` is overtaken before
//! any tick ran.
//!
//! The event tests run against packs that actually carry events and **fail if
//! none of them produced any**. `Basic.ssab` has no timeline events at all, so
//! an earlier draft of this file passed while asserting nothing.
//!
//! Advancing needs a device (stepping the tree is `SSScene::update`'s job), so
//! these skip when no adapter is available, as the other GPU tests do.

use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig, SsEvent};

/// Packs authored with timeline events, one per kind.
const EVENT_PACKS: [&str; 3] = ["overall/UserData.ssab", "overall/Signal.ssab", "overall/Sound.ssab"];

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// A scene to drive players through. `update` is what advances the clock.
fn scene(device: &wgpu::Device, queue: &wgpu::Queue) -> SSScene {
    let mut scene = SSScene::new(device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(queue, 256.0, 256.0, false);
    scene
}

/// A fixture that does not load fails the test rather than skipping it, which
/// would report `ok` having checked nothing. `fixtures.rs` says the same once for
/// the whole suite.
const FIXTURE: &str = "the fixture did not load; run scripts/deploy-examples.sh";

/// Load a pack and select its first animation, without starting it.
fn player_for(pack: &str) -> SSPlayer {
    let mut player = SSPlayer::new();
    player.load(format!("{}/{}", asset_root(), pack)).expect(FIXTURE);
    let name = player.animation_names().and_then(|names| names.first().cloned()).expect("the fixture has no animation");
    player.set_animation(&name).expect(FIXTURE);
    player
}

/// Play `pack` to completion at `delta` seconds per tick, returning everything
/// it reported.
fn play_through(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, pack: &str, delta: f32) -> Vec<SsEvent> {
    let mut scene = scene(device, queue);
    let mut player = player_for(pack);
    player.set_loop_count(1);
    player.play();

    let mut seen = Vec::new();
    for _ in 0..4000 {
        scene.update(renderer, device, queue, delta, &mut [&mut player]);
        seen.extend(player.events().iter().cloned());
        if player.just_finished() {
            break;
        }
    }
    seen
}

#[test]
fn a_player_with_no_animation_reports_nothing() {
    let player = SSPlayer::new();

    assert!(player.events().is_empty());
    assert!(!player.just_started());
    assert!(!player.just_looped());
    assert!(!player.just_finished());
}

/// Ringo, on its first animation, not started, plus the name of a second one.
fn ringo() -> (SSPlayer, String) {
    let mut player = SSPlayer::new();
    player.load(format!("{}/Ringo/Ringo.ssab", asset_root())).expect(FIXTURE);
    let names = player.animation_names().expect(FIXTURE);
    player.set_animation(names.first().expect("Ringo has no animation")).expect(FIXTURE);
    (player, names.get(1).expect("these tests need a second Ringo animation").clone())
}

#[test]
fn started_is_an_edge_raised_by_play_and_not_by_resume() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let (mut player, _) = ringo();
    let mut tick = |player: &mut SSPlayer| scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [player]);

    player.set_loop_count(-1);
    player.play();
    assert!(!player.just_started(), "the tick reports the start, not the call");
    tick(&mut player);
    assert!(player.just_started(), "the first tick after play must report it");
    for _ in 0..5 {
        tick(&mut player);
        assert!(!player.just_started(), "the edge must not repeat");
    }

    player.pause();
    tick(&mut player);
    player.resume();
    for _ in 0..5 {
        tick(&mut player);
        assert!(!player.just_started(), "resume carries on; it is not a start");
    }

    player.play_from(player.start_frame() as f32);
    tick(&mut player);
    assert!(player.just_started(), "play_from is a start too");
}

#[test]
fn a_start_is_reported_by_the_first_tick_that_runs() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let (mut player, other) = ringo();
    let mut tick = |player: &mut SSPlayer| scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [player]);

    // A culled player's ticks do not run, so they report nothing — and the
    // start waits for the first one that does.
    player.set_culled(true);
    player.play();
    for n in 0..3 {
        tick(&mut player);
        assert!(!player.just_started(), "a culled player reported a start (tick {n})");
    }
    player.set_culled(false);
    tick(&mut player);
    assert!(player.just_started(), "the play made while culled must reach the first tick that runs");

    // A hidden player's ticks do run, so its start is reported straight away.
    player.set_visible(false);
    player.play();
    tick(&mut player);
    assert!(player.just_started(), "a hidden player keeps playing, so its start must be reported");
    player.set_visible(true);

    player.play();
    player.stop();
    tick(&mut player);
    assert!(!player.just_started(), "a play stopped before any tick ran never started");

    // Selecting an animation leaves it stopped, so the play before it started nothing.
    player.play();
    player.set_animation(&other).unwrap();
    tick(&mut player);
    assert!(!player.just_started(), "set_animation overtook the play");
}

#[test]
fn finished_is_an_edge_and_survives_a_replay() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let mut player = player_for(EVENT_PACKS[0]);

    player.set_loop_count(1);
    player.play();

    // Run until it completes, counting how many updates report the edge.
    let mut edges = 0;
    for _ in 0..4000 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        if player.just_finished() {
            edges += 1;
        }
        if player.is_finished() && edges > 0 {
            break;
        }
    }
    assert_eq!(edges, 1, "finished must be reported exactly once");
    assert!(player.is_finished(), "and the sticky state should now be set");

    // The sticky flag stays true; the edge does not fire again. Reporting the
    // runtime's flag directly instead of the edge is what would fire forever.
    for _ in 0..5 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        assert!(!player.just_finished(), "the edge must not repeat");
    }

    // Replaying re-arms the latch. Without that, this second completion would be
    // compared against a leftover `true` and silently never reported.
    player.play();
    let mut second = 0;
    for _ in 0..4000 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        if player.just_finished() {
            second += 1;
            break;
        }
    }
    assert_eq!(second, 1, "a replay must be able to complete again");
}

#[test]
fn an_infinite_loop_loops_and_never_finishes() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let mut player = player_for(EVENT_PACKS[0]);

    player.set_loop_count(-1);
    player.play();

    let mut loops = 0;
    for _ in 0..4000 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        if player.just_looped() {
            loops += 1;
        }
        assert!(!player.just_finished(), "an infinite loop never completes");
    }
    assert!(loops > 0, "an infinite loop should have come round at least once");
}

#[test]
fn frame_skipping_does_not_drop_events() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);

    let mut compared = 0;
    for pack in EVENT_PACKS {
        // A play-through at 60fps, then the same one in ticks long enough that
        // most frames are jumped over entirely.
        let per_frame = play_through(&mut renderer, &device, &queue, pack, 1.0 / 60.0);
        if per_frame.is_empty() {
            continue;
        }
        let skipped = play_through(&mut renderer, &device, &queue, pack, 0.5);

        assert_eq!(skipped, per_frame, "{pack}: frame skipping changed the events — {} at 60fps vs {} in long ticks", per_frame.len(), skipped.len());
        compared += 1;
    }

    // Without this the test would pass on packs that carry nothing, which is
    // exactly how the first version of it proved nothing at all.
    assert!(compared > 0, "no pack produced any events; this test proved nothing");
}

#[test]
fn every_event_kind_decodes() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);

    let (mut user_data, mut signals, mut audios) = (0, 0, 0);
    for pack in EVENT_PACKS {
        let events = play_through(&mut renderer, &device, &queue, pack, 1.0 / 60.0);
        for event in &events {
            match event {
                SsEvent::UserData(u) => {
                    user_data += 1;
                    // A marker with nothing set at all would mean the decode read
                    // the wrong fields — the animator has to have filled in one.
                    assert!(u.integer.is_some() || u.rect.is_some() || u.point.is_some() || u.text.is_some(), "{pack}: a UserData marker decoded with every field empty");
                }
                SsEvent::Signal(s) => {
                    signals += 1;
                    // `command_id` is `required` in the schema, so an empty one
                    // means the decode landed on the wrong field.
                    assert!(!s.command_id.is_empty(), "{pack}: a signal decoded without its command");
                }
                SsEvent::Audio(a) => {
                    audios += 1;
                    assert!(!a.sound_name.is_empty(), "{pack}: an audio trigger decoded without its sound name");
                    // A play count, never zero — SpriteStudio has no infinite audio loop.
                    assert!(a.loop_num > 0, "{pack}: audio loop_num should be a play count, got {}", a.loop_num);
                }
            }
        }
    }

    assert!(user_data > 0, "no UserData events decoded (UserData.ssab should carry some)");
    assert!(signals > 0, "no Signal events decoded (Signal.ssab should carry some)");
    assert!(audios > 0, "no Audio events decoded (Sound.ssab should carry some)");
}

#[test]
fn events_describe_only_the_tick_that_just_ran() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let mut player = player_for(EVENT_PACKS[0]);

    player.set_loop_count(-1);
    player.play();

    // The list is refilled per tick, never appended to: a looping animation runs
    // its events past again every cycle, so an accumulating list would grow
    // without bound.
    let mut peak = 0;
    let mut ticks_with_events = 0;
    for _ in 0..2000 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        let n = player.events().len();
        if n > 0 {
            ticks_with_events += 1;
        }
        peak = peak.max(n);
    }

    assert!(ticks_with_events > 1, "expected a looping pack to raise events on several ticks");
    assert!(peak < 64, "events accumulated across ticks (peaked at {peak} in one tick)");
}

/// A culled player reports nothing, rather than the last tick that ran.
///
/// `set_culled(true)` skips the timeline advance entirely, so there is no tick
/// to describe — but the report fields used to be left as they were, and
/// `events()` then answered with the same event on every update for as long as
/// the player stayed culled. A host that plays a sound per event hears it
/// repeat once the player is culled, which is the whole point of polling per tick.
#[test]
fn a_culled_player_reports_no_events() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let mut player = player_for(EVENT_PACKS[0]);

    player.set_loop_count(-1);
    player.play();

    // Stop on a tick that actually reported something: with an empty list the
    // assertion below would hold either way.
    let mut reported = 0;
    for _ in 0..2000 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        reported = player.events().len();
        if reported > 0 {
            break;
        }
    }
    assert!(reported > 0, "no tick of {} raised an event — nothing to cull", EVENT_PACKS[0]);

    player.set_culled(true);
    for tick in 0..5 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        assert!(player.events().is_empty(), "a culled player replayed the last tick's events (tick {tick})");
        assert!(!player.just_looped(), "a culled player reported a loop it did not cross (tick {tick})");
        assert!(!player.just_finished(), "a culled player reported a completion it did not reach (tick {tick})");
    }

    // Unculled, the reports come from the ticks that do run.
    player.set_culled(false);
    let mut reported_again = false;
    for _ in 0..2000 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        if !player.events().is_empty() {
            reported_again = true;
            break;
        }
    }
    assert!(reported_again, "events stopped coming after the player was unculled");
}

/// A player whose tree a `load` dropped reports nothing until an animation is
/// selected again.
///
/// `load` goes through `set_pack`, which drops the tree that was playing and
/// leaves no animation selected, so the updates that follow run no tick. They
/// used to return with the last tick's reports still in place, and a character
/// swapped mid-playback replayed the old pack's events and edges on every update
/// until the host called `set_animation`.
#[test]
fn a_player_with_its_tree_dropped_reports_nothing() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let mut player = player_for(EVENT_PACKS[0]);

    player.set_loop_count(-1);
    player.play();
    let mut reported = 0;
    for _ in 0..2000 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        reported = player.events().len();
        if reported > 0 {
            break;
        }
    }
    assert!(reported > 0, "no tick of {} raised an event — nothing to drop", EVENT_PACKS[0]);

    player.load(format!("{}/{}", asset_root(), EVENT_PACKS[0])).expect("the pack loaded a moment ago");
    assert_eq!(player.current_animation(), None, "a load leaves no animation selected");
    for tick in 0..5 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        assert!(player.events().is_empty(), "a player with no tree replayed the last tick's events (tick {tick})");
        assert!(!player.just_started(), "a player with no tree reported a start (tick {tick})");
        assert!(!player.just_looped(), "a player with no tree reported a loop (tick {tick})");
        assert!(!player.just_finished(), "a player with no tree reported a completion (tick {tick})");
    }
}

/// A hidden player keeps playing, so it keeps reporting.
///
/// `set_visible(false)` only skips the draw. A host that blinks a character by
/// toggling it must still hear its footsteps on time — the timeline is the same
/// one a visible player runs, so the events are the same ones too.
#[test]
fn a_hidden_player_keeps_reporting_events() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let (mut hidden, mut shown) = (player_for(EVENT_PACKS[0]), player_for(EVENT_PACKS[0]));

    for player in [&mut hidden, &mut shown] {
        player.set_loop_count(-1);
        player.play();
    }
    hidden.set_visible(false);

    let mut reported = 0;
    for tick in 0..2000 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut hidden, &mut shown]);
        assert_eq!(hidden.frame_no(), shown.frame_no(), "a hidden player fell behind a visible one (tick {tick})");
        assert_eq!(hidden.events().len(), shown.events().len(), "a hidden player reported different events (tick {tick})");
        reported += hidden.events().len();
    }
    assert!(reported > 0, "no tick of {} raised an event — nothing was compared", EVENT_PACKS[0]);
}

/// Events keep coming across a crossfade on track 0, read per firing source,
/// and each one names the source that fired it. The fixtures have no pack with
/// two animations of which one carries events, so the defect this guards — the
/// incoming animation's indices read against the table `set_animation` chose —
/// is pinned in the runtime instead, by the source reporting its animation.
#[test]
fn events_keep_coming_across_a_crossfade_and_name_their_source() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no adapter — skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene(&device, &queue);
    let mut player = player_for(EVENT_PACKS[0]);
    let name = player.current_animation().unwrap().to_owned();
    player.set_loop_count(-1);
    player.play();
    player.crossfade(0, &name, 50.0).unwrap();

    // Only what the incoming animation fires once it holds track 0.
    let mut after = Vec::new();
    for _ in 0..240 {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        if player.track_fade_progress(0).is_none() {
            after.extend(player.events().iter().cloned());
        }
    }
    assert!(!after.is_empty(), "no event came through after the crossfade");
    let main = player.track_main_source(0).unwrap();
    assert!(after.iter().all(|e| e.source() == main), "an event after the crossfade names a source other than track 0's");
}
