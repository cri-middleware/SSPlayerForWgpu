//! Headless GPU render of `Mask.ssab` to an offscreen target. Exercises the
//! real mask-write and stencil-test pipelines (not just the CPU planner) and
//! confirms masking changes the pixel output: rendering with the stencil buffer
//! enabled must differ from rendering with it disabled (masks ignored). Skips
//! cleanly when no GPU adapter is available.

use ssplayer_wgpu::{Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};
use std::path::Path;

const W: u32 = 256;
const H: u32 = 256;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
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

/// Build a reference-mode player (host-supplied textures, with/without a stencil
/// buffer), render `Mask.ssab`/etc. at frame `advance_frames`, return the readback.
fn render_ssab(device: &wgpu::Device, queue: &wgpu::Queue, stencil: bool, pack: &str, advance_frames: u32) -> Vec<u8> {
    let mut renderer = SSRenderer::new(device);
    let mut scene = SSScene::new(device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: stencil.then_some(STENCIL_FORMAT) });
    let mut player = SSPlayer::new();
    load_textures(&mut renderer, device, queue, Path::new(asset_dir()));

    let bytes = std::fs::read(format!("{}/{pack}.ssab", asset_dir())).unwrap();
    player.load_ssab(pack, &bytes).unwrap();
    let anim = player.animation_names().unwrap()[0].clone();
    player.set_animation(&anim).unwrap();
    player.play();
    scene.set_viewport(queue, W as f32, H as f32, false);

    render_to_rgba(device, queue, &mut renderer, &mut scene, &mut player, stencil, advance_frames)
}

/// Advance an already-loaded, already-playing player and read back one frame.
fn render_to_rgba(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut SSRenderer, scene: &mut SSScene, player: &mut SSPlayer, stencil: bool, advance_frames: u32) -> Vec<u8> {
    for _ in 0..advance_frames {
        scene.update(renderer, device, queue, 1.0 / 60.0, &mut [player]);
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

    let depth_stencil_view = stencil.then(|| {
        device
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
            .create_view(&wgpu::TextureViewDescriptor::default())
    });

    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (W * H * 4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let depth_stencil_attachment = depth_stencil_view.as_ref().map(|view| wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
        });
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            depth_stencil_attachment,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        scene.draw(renderer, &mut rpass);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(H) } },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    rx.recv().unwrap().unwrap();
    let data = readback.slice(..).get_mapped_range().to_vec();
    readback.unmap();
    data
}

/// Count pixels whose RGB is not (near) black — i.e. something was drawn.
fn drawn_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4).filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

#[test]
fn render_mask_stencil_effect() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("no GPU adapter available, skipping headless render test");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("request_device");

    // Advance a few frames so the mask animation is well into playback.
    let with_mask = render_ssab(&device, &queue, true, "Mask", 10);
    let without_mask = render_ssab(&device, &queue, false, "Mask", 10);

    let drawn_on = drawn_pixels(&with_mask);
    let drawn_off = drawn_pixels(&without_mask);
    eprintln!("drawn pixels: masking_on={drawn_on} masking_off={drawn_off}");

    // Both renders must produce content...
    assert!(drawn_off > 0, "reference render (masking off) drew nothing — fixture/texture problem");
    assert!(drawn_on > 0, "masked render drew nothing — stencil pipeline problem");
    // ...and masking must actually change the output (clip content / draw masks).
    assert_ne!(with_mask, without_mask, "stencil masking had no effect on the rendered pixels");
}

#[test]
fn render_shape_content_visible() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("no GPU adapter available, skipping headless render test");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("request_device");

    // Shape.ssab's "Shape" part is textureless (fan geometry + fill colour).
    // Before Shape-content support it drew nothing; it must now be visible — in
    // both plain and masking (viewer) modes. In masking mode a pure-mask Shape
    // ("ShapeMask") must NOT paint its own fill over the visible parts.
    let plain = drawn_pixels(&render_ssab(&device, &queue, false, "Shape", 10));
    let masked = drawn_pixels(&render_ssab(&device, &queue, true, "Shape", 10));
    eprintln!("Shape.ssab drawn pixels: plain={plain} masking={masked}");
    // The Shape parts are a large fraction of the frame. A too-low count means the
    // Shape fan did not draw (e.g. the white fallback texture sampled black, so
    // `tex * colour` collapsed to zero) — guard against that specific regression.
    assert!(plain > 15_000, "Shape.ssab drew too little (plain={plain}) — Shape content not filled");
    assert!(masked > 10_000, "Shape.ssab drew too little (masking={masked}) — visible parts hidden by the pure mask");
}

