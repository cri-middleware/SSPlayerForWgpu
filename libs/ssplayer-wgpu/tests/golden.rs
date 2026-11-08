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
//! rather than a checked-in golden set. CI's `golden` job runs it that way on
//! every pull request, against the base's rendering on the same runner.
//!
//! ```text
//! # before the change
//! SS_GOLDEN_DIR=/tmp/before cargo test --test golden -- --ignored
//! # after
//! SS_GOLDEN_DIR=/tmp/after SS_GOLDEN_COMPARE=/tmp/before \
//!     cargo test --test golden -- --ignored
//! ```
//!
//! A comparison also writes `summary.md` into the dump directory, and a
//! `diff/<image>.png` for each image that moved: before, after, and where they
//! differ, for a reviewer to look at rather than count.

use ssplayer_wgpu::{MaskBackend, Retention, SSPlayer, SSRenderer, SSScene, SceneConfig};

const W: u32 = 256;
const H: u32 = 256;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
}

/// Upload every reference-mode atlas `packs` ask for, under the key `player`
/// draws each with — a host supplying its own pixels.
fn load_textures(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, player: &SSPlayer, packs: &[&str]) {
    for &pack in packs {
        for texture in player.textures(Some(pack)).unwrap_or(&[]) {
            if texture.embedded {
                continue;
            }
            let img = image::open(std::path::Path::new(asset_dir()).join(&texture.path)).unwrap().to_rgba8();
            let (w, h) = img.dimensions();
            renderer.upload_texture(device, queue, player.texture_key(Some(pack), texture.slot), &img, (w, h), Retention::Retain);
        }
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
    /// Every image this run recorded, to find the ones only the reference has.
    names: std::collections::HashSet<String>,
    /// `(name, differing pixels, largest channel difference)` for every image
    /// that moved: a difference of 1 is a rounding drift, not a moved part.
    changed: Vec<(String, usize, u8)>,
    /// References the current run produced but the reference dump does not have.
    unmatched: Vec<String>,
}

impl Golden {
    fn record(&mut self, name: &str, rgba: &[u8]) {
        std::fs::write(format!("{}/{name}.raw", self.out), rgba).unwrap();
        self.written += 1;
        self.names.insert(name.to_string());
        let Some(reference) = &self.reference else { return };
        match std::fs::read(format!("{reference}/{name}.raw")) {
            Ok(before) if before.len() == rgba.len() => {
                let moved: Vec<u8> = before.as_chunks::<4>().0.iter().zip(rgba.as_chunks::<4>().0.iter()).filter(|(a, b)| a != b).map(|(a, b)| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)).collect();
                if let Some(&largest) = moved.iter().max() {
                    self.changed.push((name.to_string(), moved.len(), largest));
                    std::fs::create_dir_all(format!("{}/diff", self.out)).unwrap();
                    triptych(&format!("{}/diff/{name}.png", self.out), &before, rgba);
                }
            }
            _ => self.unmatched.push(name.to_string()),
        }
    }

    /// The reference's images this run did not produce: the corpus shrank.
    fn dropped(&self, reference: &str) -> Vec<String> {
        let mut dropped: Vec<String> = std::fs::read_dir(reference).unwrap().flatten().filter_map(|entry| entry.path().file_name()?.to_str()?.strip_suffix(".raw").map(str::to_string)).filter(|name| !self.names.contains(name)).collect();
        dropped.sort();
        dropped
    }
}

/// Before, after, and where they differ, side by side. The two renderings lose
/// their alpha so a viewer shows what was drawn over the opaque clear; the
/// third panel is the after image dimmed, with every differing pixel — alpha
/// included — in magenta.
fn triptych(path: &str, before: &[u8], after: &[u8]) {
    const GAP: u32 = 4;
    let mut image = image::RgbaImage::from_pixel(3 * W + 2 * GAP, H, image::Rgba([64, 64, 64, 255]));
    let opaque = |p: &[u8]| image::Rgba([p[0], p[1], p[2], 255]);
    for (i, (b, a)) in before.as_chunks::<4>().0.iter().zip(after.as_chunks::<4>().0.iter()).enumerate() {
        let (x, y) = (i as u32 % W, i as u32 / W);
        image.put_pixel(x, y, opaque(b));
        image.put_pixel(W + GAP + x, y, opaque(a));
        let diff = if a == b { image::Rgba([a[0] / 4, a[1] / 4, a[2] / 4, 255]) } else { image::Rgba([255, 0, 255, 255]) };
        image.put_pixel(2 * (W + GAP) + x, y, diff);
    }
    image.save(path).unwrap();
}

#[test]
#[ignore = "whole-corpus before/after comparison; needs SS_GOLDEN_DIR (see module docs)"]
fn golden_corpus() {
    let out = std::env::var("SS_GOLDEN_DIR").expect("set SS_GOLDEN_DIR to the directory to dump into");
    std::fs::create_dir_all(&out).unwrap();
    // A reused dump directory must not keep the triptychs of an earlier run.
    let _ = std::fs::remove_dir_all(format!("{out}/diff"));
    let _ = std::fs::remove_file(format!("{out}/summary.md"));
    let mut golden = Golden { out, reference: std::env::var("SS_GOLDEN_COMPARE").ok(), written: 0, names: Default::default(), changed: Vec::new(), unmatched: Vec::new() };
    let out = golden.out.clone();

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

    let mut packs: Vec<String> = std::fs::read_dir(asset_dir()).unwrap().flatten().filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("ssab")).map(|e| e.path().file_stem().unwrap().to_str().unwrap().to_string()).collect();
    packs.sort();

    for stencil in [false, true] {
        for pack in &packs {
            let mut renderer = SSRenderer::new(&device);
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
            let names: Vec<&str> = packs.iter().map(String::as_str).collect();
            load_textures(&mut renderer, &device, &queue, &player, &names);
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
        load_textures(&mut renderer, &device, &queue, &p1, &[a]);
        load_textures(&mut renderer, &device, &queue, &p2, &[b]);
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

    let dropped = golden.dropped(&reference);
    let mut summary = format!("## Golden images\n\n{} images against `{reference}`: {} changed, {} new, {} gone.\n", golden.written, golden.changed.len(), golden.unmatched.len(), dropped.len());
    if !golden.changed.is_empty() {
        summary += "\nEach changed image has a before | after | difference triptych in `diff/`.\n\n| Image | Pixels changed | Largest channel difference |\n|---|---:|---:|\n";
        for (name, differing, largest) in &golden.changed {
            eprintln!("  CHANGED {differing:6} px (largest difference {largest:3})  {name}");
            summary += &format!("| `{name}` | {differing} | {largest} |\n");
        }
    }
    for (heading, names) in [("New, with no counterpart in the reference", &golden.unmatched), ("Gone, in the reference only", &dropped)] {
        if names.is_empty() {
            continue;
        }
        summary += &format!("\n{heading}:\n\n");
        for name in names {
            eprintln!("  {heading}: {name}");
            summary += &format!("- `{name}`\n");
        }
    }
    std::fs::write(format!("{out}/summary.md"), summary).unwrap();
    assert!(golden.unmatched.is_empty() && dropped.is_empty(), "{} image(s) are new and {} are gone against {reference} — the two runs did not cover the same corpus", golden.unmatched.len(), dropped.len());
    assert!(golden.changed.is_empty(), "{} of {} images differ from {reference}; the triptychs are in {out}/diff", golden.changed.len(), golden.written);
    eprintln!("all {} images match {reference}", golden.written);
}
