//! Several players sharing one render target and one renderer (SPEC §13, phases
//! P7-1 / P7-2 / P7-3).
//!
//! The load-bearing test here is [`stencil_unwind_isolates_players`]. §6 states
//! the invariant a player must honour: it leaves the stencil buffer exactly as
//! it found it. Two players stacked into one render pass must therefore render
//! identically to the same two players drawn into two passes, each with its own
//! stencil clear — the two-pass arrangement cannot leak coverage by
//! construction, so it is the reference the one-pass arrangement has to match.
//!
//! The rest cover the per-player placement API (`set_transform` / `set_visible`
//! / `set_culled` / `set_alpha`), which is what makes putting several players on one target
//! useful in the first place.
//!
//! Every test skips cleanly when no GPU adapter is available.

use ssplayer_wgpu::{MaskBackend, Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};
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
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// Decode every PNG in the asset dir and upload it keyed by fnv1a(stem), like
/// the viewer's reference-mode texture supply.
fn load_textures(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("png") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let hash = ssruntime::fnv1a_hash_str(stem);
        let img = image::open(&path).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        renderer.upload_texture(device, queue, hash, &img, (w, h), Retention::Retain);
    }
}

/// A scene sized for the W x H offscreen target.
fn make_scene(device: &wgpu::Device, queue: &wgpu::Queue, stencil: bool) -> SSScene {
    let mut scene = SSScene::new(device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: stencil.then_some(STENCIL_FORMAT), mask: if stencil { MaskBackend::Stencil } else { MaskBackend::None } });
    scene.set_viewport(queue, W as f32, H as f32, false);
    scene
}

/// A reference-mode player playing `pack`'s first animation. Holds no GPU
/// resource of its own (SPEC §1.2) — the atlas goes to the caller's shared
/// `renderer`, which is why several players cost one upload between them.
fn make_player(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut SSRenderer, pack: &str) -> SSPlayer {
    load_textures(renderer, device, queue, Path::new(asset_dir()));
    let mut player = SSPlayer::new();
    let bytes = std::fs::read(format!("{}/{pack}.ssab", asset_dir())).unwrap();
    player.load_ssab(pack, &bytes).unwrap();
    let anim = player.animation_names().unwrap()[0].clone();
    player.set_animation(&anim).unwrap();
    player.play();
    player
}

fn advance(scene: &mut SSScene, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, players: &mut [&mut SSPlayer], frames: u32) {
    for _ in 0..frames {
        scene.update(renderer, device, queue, 1.0 / 60.0, players);
    }
}

/// Rebuild the frame's geometry without moving the timeline, so two renders can
/// be compared pixel for pixel with only the property under test differing.
fn rebuild(scene: &mut SSScene, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, players: &mut [&mut SSPlayer]) {
    scene.update(renderer, device, queue, 0.0, players);
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
    let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    data
}

/// Draw one scene — that is, every player it was updated with — into a single
/// render pass.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &SSRenderer, scene: &SSScene) -> Vec<u8> {
    render_scenes(device, queue, renderer, &[scene])
}

/// Draw each scene into its **own** render pass, accumulating colour and
/// clearing the stencil every time. No scene can see another's coverage here,
/// which is what makes a per-scene split the reference a shared scene must match.
fn render_scenes(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &SSRenderer, scenes: &[&SSScene]) -> Vec<u8> {
    let (color, color_view, stencil_view) = offscreen_target(device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    for (i, scene) in scenes.iter().enumerate() {
        let load = if i == 0 { wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }) } else { wgpu::LoadOp::Load };
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &color_view, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store }, depth_slice: None })],
            depth_stencil_attachment: Some(depth_stencil_attachment(&stencil_view)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        scene.draw(renderer, &mut rpass);
    }
    readback(device, queue, encoder, &color)
}

