//! Several players sharing one render target (SPEC §13, phases P7-1 / P7-2).
//!
//! The load-bearing test here is [`stencil_unwind_isolates_players`]. §6 states
//! the invariant a player must honour: it leaves the stencil buffer exactly as
//! it found it. Two players stacked into one render pass must therefore render
//! identically to the same two players drawn into two passes, each with its own
//! stencil clear — the two-pass arrangement cannot leak coverage by
//! construction, so it is the reference the one-pass arrangement has to match.
//!
//! The rest cover the per-player placement API (`set_transform` / `set_visible`
//! / `set_alpha`), which is what makes putting several players on one target
//! useful in the first place.
//!
//! Every test skips cleanly when no GPU adapter is available.

use ssplayer_wgpu::{SSPlayer, SSPlayerConfig};
use std::path::Path;

const W: u32 = 256;
const H: u32 = 256;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
}

/// A headless device, or `None` when the machine has no adapter (CI without a
/// GPU) — every test returns early in that case rather than failing.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// Decode every PNG in the asset dir and upload it keyed by fnv1a(stem), like
/// the viewer's reference-mode texture supply.
fn load_textures(player: &mut SSPlayer, device: &wgpu::Device, queue: &wgpu::Queue, dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("png") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let hash = ssruntime::fnv1a_hash_str(stem);
        let img = image::open(&path).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        player.upload_texture(device, queue, hash, &img, w, h);
    }
}

/// A reference-mode player playing `pack`'s first animation, positioned for a
/// W x H target.
fn make_player(device: &wgpu::Device, queue: &wgpu::Queue, pack: &str, stencil: bool) -> SSPlayer {
    let mut player = SSPlayer::new(device, &SSPlayerConfig { color_format: COLOR_FORMAT, stencil_format: stencil.then_some(STENCIL_FORMAT) });
    load_textures(&mut player, device, queue, Path::new(asset_dir()));

    let bytes = std::fs::read(format!("{}/{pack}.ssab", asset_dir())).unwrap();
    player.load_ssab(pack, &bytes).unwrap();
    let anim = player.animation_names().unwrap()[0].clone();
    player.set_animation(&anim).unwrap();
    player.play();
    player.set_viewport(queue, W as f32, H as f32, false);
    player
}

fn advance(player: &mut SSPlayer, device: &wgpu::Device, queue: &wgpu::Queue, frames: u32) {
    for _ in 0..frames {
        player.update(device, queue, 1.0 / 60.0);
    }
}

/// Rebuild the frame's geometry without moving the timeline, so two renders can
/// be compared pixel for pixel with only the player property under test
/// differing between them.
fn rebuild(player: &mut SSPlayer, device: &wgpu::Device, queue: &wgpu::Queue) {
    player.update(device, queue, 0.0);
}

fn offscreen_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView, wgpu::TextureView) {
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen color"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let stencil_view = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen stencil"),
            size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: STENCIL_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    (color, color_view, stencil_view)
}

fn depth_stencil_attachment(view: &wgpu::TextureView) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
        // Every pass starts from zero coverage. That is exactly the state each
        // player promises to hand back (§6).
        stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
    }
}

/// Read the colour target back as RGBA8.
fn readback(device: &wgpu::Device, queue: &wgpu::Queue, encoder: wgpu::CommandEncoder, color: &wgpu::Texture) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (W * H * 4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let mut encoder = encoder;
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
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
    let data = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    data
}

