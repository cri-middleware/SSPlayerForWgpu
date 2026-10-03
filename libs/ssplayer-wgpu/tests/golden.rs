//! Whole-corpus regression net for rendering changes.
//!
//! The suite's other rendering tests assert on drawn-pixel counts and on
//! specific invariants; neither notices a part quietly moving by a pixel or a
//! colour drifting. This renders **every** fixture — each pack, its first few
//! animations, several frames, with and without a stencil buffer, plus a handful
//! of two-player arrangements — and compares the readback byte for byte against
//! a dump taken before the change.
//!
//! It is `#[ignore]`d and opt-in because the images are GPU- and driver-specific:
//! the reference has to come from the same machine, so it is a before/after tool
//! rather than a checked-in golden set.
//!
//! ```text
//! # before the change
//! SS_GOLDEN_DIR=/tmp/before cargo test --test golden -- --ignored
//! # after
//! SS_GOLDEN_DIR=/tmp/after SS_GOLDEN_COMPARE=/tmp/before \
//!     cargo test --test golden -- --ignored
//! ```
//!
//! This is what verified the P7-6 SoA passthrough: moving vertex expansion from
//! the CPU to the vertex stage left all 134 images bit-identical.

use ssplayer_wgpu::{MaskBackend, Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};

const W: u32 = 256;
const H: u32 = 256;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
}

fn load_textures(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
    for entry in std::fs::read_dir(asset_dir()).unwrap().flatten() {
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

fn readback(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &SSRenderer, scene: &SSScene, stencil: bool) -> Vec<u8> {
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let ds = stencil.then(|| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
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
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (W * H * 4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
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
            depth_stencil_attachment: ds.as_ref().map(|view| wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
            }),
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

/// Write `rgba` to the dump directory, and — when a reference dump was named —
/// record whether it still matches.
struct Golden {
    out: String,
    reference: Option<String>,
    written: usize,
    /// `(name, differing pixels)` for every image that moved.
    changed: Vec<(String, usize)>,
    /// References the current run produced but the reference dump does not have.
    unmatched: Vec<String>,
}

impl Golden {
    fn record(&mut self, name: &str, rgba: &[u8]) {
        std::fs::write(format!("{}/{name}.raw", self.out), rgba).unwrap();
        self.written += 1;
        let Some(reference) = &self.reference else { return };
        match std::fs::read(format!("{reference}/{name}.raw")) {
            Ok(before) if before.len() == rgba.len() => {
                let differing = before.as_chunks::<4>().0.iter().zip(rgba.as_chunks::<4>().0.iter()).filter(|(a, b)| a != b).count();
                if differing > 0 {
                    self.changed.push((name.to_string(), differing));
                }
            }
            _ => self.unmatched.push(name.to_string()),
        }
    }
}

#[test]
#[ignore = "whole-corpus before/after comparison; needs SS_GOLDEN_DIR (see module docs)"]
fn golden_corpus() {
    let out = std::env::var("SS_GOLDEN_DIR").expect("set SS_GOLDEN_DIR to the directory to dump into");
    std::fs::create_dir_all(&out).unwrap();
    let mut golden = Golden { out, reference: std::env::var("SS_GOLDEN_COMPARE").ok(), written: 0, changed: Vec::new(), unmatched: Vec::new() };
    let out = golden.out.clone();

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

    let mut packs: Vec<String> = std::fs::read_dir(asset_dir()).unwrap().flatten().filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("ssab")).map(|e| e.path().file_stem().unwrap().to_str().unwrap().to_string()).collect();
    packs.sort();

    for stencil in [false, true] {
        for pack in &packs {
            let mut renderer = SSRenderer::new(&device);
            load_textures(&mut renderer, &device, &queue);
            let mut scene = SSScene::new(&device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: stencil.then_some(STENCIL_FORMAT), mask: if stencil { MaskBackend::Stencil } else { MaskBackend::None } });
            scene.set_viewport(&queue, W as f32, H as f32, false);

            let mut player = SSPlayer::new();
            // Register every pack so external instance references resolve.
            for p in &packs {
                let bytes = std::fs::read(format!("{}/{p}.ssab", asset_dir())).unwrap();
                let _ = player.add_ssab(p, &bytes);
            }
            let bytes = std::fs::read(format!("{}/{pack}.ssab", asset_dir())).unwrap();
            if player.load_ssab(pack, &bytes).is_err() {
                continue;
            }
            let Some(anims) = player.animation_names() else { continue };

            for anim in anims.iter().take(4) {
                if player.set_animation(anim).is_err() {
                    continue;
                }
                player.play();
                for frame in 1..=24 {
                    scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut player]);
                    if frame % 8 != 0 {
                        continue;
                    }
                    let rgba = readback(&device, &queue, &renderer, &scene, stencil);
                    let name = format!("{pack}__{anim}__f{frame}__s{}", stencil as u8).replace(['/', ' '], "_");
                    golden.record(&name, &rgba);
                }
            }
        }
    }

    // Multi-player arrangements: the interactions the single-player dumps miss.
    for (label, a, b) in [("mask_mask", "Mask", "Mask"), ("mask_basic", "Mask", "Basic"), ("shape_effect", "Shape", "Effect"), ("shader_mesh", "Shader", "Mesh")] {
        let mut renderer = SSRenderer::new(&device);
        load_textures(&mut renderer, &device, &queue);
        let mut scene = SSScene::new(&device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: Some(STENCIL_FORMAT), mask: MaskBackend::Stencil });
        scene.set_viewport(&queue, W as f32, H as f32, false);

        let make = |pack: &str, x: f32| {
            let mut p = SSPlayer::new();
            let bytes = std::fs::read(format!("{}/{pack}.ssab", asset_dir())).unwrap();
            p.load_ssab(pack, &bytes).unwrap();
            let anim = p.animation_names().unwrap()[0].clone();
            p.set_animation(&anim).unwrap();
            p.play();
            p.set_transform(x, 0.0, 0.8, 0.8, 0.0);
            p
        };
        let (mut p1, mut p2) = (make(a, -30.0), make(b, 30.0));
        p1.set_order(0);
        p2.set_order(1);
        for frame in 1..=16 {
            scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [&mut p1, &mut p2]);
            if frame % 8 != 0 {
                continue;
            }
            let rgba = readback(&device, &queue, &renderer, &scene, true);
            golden.record(&format!("pair_{label}__f{frame}"), &rgba);
        }
    }

    eprintln!("wrote {} images to {out}", golden.written);
    let Some(reference) = golden.reference.clone() else { return };

    for (name, differing) in &golden.changed {
        eprintln!("  CHANGED {differing:6} px  {name}");
    }
    for name in &golden.unmatched {
        eprintln!("  MISSING from the reference dump: {name}");
    }
    assert!(golden.unmatched.is_empty(), "{} image(s) had no counterpart in {reference} — the two runs did not cover the same corpus", golden.unmatched.len());
    assert!(golden.changed.is_empty(), "{} of {} images differ from {reference}", golden.changed.len(), golden.written);
    eprintln!("all {} images match {reference}", golden.written);
}