/// Count pixels whose RGB is not (near) black — i.e. something was drawn.
fn drawn_pixels(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>().0.iter().filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

/// Total RGB energy, for comparing fades.
fn luminance_sum(rgba: &[u8]) -> u64 {
    rgba.as_chunks::<4>().0.iter().map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum()
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
/// SPEC §6: a player restores the stencil buffer before it returns, so putting
/// two of them in one scene is identical to giving each its own scene and pass.
///
/// Both players run `Mask.ssab` deliberately: the second one has to *test*
/// against the stencil for leaked coverage to be visible at all. A mask-free
/// second player would draw through `CompareFunction::Always` and pass this test
/// even with the bug present.
#[test]
fn stencil_unwind_isolates_players() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let mut renderer = SSRenderer::new(&device);

    // Subject: one scene holding both players.
    let mut shared = make_scene(&device, &queue, true);
    let mut a = make_player(&device, &queue, &mut renderer, "Mask");
    let mut b = make_player(&device, &queue, &mut renderer, "Mask");
    advance(&mut shared, &mut renderer, &device, &queue, &mut [&mut a, &mut b], 10);
    let one_scene = render(&device, &queue, &renderer, &shared);

    // Reference: a scene each, a pass each — coverage cannot leak by construction.
    let mut scene_a = make_scene(&device, &queue, true);
    let mut scene_b = make_scene(&device, &queue, true);
    let mut a2 = make_player(&device, &queue, &mut renderer, "Mask");
    let mut b2 = make_player(&device, &queue, &mut renderer, "Mask");
    advance(&mut scene_a, &mut renderer, &device, &queue, &mut [&mut a2], 10);
    advance(&mut scene_b, &mut renderer, &device, &queue, &mut [&mut b2], 10);
    let split = render_scenes(&device, &queue, &renderer, &[&scene_a, &scene_b]);

    let drawn = drawn_pixels(&one_scene);
    eprintln!("two masked players: shared scene drew {drawn} px, reference drew {}", drawn_pixels(&split));
    assert!(drawn > 0, "nothing was drawn — fixture or texture problem, the comparison below would be vacuous");

    let differing = one_scene.as_chunks::<4>().0.iter().zip(split.as_chunks::<4>().0.iter()).filter(|(x, y)| x != y).count();
    assert_eq!(differing, 0, "{differing} px differ: sharing a scene is not equivalent to one scene each — stencil coverage leaked between players (SPEC §6)");
}

/// The same invariant with a mask-free player in the mix, in both orders.
/// Coverage must not leak in either direction.
#[test]
fn stencil_unwind_survives_mixed_players() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };

    for (label, first_pack, second_pack) in [("mask first", "Mask", "Basic"), ("plain first", "Basic", "Mask")] {
        let mut renderer = SSRenderer::new(&device);

        let mut shared = make_scene(&device, &queue, true);
        let mut p1 = make_player(&device, &queue, &mut renderer, first_pack);
        let mut p2 = make_player(&device, &queue, &mut renderer, second_pack);
        advance(&mut shared, &mut renderer, &device, &queue, &mut [&mut p1, &mut p2], 10);
        let one_scene = render(&device, &queue, &renderer, &shared);

        let mut scene_1 = make_scene(&device, &queue, true);
        let mut scene_2 = make_scene(&device, &queue, true);
        let mut q1 = make_player(&device, &queue, &mut renderer, first_pack);
        let mut q2 = make_player(&device, &queue, &mut renderer, second_pack);
        advance(&mut scene_1, &mut renderer, &device, &queue, &mut [&mut q1], 10);
        advance(&mut scene_2, &mut renderer, &device, &queue, &mut [&mut q2], 10);
        let split = render_scenes(&device, &queue, &renderer, &[&scene_1, &scene_2]);

        assert_eq!(one_scene, split, "{label}: a shared scene differs from one scene each — stencil coverage leaked");
    }
}

