//! Shared packs and the borrowed texture channel.
//!
//! Two claims are worth a GPU here.
//!
//! * **A pack is shared, not copied.** Twenty of the same character must cost one
//!   parse, one PNG decode and one atlas upload. `texture_count` cannot show that
//!   — N overwrites of one hash look identical to one upload — so
//!   [`SSRenderer::texture_upload_count`] is what these assert on.
//! * **A texture can be borrowed rather than owned.** `set_texture` takes a view
//!   the supplier keeps, which is how an engine's asset system stays the single
//!   owner. It has to produce the same pixels as `upload_texture`, and the
//!   [`AlphaMode`] that comes with it has to actually change how the fragment
//!   stage reads the texel — the mislabelled control below is what proves it does.
//!
//! Every test skips cleanly when no GPU adapter is available.

use ssplayer_wgpu::{AlphaMode, MaskBackend, Retention, SSPack, SSPlayer, SSRenderer, SSScene, SceneConfig};

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
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// Straight-alpha RGBA -> premultiplied, the same rounding `upload_texture` uses.
fn premultiply(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.as_chunks_mut::<4>().0 {
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

/// The key a player registering `pack` from its bytes draws `slot` under.
fn texture_key(pack: &SSPack, slot: u32) -> u32 {
    let mut player = SSPlayer::new();
    player.add_pack(pack);
    player.texture_key(Some(pack.name()), slot)
}

/// The atlases `pack` asks for, loaded the way a host is meant to load them:
/// the `.ssab` names each image and where it sits relative to itself, and the
/// player says which key it draws each under.
fn atlases(pack: &SSPack) -> Vec<(u32, Vec<u8>, u32, u32)> {
    let mut out = Vec::new();
    for texture in pack.textures() {
        if texture.embedded {
            continue;
        }
        let img = image::open(std::path::Path::new(asset_dir()).join(&texture.path)).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        out.push((texture_key(pack, texture.slot), img.into_raw(), w, h));
    }
    out
}

/// Create a texture the *test* owns, for handing to `set_texture`.
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
    render_to(device, queue, renderer, scene, players, frames, COLOR_FORMAT)
}

/// As [`render`], but into a target of `format` — the scene's pipelines are baked
/// against a colour format, so a second format is how a second *canvas* differs.
fn render_to(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut SSRenderer, scene: &mut SSScene, players: &mut [&mut SSPlayer], frames: u32, format: wgpu::TextureFormat) -> Vec<u8> {
    for _ in 0..frames {
        scene.update(renderer, device, queue, 1.0 / 60.0, players);
    }

    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen color"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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
    let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    data
}

fn drawn_pixels(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>().0.iter().filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8).count()
}

/// Mean absolute channel difference, in 0..255 units.
fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    sum as f64 / a.len() as f64
}