/// Draw every player into a **single** render pass, in the given order.
fn render_one_pass(device: &wgpu::Device, queue: &wgpu::Queue, players: &[&SSPlayer]) -> Vec<u8> {
    let (color, color_view, stencil_view) = offscreen_target(device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("one pass, N players"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(depth_stencil_attachment(&stencil_view)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for player in players {
            player.draw(&mut rpass);
        }
    }
    readback(device, queue, encoder, &color)
}

/// Draw each player into its **own** render pass, accumulating colour and
/// clearing the stencil every time. No player can see another's coverage here,
/// which is what makes this the reference.
fn render_pass_per_player(device: &wgpu::Device, queue: &wgpu::Queue, players: &[&SSPlayer]) -> Vec<u8> {
    let (color, color_view, stencil_view) = offscreen_target(device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    for (i, player) in players.iter().enumerate() {
        let load = if i == 0 { wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }) } else { wgpu::LoadOp::Load };
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("one pass per player"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &color_view, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store }, depth_slice: None })],
            depth_stencil_attachment: Some(depth_stencil_attachment(&stencil_view)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        player.draw(&mut rpass);
    }
    readback(device, queue, encoder, &color)
}

/// Count pixels whose RGB is not (near) black — i.e. something was drawn.
fn drawn_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4).filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

/// Total RGB energy, for comparing fades.
fn luminance_sum(rgba: &[u8]) -> u64 {
    rgba.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum()
}

/// SPEC §6: a player restores the stencil buffer before it returns, so stacking
/// two of them into one pass is identical to giving each its own pass.
///
/// Both players run `Mask.ssab` deliberately: the second one has to *test*
/// against the stencil for leaked coverage to be visible at all. A mask-free
/// second player would draw through `CompareFunction::Always` and pass this test
/// even with the bug present.
///
/// Before the unwind existed this failed: `render_node` pre-wrote every pure
/// mask but only removed the ones the scoped replay reached, so the first
/// player's leftover coverage inverted the second one's polarity tests.
#[test]
fn stencil_unwind_isolates_players() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };

    let mut a = make_player(&device, &queue, "Mask", true);
    let mut b = make_player(&device, &queue, "Mask", true);
    advance(&mut a, &device, &queue, 10);
    advance(&mut b, &device, &queue, 10);

    let one_pass = render_one_pass(&device, &queue, &[&a, &b]);
    let per_pass = render_pass_per_player(&device, &queue, &[&a, &b]);

    let drawn = drawn_pixels(&one_pass);
    eprintln!("two masked players: one_pass drew {drawn} px, reference drew {}", drawn_pixels(&per_pass));
    assert!(drawn > 0, "nothing was drawn — fixture or texture problem, the comparison below would be vacuous");

    let differing = one_pass.chunks_exact(4).zip(per_pass.chunks_exact(4)).filter(|(x, y)| x != y).count();
    assert_eq!(differing, 0, "{differing} px differ: stacking two players in one pass is not equivalent to one pass each — stencil coverage leaked between them (SPEC §6)");
}

/// The same invariant, order reversed. Coverage must not leak in either
/// direction, and a mask-free player must not be disturbed either.
#[test]
fn stencil_unwind_survives_mixed_players() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };

    let mut masked = make_player(&device, &queue, "Mask", true);
    let mut plain = make_player(&device, &queue, "Basic", true);
    advance(&mut masked, &device, &queue, 10);
    advance(&mut plain, &device, &queue, 10);

    for (label, order) in [("mask first", [&masked, &plain]), ("plain first", [&plain, &masked])] {
        let one_pass = render_one_pass(&device, &queue, &order);
        let per_pass = render_pass_per_player(&device, &queue, &order);
        assert_eq!(one_pass, per_pass, "{label}: one pass differs from one-pass-per-player — stencil coverage leaked");
    }
}

