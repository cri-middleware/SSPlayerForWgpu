//! Runtime texture replacement on `SSPlayer`.
//!
//! None of this needs a GPU: an override is a redirect of the player's own
//! atlas-key table, which every draw resolves through and which `texture_key`
//! reads back. The pixels reach the renderer at the next `update`, but *where a
//! cellmap resolves* is decided on the CPU the moment the call is made — that is
//! what these pin.
//!
//! What is worth pinning here, beyond "the call forwards":
//!
//! * the redirect actually moves where a cellmap resolves, and a restore moves it
//!   back to the untouched original — no reload involved;
//! * the override is **per player**, because the table is: two players of one
//!   pack do not share it;
//! * it survives a tree rebuild (`set_animation`), because it lives in the
//!   player's table rather than the tree;
//! * enumeration lists **both** name spaces — cellmaps and TextureChange targets.

use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSRuntimeError, SSScene, SceneConfig};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

/// fnv1a-32 over a name, the construction the runtime hashes a cellmap name with
/// (`ssruntime::fnv1a_hash_str`) — so the value matches the slot a draw batch and
/// `texture_key` speak in. Copied rather than imported to keep this an
/// `ssplayer_wgpu`-only integration test.
fn fnv1a(name: &str) -> u32 {
    let mut hash: u32 = 2166136261;
    for b in name.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

/// A player with `Basic.ssab` loaded and its first animation selected. Missing
/// atlases do not matter here — the table records a pack's cellmap slots when the
/// load walk *reaches* them, before it reads a single pixel.
fn loaded() -> SSPlayer {
    let mut player = SSPlayer::new();
    player.load(format!("{}/overall/Basic.ssab", asset_root())).unwrap();
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();
    player
}

/// The first replaceable name the loaded pack offers, so a test never hard-codes
/// a cellmap out of someone else's asset.
fn a_cellmap(player: &SSPlayer) -> String {
    player.replaceable_cellmap_names(None).and_then(|names| names.into_iter().next()).expect("Basic.ssab names at least one cellmap")
}

#[test]
fn an_override_redirects_the_cellmap_and_a_restore_undoes_it() {
    let mut player = loaded();
    let name = a_cellmap(&player);
    let hash = fnv1a(&name);
    let original = player.texture_key(None, hash);

    assert!(!player.is_cellmap_overridden(None, &name), "a fresh player reports no override");

    player.set_cellmap_texture(None, &name, &[255u8; 2 * 2 * 4], 2, 2).unwrap();
    assert!(player.is_cellmap_overridden(None, &name));
    let redirected = player.texture_key(None, hash);
    assert_ne!(redirected, original, "the override did not move where the cellmap resolves");

    let freed = player.restore_cellmap_texture(None, &name).expect("restore reports the retired key");
    assert_eq!(freed, redirected, "restore retires the key the override installed");
    assert_eq!(player.texture_key(None, hash), original, "restore returns the cellmap to its untouched atlas");
    assert!(!player.is_cellmap_overridden(None, &name));

    assert!(player.restore_cellmap_texture(None, &name).is_none(), "restoring an un-overridden cellmap is a no-op");
}

#[test]
fn overrides_are_per_player() {
    let mut a = loaded();
    let mut b = loaded();
    let name = a_cellmap(&a);
    let hash = fnv1a(&name);
    let b_before = b.texture_key(None, hash);

    a.set_cellmap_texture(None, &name, &[0u8; 4], 1, 1).unwrap();

    assert!(a.is_cellmap_overridden(None, &name));
    assert!(!b.is_cellmap_overridden(None, &name), "one player's override reached another");
    assert_eq!(b.texture_key(None, hash), b_before, "the other player's cellmap moved");

    // And two players overriding the same cellmap take distinct private keys, so
    // their replacements do not collide in the shared renderer map.
    let key_a = a.override_cellmap_texture_key(None, &name).unwrap();
    let key_b = b.override_cellmap_texture_key(None, &name).unwrap();
    assert_ne!(key_a, key_b, "two players share one override key");
}

#[test]
fn re_overriding_reuses_the_key_and_a_rebuild_keeps_it() {
    let mut player = loaded();
    let name = a_cellmap(&player);
    let hash = fnv1a(&name);

    let key = player.override_cellmap_texture_key(None, &name).unwrap();
    assert_eq!(player.override_cellmap_texture_key(None, &name).unwrap(), key, "re-overriding one cellmap allocated a second key");

    // A tree rebuild reads the table, not the other way round, so the override
    // rides through selecting another animation.
    if let Some(other) = player.animation_names().unwrap().into_iter().find(|n| Some(n.as_str()) != player.current_animation()) {
        player.set_animation(&other).unwrap();
        assert!(player.is_cellmap_overridden(None, &name), "the override was lost across set_animation");
        assert_eq!(player.texture_key(None, hash), key, "the redirect did not survive the tree rebuild");
    }
}

#[test]
fn bad_input_is_rejected_before_it_can_lie() {
    let mut player = loaded();
    let name = a_cellmap(&player);

    // Wrong byte count for the stated size: caught before anything is queued.
    assert_eq!(player.set_cellmap_texture(None, &name, &[0u8; 3], 2, 2), Err(SSRuntimeError::InvalidParameter));
    assert!(!player.is_cellmap_overridden(None, &name), "a rejected call still left an override behind");

    // No pack to attach an override to.
    let mut empty = SSPlayer::new();
    assert_eq!(empty.set_cellmap_texture(None, "whatever", &[0u8; 4], 1, 1), Err(SSRuntimeError::NotLoaded));
    assert!(empty.replaceable_cellmap_names(None).is_none());
}

#[test]
fn enumeration_lists_cellmaps_and_texture_change_targets() {
    let mut player = SSPlayer::new();
    let Ok(()) = player.load(format!("{}/overall/TextureChange.ssab", asset_root())) else {
        eprintln!("TextureChange fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let names = player.replaceable_cellmap_names(None).expect("a loaded pack names its cellmaps");

    // The cellmap itself, under its bare name...
    assert!(names.iter().any(|n| n == "common"), "the cellmap name is missing from {names:?}");
    // ...and a TextureChange target the cellmaps never mention, under its file
    // name. Missing this second table leaves those parts silently undrawn.
    assert!(names.iter().any(|n| n == "box_00_00.png"), "an external TextureChange target is missing from {names:?}");
}

// --- The override on the GPU -----------------------------------
//
// The CPU tests above prove the redirect moves where a cellmap resolves. This one
// proves the rest of the round trip: that the flush uploads the replacement under
// the private key, that the draw binds it, and that a restore returns the drawn
// pixels to the original — the parts of the feature only a readback can see. It
// skips itself when no GPU adapter is present, as the other render tests do
// (`render.rs`), so a machine without one verifies nothing here rather than
// failing.

const W: u32 = 128;
const H: u32 = 128;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// Advance one frame and read the scene back as RGBA8.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut SSRenderer, scene: &mut SSScene, player: &mut SSPlayer) -> Vec<u8> {
    scene.update(renderer, device, queue, 1.0 / 60.0, &mut [player]);

    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (W * H * 4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
        scene.draw(renderer, &mut rpass);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(H) } },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    rx.recv().unwrap().unwrap();
    let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    data
}

fn drawn_pixels(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>().0.iter().filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

/// Mean absolute channel difference, in 0..255 units.
fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    sum as f64 / a.len() as f64
}

#[test]
fn the_override_reaches_the_drawn_pixels_and_a_restore_undoes_it() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping the render half of texture replacement.");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: FORMAT, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(&queue, W as f32, H as f32, false);

    let mut player = loaded();
    let name = a_cellmap(&player);

    // Freeze the timeline so the *only* thing that changes between renders is the
    // texture: play a few frames off the head, then pause.
    player.play();
    render(&device, &queue, &mut renderer, &mut scene, &mut player);
    render(&device, &queue, &mut renderer, &mut scene, &mut player);
    player.pause();

    let baseline = render(&device, &queue, &mut renderer, &mut scene, &mut player);
    assert!(drawn_pixels(&baseline) > 0, "the fixture drew nothing to override");

    // Opaque magenta, uniform, so it reads the same however the original UVs
    // normalise against this texture's size.
    let magenta = [255u8, 0, 255, 255].repeat((64 * 64) as usize);
    player.set_cellmap_texture(None, &name, &magenta, 64, 64).unwrap();
    let overridden = render(&device, &queue, &mut renderer, &mut scene, &mut player);
    assert!(mean_abs_diff(&baseline, &overridden) > 2.0, "the override did not change the drawn pixels (diff {})", mean_abs_diff(&baseline, &overridden));

    player.restore_cellmap_texture(None, &name);
    let restored = render(&device, &queue, &mut renderer, &mut scene, &mut player);
    assert!(mean_abs_diff(&baseline, &restored) < 0.5, "restore did not return the drawn pixels to the original (diff {})", mean_abs_diff(&baseline, &restored));
}

