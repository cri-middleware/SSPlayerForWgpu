//! Instance / effect **slot stepping**.
//!
//! Whose clock moves and what gets drawn are different questions, and the player
//! answers them in two separate walks for a reason: the runtime drops `Hide`-ed
//! parts from `draw_order` / `draw_batches` entirely. A player that steps its
//! slots from the draw walk therefore stops stepping a slot the moment its part
//! is hidden — and an independently-played child, which runs on its own elapsed
//! time rather than the parent's frame, then freezes and resumes late by exactly
//! the time it spent hidden. The doc calls this out as the mistake to avoid, and
//! `Instance.ssab` ships `INDEP_MID_INFINITE_HIDE` — an independent infinite
//! child hidden through the middle of the parent's timeline — for the check.
//!
//! The player's other loop-edge rule — read the pulse from `Context::is_looped`
//! rather than inferring it from "the frame went backwards" — has **no test
//! here on purpose**: the two agree on every path today's public API can reach.
//! `set_speed_scale` cannot make the frame run backwards (the runtime clamps a
//! non-positive rate to 1.0) and neither reverse nor ping-pong direction is
//! exposed, so nothing distinguishes them until one of those lands. The reason
//! to read the pulse anyway is that it is the runtime's own answer, and the
//! inference silently becomes wrong the moment the frame can move backwards for
//! a reason other than a loop.
//!
//! Skips cleanly when no GPU adapter is available: stepping happens inside
//! `SSScene::update`, which borrows a device.

use ssplayer_wgpu::{MaskBackend, Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};

const W: u32 = 256;
const H: u32 = 256;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// `Instance.ssab` with its referenced pack registered, playing `anime_1` — the
/// animation whose instance parts carry the authored hide windows.
fn instance_player() -> Option<SSPlayer> {
    let parent = std::fs::read(format!("{}/Instance.ssab", asset_dir())).ok()?;
    let source = std::fs::read(format!("{}/Instance_SourceAnime.ssab", asset_dir())).ok()?;
    let mut player = SSPlayer::new();
    player.add_ssab("Instance_SourceAnime", &source).ok()?;
    player.load_ssab("Instance", &parent).ok()?;
    player.set_animation("anime_1").ok()?;
    player.play();
    Some(player)
}

/// Part index of `name` in `Instance.ssab`, so the tests name the authored part
/// rather than hard-coding an index the fixture is free to renumber.
fn part_index(name: &str) -> Option<usize> {
    let bytes = std::fs::read(format!("{}/Instance.ssab", asset_dir())).ok()?;
    // SAFETY: the file is the converter's own output, loaded the same way the
    // player loads it.
    let ab = unsafe { ssab::ssab::ss::format::root_as_ss_anime_binary_unchecked(&bytes) };
    ab.parts().iter().position(|p| p.name() == name)
}

fn child_frame(player: &SSPlayer, part_idx: usize) -> f32 {
    player.instance_child_frames().iter().find(|&&(i, _)| i == part_idx).map(|&(_, f)| f).expect("no instance child at that part index")
}

/// Step `player` one tick at a time, sampling one child's frame after each.
fn sample_child_frames(player: &mut SSPlayer, part_idx: usize, ticks: usize) -> Option<Vec<f32>> {
    let (device, queue) = gpu()?;
    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(&queue, W as f32, H as f32, false);

    let mut frames = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [player]);
        frames.push(child_frame(player, part_idx));
    }
    Some(frames)
}

