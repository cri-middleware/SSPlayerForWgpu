//! What only a running Bevy app can answer.
//!
//! The player crate's own tests establish that its two mask backends agree, and
//! that its geometry is what it was. Neither says anything about *this* crate:
//! whether the draw system lands on Bevy's `ViewTarget` the right way up, whether
//! the coverage bake really reaches the encoder before the content pass, whether
//! the ordering edge against `main_transparent_pass_2d` is doing anything. Those
//! are Bevy-shaped questions and they need a Bevy app to ask.
//!
//! So these run one headless, render to an `Image`, and read the pixels back.
//! Skips itself when the machine has no adapter, like the rendering tests next
//! door.

use bevy::app::{App, PluginGroup};
use bevy::asset::{AssetPlugin, AssetServer, Assets, Handle, RecursiveDependencyLoadState, RenderAssetUsages};
use bevy::camera::{Camera, Camera2d, ClearColorConfig, Hdr, RenderTarget};
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::image::Image;
use bevy::math::Vec2;
use bevy::prelude::default;
use bevy::render::RenderPlugin;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::Msaa;
use bevy::sprite::Sprite;
use bevy::window::WindowPlugin;
use ssplayer_bevy::{PlaybackDirection, SsPlay, SsPlayback, SsPlaybackState, SsPlayer, SsPlayerPlugin, SsSeek, SsStop, SsTrackCommand, SsTrackState, SsTransform, TrackCommand, TrackSync};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SIZE: u32 = 384;
/// The mask fixture `deploy-examples` generates. Everything here needs a mask —
/// an unmasked character would pass whether or not the coverage bake ran.
const MASK_PACK: &str = "assets/overall/Mask.ssab";
const MASK_ANIM: &str = "anime_1";
/// A pack whose atlas is a file beside it, for comparing what Bevy lends against
/// what the player loads for itself.
const RINGO_PACK: &str = "assets/Ringo/Ringo.ssab";
/// Frames to pump once every pack has settled. Enough for the first coverage
/// bake and a readback to make it back to the CPU.
const FRAMES: u32 = 12;
/// A ceiling on the frames a load may take, so a pack that never settles fails
/// the test instead of hanging it.
const LOAD_FRAMES: u32 = 600;

fn repo_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../..")
}

fn fixture(rel: &str) -> String {
    format!("{}/{rel}", repo_root())
}

/// Where a finished readback lands. A resource rather than a return value
/// because the readback arrives on an observer, several frames after it is asked
/// for.
#[derive(Resource, Clone, Default)]
struct Captured(Arc<Mutex<Option<Vec<u8>>>>);

/// Build the headless app: a render app with no window, our plugin, and an
/// `Image` for the camera to draw into.
fn headless_app() -> Option<(App, Handle<Image>, Captured)> {
    // Bevy panics rather than returning when it cannot make a device, so ask wgpu
    // first and skip quietly if the machine has none.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;

    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            // No window, and no event loop to open one with: winit refuses to build
            // one off the main thread, and only ever once per process — which a
            // test harness running several cases cannot honour.
            .disable::<bevy::winit::WinitPlugin>()
            .set(WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() })
            .set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() })
            // The repository root, so the fixture paths below are asset paths.
            .set(AssetPlugin { file_path: repo_root().into(), ..default() }),
    )
    // `GpuReadbackPlugin` already comes with DefaultPlugins.
    .add_plugins(SsPlayerPlugin);

    let mut target = Image::new_fill(Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 }, TextureDimension::D2, &[0, 0, 0, 255], TextureFormat::Rgba8Unorm, RenderAssetUsages::default());
    // COPY_SRC so the readback can take it back off the GPU.
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT;
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(target);

    let captured = Captured::default();
    app.insert_resource(captured.clone());

    let sink = captured.0.clone();
    app.world_mut().spawn((Readback::texture(handle.clone()),)).observe(move |event: On<ReadbackComplete>| {
        *sink.lock().unwrap() = Some(event.data.clone());
    });

    Some((app, handle, captured))
}

/// Spawn a camera drawing into `handle`, plus whatever `setup` adds.
fn run(mut app: App, handle: Handle<Image>, captured: Captured, setup: impl FnOnce(&mut World)) -> Vec<u8> {
    app.world_mut().spawn((
        Camera2d,
        Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
        // The target is its own component beside `Camera`, not a field on it.
        RenderTarget::Image(handle.into()),
        // The player's pipelines are single-sampled.
        Msaa::Off,
    ));
    setup(app.world_mut());

    // `run()` would do these; driving `update()` by hand does not, and without
    // them `RenderPlugin::finish` never unpacks the device into either world.
    app.finish();
    app.cleanup();

    pump(&mut app);
    let pixels = captured.0.lock().unwrap().clone();
    pixels.expect("the readback never arrived — the render app did not produce a frame")
}