#[test]
fn render_integration_shaders() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("no GPU adapter available, skipping headless render test");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("request_device");

    // Shader.ssab is the integration-shader (`ss-*`) showcase: a grid of parts,
    // each with a different SHDR attribute (pixelate, noise, outline, circle, …).
    // Building the player compiles the SS WGSL module (naga-validated), and the
    // per-part shader dispatch + params must produce output. A too-low count means
    // the shader pipelines / dynamic params plumbing broke and parts vanished.
    let plain = render_ssab(&device, &queue, false, "Shader", 15);
    let drawn = drawn_pixels(&plain);
    eprintln!("Shader.ssab drawn pixels: {drawn}");
    assert!(drawn > 15_000, "Shader.ssab drew too little ({drawn}) — integration-shader dispatch broke");

    // The viewer runs with a stencil buffer. Shader parts carry the
    // mask_influence opt-in, so a stencil-enabled renderer must NOT route them
    // through the masked path when no mask writers exist — otherwise they draw as
    // plain textures with no effect. Guard: stencil on/off must be pixel-identical.
    let with_stencil = render_ssab(&device, &queue, true, "Shader", 15);
    assert_eq!(plain, with_stencil, "Shader.ssab differs with a stencil buffer — shaders skipped by the masked path (no-writer gating regressed)");
}

/// Device-loss recovery (SPEC §14). The player's pipelines, buffers, textures and
/// projection all belong to the device that made them, so `restore_device` has to
/// rebuild every one of them against the replacement. Restoring onto a *second*
/// device (not the same one again) is what makes this a real test: a leftover
/// resource from the first device is either a wgpu validation error or a wrong
/// pixel, never a silent pass.
#[test]
fn restore_device_rebuilds_gpu_state() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("no GPU adapter available, skipping headless render test");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("request_device");
    // The replacement device, standing in for the one the browser hands back after
    // `GPUDevice.lost`.
    let (device2, queue2) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("request_device (replacement)");

    // What frame 11 must look like: one uninterrupted player on the first device.
    let expected = render_ssab(&device, &queue, true, "Mask", 11);

    // Reference mode (host-supplied textures): play 10 frames, lose the device,
    // restore onto the replacement, then render frame 11 there.
    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: Some(STENCIL_FORMAT) });
    let mut player = SSPlayer::new();
    load_textures(&mut renderer, &device, &queue, Path::new(asset_dir()));
    let bytes = std::fs::read(format!("{}/Mask.ssab", asset_dir())).unwrap();
    player.load_ssab("Mask", &bytes).unwrap();
    let anim = player.animation_names().unwrap()[0].clone();
    player.set_animation(&anim).unwrap();
    player.play();
    scene.set_viewport(&queue, W as f32, H as f32, false);
    let _ = render_to_rgba(&device, &queue, &mut renderer, &mut scene, &mut player, true, 10);

    // Restore runs widest scope first (SPEC §14): the device-scoped renderer once,
    // then each render target, then each player's reproducible atlases.
    renderer.restore_device(&device2, &queue2);
    scene.restore_device(&device2, &queue2);
    player.restore_textures(&mut renderer, &device2, &queue2);
    let restored = render_to_rgba(&device2, &queue2, &mut renderer, &mut scene, &mut player, true, 1);

    assert!(drawn_pixels(&restored) > 0, "nothing drawn after restore_device — pipelines/projection not rebuilt");
    assert_eq!(restored, expected, "restored render differs from the uninterrupted one — some GPU state was not rebuilt");

    // Negative control: with the retained sources dropped, the same restore leaves
    // the host textures missing and the render changes. If this passed as equal,
    // the assertion above would prove nothing about texture restoration.
    renderer.forget_texture_sources();
    renderer.restore_device(&device2, &queue2);
    scene.restore_device(&device2, &queue2);
    player.restore_textures(&mut renderer, &device2, &queue2);
    let without_sources = render_to_rgba(&device2, &queue2, &mut renderer, &mut scene, &mut player, true, 1);
    assert_ne!(without_sources, restored, "render unchanged after forget_texture_sources — textures are not actually coming from the retained sources");

    // Embedded mode (SPEC §8.1) retains no pixels: the atlas is re-decoded from the
    // `.ssab` the player still owns. Same fixture-optional skip as render_embedded_ssab.
    let emb_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall_emb/Basic.ssab");
    let Ok(emb_bytes) = std::fs::read(emb_path) else {
        eprintln!("embedded fixture missing ({emb_path}); run scripts/deploy-examples.sh to generate it. skipping the embedded half.");
        return;
    };
    let emb_expected = {
        let mut emb_renderer = SSRenderer::new(&device);
        let mut emb_scene = SSScene::new(&device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: None });
        let mut p = SSPlayer::new();
        p.load_ssab("Basic", &emb_bytes).unwrap();
        let anim = p.animation_names().unwrap()[0].clone();
        p.set_animation(&anim).unwrap();
        p.play();
        emb_scene.set_viewport(&queue, W as f32, H as f32, false);
        render_to_rgba(&device, &queue, &mut emb_renderer, &mut emb_scene, &mut p, false, 11)
    };

    let mut emb_renderer = SSRenderer::new(&device);
    let mut emb_scene = SSScene::new(&device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: None });
    let mut emb = SSPlayer::new();
    emb.load_ssab("Basic", &emb_bytes).unwrap();
    let anim = emb.animation_names().unwrap()[0].clone();
    emb.set_animation(&anim).unwrap();
    emb.play();
    emb_scene.set_viewport(&queue, W as f32, H as f32, false);
    let _ = render_to_rgba(&device, &queue, &mut emb_renderer, &mut emb_scene, &mut emb, false, 10);

    emb_renderer.restore_device(&device2, &queue2);
    emb_scene.restore_device(&device2, &queue2);
    emb.restore_textures(&mut emb_renderer, &device2, &queue2);
    let emb_restored = render_to_rgba(&device2, &queue2, &mut emb_renderer, &mut emb_scene, &mut emb, false, 1);
    assert!(drawn_pixels(&emb_restored) > 5_000, "embedded atlas not re-decoded after restore_device");
    assert_eq!(emb_restored, emb_expected, "restored embedded render differs — the atlas was not re-decoded from the retained .ssab");
}