/// The clock of an independently-played child is not the draw order's business:
/// it keeps advancing while the part is hidden, so the child is where it should
/// be when it comes back rather than resuming late.
#[test]
fn a_hidden_independent_child_keeps_its_clock() {
    let Some(mut player) = instance_player() else {
        eprintln!("Instance fixtures missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let Some(part_idx) = part_index("INDEP_MID_INFINITE_HIDE") else {
        eprintln!("the fixture no longer carries INDEP_MID_INFINITE_HIDE — this test would prove nothing. skipping.");
        return;
    };
    // The parent runs 0..60; the part is hidden for 51 of those frames.
    let Some(frames) = sample_child_frames(&mut player, part_idx, 61) else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    // Start measuring once the child is actually running: before its playback
    // key fires it sits still for legitimate reasons.
    let Some(start) = frames.windows(2).position(|w| w[0] != w[1]) else {
        panic!("the independent child never advanced at all — the fixture or the slot wiring changed");
    };

    // At 60 fps with a 1/60 s tick the child moves a frame per tick, so the only
    // repeats should be the wrap at the loop edge.
    let mut run = 1usize;
    let mut worst = (1usize, 0usize); // (length, tick where it ended)
    for (i, w) in frames[start..].windows(2).enumerate() {
        run = if w[0] == w[1] { run + 1 } else { 1 };
        if run > worst.0 {
            worst = (run, start + i + 1);
        }
    }
    assert!(worst.0 <= 2, "the independent child's frame stood still for {} ticks (up to tick {}): its clock stopped while the part was hidden, which is what stepping from the draw order does. frames={frames:?}", worst.0, worst.1);
}

/// `Effect.ssab` playing `anime_1`, whose parts are all effects or nulls — so
/// every primitive it draws is a particle.
fn effect_player() -> Option<SSPlayer> {
    let bytes = std::fs::read(format!("{}/Effect.ssab", asset_dir())).ok()?;
    let mut player = SSPlayer::new();
    player.load_ssab("Effect", &bytes).ok()?;
    player.set_animation("anime_1").ok()?;
    player.play();
    Some(player)
}

/// Upload every atlas in the asset dir, keyed as the reference-mode packs name them.
fn load_textures(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
    for entry in std::fs::read_dir(asset_dir()).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("png") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let img = image::open(&path).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        renderer.upload_texture(device, queue, ssruntime::fnv1a_hash_str(stem), &img, (w, h), Retention::Retain);
    }
}

/// Draw `scene` into a fresh W x H target and read it back as RGBA8.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &SSRenderer, scene: &SSScene) -> Vec<u8> {
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (W * H * 4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store }, depth_slice: None })],
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

/// A hidden effect part steps without building particles, and its clock still
/// moves: shown again, it draws what a player that never hid it draws. The
/// fixture's effects start on frame 0 and on a later key, synced and
/// independent, and the particles are checked to be there, so two players
/// drawing none would not pass.
#[test]
fn a_hidden_effect_draws_what_the_never_hidden_draws_once_shown() {
    let (Some(mut shown), Some(mut hidden)) = (effect_player(), effect_player()) else {
        eprintln!("Effect.ssab missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    load_textures(&mut renderer, &device, &queue);
    let make_scene = || {
        let mut scene = SSScene::new(&device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
        scene.set_viewport(&queue, W as f32, H as f32, false);
        scene
    };
    let (mut shown_scene, mut hidden_scene) = (make_scene(), make_scene());

    let mut compared = 0;
    for tick in 0..90 {
        // Part 0 is the root, so the cascade hides every effect part with it.
        if tick == 10 {
            hidden.set_part_visibility_override_by_index(0, true, true).unwrap();
        }
        if tick == 50 {
            hidden.clear_part_visibility_override_by_index(0).unwrap();
        }
        shown_scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut shown]);
        hidden_scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut hidden]);
        if (10..50).contains(&tick) {
            assert_eq!(hidden_scene.stats().prims, 0, "tick {tick}: the hidden player still draws");
        }
        if tick < 60 {
            continue;
        }
        let (prims, hidden_prims) = (shown_scene.stats().prims, hidden_scene.stats().prims);
        assert_eq!(prims, hidden_prims, "tick {tick}: {hidden_prims} particles where the never-hidden player draws {prims} — the effect lost the time it spent hidden");
        assert!(render(&device, &queue, &renderer, &shown_scene) == render(&device, &queue, &renderer, &hidden_scene), "tick {tick}: the effect draws differently from the never-hidden player's — it lost the time it spent hidden");
        if prims > 0 {
            compared += 1;
        }
    }
    assert!(compared > 0, "no particle was drawn after the hidden stretch — the test would pass either way");
}

/// A synced child follows its parent on the tick its key is reached, too.
///
/// The slot seeks it to the parent-relative frame; the host's play() on that
/// tick then resets the controller to the section start, which the next step
/// seeks again. Drawing what the controller says after the play() rather than
/// the slot's frame put the child half a frame behind its parent on that tick.
#[test]
fn a_synced_child_follows_the_parent_on_its_transition_tick() {
    let Some(mut player) = instance_player() else {
        eprintln!("Instance fixtures missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let Some(part_idx) = part_index("NO_KEY") else {
        eprintln!("the fixture no longer carries NO_KEY — this test would prove nothing. skipping.");
        return;
    };
    let Some(frames) = sample_child_frames(&mut player, part_idx, 1) else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    // A 1/60 s tick lands the 30 fps parent between frames, which is what makes
    // the section start and the parent-relative frame different numbers.
    let parent = player.frame_no();
    assert!(parent.fract() != 0.0, "the first tick landed on a whole frame ({parent}) — the test would pass either way");
    assert!((frames[0] - parent).abs() < 1e-3, "on its transition tick the synced child is at {} while its parent is at {parent}", frames[0]);
}