/// SPEC §9.2: the scene draws its players in ascending `order`, so swapping the
/// two changes which one ends up on top. Both are opaque and overlapping here,
/// so the composite genuinely differs.
#[test]
fn order_decides_who_draws_on_top() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = make_scene(&device, &queue, true);

    let mut back = make_player(&device, &queue, &mut renderer, "Basic");
    let mut front = make_player(&device, &queue, &mut renderer, "Mask");
    back.set_order(0);
    front.set_order(1);
    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut back, &mut front], 10);
    let basic_behind = render(&device, &queue, &renderer, &scene);

    // Swap the order at the same animation frame; nothing else changes.
    back.set_order(1);
    front.set_order(0);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut back, &mut front]);
    let basic_in_front = render(&device, &queue, &renderer, &scene);

    assert!(drawn_pixels(&basic_behind) > 0, "nothing drawn — fixture problem");
    assert_ne!(basic_behind, basic_in_front, "swapping `order` did not change the composite — the scene is not sorting by it");

    // And the sort is by value, not by the order the slice happened to be in:
    // handing the same players over reversed must reproduce the first image.
    back.set_order(0);
    front.set_order(1);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut front, &mut back]);
    assert_eq!(render(&device, &queue, &renderer, &scene), basic_behind, "the scene drew in slice order rather than `order` order");
}

/// `set_transform` places a player in the scene (SPEC §5.4). The projection is
/// `2/width` about a centred origin, so one world unit is one pixel and a whole-
/// pixel translation must reproduce the original image shifted by exactly that
/// many columns — same geometry, same rasterisation, no resampling.
#[test]
fn set_transform_moves_the_player() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    const SHIFT: usize = 20;

    let mut renderer = SSRenderer::new(&device);
    let mut scene = make_scene(&device, &queue, true);
    let mut player = make_player(&device, &queue, &mut renderer, "Basic");
    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut player], 10);
    let at_origin = render(&device, &queue, &renderer, &scene);
    assert!(drawn_pixels(&at_origin) > 0, "nothing drawn at the origin — fixture problem");

    // Same animation frame, only the transform differs.
    player.set_transform(SHIFT as f32, 0.0, 1.0, 1.0, 0.0);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    let shifted = render(&device, &queue, &renderer, &scene);
    assert_ne!(at_origin, shifted, "set_transform did not change the output at all");

    let row = W as usize;
    let mut differing = 0usize;
    for y in 0..H as usize {
        for x in 0..row - SHIFT {
            if at_origin[(y * row + x) * 4..][..4] != shifted[(y * row + x + SHIFT) * 4..][..4] {
                differing += 1;
            }
        }
    }
    eprintln!("translation by {SHIFT} px: {differing} px mismatched out of {}", (row - SHIFT) * H as usize);
    assert_eq!(differing, 0, "the shifted render is not the original moved {SHIFT} px right");

    player.reset_transform();
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    assert_eq!(render(&device, &queue, &renderer, &scene), at_origin, "reset_transform did not restore the original placement");
}

/// Both `set_visible(false)` and `set_culled(true)` leave nothing on screen, and
/// undoing either resumes drawing.
#[test]
fn hidden_and_culled_players_draw_nothing() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = make_scene(&device, &queue, true);
    let mut player = make_player(&device, &queue, &mut renderer, "Basic");

    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut player], 10);
    assert!(drawn_pixels(&render(&device, &queue, &renderer, &scene)) > 0, "nothing drawn while visible — fixture problem");

    player.set_visible(false);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    assert_eq!(drawn_pixels(&render(&device, &queue, &renderer, &scene)), 0, "a hidden player still drew");

    player.set_visible(true);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    assert!(drawn_pixels(&render(&device, &queue, &renderer, &scene)) > 0, "the player did not come back when made visible again");

    player.set_culled(true);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    assert_eq!(drawn_pixels(&render(&device, &queue, &renderer, &scene)), 0, "a culled player still drew");

    player.set_culled(false);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    assert!(drawn_pixels(&render(&device, &queue, &renderer, &scene)) > 0, "the player did not come back when unculled");
}

/// What tells the two apart is the clock. A hidden player keeps time with a
/// visible one — playhead and part poses both, so a follower pinned to a hidden
/// character does not freeze — while a culled one stops where it was.
#[test]
fn a_hidden_player_keeps_time_and_a_culled_one_stops() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = make_scene(&device, &queue, true);
    let mut shown = make_player(&device, &queue, &mut renderer, "Basic");
    let mut hidden = make_player(&device, &queue, &mut renderer, "Basic");
    let mut culled = make_player(&device, &queue, &mut renderer, "Basic");
    hidden.set_visible(false);
    culled.set_culled(true);

    let start = culled.frame_no();
    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut shown, &mut hidden, &mut culled], 30);

    assert!(shown.frame_no() > start, "the visible player did not advance — fixture problem");
    assert_eq!(hidden.frame_no(), shown.frame_no(), "a hidden player fell behind a visible one");
    assert_eq!(culled.frame_no(), start, "a culled player advanced");

    let parts = shown.part_count() as u32;
    assert!(parts > 0, "Basic has no parts — fixture problem");
    for part in 0..parts {
        assert_eq!(hidden.part_transform_by_index(part), shown.part_transform_by_index(part), "part {part} of a hidden player is not where the visible one has it");
    }
}