#[test]
fn render_embedded_ssab() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("no GPU adapter available, skipping headless render test");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("request_device");

    // Embedded-mode ssab (SPEC §8.1): the atlas PNG is baked into the .ssab, so the
    // player decodes + uploads it itself — the host supplies NO textures. The
    // result must match the reference-mode render (same pack, host-supplied atlas).
    // The embedded fixture is a separate conversion; skip if it wasn't produced
    // (`scripts/deploy-examples.sh` emits both modes).
    let emb_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall_emb");
    let emb_path = format!("{emb_dir}/Basic.ssab");
    let Ok(bytes) = std::fs::read(&emb_path) else {
        eprintln!("embedded fixture missing ({emb_path}); run scripts/deploy-examples.sh to generate it. skipping.");
        return;
    };

    let reference = render_ssab(&device, &queue, false, "Basic", 10);

    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: None });
    let mut player = SSPlayer::new();
    player.load_ssab("Basic", &bytes).unwrap(); // no load_textures — embedded supplies them
    let anim = player.animation_names().unwrap()[0].clone();
    player.set_animation(&anim).unwrap();
    player.play();
    scene.set_viewport(&queue, W as f32, H as f32, false);
    let embedded = render_to_rgba(&device, &queue, &mut renderer, &mut scene, &mut player, false, 10);

    let drawn = drawn_pixels(&embedded);
    eprintln!("Basic.ssab embedded drawn pixels: {drawn} (reference {})", drawn_pixels(&reference));
    assert!(drawn > 5_000, "embedded Basic.ssab drew too little ({drawn}) — embedded atlas not decoded/uploaded");
    assert_eq!(embedded, reference, "embedded render differs from reference — texture hash/premul mismatch");
}