/// Pump until every character's pack has settled, then `FRAMES` more. A load
/// runs on Bevy's IO pool, so how many frames it spans is the machine's business.
fn pump(app: &mut App) {
    for _ in 0..LOAD_FRAMES {
        let world = app.world_mut();
        let server = world.resource::<AssetServer>().clone();
        let settled = world.query::<&SsPlayer>().iter(world).all(|player| matches!(server.get_recursive_dependency_load_state(&player.pack), Some(RecursiveDependencyLoadState::Loaded | RecursiveDependencyLoadState::Failed(_))));
        if settled {
            break;
        }
        app.update();
    }
    for _ in 0..FRAMES {
        app.update();
    }
}

/// The masked character every test here draws, at half size.
fn masked_character(world: &World) -> (SsPlayer, SsTransform, SsPlayback) {
    let pack = world.resource::<AssetServer>().load(MASK_PACK);
    (SsPlayer { pack, animation: MASK_ANIM.into() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback::default())
}

fn drawn_pixels(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>().0.iter().filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

/// Write a readback out, so what these assert about can be looked at:
/// `SS_BEVY_DUMP=<dir> cargo test -p ssplayer-bevy --test bevy_render`.
///
/// A pixel count is a poor description of a picture, and a rendering test that
/// can only be believed is not much of a test.
fn dump(name: &str, pixels: &[u8]) {
    let Ok(dir) = std::env::var("SS_BEVY_DUMP") else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Some(image) = image::RgbaImage::from_raw(SIZE, SIZE, pixels.to_vec()) {
        let _ = image.save(format!("{dir}/{name}.png"));
    }
}

/// Whether the fixtures this file needs were generated.
fn fixtures_present() -> bool {
    Path::new(&fixture(MASK_PACK)).exists()
}

#[test]
fn a_masked_character_draws_inside_bevy() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((app, handle, captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    let pixels = run(app, handle, captured, |world| {
        let character = masked_character(world);
        world.spawn(character);
    });

    dump("01-masked-character-in-bevy", &pixels);
    assert!(drawn_pixels(&pixels) > 0, "nothing reached Bevy's render target");
}

/// The mask has to actually clip.
///
/// Rendering the same character with masking available and with it turned off
/// must differ: if the coverage bake never reached the encoder, or reached it
/// after the content pass, the masked parts draw unclipped and the two are the
/// same picture. That is the failure this catches — and it is invisible to the
/// player crate's tests, which never open Bevy's pass.
#[test]
fn the_mask_clips_and_is_not_a_no_op() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((app, handle, captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    let masked = run(app, handle, captured, |world| {
        let character = masked_character(world);
        world.spawn(character);
    });

    // The same animation drawn by the player crate with masking off, as the
    // "unclipped" reference. Same pack, same frame count, same projection.
    let unmasked = crate::reference::render_unmasked(SIZE, MASK_PACK, MASK_ANIM, true);

    dump("02-masked", &masked);
    dump("02-unmasked-reference", &unmasked);
    let differing = masked.as_chunks::<4>().0.iter().zip(unmasked.as_chunks::<4>().0.iter()).filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 8)).count();
    assert!(differing > (SIZE * SIZE / 200) as usize, "masked and unmasked renders are nearly identical ({differing} pixels differ) — the mask is not clipping anything");
}

/// The characters land *over* Bevy's own 2d content.
///
/// The draw system is ordered after `main_transparent_pass_2d`; without that
/// edge the two command buffers reach the queue in whatever order the scheduler
/// happened to finish them, and the sprite covers the character about half the
/// time. Checking the overlap is the only way that shows up.
#[test]
fn characters_draw_over_bevy_sprites() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((app, handle, captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    let pixels = run(app, handle, captured, |world| {
        // A saturated sprite covering the whole target: anything the player draws
        // has to change these pixels, and nothing the player draws is this colour.
        world.spawn(Sprite { color: Color::srgb(0.0, 1.0, 0.0), custom_size: Some(Vec2::splat(SIZE as f32)), ..default() });
        let character = masked_character(world);
        world.spawn(character);
    });

    dump("03-over-a-bevy-sprite", &pixels);
    let pure_green = pixels.as_chunks::<4>().0.iter().filter(|p| p[1] > 200 && p[0] < 60 && p[2] < 60).count();
    let total = (SIZE * SIZE) as usize;
    assert!(pure_green > 0, "the Bevy sprite did not draw at all — this test would pass vacuously");
    assert!(pure_green < total - total / 50, "the sprite covers the whole target: the character drew under it, so the ordering against main_transparent_pass_2d is not holding");
}

/// A masking character draws in an HDR view.
///
/// An `Hdr` camera renders into an `Rgba16Float` target rather than the usual
/// `Rgba8Unorm`, so the pipelines baked against the old format are invalid and
/// `draw_players` has to rebuild the scene for the new one. If that swap does not
/// happen the draw either panics on the format mismatch or produces nothing; a
/// masked character coming back proves the rebuild fired and the coverage mask
/// still clips at the wider format.
#[test]
fn a_masked_character_draws_in_hdr() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Hdr, Msaa::Off));
    let character = masked_character(app.world());
    app.world_mut().spawn(character);

    app.finish();
    app.cleanup();
    pump(&mut app);
    let pixels = captured.0.lock().unwrap().clone().expect("the readback never arrived — the render app did not produce a frame");

    dump("04-hdr-masked-character", &pixels);
    assert!(drawn_pixels(&pixels) > 0, "nothing drawn in an HDR (Rgba16Float) view — the format-swap path in draw_players may not be rebuilding the scene");
}

