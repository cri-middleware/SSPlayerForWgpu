//! Shared packs and the borrowed texture channel (SPEC §8.2, phase P7-5).
//!
//! Two claims are worth a GPU here.
//!
//! * **A pack is shared, not copied.** Twenty of the same character must cost one
//!   parse, one PNG decode and one atlas upload. `texture_count` cannot show that
//!   — N overwrites of one hash look identical to one upload — so
//!   [`SSRenderer::texture_upload_count`] is what these assert on.
//! * **A texture can be borrowed rather than owned.** `set_texture` takes a view
//!   the supplier keeps, which is how an engine's asset system stays the single
//!   owner (§8.2). It has to produce the same pixels as `upload_texture`, and the
//!   [`AlphaMode`] that comes with it has to actually change how the fragment
//!   stage reads the texel — the mislabelled control below is what proves it does.
//!
//! Every test skips cleanly when no GPU adapter is available.

use ssplayer_wgpu::{AlphaMode, Retention, SSPack, SSPlayer, SSRenderer, SSScene, SceneConfig};

const W: u32 = 256;
const H: u32 = 256;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
}

fn embedded_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall_emb")
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// Straight-alpha RGBA -> premultiplied, the same rounding `upload_texture` uses.
fn premultiply(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u32;
        px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
        px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
        px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
    }
    out
}

/// A `width` x `height` straight-alpha atlas whose alpha ramps left to right over
/// a constant colour. Partial coverage everywhere is what makes premultiplied and
/// straight sampling distinguishable; the floor of 16 keeps the fragment stage off
/// its `a == 0` early-out.
fn alpha_ramp(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..height {
        for x in 0..width {
            let a = 16 + (x * 239 / width.max(1)) as u8;
            out.extend_from_slice(&[255, 160, 64, a]);
        }
    }
    out
}

/// Every atlas in the asset dir, keyed the way the viewer keys them.
fn atlases() -> Vec<(u32, Vec<u8>, u32, u32)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(asset_dir()).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("png") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let img = image::open(&path).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        out.push((ssruntime::fnv1a_hash_str(stem), img.into_raw(), w, h));
    }
    out
}

/// Create a texture the *test* owns, for handing to `set_texture` (SPEC §8.2).
/// The returned view is what the renderer borrows; the texture behind it stays
/// the caller's.
fn supplier_texture(device: &wgpu::Device, queue: &wgpu::Queue, rgba: &[u8], width: u32, height: u32) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("supplier texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * width), rows_per_image: Some(height) },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

fn play(pack: &SSPack) -> SSPlayer {
    let mut player = SSPlayer::new();
    player.set_pack(pack);
    let anim = pack.animation_names()[0].clone();
    player.set_animation(&anim).unwrap();
    player.play();
    player
}

/// Advance `players` through `frames` and read the scene back as RGBA8.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut SSRenderer, scene: &mut SSScene, players: &mut [&mut SSPlayer], frames: u32) -> Vec<u8> {
    for _ in 0..frames {
        scene.update(renderer, device, queue, 1.0 / 60.0, players);
    }

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
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (W * H * 4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
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
    let data = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    data
}

fn drawn_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4).filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

/// Mean absolute channel difference, in 0..255 units.
fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    sum as f64 / a.len() as f64
}

fn scene_for(device: &wgpu::Device, queue: &wgpu::Queue) -> SSScene {
    let mut scene = SSScene::new(device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: None });
    scene.set_viewport(queue, W as f32, H as f32, false);
    scene
}

/// A pack names itself, so a host does not have to reconstruct the name that
/// instance parts reference (SPEC §7.1 / §8.2). `Instance.ssab` points at
/// `Instance_SourceAnime`, and that resolves only if the two agree.
#[test]
fn a_pack_carries_its_own_name() {
    let bytes = std::fs::read(format!("{}/Instance.ssab", asset_dir())).unwrap();
    let pack = SSPack::load(&bytes).unwrap();
    assert_eq!(pack.name(), "Instance", "the pack did not name itself after the .ssab");

    let src = std::fs::read(format!("{}/Instance_SourceAnime.ssab", asset_dir())).unwrap();
    let source = SSPack::load(&src).unwrap();
    assert_eq!(source.name(), "Instance_SourceAnime");

    // Registered by their own names, the external reference has to resolve.
    let mut player = SSPlayer::new();
    player.add_pack(&source);
    player.set_pack(&pack);
    let anim = pack.animation_names().into_iter().find(|a| a == "anime_1").unwrap_or_else(|| pack.animation_names()[0].clone());
    player.set_animation(&anim).unwrap();
    assert!(player.instance_node_count() > 0, "no instance child was built — the sub-animation pack did not resolve by name");

    // An explicit name still wins, for hosts that key packs off file paths.
    assert_eq!(SSPack::load_named("Renamed", &bytes).unwrap().name(), "Renamed");
}

