//! GPU pass timing.
//!
//! Alone in its own test binary on purpose. Cargo runs the tests inside one
//! binary concurrently, and a Metal device asked to sample pass-boundary
//! counters while sibling devices are alive simply does not sample them — the
//! resolve comes back as zeros. That is a property of the driver, not of this
//! code, and the player already treats it as "no reading" rather than as a
//! zero-length frame; but a test that shares a binary with ten other GPU tests
//! would fail on it perhaps half the time.

use ssplayer_wgpu::{MaskBackend, Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};
use std::path::Path;

const W: u32 = 256;
const H: u32 = 256;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
}

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

fn make_scene(device: &wgpu::Device, queue: &wgpu::Queue, stencil: bool) -> SSScene {
    let mut scene = SSScene::new(device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: stencil.then_some(STENCIL_FORMAT), mask: if stencil { MaskBackend::Stencil } else { MaskBackend::None } });
    scene.set_viewport(queue, W as f32, H as f32, false);
    scene
}

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
    wgpu::RenderPassDepthStencilAttachment { view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }) }
}

/// GPU half: a device created with `TIMESTAMP_QUERY` must
/// actually produce a pass time.
///
/// The counters in [`a_frame_reports_what_it_cost`] would look identical if the
/// timestamp plumbing were inert, so this walks the whole handshake — writes on
/// the pass, resolve after it, submit, then the map on the following frame — and
/// requires a number at the end. Skipped when the adapter has no timestamp
/// query, which is the honest outcome rather than a silent pass.
#[test]
fn gpu_pass_time_comes_back() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })) else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        eprintln!("adapter has no TIMESTAMP_QUERY, skipping GPU timing test");
        return;
    }
    let desc = wgpu::DeviceDescriptor { required_features: wgpu::Features::TIMESTAMP_QUERY, ..Default::default() };
    let (device, queue) = pollster::block_on(adapter.request_device(&desc)).expect("request_device with TIMESTAMP_QUERY");

    let mut renderer = SSRenderer::new(&device);
    let mut scene = make_scene(&device, &queue, true);
    let mut player = make_player(&device, &queue, &mut renderer, "Basic");

    // The reading is deliberately read back late, so the first frame cannot have
    // it. The budget is generous rather than tight: cargo runs this file's tests
    // concurrently, and a contended GPU takes longer to drain than an idle one —
    // a tight bound here is a flaky test, not a stricter one.
    const BUDGET: u32 = 60;
    let mut measured = None;
    let mut frames = 0;
    for _ in 0..BUDGET {
        frames += 1;
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);

        let (color, color_view, stencil_view) = offscreen_target(&device);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("timed pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(depth_stencil_attachment(&stencil_view)),
                timestamp_writes: scene.timestamp_writes(),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            scene.draw(&renderer, &mut rpass);
        }
        scene.resolve_timestamps(&mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        drop(color);
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();

        if let Some(ns) = scene.stats().gpu_pass_time_ns {
            measured = Some(ns);
            break;
        }
    }

    let ns = measured.unwrap_or_else(|| panic!("no GPU pass time after {BUDGET} timed frames — the timestamp handshake is not completing"));
    eprintln!("GPU pass time: {:.3} ms ({ns} ns), arrived on frame {frames}", ns as f64 / 1.0e6);
    assert!(ns > 0, "the pass reported zero nanoseconds");
    // A 256x256 pass drawing one character is fast, but not free and not a
    // second. A reading outside this is a unit or period-conversion mistake.
    assert!(ns < 1_000_000_000, "implausible pass time {ns} ns — check the timestamp period conversion");
}
