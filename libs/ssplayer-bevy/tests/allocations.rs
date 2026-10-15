//! A steady frame allocates nothing of this crate's own.
//!
//! The player crate's `tests/allocations.rs` holds the walk to that; this holds
//! what runs around it for every character on every frame — the extract, the
//! sync onto the player, the draw, and the feedback that crosses back to the
//! main world.
//!
//! Bevy allocates on every frame, on every thread of its executor, so a count
//! cannot tell the two apart. Each allocation made while a measured frame runs,
//! on any thread, is attributed instead, by its backtrace: to this crate when
//! the innermost frame that names a known crate is this crate's. Only those fail
//! the test. The player's and the runtime's are the other test's to hold, and
//! here one of their buffers can still grow: it is sized by the frame's delta,
//! which in Bevy is wall-clock time, and taking backtraces slows the frames.
//!
//! Skips itself when no GPU adapter is available, or the fixtures were not
//! generated, like `bevy_render.rs`.

use bevy::app::{App, PluginGroup};
use bevy::asset::{AssetPlugin, AssetServer, Assets, RecursiveDependencyLoadState, RenderAssetUsages};
use bevy::camera::{Camera, Camera2d, ClearColorConfig, RenderTarget};
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::image::{Image, ImageLoaderSettings};
use bevy::platform::collections::HashMap;
use bevy::prelude::default;
use bevy::render::RenderPlugin;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::Msaa;
use bevy::window::WindowPlugin;
use ssplayer_bevy::{CrowdMember, SsCellmapTextures, SsCrowd, SsPlayback, SsPlayer, SsPlayerPlugin, SsSceneSettings, SsTrackCommand, SsTransform, TrackCommand, TrackSync};
use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

const SIZE: u32 = 256;
const MASK_PACK: &str = "assets/overall/Mask.ssab";
const MASK_ANIM: &str = "anime_1";
/// Its atlas is a file beside it, which the override below reuses as is.
const RINGO_PACK: &str = "assets/Ringo/Ringo.ssab";
const RINGO_ATLAS: &str = "assets/Ringo/Ringo.png";
/// A ceiling on the frames a load may take, so a pack that never settles fails
/// the test instead of hanging it.
const LOAD_FRAMES: u32 = 600;
/// After the packs settle: long enough for the override's image to be prepared
/// and for every buffer here to have grown to what this cast needs.
const WARM_UP: u32 = 120;
const MEASURED: u32 = 8;

/// Set while a measured frame runs. Global rather than per thread, since Bevy
/// runs the systems on its executor's threads.
static ARMED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Set while this thread records an allocation, which allocates too.
    static RECORDING: Cell<bool> = const { Cell::new(false) };
}

/// Captured unresolved: resolving each one as it is taken would slow the
/// measured frames by orders of magnitude.
static TRACES: Mutex<Vec<Backtrace>> = Mutex::new(Vec::new());

fn note() {
    if !ARMED.load(Ordering::Relaxed) || RECORDING.try_with(|recording| recording.replace(true)).unwrap_or(true) {
        return;
    }
    let trace = Backtrace::force_capture();
    TRACES.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(trace);
    RECORDING.with(|recording| recording.set(false));
}

struct Attributing;

unsafe impl GlobalAlloc for Attributing {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Attributing = Attributing;

enum Owner {
    /// This crate's.
    Adapter,
    /// Bevy's, wgpu's, the player's or the runtime's.
    Other,
}

/// Whose allocation this is: the innermost frame that names a known crate
/// decides. `None` when no frame does — a driver's own thread, or a build
/// without symbols.
fn owner(trace: &str) -> Option<Owner> {
    for line in trace.lines().map(str::trim) {
        // Frames read `N: path::to::fn`; the `at file:line` lines between them do not.
        let Some((_, symbol)) = line.split_once(": ") else { continue };
        let symbol = symbol.trim_start_matches('<');
        if symbol.starts_with("ssplayer_bevy::") {
            return Some(Owner::Adapter);
        }
        if ["bevy", "wgpu", "naga::", "ssplayer_wgpu::", "ssruntime::"].iter().any(|krate| symbol.starts_with(krate)) {
            return Some(Owner::Other);
        }
    }
    None
}

fn repo_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../..")
}