/// The atlas reaches the player as Bevy's texture, in its own colours.
///
/// A pack's atlas is a Bevy `Image` lent to the player, not a file the player
/// decodes. Were the lending to miss, the parts would draw as white silhouettes;
/// were the image loaded as sRGB, every colour would come out darker. Either way
/// the picture parts from the one the player draws when it loads the atlas itself,
/// which is what this compares against — on the first frame, held, so the two
/// clocks do not matter.
#[test]
fn the_lent_atlas_draws_as_the_player_draws_it() {
    if !Path::new(&fixture(RINGO_PACK)).exists() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((app, handle, captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    let animation = ssplayer_wgpu::SSPack::load(&std::fs::read(fixture(RINGO_PACK)).unwrap()).unwrap().animation_names()[0].clone();

    let pixels = run(app, handle, captured, |world| {
        let pack = world.resource::<AssetServer>().load(RINGO_PACK);
        world.spawn((SsPlayer { pack, animation: animation.clone() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback { playing: false, ..default() }));
    });
    let reference = crate::reference::render_unmasked(SIZE, RINGO_PACK, &animation, false);

    dump("06-lent-atlas", &pixels);
    dump("06-lent-atlas-reference", &reference);
    let drawn = drawn_pixels(&reference);
    assert!(drawn > 0, "the reference drew nothing — this test would pass vacuously");
    let differing = pixels.as_chunks::<4>().0.iter().zip(reference.as_chunks::<4>().0.iter()).filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 8)).count();
    assert!(differing < drawn / 100, "{differing} of {drawn} drawn pixels differ from the player's own render — the atlas was not lent, or was lent in the wrong colour space");
}

/// Playback state makes the round trip from the render world back to the main
/// world.
///
/// The timeline advances in the render world; its result reaches gameplay only
/// through the shared feedback buffer and the `PreUpdate` system that drains it.
/// If any link in that chain is missing, `SsPlaybackState::frame_no` never moves
/// off its default of zero however long the app runs. Pumping frames and finding
/// it advanced is the proof the whole path is connected.
#[test]
fn playback_state_crosses_back_to_the_main_world() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, _captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    let character = masked_character(app.world());
    let player = app.world_mut().spawn(character).id();

    app.finish();
    app.cleanup();
    pump(&mut app);

    let state = app.world().entity(player).get::<SsPlaybackState>().expect("SsPlaybackState is a required component of SsPlayer and must be present");
    assert!(state.frame_no > 0.0, "playback state did not cross back: frame_no is still {} after {FRAMES} frames — the render->main feedback never arrived", state.frame_no);
    assert!(state.playing_forward, "a character on the default heading reported it was not playing forward");
}

/// The heading set on `SsPlayback` reaches the player, and the way it is
/// travelling comes back through `SsPlaybackState` — what a host gates its
/// audio on.
#[test]
fn a_backward_character_reports_it_is_not_playing_forward() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, _captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    let (character, transform, playback) = masked_character(app.world());
    let player = app.world_mut().spawn((character, transform, SsPlayback { direction: PlaybackDirection::Backward, ..playback })).id();

    app.finish();
    app.cleanup();
    pump(&mut app);

    let state = app.world().entity(player).get::<SsPlaybackState>().expect("SsPlaybackState is a required component of SsPlayer and must be present");
    assert!(!state.playing_forward, "a character set to play backward reported it was playing forward");
}