fn scene_for(device: &wgpu::Device, queue: &wgpu::Queue) -> SSScene {
    let mut scene = SSScene::new(device, &SceneConfig { color_format: COLOR_FORMAT, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(queue, W as f32, H as f32, false);
    scene
}

/// A pack names itself, so a host does not have to reconstruct the name that
/// instance parts reference. `Instance.ssab` points at
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

/// A pack names the atlases it needs **and where they are**, so
/// a host can load a reference-mode `.ssab` without knowing anything about the
/// directory it came from.
///
/// This is the same contract the sibling players expose (`ssplayer-core`'s
/// `extractTexturePaths`), and it is what makes a `.ssab` dropped onto a viewer
/// from an arbitrary folder playable: before it, hosts globbed for `*.png` and
/// derived the hash from the file stem, which only worked because the converter
/// happens to name its output after the cellmap.
///
/// The pair of exports also pins `embedded`, the flag that tells a host to load
/// nothing at all: the same animation, converted both ways, must agree on the
/// texture keys and disagree only on where the bytes live.
#[test]
fn a_pack_names_the_atlases_it_needs() {
    let Ok(bytes) = std::fs::read(format!("{}/Basic.ssab", asset_dir())) else {
        eprintln!("reference fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let reference = SSPack::load(&bytes).unwrap();
    assert!(!reference.has_embedded_textures(), "the fixture is not a reference-mode pack — this test would prove nothing");
    assert!(!reference.textures().is_empty(), "the pack asks for no textures — nothing to name");

    for texture in reference.textures() {
        assert!(!texture.embedded, "a reference-mode pack claims its atlas rides inside the .ssab");
        // Resolved against the file's own directory, exactly as a host would.
        let path = std::path::Path::new(asset_dir()).join(&texture.path);
        assert!(path.is_file(), "{path:?} does not exist — the path is not relative to the .ssab");
        let (width, height) = image::open(&path).unwrap().to_rgba8().dimensions();
        assert!(width > 0 && height > 0);
    }

    // The same list through a player, which is the route the C and js faces take
    // (`ss_player_texture_at` / `textureRefs`): a host that only holds a player
    // must be able to ask what the pack it just registered needs.
    let mut player = SSPlayer::new();
    player.load_ssab("Basic", &bytes).unwrap();
    let by_player: Vec<u32> = player.textures(None).unwrap().iter().map(|t| t.slot).collect();
    assert_eq!(by_player, reference.textures().iter().map(|t| t.slot).collect::<Vec<_>>(), "the main pack's textures did not come back through the player");
    assert_eq!(player.textures(Some("Basic")).unwrap().len(), by_player.len(), "naming the pack explicitly found a different list");
    assert!(player.textures(Some("nope")).is_none(), "an unregistered pack name reported textures");

    let Ok(emb_bytes) = std::fs::read(format!("{}/Basic.ssab", embedded_dir())) else {
        eprintln!("embedded fixture missing; run scripts/deploy-examples.sh. skipping the export-mode comparison.");
        return;
    };
    let embedded = SSPack::load(&emb_bytes).unwrap();
    let keys: Vec<u32> = reference.textures().iter().map(|t| t.slot).collect();
    let emb_keys: Vec<u32> = embedded.textures().iter().map(|t| t.slot).collect();
    assert_eq!(keys, emb_keys, "the two export modes disagree about the texture keys a draw batch will ask for");
    assert!(embedded.textures().iter().all(|t| t.embedded), "an embedded pack reported an atlas the host would have to load");
}

/// An embedded pack can hand out its atlases **without decoding them**,
/// which is what lets a host with its own decoder — the browser, via
/// `createImageBitmap` — support embedded packs while the player carries no
/// decoder at all (the `web` feature builds without `embedded` for exactly this).
///
/// Pinned natively because the face that relies on it has no test of its own:
/// nothing in `web.rs` is covered, so the invariant is nailed down on the side
/// that can be (the same reason `scenes_share_one_renderer` exists).
#[test]
fn an_embedded_pack_hands_out_its_atlases_undecoded() {
    let Ok(bytes) = std::fs::read(format!("{}/Basic.ssab", embedded_dir())) else {
        eprintln!("embedded fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let pack = SSPack::load(&bytes).unwrap();
    let images = pack.embedded_images();
    assert!(!images.is_empty(), "an embedded pack carried no images — this test would prove nothing");

    // The keys must be the ones a draw batch asks for (`name_hash`), not the ones
    // the blobs are stored under (`image_path_hash`) — swapping the two is the
    // classic way to break embedded loading.
    let undecoded: Vec<u32> = images.iter().map(|i| i.slot).collect();
    let decoded: Vec<u32> = pack.decode_embedded().iter().map(|t| t.slot).collect();
    assert_eq!(undecoded, decoded, "the encoded and decoded routes disagree about which atlas is which");

    for image in &images {
        // Encoded, not pixels: whoever takes these still has to decode them.
        assert_eq!(&image.bytes[..8], b"\x89PNG\r\n\x1a\n", "an embedded image is not the encoded file the exporter wrote");
        let decoded = image::load_from_memory(image.bytes).expect("the bytes handed out are not a decodable image");
        assert!(decoded.width() > 0 && decoded.height() > 0);
        assert!(image.bytes.len() < (decoded.width() * decoded.height() * 4) as usize, "the encoded image is no smaller than its pixels — retaining it for device loss would be pointless");
    }

    // Through a player, which is the route the js face takes (`embeddedTextures`).
    let mut player = SSPlayer::new();
    player.load_ssab("Basic", &bytes).unwrap();
    assert_eq!(player.embedded_images(None).unwrap().iter().map(|i| i.slot).collect::<Vec<_>>(), undecoded, "the main pack's embedded images did not come back through the player");
    assert!(player.embedded_images(Some("nope")).is_none(), "an unregistered pack name reported embedded images");

    // A reference-mode pack carries none: there is nothing to hand out, and the
    // host is told where to fetch instead (`textures()`).
    let Ok(ref_bytes) = std::fs::read(format!("{}/Basic.ssab", asset_dir())) else { return };
    assert!(SSPack::load(&ref_bytes).unwrap().embedded_images().is_empty(), "a reference-mode pack claims to carry its atlases");
}

/// A `DrawBatch.texture_hash` is a cellmap's `name_hash` for an
/// ordinary part, but a texture **file name**'s hash for one the author gave a
/// TextureChange (`TCHG`). Both `cellmaps()` and `external_textures()` have to be
/// named, or those batches find nothing.
///
/// Worth a test of its own because the failure is quiet: a missing hash falls
/// back to the 1x1 white texture, so the part still draws — as a white
/// silhouette — and no count, no error and no panic marks it.
#[test]
fn a_pack_names_its_texture_change_targets() {
    let Ok(bytes) = std::fs::read(format!("{}/TextureChange.ssab", asset_dir())) else {
        eprintln!("TextureChange fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let pack = SSPack::load(&bytes).unwrap();

    // The fixture is one cellmap plus three external textures, and one of those
    // three *is* the cellmap's own image — reachable under a second hash, because
    // an `ExternalTexture` is keyed by its file name rather than by a cellmap
    // name. A host fetching per entry therefore has to expect a repeated path.
    let common: Vec<u32> = pack.textures().iter().filter(|t| t.path == "common.png").map(|t| t.slot).collect();
    assert_eq!(common.len(), 2, "common.png should be named twice — as the cellmap's image and as a TextureChange target — got {:?}", pack.textures());
    assert_ne!(common[0], common[1], "the two entries for one file carry the same hash; one of the two tables was keyed wrongly");
    assert!(pack.textures().iter().any(|t| t.path == "box_00_00.png"), "an external texture the cellmaps never mention is missing — external_textures() is not being read");

    // Embedded export carries the external textures too, under those same keys.
    if let Ok(emb_bytes) = std::fs::read(format!("{}/TextureChange.ssab", embedded_dir())) {
        let emb = SSPack::load(&emb_bytes).unwrap();
        let carried: Vec<u32> = emb.embedded_images().iter().map(|i| i.slot).collect();
        for texture in emb.textures() {
            assert!(carried.contains(&texture.slot), "embedded pack does not carry {} ({:#010x}) — the TCHG target would render white", texture.path, texture.slot);
        }
    }

    // And the parts really do sample them: drop one external atlas and the frame
    // has to change. If `textures()` ever regresses to cellmaps only, this atlas
    // is not in either list and the two renders come out identical.
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping the render half");
        return;
    };
    let all = atlases(&pack);
    let box_hash = texture_key(&pack, pack.textures().iter().find(|t| t.path == "box_00_00.png").unwrap().slot);
    let minus_box: Vec<_> = all.iter().filter(|(hash, ..)| *hash != box_hash).cloned().collect();
    assert_eq!(minus_box.len() + 1, all.len(), "the atlas to drop was not in the supplied set");

    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene_for(&device, &queue);
    let mut player = play(&pack);
    for (hash, rgba, w, h) in &all {
        renderer.upload_texture(&device, &queue, *hash, rgba, (*w, *h), Retention::Retain);
    }
    let complete = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 12);

    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene_for(&device, &queue);
    let mut player = play(&pack);
    for (hash, rgba, w, h) in &minus_box {
        renderer.upload_texture(&device, &queue, *hash, rgba, (*w, *h), Retention::Retain);
    }
    let missing_one = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 12);

    assert_ne!(complete, missing_one, "dropping a TextureChange target changed nothing — the parts are not sampling it");
}

/// Sharing a pack means sharing its atlas. Two players of one embedded
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
    let atlas_count = pack.textures().len();
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

/// The renderer is scoped to the **device**, not to a render
/// target, and it is format-independent — so several canvases share one of it,
/// and one atlas upload covers all of them.
///
/// This is the claim the web face rests on. Its predecessor requested an
/// adapter and a device per canvas, which put every canvas on its own device;
/// nothing is shared across a device, so N canvases of the same character meant N
/// atlas uploads and N sets of pipelines. Two scenes here, deliberately with
/// **different colour formats** (a canvas takes whatever its browser prefers), to
/// pin down that the format lives in the scene's pipeline key rather than in the
/// renderer.
#[test]
fn scenes_share_one_renderer() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let path = format!("{}/Basic.ssab", embedded_dir());
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("embedded fixture missing ({path}); run scripts/deploy-examples.sh to generate it. skipping.");
        return;
    };

    const SECOND_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
    let pack = SSPack::load(&bytes).unwrap();
    let atlas_count = pack.textures().len();
    assert!(atlas_count > 0, "the pack asks for no textures — nothing to share");

    // One renderer for the whole "page".
    let mut renderer = SSRenderer::new(&device);

    let mut first_canvas = scene_for(&device, &queue);
    let mut first = play(&pack);
    let one = render(&device, &queue, &mut renderer, &mut first_canvas, &mut [&mut first], 10);
    assert!(drawn_pixels(&one) > 5_000, "the first canvas drew nothing — the embedded atlas never reached the GPU");
    assert_eq!(renderer.texture_upload_count(), atlas_count, "the first canvas did not upload exactly the pack's atlases");
    let after_one = renderer.pipeline_count();
    assert!(after_one > 0, "the first canvas compiled nothing — the comparison below would be vacuous");

    // A second canvas, in a different colour format, on the same renderer.
    let mut second_canvas = SSScene::new(&device, &SceneConfig { color_format: SECOND_FORMAT, stencil_format: None, mask: MaskBackend::None });
    second_canvas.set_viewport(&queue, W as f32, H as f32, false);
    let mut second = play(&pack);
    let two = render_to(&device, &queue, &mut renderer, &mut second_canvas, &mut [&mut second], 10, SECOND_FORMAT);
    assert!(drawn_pixels(&two) > 5_000, "the second canvas drew nothing — a renderer shared across formats is broken");

    // The expensive thing — the atlas — is not paid for twice. Pipelines are, and
    // must be: they are baked against a colour format.
    assert_eq!(renderer.texture_upload_count(), atlas_count, "the second canvas uploaded the atlas again — the renderer is not shared across targets");
    assert_eq!(renderer.texture_count(), atlas_count, "the texture map grew for the second canvas");
    assert!(renderer.pipeline_count() > after_one, "the second format compiled no pipeline of its own, so the two canvases cannot both be drawing correctly");

    // And the first canvas still works afterwards: the second one did not take
    // the renderer's state with it.
    let again = render(&device, &queue, &mut renderer, &mut first_canvas, &mut [&mut first], 1);
    assert!(drawn_pixels(&again) > 5_000, "the first canvas stopped drawing once a second one shared its renderer");
}

/// `set_texture` borrows a view the supplier owns and builds nothing
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
    let atlases = atlases(&pack);

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
    // 1x1 white fallback, so the frame must change.
    for (hash, ..) in &atlases {
        renderer.forget_texture(*hash);
    }
    assert_eq!(renderer.texture_count(), 0, "forget_texture left the registration behind");
    let mut player = play(&pack);
    let forgotten = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10);
    assert_ne!(forgotten, borrowed, "forgetting every texture changed nothing — they were not actually being sampled");
}

/// Engines hand over straight-alpha textures, so `AlphaMode` tells the
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
    let atlases: Vec<(u32, Vec<u8>, u32, u32)> = atlases(&pack).into_iter().map(|(hash, _, w, h)| (hash, alpha_ramp(w, h), w, h)).collect();

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

/// A pack hands over pixels; it never owns a GPU texture. The decode
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
    assert_eq!(decoded.len(), pack.textures().len(), "decode_embedded did not produce one image per cellmap");

    let mut renderer = SSRenderer::new(&device);
    // Before the views, because the key to register under is the player's answer
    // and it can only answer for a pack it holds.
    let mut player = play(&pack);
    let _views: Vec<wgpu::TextureView> = decoded
        .iter()
        .map(|tex| {
            let view = supplier_texture(&device, &queue, &premultiply(&tex.rgba), tex.width, tex.height);
            // Under the player's key, not the cellmap hash: the hash names a slot
            // inside this pack and two packs can share one, so what the renderer
            // is keyed by is the image the player resolved that slot to.
            renderer.set_texture(&device, player.texture_key(None, tex.slot), &view, (tex.width, tex.height), AlphaMode::Premultiplied);
            view
        })
        .collect();
    let mut scene = scene_for(&device, &queue);
    let by_supplier = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10);

    assert_eq!(renderer.texture_upload_count(), 0, "the player uploaded the embedded atlas even though it was already registered");
    assert_eq!(by_supplier, by_player, "routing the embedded pixels through set_texture rendered differently");
}