/// SPEC §5.2: pipelines are compiled on first use, not up front. Eager
/// construction built the whole 12 blends x 3 stencil variants plus 3 mask
/// writers — 39 objects — before the first frame, whatever the animation needed.
#[test]
fn pipelines_are_built_on_demand() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    assert_eq!(renderer.pipeline_count(), 0, "constructing a renderer compiled pipelines before anything had been drawn");

    let mut scene = make_scene(&device, &queue, true);
    let mut player = make_player(&device, &queue, &mut renderer, "Basic");
    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut player], 10);
    let after = renderer.pipeline_count();
    eprintln!("Basic.ssab compiled {after} pipelines (eager construction built 39)");
    assert!(after > 0, "nothing was compiled — the player drew nothing, so this test proves nothing");
    assert!(after < 39, "compiled {after} pipelines; on-demand building is supposed to stay well under the eager 39");

    // Re-drawing the same frame must not compile anything new. A key that varies
    // between two identical frames would grow the cache without bound.
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    assert_eq!(renderer.pipeline_count(), after, "rebuilding the same frame compiled more pipelines — the cache key is not stable");
}

/// SPEC §1.2: the renderer is device-scoped, so a second player on the same
/// device costs no extra pipelines and no extra atlas upload.
///
/// Before P7-3 each player owned its renderer outright, so two characters on one
/// screen meant two copies of every pipeline and every atlas — 16 MB of VRAM for
/// one 2048² page, doubled.
#[test]
fn players_share_one_renderer() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = make_scene(&device, &queue, true);

    let mut first = make_player(&device, &queue, &mut renderer, "Mask");
    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut first], 10);
    let after_one = renderer.pipeline_count();

    let mut second = make_player(&device, &queue, &mut renderer, "Mask");
    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut first, &mut second], 10);
    let after_two = renderer.pipeline_count();

    eprintln!("pipelines: one player {after_one}, two players {after_two}");
    assert!(after_one > 0, "the first player compiled nothing — the comparison below would be vacuous");
    assert_eq!(after_one, after_two, "a second player of the same pack compiled {} more pipelines — it is not sharing the renderer", after_two - after_one);
    assert!(drawn_pixels(&render(&device, &queue, &renderer, &scene)) > 0, "the shared scene drew nothing");
}

/// SPEC §5.5: there is no vertex buffer. Each geometry kind is expanded by its
/// own vertex stage — `vs_quad` / `vs_mesh` / `vs_shape` / `vs_effect` — reading
/// the runtime's SoA buffers straight out of the scene's arena, so the pipeline
/// key gained a geometry class and each stage is a separate way to draw nothing.
///
/// The four fixtures below are one per class. Counts are floors well under what
/// each currently draws (Basic ~17k, Effect ~14k, Shape ~26k, Mesh ~5k at frame
/// 8), so this catches a class vanishing rather than tracking its exact output —
/// that is [`golden_corpus`]'s job.
///
/// [`golden_corpus`]: ../golden/fn.golden_corpus.html
#[test]
fn every_geometry_class_draws() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };

    for (pack, floor) in [("Basic", 8_000), ("Mesh", 2_000), ("Shape", 8_000), ("Effect", 5_000)] {
        let mut renderer = SSRenderer::new(&device);
        let mut scene = make_scene(&device, &queue, true);
        let mut player = make_player(&device, &queue, &mut renderer, pack);
        advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut player], 8);

        let drawn = drawn_pixels(&render(&device, &queue, &renderer, &scene));
        eprintln!("{pack}.ssab drew {drawn} px with {} pipelines", renderer.pipeline_count());
        assert!(drawn > floor, "{pack}.ssab drew {drawn} px (floor {floor}) — its geometry class produced nothing");
    }
}