/// A texture the device cannot hold is left out, not fatal.
///
/// wgpu reports a texture past `max_texture_dimension_2d`, or pixels that do not
/// fill it, as a validation error, and its default handler panics. The C and
/// JavaScript faces own their device, so an oversized atlas would take the host
/// process with it. The renderer refuses such a texture where it would create it:
/// the key keeps whatever it held, and the frame still draws.
#[test]
fn a_texture_the_device_cannot_hold_is_left_out_and_the_frame_still_draws() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping the oversized-texture check.");
        return;
    };
    let max = device.limits().max_texture_dimension_2d;
    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: FORMAT, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(&queue, W as f32, H as f32, false);

    // Straight through the renderer: one row past the limit, then pixels that
    // fall short of the size they claim.
    let key = fnv1a("too-large");
    let row = [255u8; 4].repeat(max as usize + 1);
    renderer.upload_texture(&device, &queue, key, &row, (max + 1, 1), ssplayer_wgpu::Retention::Retain);
    assert_eq!(renderer.texture_size(key), None, "a texture past the device's limit was registered");
    renderer.upload_texture(&device, &queue, key, &[255u8; 4], (2, 2), ssplayer_wgpu::Retention::Retain);
    assert_eq!(renderer.texture_size(key), None, "4 bytes were registered as a 2x2 texture");

    // A good texture, then a bad replacement for the same key: the good one stays.
    renderer.upload_texture(&device, &queue, key, &[255u8; 16], (2, 2), ssplayer_wgpu::Retention::Retain);
    renderer.upload_texture(&device, &queue, key, &row, (max + 1, 1), ssplayer_wgpu::Retention::Retain);
    assert_eq!(renderer.texture_size(key), Some((2.0, 2.0)), "a refused replacement dropped the texture it would have replaced");

    // Through a player: the override is queued by the call and uploaded by the
    // update, which has no caller to refuse it to.
    let mut player = loaded();
    let name = a_cellmap(&player);
    player.play();
    player.set_cellmap_texture(None, &name, &row, max + 1, 1).unwrap();
    let frame = render(&device, &queue, &mut renderer, &mut scene, &mut player);
    assert!(drawn_pixels(&frame) > 0, "the frame after a refused override drew nothing");
}
