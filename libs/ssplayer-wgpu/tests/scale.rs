//! How a CPU frame scales with the cast.
//!
//! `#[ignore]`d: it is a measurement, not an assertion. Wall-clock on a shared
//! machine is not something to gate a build on, and the numbers are what decide
//! whether an optimisation earns its keep.
//!
//! ```sh
//! cargo test --release --test scale -- --ignored --nocapture
//! ```
//!
//! Run it in **release**: a frame is arithmetic over the runtime's buffers, and
//! a debug build measures the absence of inlining more than anything else.
//!
//! It is what retired the threaded player walk (item 6) and what rejected the
//! render bundle (item 5) — both were implemented, measured and removed. Re-run
//! it after anything that changes what a frame does per player; those two are
//! worth reconsidering only if these numbers move by an order of magnitude.
//!
//! The last three columns are for the items that are still open (7 and 4). They
//! are about issuing *fewer draws*, not about spending less CPU per draw — so
//! what matters is `gpu (ms)` against `draws`, and how much of `draws` is
//! avoidable (`mergeable`, `rebinds`). Deciding those two from CPU time is how
//! items 5 and 6 went wrong.
//!
//! **`update` and the pass are timed in one loop, deliberately.** Timing them
//! separately flatters anything that caches across frames: a loop that never
//! updates replays a recording that never goes stale. That is exactly how the
//! render bundle looked like a 300x win before it was measured properly.

use ssplayer_wgpu::{MaskBackend, Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};
use std::path::Path;
use std::time::{Duration, Instant};

const W: u32 = 1280;
const H: u32 = 720;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
}

fn load_textures(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
    for entry in std::fs::read_dir(Path::new(asset_dir())).unwrap().flatten() {
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

fn make_player(pack: &str) -> SSPlayer {
    let mut player = SSPlayer::new();
    let bytes = std::fs::read(format!("{}/{pack}.ssab", asset_dir())).unwrap();
    player.load_ssab(pack, &bytes).unwrap();
    let anim = player.animation_names().unwrap()[0].clone();
    player.set_animation(&anim).unwrap();
    player.play();
    player
}

fn attachment(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

#[test]
#[ignore = "measurement, not an assertion — see the module docs"]
fn frame_cost_by_player_count() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    // GPU timing if the adapter has it; without it the `gpu` column stays blank
    // rather than the whole measurement being useless.
    let desc = wgpu::DeviceDescriptor { required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY, ..Default::default() };
    let (device, queue) = pollster::block_on(adapter.request_device(&desc)).unwrap();
    if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        eprintln!("adapter has no TIMESTAMP_QUERY — the gpu column will be blank");
    }

    eprintln!("{:>7} {:>11} {:>10} {:>9} {:>8} {:>10} {:>9}", "players", "cpu (ms)", "gpu (ms)", "prims", "draws", "mergeable", "rebinds");

    let color_view = attachment(&device, COLOR_FORMAT);
    let stencil_view = attachment(&device, STENCIL_FORMAT);

    for count in [1usize, 8, 32, 64, 128, 256] {
        let mut renderer = SSRenderer::new(&device);
        load_textures(&mut renderer, &device, &queue);
        let mut scene = SSScene::new(&device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: Some(STENCIL_FORMAT), mask: MaskBackend::Stencil });
        scene.set_viewport(&queue, W as f32, H as f32, false);

        // A mixed cast: part counts differ per pack, so the per-player cost is
        // not one number multiplied by `count`, which is what a real scene looks
        // like — and what made the threaded walk's naive split unbalanced.
        let packs = ["Basic", "Mask", "Shape", "Effect"];
        let mut owned: Vec<SSPlayer> = (0..count).map(|i| make_player(packs[i % packs.len()])).collect();
        for (i, player) in owned.iter_mut().enumerate() {
            player.set_transform((i as f32 % 8.0) * 120.0 - 420.0, (i as f32 / 8.0).floor() * 140.0 - 200.0, 0.5, 0.5, 0.0);
            player.set_order(i as i32);
        }

        const WARMUP: u32 = 30;
        const SAMPLES: u32 = 200;
        let mut elapsed = Duration::ZERO;
        let mut gpu_ns: Vec<u64> = Vec::new();

        for frame in 0..WARMUP + SAMPLES {
            let started = Instant::now();

            let mut refs: Vec<&mut SSPlayer> = owned.iter_mut().collect();
            scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut refs);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &color_view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store }, depth_slice: None })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &stencil_view,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                        stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                    }),
                    timestamp_writes: scene.timestamp_writes(),
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                scene.draw(&renderer, &mut rpass);
            }
            scene.resolve_timestamps(&mut encoder);
            // The CPU column is `update` + recording the pass, and stops here:
            // submitting and waiting for the GPU is not CPU work this player does.
            let cpu = started.elapsed();
            queue.submit(std::iter::once(encoder.finish()));
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();

            // The first frames build instance trees and compile pipelines.
            if frame >= WARMUP {
                elapsed += cpu;
                // Readings arrive every couple of frames; keep the distinct ones.
                if let Some(ns) = scene.stats().gpu_pass_time_ns
                    && gpu_ns.last() != Some(&ns)
                {
                    gpu_ns.push(ns);
                }
            }
        }

        let frame_ms = elapsed.as_secs_f64() * 1.0e3 / SAMPLES as f64;
        gpu_ns.sort_unstable();
        // Median: a wall-clock GPU sample on a shared machine has outliers.
        let gpu = match gpu_ns.len() {
            0 => "-".to_string(),
            n => format!("{:.4}", gpu_ns[n / 2] as f64 / 1.0e6),
        };
        let stats = scene.stats();
        eprintln!("{count:>7} {frame_ms:>11.4} {gpu:>10} {:>9} {:>8} {:>10} {:>9}", stats.prims, stats.draw_calls, stats.mergeable_draws, stats.redundant_texture_binds);
    }
}