/// A render app with no window, this crate's plugin, and a camera drawing into
/// an `Image`.
fn headless_app() -> Option<App> {
    // Bevy panics rather than returning when it cannot make a device, so ask wgpu
    // first and skip quietly if the machine has none.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;

    let mut app = App::new();
    app.add_plugins(
        bevy::DefaultPlugins
            .build()
            .disable::<bevy::winit::WinitPlugin>()
            .set(WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() })
            .set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() })
            .set(AssetPlugin { file_path: repo_root().into(), ..default() }),
    )
    .add_plugins(SsPlayerPlugin);

    let mut target = Image::new_fill(Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 }, TextureDimension::D2, &[0, 0, 0, 255], TextureFormat::Rgba8Unorm, RenderAssetUsages::default());
    target.texture_descriptor.usage |= TextureUsages::RENDER_ATTACHMENT;
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.world_mut().spawn((Camera2d, Camera { clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Image(handle.into()), Msaa::Off));
    Some(app)
}

/// Every kind of per-character state this crate carries across a frame: a track
/// stack of two with an added source, a texture override, a crowd, a character
/// faded through a group-alpha layer, and draw orders the scene has to sort.
fn spawn_cast(world: &mut World) {
    let server = world.resource::<AssetServer>().clone();
    world.resource_mut::<SsSceneSettings>().group_alpha = true;

    let crowd = SsCrowd { members: vec![CrowdMember { x: -30.0, ..default() }, CrowdMember { x: 30.0, alpha: 0.5, tint: 0x80_C0_FF, ..default() }] };
    let masked = world.spawn((SsPlayer { pack: server.load(MASK_PACK), animation: MASK_ANIM.into(), ..default() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback { order: 1, ..default() }, crowd)).id();
    for command in [TrackCommand::SetSync { track: 1, sync: TrackSync::Start }, TrackCommand::AddSource { track: 0, animation: MASK_ANIM.into(), weight: 0.5 }] {
        world.write_message(SsTrackCommand { entity: masked, command });
    }

    // The names to give the components, read from the pack the way a host would.
    let mut ringo = ssplayer_wgpu::SSPlayer::new();
    ringo.load(format!("{}/{RINGO_PACK}", repo_root())).expect("Ringo.ssab did not load");
    let animation = ringo.animation_names().and_then(|names| names.first().cloned()).expect("Ringo.ssab has no animation");
    let cellmap = ringo.cellmap_names().and_then(|names| names.first().cloned()).expect("Ringo.ssab has no cellmap");
    let atlas = server.load_builder().with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false).load(RINGO_ATLAS);
    world.spawn((
        SsPlayer { pack: server.load(RINGO_PACK), animation, ..default() },
        SsTransform { x: 40.0, scale_x: 0.25, scale_y: 0.25, ..default() },
        SsPlayback { order: -1, alpha: 0.5, ..default() },
        SsCellmapTextures { overrides: HashMap::from_iter([(cellmap, atlas)]) },
    ));
}

#[test]
fn a_steady_frame_allocates_nothing_of_the_adapters_own() {
    if !Path::new(&format!("{}/{MASK_PACK}", repo_root())).exists() {
        eprintln!("assets not generated, skipping");
        return;
    }
    let Some(mut app) = headless_app() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    spawn_cast(app.world_mut());
    // `run()` would do these; driving `update()` by hand does not.
    app.finish();
    app.cleanup();

    for _ in 0..LOAD_FRAMES {
        let world = app.world_mut();
        let server = world.resource::<AssetServer>().clone();
        if world.query::<&SsPlayer>().iter(world).all(|player| matches!(server.get_recursive_dependency_load_state(&player.pack), Some(RecursiveDependencyLoadState::Loaded))) {
            break;
        }
        app.update();
    }
    for _ in 0..WARM_UP {
        app.update();
    }

    ARMED.store(true, Ordering::Relaxed);
    for _ in 0..MEASURED {
        app.update();
    }
    ARMED.store(false, Ordering::Relaxed);

    let traces = std::mem::take(&mut *TRACES.lock().unwrap());
    let (mut adapter, mut other) = (Vec::new(), 0);
    for trace in traces.iter().map(Backtrace::to_string) {
        match owner(&trace) {
            Some(Owner::Adapter) => adapter.push(trace),
            Some(Owner::Other) => other += 1,
            None => {}
        }
    }
    // Without this the attribution could be classifying nothing: Bevy allocates
    // on every frame, so a build with symbols must have seen it to.
    assert!(other > 0, "no allocation over {MEASURED} frames was attributed to Bevy or wgpu — does this build carry symbols? {} were taken", traces.len());
    assert!(adapter.is_empty(), "{} allocation(s) over {MEASURED} steady-state frames came from ssplayer-bevy. The first:\n{}", adapter.len(), adapter[0]);
}