/// A section set on `SsPlayback` reaches the player: with the playhead stopped,
/// it sits on the section's first frame. The mask fixture's animation runs
/// frames 0..=10.
#[test]
fn a_section_puts_the_playhead_at_its_start() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, _captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    let (character, transform, playback) = masked_character(app.world());
    let player = app.world_mut().spawn((character, transform, SsPlayback { speed_scale: 0.0, animation_section: Some((6, 9)), ..playback })).id();

    app.finish();
    app.cleanup();
    pump(&mut app);

    let state = app.world().entity(player).get::<SsPlaybackState>().expect("SsPlaybackState is a required component of SsPlayer and must be present");
    assert_eq!(state.frame_no, 6.0, "a stopped playhead should sit on the section's first frame");
}

/// A seek written before the pack has loaded is kept, and lands once the
/// animation is set up — after its start, which would otherwise rewind it.
#[test]
fn a_seek_sent_before_the_pack_loads_lands_once_it_has() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, _captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    let (character, transform, playback) = masked_character(app.world());
    let player = app.world_mut().spawn((character, transform, SsPlayback { speed_scale: 0.0, ..playback })).id();
    app.world_mut().write_message(SsSeek { entity: player, frame: 5.0 });

    app.finish();
    app.cleanup();
    pump(&mut app);

    let state = app.world().entity(player).get::<SsPlaybackState>().expect("SsPlaybackState is a required component of SsPlayer and must be present");
    assert_eq!(state.frame_no, 5.0, "the seek did not land");
}

/// Track commands written before the pack has loaded are kept and applied, in
/// order, once the animation is set up, and the stack comes back through
/// `SsTrackState` — including the handle of a source laid with `AddSource`,
/// which is what the `Source*` commands that follow need.
#[test]
fn track_commands_reach_the_stack_and_it_crosses_back() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, _captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    let character = masked_character(app.world());
    let player = app.world_mut().spawn(character).id();
    for command in [TrackCommand::SetSync { track: 1, sync: TrackSync::Start }, TrackCommand::AddSource { track: 0, animation: MASK_ANIM.into(), weight: 0.5 }] {
        app.world_mut().write_message(SsTrackCommand { entity: player, command });
    }

    app.finish();
    app.cleanup();
    pump(&mut app);

    let stack = app.world().entity(player).get::<SsTrackState>().expect("SsTrackState is a required component of SsPlayer and must be present").clone();
    assert!(stack.tracks.len() >= 2, "SetSync did not open track 1: {stack:?}");
    assert_eq!(stack.tracks[1].sync, TrackSync::Start);
    assert!(stack.tracks[0].main_source.is_some(), "track 0 reports no main source");
    let [added] = stack.sources.as_slice() else { panic!("expected the one added source back, got {:?}", stack.sources) };
    assert_eq!((added.track, added.animation.as_str(), added.weight), (0, MASK_ANIM, 0.5));
    assert_ne!(Some(added.source), stack.tracks[0].main_source, "the added source is not the main one");

    app.world_mut().write_message(SsTrackCommand { entity: player, command: TrackCommand::SetSourceWeight { source: added.source, weight: 0.25 } });
    for _ in 0..3 {
        app.update();
    }
    let stack = app.world().entity(player).get::<SsTrackState>().unwrap();
    assert_eq!(stack.sources[0].weight, 0.25, "the weight sent by handle did not land");
}

/// `SsPlay` with a start frame lands there, like `play_from`, even when sent
/// before the pack has loaded.
#[test]
fn a_play_from_a_frame_starts_there() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, _captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    let (character, transform, playback) = masked_character(app.world());
    let player = app.world_mut().spawn((character, transform, SsPlayback { speed_scale: 0.0, ..playback })).id();
    app.world_mut().write_message(SsPlay { entity: player, start_frame: Some(7.0) });

    app.finish();
    app.cleanup();
    pump(&mut app);

    let state = app.world().entity(player).get::<SsPlaybackState>().expect("SsPlaybackState is a required component of SsPlayer and must be present");
    assert_eq!(state.frame_no, 7.0, "the play did not start from frame 7");
}

