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
use bevy::asset::{Assets, Handle, RenderAssetUsages};
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
use ssplayer_wgpu_bevy::{SsPlayback, SsPlaybackState, SsPlayer, SsPlayerPlugin, SsTransform};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SIZE: u32 = 384;
/// The mask fixture `deploy-examples` generates. Everything here needs a mask —
/// an unmasked character would pass whether or not the coverage bake ran.
const MASK_PACK: &str = "assets/overall/Mask.ssab";
const MASK_ANIM: &str = "anime_1";
/// Frames to pump. Enough for the asset load, the first coverage bake and a
/// readback to make it back to the CPU.
const FRAMES: u32 = 12;

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
            .set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() }),
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
        // In 0.19 the target is its own component beside `Camera`, not a field on it.
        RenderTarget::Image(handle.into()),
        // The player's pipelines are single-sampled.
        Msaa::Off,
    ));
    setup(app.world_mut());

    // `run()` would do these; driving `update()` by hand does not, and without
    // them `RenderPlugin::finish` never unpacks the device into either world.
    app.finish();
    app.cleanup();

    for _ in 0..FRAMES {
        app.update();
    }
    let pixels = captured.0.lock().unwrap().clone();
    pixels.expect("the readback never arrived — the render app did not produce a frame")
}

fn drawn_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4).filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

/// Write a readback out, so what these assert about can be looked at:
/// `SS_BEVY_DUMP=<dir> cargo test -p ssplayer-wgpu-bevy --test bevy_render`.
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
        world.spawn((SsPlayer { src: fixture(MASK_PACK), animation: MASK_ANIM.into() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback::default()));
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
        world.spawn((SsPlayer { src: fixture(MASK_PACK), animation: MASK_ANIM.into() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback::default()));
    });

    // The same animation drawn by the player crate with masking off, as the
    // "unclipped" reference. Same pack, same frame count, same projection.
    let unmasked = crate::reference::render_unmasked(SIZE, MASK_PACK, MASK_ANIM);

    dump("02-masked", &masked);
    dump("02-unmasked-reference", &unmasked);
    let differing = masked.chunks_exact(4).zip(unmasked.chunks_exact(4)).filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 8)).count();
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
        world.spawn((SsPlayer { src: fixture(MASK_PACK), animation: MASK_ANIM.into() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback::default()));
    });

    dump("03-over-a-bevy-sprite", &pixels);
    let pure_green = pixels.chunks_exact(4).filter(|p| p[1] > 200 && p[0] < 60 && p[2] < 60).count();
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
    app.world_mut().spawn((SsPlayer { src: fixture(MASK_PACK), animation: MASK_ANIM.into() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback::default()));

    app.finish();
    app.cleanup();
    for _ in 0..FRAMES {
        app.update();
    }
    let pixels = captured.0.lock().unwrap().clone().expect("the readback never arrived — the render app did not produce a frame");

    dump("04-hdr-masked-character", &pixels);
    assert!(drawn_pixels(&pixels) > 0, "nothing drawn in an HDR (Rgba16Float) view — the format-swap path in draw_players may not be rebuilding the scene");
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
    let player = app.world_mut().spawn((SsPlayer { src: fixture(MASK_PACK), animation: MASK_ANIM.into() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback::default())).id();

    app.finish();
    app.cleanup();
    for _ in 0..FRAMES {
        app.update();
    }

    let state = app.world().entity(player).get::<SsPlaybackState>().expect("SsPlaybackState is a required component of SsPlayer and must be present");
    assert!(state.frame_no > 0.0, "playback state did not cross back: frame_no is still {} after {FRAMES} frames — the render->main feedback never arrived", state.frame_no);
}

/// The player crate rendering the same thing, for the comparisons above.
mod reference {
    use ssplayer_wgpu::{MaskBackend, Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};
    use std::path::Path;

    /// Render `pack` with masking switched off, at `size`, matching the Bevy
    /// camera's 1:1 pixel projection.
    pub fn render_unmasked(size: u32, pack: &str, anim: &str) -> Vec<u8> {
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
        player.play();
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
        let padded = readback.slice(..).get_mapped_range().to_vec();
        readback.unmap();

        let mut tight = Vec::with_capacity((size * size * 4) as usize);
        for row in 0..size {
            let start = (row * bpr) as usize;
            tight.extend_from_slice(&padded[start..start + (size * 4) as usize]);
        }
        tight
    }
}