/// SPEC §8.2: sharing a pack means sharing its atlas. Two players of one embedded
/// pack must decode and upload it exactly once between them — the decode is
/// skipped too, because the filter runs before the PNG is touched.
#[test]
fn a_shared_pack_uploads_its_atlas_once() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let path = format!("{}/Basic.ssab", embedded_dir());
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("embedded fixture missing ({path}); run scripts/deploy-examples.sh to generate it. skipping.");
        return;
    };

    let pack = SSPack::load(&bytes).unwrap();
    assert!(pack.has_embedded_textures(), "the fixture is not an embedded pack — this test would prove nothing");
    let atlas_count = pack.required_textures().len();
    assert!(atlas_count > 0, "the pack asks for no textures — nothing to share");

    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene_for(&device, &queue);

    let mut first = play(&pack);
    let one = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut first], 10);
    assert!(drawn_pixels(&one) > 5_000, "the first player drew nothing — the embedded atlas never reached the GPU");
    assert_eq!(renderer.texture_upload_count(), atlas_count, "the first player did not upload exactly the pack's atlases");

    // A second player of the same pack. Nothing new may reach the GPU.
    let mut second = play(&pack);
    second.set_transform(40.0, 0.0, 1.0, 1.0, 0.0);
    let two = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut first, &mut second], 10);
    assert_eq!(renderer.texture_upload_count(), atlas_count, "the second player uploaded the atlas again — the pack is not being shared");
    assert_eq!(renderer.texture_count(), atlas_count, "the texture map grew for the second player");
    assert!(drawn_pixels(&two) > drawn_pixels(&one), "the second player added nothing to the frame, so the sharing claim is untested");
}

/// SPEC §8.2: `set_texture` borrows a view the supplier owns and builds nothing
/// but a bind group around it. Given the same pixels, it has to render exactly
/// what `upload_texture` renders — the only difference is who owns the texture.
#[test]
fn a_borrowed_texture_renders_like_an_owned_one() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let bytes = std::fs::read(format!("{}/Basic.ssab", asset_dir())).unwrap();
    let pack = SSPack::load(&bytes).unwrap();
    let atlases = atlases();

    // Owned: the renderer creates the texture and premultiplies on the way in.
    let owned = {
        let mut renderer = SSRenderer::new(&device);
        for (hash, rgba, w, h) in &atlases {
            renderer.upload_texture(&device, &queue, *hash, rgba, (*w, *h), Retention::Retain);
        }
        let mut scene = scene_for(&device, &queue);
        let mut player = play(&pack);
        render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10)
    };
    assert!(drawn_pixels(&owned) > 5_000, "the owned-texture render drew nothing — fixture problem");

    // Borrowed: the test owns the textures, the renderer only points at them.
    let mut renderer = SSRenderer::new(&device);
    let _views: Vec<wgpu::TextureView> = atlases
        .iter()
        .map(|(hash, rgba, w, h)| {
            let view = supplier_texture(&device, &queue, &premultiply(rgba), *w, *h);
            renderer.set_texture(&device, *hash, &view, (*w, *h), AlphaMode::Premultiplied);
            view
        })
        .collect();
    assert_eq!(renderer.texture_upload_count(), 0, "set_texture created a texture of its own — it is supposed to borrow");
    assert_eq!(renderer.texture_alpha_mode(atlases[0].0), Some(AlphaMode::Premultiplied));

    let mut scene = scene_for(&device, &queue);
    let mut player = play(&pack);
    let borrowed = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10);
    assert_eq!(borrowed, owned, "a borrowed texture rendered differently from the same pixels uploaded");

    // Handing the hash back drops the registration; the draw falls through to the
    // 1x1 white fallback (SPEC §8), so the frame must change.
    for (hash, ..) in &atlases {
        renderer.forget_texture(*hash);
    }
    assert_eq!(renderer.texture_count(), 0, "forget_texture left the registration behind");
    let mut player = play(&pack);
    let forgotten = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10);
    assert_ne!(forgotten, borrowed, "forgetting every texture changed nothing — they were not actually being sampled");
}

