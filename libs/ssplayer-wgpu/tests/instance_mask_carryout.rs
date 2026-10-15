//! Instance-internal clipping masks must carry **past the instance boundary** and
//! keep clipping the parent's parts drawn after the instance
//! (the fix ForUnity #133 / ForGodot #295). The `InstancePropagation` fixture is
//! built for exactly this: four `Source01_*` instances (a 2×2 of maskInfluence ×
//! visibleInsideMask), each playing a child whose lower `Check_*_DM_v` rows are
//! clipping writers, with `Arrow*` parts drawn after the instances and a `Hole32`
//! outer mask.
//!
//! Because the two mask backends are independent implementations of the same
//! spec, the load-bearing check is that they **agree** on this fixture — if only
//! one carries the mask out, they diverge. This test also dumps both renders to
//! PNGs under the scratch dir for eyeballing: look at the pixels, do not trust
//! counts alone.
//!
//! Skips itself when no GPU adapter is present, or when the fixture is absent.

use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};
use std::path::Path;

const FIXTURE: &str = "InstancePropagation";
const FRAME: u32 = 15;
const CANVAS_MARGIN: f32 = 1.8;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

fn assets() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

fn adapter() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// Render the fixture after `frames` updates through `backend`, returning
/// `(width, height, tightly-packed RGBA)`.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, path: &str, backend: MaskBackend, frames: u32) -> (u32, u32, Vec<u8>) {
    let stencil_format = (backend == MaskBackend::Stencil).then_some(STENCIL_FORMAT);
    let mut renderer = SSRenderer::new(device);
    let mut scene = SSScene::new(device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format, mask: backend });

    let mut player = SSPlayer::new();
    player.load(path).unwrap();
    let anim = player.animation_names().unwrap()[0].clone();
    player.set_animation(&anim).unwrap();
    player.play();

    let (cw, ch) = player.canvas_size().unwrap_or((512.0, 512.0));
    let (w, h) = ((cw * CANVAS_MARGIN).round().max(64.0) as u32, (ch * CANVAS_MARGIN).round().max(64.0) as u32);
    scene.set_viewport(queue, w as f32, h as f32, false);
    for _ in 0..frames {
        scene.update(&mut renderer, device, queue, 1.0 / 60.0, &mut [&mut player]);
    }

    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen color"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());

    let depth_view = stencil_format.map(|format| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen depth-stencil"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    });

    let bytes_per_row = (w * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (bytes_per_row * h) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    scene.encode_offscreen(&mut renderer, device, &mut encoder);
    {
        let depth_stencil_attachment = depth_view.as_ref().map(|view| wgpu::RenderPassDepthStencilAttachment {
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
        scene.draw(&renderer, &mut rpass);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    rx.recv().unwrap().unwrap();
    let padded = readback.slice(..).get_mapped_range().unwrap().to_vec();
    readback.unmap();

    // Strip the 256-byte row padding into a tightly-packed RGBA buffer.
    let mut tight = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let row = (y * bytes_per_row) as usize;
        tight.extend_from_slice(&padded[row..row + (w * 4) as usize]);
    }
    (w, h, tight)
}

/// Dump a render for eyeballing, but only when asked — `SS_CARRYOUT_DUMP=<dir>`.
/// Without it the test writes nothing (so a plain `cargo test` leaves no files).
fn scratch_png(name: &str, w: u32, h: u32, rgba: &[u8]) {
    let Ok(dir) = std::env::var("SS_CARRYOUT_DUMP") else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = format!("{dir}/{name}");
    if let Some(img) = image::RgbaImage::from_raw(w, h, rgba.to_vec()) {
        img.save(&path).unwrap();
        eprintln!("dumped {path}");
    }
}

/// Total brightness composited over the black clear: sum of each pixel's
/// brightest channel. A part the carried mask clips away removes its contribution.
fn brightness(rgba: &[u8]) -> u64 {
    rgba.as_chunks::<4>().0.iter().map(|p| p[0].max(p[1]).max(p[2]) as u64).sum()
}

/// Pixels drawn over the black clear (any channel non-zero).
fn drawn(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>().0.iter().filter(|p| p[0] | p[1] | p[2] != 0).count()
}

/// Pixels whose worst channel differs by more than `tolerance`, and a diff mask.
/// Like `coverage.rs`: the two backends rasterise coverage into different things
/// (hardware stencil vs a bitmap), so their mask *edges* land on slightly
/// different pixels; the interiors are what must agree.
fn differing(a: &[u8], b: &[u8], tolerance: u8) -> (usize, Vec<u8>) {
    let mut count = 0;
    let mut mask = Vec::with_capacity(a.len());
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
        let delta = pa.iter().zip(pb).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        let hit = delta > tolerance;
        count += hit as usize;
        let v = if hit { 255 } else { 0 };
        mask.extend_from_slice(&[v, 0, 0, 255]);
    }
    (count, mask)
}

#[test]
fn instance_internal_clip_carries_out_and_both_backends_agree() {
    let path = format!("{}/overall/{FIXTURE}.ssab", assets());
    if !Path::new(&path).exists() {
        eprintln!("skipping: fixture {path} is not present — run deploy-examples");
        return;
    }
    let Some((device, queue)) = adapter() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };

    let (cw, ch, cov) = render(&device, &queue, &path, MaskBackend::Coverage, FRAME);
    let (_sw, _sh, sten) = render(&device, &queue, &path, MaskBackend::Stencil, FRAME);
    let (_nw, _nh, none) = render(&device, &queue, &path, MaskBackend::None, FRAME);

    let (differing, diff_mask) = differing(&cov, &sten, 8);

    scratch_png("carryout_coverage.png", cw, ch, &cov);
    scratch_png("carryout_stencil.png", cw, ch, &sten);
    scratch_png("carryout_none.png", cw, ch, &none);
    scratch_png("carryout_diff.png", cw, ch, &diff_mask);

    // The carried clip must remove something the unmasked render draws — on BOTH
    // backends, so neither is silently ignoring the instance-internal mask.
    assert!(brightness(&cov) < brightness(&none), "coverage carry-out removed nothing vs unmasked (cov={} none={})", brightness(&cov), brightness(&none));
    assert!(brightness(&sten) < brightness(&none), "stencil carry-out removed nothing vs unmasked (sten={} none={})", brightness(&sten), brightness(&none));

    // The two backends are independent implementations of the same spec — a bitmap
    // replay vs the hardware stencil — so on this fixture their interiors must
    // agree; only the mask edges may land on different pixels (the same 5% budget
    // `coverage.rs` allows). A backend that clips an instance differently — e.g.
    // one that lets a sub-animation's own masks miss its own parts, which
    // SpriteStudio 7.5 clips — blows far past this.
    let budget = drawn(&sten) / 20;
    assert!(differing <= budget, "coverage and stencil disagree on {differing} pixels (budget {budget}) — the instance mask is not treated the same on both backends");
}