/// `SsStop` holds the playhead where it stands, and — unlike a pause — turning
/// `playing` off and on again does not start it: resuming only carries on what
/// is running.
#[test]
fn a_stop_holds_the_frame_and_playing_does_not_resume_it() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, _captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    let character = masked_character(app.world());
    let player = app.world_mut().spawn(character).id();

    app.finish();
    app.cleanup();
    pump(&mut app);

    app.world_mut().write_message(SsStop { entity: player });
    for _ in 0..FRAMES {
        app.update();
    }
    let frame_no = |app: &App| app.world().entity(player).get::<SsPlaybackState>().expect("SsPlaybackState is a required component of SsPlayer and must be present").frame_no;
    let stopped_at = frame_no(&app);

    app.world_mut().entity_mut(player).get_mut::<SsPlayback>().expect("SsPlayback is required").playing = false;
    app.update();
    app.world_mut().entity_mut(player).get_mut::<SsPlayback>().expect("SsPlayback is required").playing = true;
    for _ in 0..FRAMES {
        app.update();
    }
    assert_eq!(frame_no(&app), stopped_at, "a stopped animation moved: the stop did not hold, or `playing` resumed it");
}

/// A multisampled view is skipped, not a hard crash.
///
/// The player's pipelines are single-sampled, so drawing into a multisampled
/// target is a wgpu validation panic — and Bevy's default camera has MSAA on.
/// The adapter has to notice and skip. Without that guard this test panics
/// instead of asserting; with it, the character is simply absent.
#[test]
fn a_multisampled_view_is_skipped_not_a_panic() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    // Msaa::Sample4 rather than the usual Msaa::Off: this is the misconfiguration.
    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Sample4));
    let character = masked_character(app.world());
    app.world_mut().spawn(character);

    app.finish();
    app.cleanup();
    pump(&mut app);
    let pixels = captured.0.lock().unwrap().clone().expect("the readback never arrived — a multisampled view likely panicked instead of being skipped");
    assert_eq!(drawn_pixels(&pixels), 0, "a multisampled view drew the character; it should have been skipped");
}

/// Drawing survives the bound camera being replaced.
///
/// The scene binds to one view; that binding must not latch across frames, or an
/// app that despawns its camera and spawns another (a scene transition) leaves
/// every later frame refused as a "second view" and nothing draws again. Asserted
/// on drawn pixels rather than `frame_no` so it does not depend on wall-clock
/// deltas — under the bug the character is simply absent after the swap.
#[test]
fn drawing_survives_the_bound_camera_being_replaced() {
    if !fixtures_present() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some((mut app, handle, captured)) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    let camera_a = app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.clone().into()), Msaa::Off)).id();
    let character = masked_character(app.world());
    app.world_mut().spawn(character);

    app.finish();
    app.cleanup();
    pump(&mut app);

    // Swap the camera: despawn the bound one, spawn another onto the same target.
    app.world_mut().entity_mut(camera_a).despawn();
    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    for _ in 0..FRAMES {
        app.update();
    }

    let pixels = captured.0.lock().unwrap().clone().expect("the readback never arrived after the camera swap");
    dump("05-after-camera-swap", &pixels);
    assert!(drawn_pixels(&pixels) > 0, "the character stopped drawing after the bound camera was replaced — the view binding latched to the despawned camera");
}

/// The player crate rendering the same thing, for the comparisons above.
mod reference {
    use ssplayer_wgpu::{MaskBackend, Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};
    use std::path::Path;

    /// Render `pack` with masking switched off, at `size`, matching the Bevy
    /// camera's 1:1 pixel projection. Unless `playing`, it holds the first frame.
    pub fn render_unmasked(size: u32, pack: &str, anim: &str, playing: bool) -> Vec<u8> {
        let path = super::fixture(pack);
        let dir = Path::new(&path).parent().unwrap().to_path_buf();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

        let mut renderer = SSRenderer::new(&device);
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("png") {
                continue;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let img = image::open(&p).unwrap().to_rgba8();
            let (w, h) = img.dimensions();
            renderer.upload_texture(&device, &queue, ssruntime::fnv1a_hash_str(stem), &img, (w, h), Retention::Retain);
        }

        let mut scene = SSScene::new(&device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
        let mut player = SSPlayer::new();
        player.load(&path).unwrap();
        player.set_animation(anim).unwrap();
        player.set_transform(0.0, 0.0, 0.5, 0.5, 0.0);
        if playing {
            player.play();
        }
        scene.set_viewport(&queue, size as f32, size as f32, false);
        for _ in 0..super::FRAMES {
            scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
        }

        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let bpr = (size * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (bpr * size) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            scene.draw(&renderer, &mut pass);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(size) } },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        rx.recv().unwrap().unwrap();
        let padded = readback.slice(..).get_mapped_range().unwrap().to_vec();
        readback.unmap();

        let mut tight = Vec::with_capacity((size * size * 4) as usize);
        for row in 0..size {
            let start = (row * bpr) as usize;
            tight.extend_from_slice(&padded[start..start + (size * 4) as usize]);
        }
        tight
    }
}