/// SPEC §8.2: engines hand over straight-alpha textures, so `AlphaMode` tells the
/// fragment stage whether to divide the alpha back out. Declaring it correctly
/// must land within rounding of our own premultiplied path; declaring it wrongly
/// must be far worse — that gap is the evidence the flag reaches the shader.
///
/// The atlas is synthetic on purpose. The two modes agree wherever alpha is 0 or
/// 1, and the shipped fixtures are almost entirely opaque, so run against them
/// this test passes whether or not the flag does anything. A horizontal alpha
/// ramp puts every part on partial coverage, where mislabelling brightens the
/// texel by 1/alpha.
#[test]
fn alpha_mode_decides_how_the_texel_is_read() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let bytes = std::fs::read(format!("{}/Basic.ssab", asset_dir())).unwrap();
    let pack = SSPack::load(&bytes).unwrap();
    // Same hashes and sizes as the real atlases, so the animation samples them,
    // but with alpha that actually varies.
    let atlases: Vec<(u32, Vec<u8>, u32, u32)> = atlases().into_iter().map(|(hash, _, w, h)| (hash, alpha_ramp(w, h), w, h)).collect();

    // Reference: the pixels premultiplied on the way in, as our own path does.
    let reference = {
        let mut renderer = SSRenderer::new(&device);
        for (hash, rgba, w, h) in &atlases {
            renderer.upload_texture(&device, &queue, *hash, rgba, (*w, *h), Retention::Retain);
        }
        let mut scene = scene_for(&device, &queue);
        let mut player = play(&pack);
        render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10)
    };
    assert!(drawn_pixels(&reference) > 5_000, "the reference render drew nothing — fixture problem");

    // The same straight pixels, declared honestly and then dishonestly.
    let mut rendered = Vec::new();
    for mode in [AlphaMode::Straight, AlphaMode::Premultiplied] {
        let mut renderer = SSRenderer::new(&device);
        let _views: Vec<wgpu::TextureView> = atlases
            .iter()
            .map(|(hash, rgba, w, h)| {
                let view = supplier_texture(&device, &queue, rgba, *w, *h);
                renderer.set_texture(&device, *hash, &view, (*w, *h), mode);
                view
            })
            .collect();
        let mut scene = scene_for(&device, &queue);
        let mut player = play(&pack);
        rendered.push(render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10));
    }
    let (honest, mislabelled) = (&rendered[0], &rendered[1]);

    let honest_err = mean_abs_diff(honest, &reference);
    let mislabelled_err = mean_abs_diff(mislabelled, &reference);
    eprintln!("mean |diff| vs premultiplied reference: AlphaMode::Straight {honest_err:.3}, mislabelled {mislabelled_err:.3}");

    // Straight and premultiplied differ only by u8 rounding and by how linear
    // filtering blends across alpha edges, so a small residue is expected.
    assert!(honest_err < 2.0, "AlphaMode::Straight is {honest_err:.3} away from the premultiplied reference — the shader is still dividing the alpha out");
    assert!(mislabelled_err > 10.0, "mislabelling straight pixels as premultiplied moved the image by only {mislabelled_err:.3} — AlphaMode is not reaching the shader");
}

/// A pack hands over pixels; it never owns a GPU texture (SPEC §8.2). The decode
/// is the caller's to place — here into a texture the test owns and lends back —
/// which is exactly the route a Bevy adapter takes into `Assets<Image>`.
#[test]
fn embedded_pixels_can_be_routed_through_the_borrowed_channel() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let emb_path = format!("{}/Basic.ssab", embedded_dir());
    let Ok(emb_bytes) = std::fs::read(&emb_path) else {
        eprintln!("embedded fixture missing ({emb_path}); run scripts/deploy-examples.sh to generate it. skipping.");
        return;
    };
    let pack = SSPack::load(&emb_bytes).unwrap();

    // The player's own convenience route: decode + upload on the next update.
    let by_player = {
        let mut renderer = SSRenderer::new(&device);
        let mut scene = scene_for(&device, &queue);
        let mut player = play(&pack);
        render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10)
    };
    assert!(drawn_pixels(&by_player) > 5_000, "the embedded fixture drew nothing — fixture problem");

    // The engine route: take the pixels, own them elsewhere, lend the view back.
    let decoded = pack.decode_embedded();
    assert_eq!(decoded.len(), pack.required_textures().len(), "decode_embedded did not produce one image per cellmap");

    let mut renderer = SSRenderer::new(&device);
    let _views: Vec<wgpu::TextureView> = decoded
        .iter()
        .map(|tex| {
            let view = supplier_texture(&device, &queue, &premultiply(&tex.rgba), tex.width, tex.height);
            renderer.set_texture(&device, tex.hash, &view, (tex.width, tex.height), AlphaMode::Premultiplied);
            view
        })
        .collect();
    let mut scene = scene_for(&device, &queue);
    let mut player = play(&pack);
    let by_supplier = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10);

    assert_eq!(renderer.texture_upload_count(), 0, "the player uploaded the embedded atlas even though it was already registered");
    assert_eq!(by_supplier, by_player, "routing the embedded pixels through set_texture rendered differently");
}