/// SPEC §14.2 item 10: a frame reports what it cost. Every remaining item in
/// that section is a claim about issuing less work, and none of them can be
/// judged against a player that reports nothing.
///
/// The counters are the part that is always available (GPU timing needs a device
/// feature and lags a frame, so it cannot be asserted on here). They have to
/// track the scene rather than be plausible constants, so this checks them
/// against changes whose direction is known: a second player adds primitives, an
/// invisible one contributes none.
#[test]
fn a_frame_reports_what_it_cost() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut scene = make_scene(&device, &queue, true);
    let mut one = make_player(&device, &queue, &mut renderer, "Basic");

    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut one], 10);
    let single = scene.stats();
    eprintln!("one player: {} prims, {} draws, {} B", single.prims, single.draw_calls, single.uploaded_bytes);
    assert_eq!(single.players, 1);
    assert!(single.prims > 0, "a frame that drew something reported no primitives");
    assert!(single.draw_calls > 0, "a frame that drew something reported no draw calls");
    assert!(single.draw_calls <= single.prims, "more draw calls than primitives — every draw covers at least one");
    assert!(single.uploaded_bytes > 0, "geometry reached the GPU but no bytes were counted");

    // A second player of the same pack roughly doubles the geometry. Exactness
    // is not the point; tracking is.
    let mut two = make_player(&device, &queue, &mut renderer, "Basic");
    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut one, &mut two], 10);
    let pair = scene.stats();
    assert_eq!(pair.players, 2);
    assert!(pair.prims > single.prims, "a second player added no primitives ({} vs {})", pair.prims, single.prims);
    assert!(pair.uploaded_bytes > single.uploaded_bytes, "a second player uploaded nothing");

    // Hiding one takes its share back out rather than merely skipping the draw.
    // Compared against the same frame with the player simply absent — the part
    // count varies along the timeline, so `single` above is not the reference.
    two.set_visible(false);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut one, &mut two]);
    let hidden = scene.stats();

    two.set_visible(true);
    two.set_culled(true);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut one, &mut two]);
    let culled = scene.stats();

    two.set_culled(false);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut one]);
    let solo = scene.stats();

    assert_eq!(hidden.players, 1, "a hidden player was still counted as drawing");
    assert_eq!(hidden.prims, solo.prims, "a hidden player still contributed primitives");
    assert_eq!(hidden.uploaded_bytes, solo.uploaded_bytes, "a hidden player still uploaded geometry");
    assert_eq!(culled.players, 1, "a culled player was still counted as drawing");
    assert_eq!(culled.prims, solo.prims, "a culled player still contributed primitives");
    assert_eq!(culled.uploaded_bytes, solo.uploaded_bytes, "a culled player still uploaded geometry");
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
    let mut renderer = SSRenderer::new(&device);
    let mut scene = make_scene(&device, &queue, true);
    let mut player = make_player(&device, &queue, &mut renderer, "Basic");

    advance(&mut scene, &mut renderer, &device, &queue, &mut [&mut player], 10);
    let opaque = luminance_sum(&render(&device, &queue, &renderer, &scene));

    // Same animation frame for all three, so only alpha differs.
    player.set_alpha(0.5);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    let half = luminance_sum(&render(&device, &queue, &renderer, &scene));

    player.set_alpha(0.0);
    rebuild(&mut scene, &mut renderer, &device, &queue, &mut [&mut player]);
    let clear = render(&device, &queue, &renderer, &scene);

    eprintln!("luminance: alpha=1 {opaque}, alpha=0.5 {half}, alpha=0 {}", luminance_sum(&clear));
    assert!(opaque > 0, "nothing drawn at full alpha — fixture problem");
    assert_eq!(drawn_pixels(&clear), 0, "alpha 0 still drew");
    assert!(half < opaque && half > 0, "alpha 0.5 should sit strictly between (got {half}, full {opaque})");

    // Clamped, so an out-of-range value cannot brighten the player.
    player.set_alpha(4.0);
    assert_eq!(player.alpha(), 1.0, "set_alpha did not clamp");
}