/// Re-registering a pack a live animation plays from must not pull the resource
/// out from under it.
///
/// `build_node` binds every `Context` to its pack's resource by **raw pointer**,
/// and the only `Arc` keeping that resource alive is the one in the player's pack
/// map, so `add_pack` / `add_ssab` must not free the resource the tree is still
/// evaluating — a use-after-free reachable from safe Rust and from
/// `ss_player_add_ssab` / `addSsab`. The pack that arrives is registered for the *next* tree; what is
/// playing keeps the data it was built from.
#[test]
fn re_registering_a_pack_does_not_pull_it_out_from_under_the_tree() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping headless render test");
        return;
    };
    let main_path = format!("{}/Instance.ssab", asset_dir());
    let sub_bytes = std::fs::read(format!("{}/Instance_SourceAnime.ssab", asset_dir())).unwrap();

    let mut renderer = SSRenderer::new(&device);
    let mut scene = scene_for(&device, &queue);

    // What eleven uninterrupted frames look like.
    let mut control = SSPlayer::new();
    control.load(&main_path).unwrap();
    let anim = control.animation_names().unwrap()[0].clone();
    control.set_animation(&anim).unwrap();
    control.play();
    let expected = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut control], 11);
    assert!(drawn_pixels(&expected) > 0, "the instance fixture drew nothing — fixture/texture problem");

    // The same eleven frames, with the sub-pack re-registered after the first.
    let mut player = SSPlayer::new();
    player.load(&main_path).unwrap();
    player.set_animation(&anim).unwrap();
    player.play();
    assert!(player.instance_node_count() > 0, "the fixture built no instance child, so nothing points into the sub-pack");
    let _ = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 1);

    player.add_ssab("Instance_SourceAnime", &sub_bytes).unwrap();
    let after = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 10);
    assert_eq!(after, expected, "playback changed (or read freed memory) when the pack it plays from was re-registered");

    // The replacement is what the next tree is built from, and building it is what
    // releases the retired one — so this both picks up the new pack and frees the
    // old. The bytes are identical, so the render must be too.
    player.set_animation(&anim).unwrap();
    player.play();
    let rebuilt = render(&device, &queue, &mut renderer, &mut scene, &mut [&mut player], 11);
    assert_eq!(rebuilt, expected, "the tree rebuilt from the re-registered pack does not match the original");
}