/// `set_transform` places a player in the scene (SPEC §5.4). The projection is
/// `2/width` about a centred origin, so one world unit is one pixel and a whole-
/// pixel translation must reproduce the original image shifted by exactly that
/// many columns — same geometry, same rasterisation, no resampling.
///
/// Both renders are taken at the same animation frame (`rebuild`, not
/// `advance`), so the only difference between them is the transform.
#[test]
fn set_transform_moves_the_player() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };

    const SHIFT: usize = 20;

    // Stencil on: the shared render helpers always attach one, and a pipeline
    // built without a depth-stencil state cannot be used in such a pass.
    let mut player = make_player(&device, &queue, "Basic", true);
    advance(&mut player, &device, &queue, 10);
    let at_origin = render_one_pass(&device, &queue, &[&player]);
    assert!(drawn_pixels(&at_origin) > 0, "nothing drawn at the origin — fixture problem");

    player.set_transform(SHIFT as f32, 0.0, 1.0, 1.0, 0.0);
    rebuild(&mut player, &device, &queue);
    let shifted = render_one_pass(&device, &queue, &[&player]);
    assert_ne!(at_origin, shifted, "set_transform did not change the output at all");

    let row = W as usize;
    let mut differing = 0usize;
    for y in 0..H as usize {
        for x in 0..row - SHIFT {
            let src = &at_origin[(y * row + x) * 4..][..4];
            let dst = &shifted[(y * row + x + SHIFT) * 4..][..4];
            if src != dst {
                differing += 1;
            }
        }
    }
    eprintln!("translation by {SHIFT} px: {differing} px mismatched out of {}", (row - SHIFT) * H as usize);
    assert_eq!(differing, 0, "the shifted render is not the original moved {SHIFT} px right");

    // And the placement is reversible.
    player.reset_transform();
    rebuild(&mut player, &device, &queue);
    assert_eq!(render_one_pass(&device, &queue, &[&player]), at_origin, "reset_transform did not restore the original placement");
}

/// `set_visible(false)` skips the player entirely — no timeline advance, no
/// geometry — and turning it back on resumes drawing.
#[test]
fn invisible_player_draws_nothing() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };

    // Stencil on: the shared render helpers always attach one, and a pipeline
    // built without a depth-stencil state cannot be used in such a pass.
    let mut player = make_player(&device, &queue, "Basic", true);
    advance(&mut player, &device, &queue, 10);
    assert!(drawn_pixels(&render_one_pass(&device, &queue, &[&player])) > 0, "nothing drawn while visible — fixture problem");

    player.set_visible(false);
    advance(&mut player, &device, &queue, 1);
    assert_eq!(drawn_pixels(&render_one_pass(&device, &queue, &[&player])), 0, "an invisible player still drew");

    player.set_visible(true);
    advance(&mut player, &device, &queue, 1);
    assert!(drawn_pixels(&render_one_pass(&device, &queue, &[&player])) > 0, "the player did not come back when made visible again");
}

/// `set_alpha` fades the whole player. The target is cleared to opaque black and
/// the blend table is premultiplied throughout, so alpha 0 leaves the backdrop
/// untouched under every blend mode and a half fade lands strictly in between.
#[test]
fn set_alpha_fades_the_player() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };

    // Stencil on: the shared render helpers always attach one, and a pipeline
    // built without a depth-stencil state cannot be used in such a pass.
    let mut player = make_player(&device, &queue, "Basic", true);
    advance(&mut player, &device, &queue, 10);
    let opaque = luminance_sum(&render_one_pass(&device, &queue, &[&player]));

    player.set_alpha(0.5);
    advance(&mut player, &device, &queue, 1);
    let half = luminance_sum(&render_one_pass(&device, &queue, &[&player]));

    player.set_alpha(0.0);
    advance(&mut player, &device, &queue, 1);
    let clear = render_one_pass(&device, &queue, &[&player]);

    eprintln!("luminance: alpha=1 {opaque}, alpha=0.5 {half}, alpha=0 {}", luminance_sum(&clear));
    assert!(opaque > 0, "nothing drawn at full alpha — fixture problem");
    assert_eq!(drawn_pixels(&clear), 0, "alpha 0 still drew");
    assert!(half < opaque && half > 0, "alpha 0.5 should sit strictly between (got {half}, full {opaque})");

    // Clamped, so an out-of-range value cannot brighten the player.
    player.set_alpha(4.0);
    assert_eq!(player.alpha(), 1.0, "set_alpha did not clamp");
}
