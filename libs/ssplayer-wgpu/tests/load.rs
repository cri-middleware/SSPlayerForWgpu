//! One call loads everything (SPEC §8.1, §7.1).
//!
//! `SSPlayer::load_with` is the walk both faces share — the js face plugs the
//! browser into it, `SSPlayer::load` plugs in the filesystem. Nothing in `web.rs`
//! is covered by a test, so this is where that walk is actually pinned: what the
//! browser contributes is fetching and decoding, and what is checked here is
//! everything else.
//!
//! Every assertion runs **after a frame**, because that is when a load's atlases
//! reach the GPU: `load` borrows no `Device` (SPEC §8.1), so the first
//! `SSScene::update` is what flushes them — the same timeline an embedded atlas
//! has always taken.
//!
//! The point of each assertion is that a *host* never had to know it: not that
//! there are two texture tables, not that a cellmap's image can also be named by
//! a TextureChange under a second hash, not that an instance part's pack is a
//! sibling file named after the pack. Getting any of them wrong is silent — a
//! white silhouette, or a part that simply does not appear.

use ssplayer_wgpu::{AssetLoader, MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

/// Run one frame, which is what moves a load's atlases onto the GPU.
fn flush(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, players: &mut [&mut SSPlayer]) {
    let mut scene = SSScene::new(device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(queue, 256.0, 256.0, false);
    scene.update(renderer, device, queue, 1.0 / 60.0, players);
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// Every atlas every registered pack asks for is on the renderer.
fn assert_ready(player: &SSPlayer, renderer: &SSRenderer, packs: &[&str]) {
    for name in packs {
        let refs = player.textures(Some(name)).unwrap_or_else(|| panic!("pack \"{name}\" is not registered"));
        assert!(!refs.is_empty(), "pack \"{name}\" asks for no atlas at all — the fixture would prove nothing");
        for texture in refs {
            // Through the player: `TextureRef::hash` names a slot inside this
            // pack, and what the renderer is keyed by is the image the player
            // resolved that slot to.
            let key = player.texture_key(Some(name), texture.slot);
            assert!(renderer.texture_size(key).is_some(), "pack \"{name}\" needs {} ({:#010x} -> {key:#010x}), which never reached the renderer", texture.path, texture.slot);
        }
    }
}

/// Reference mode: the pack, its sibling PNGs, and the sibling `.ssab` its
/// instance parts play from — none of which the caller named.
#[test]
fn one_call_loads_the_pack_its_atlases_and_the_packs_it_plays_from() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut player = SSPlayer::new();
    if player.load(format!("{}/overall/Instance.ssab", asset_root())).is_err() {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    }
    flush(&mut renderer, &device, &queue, &mut [&mut player]);

    // The name came out of the `.ssab`, not from the file stem the caller passed —
    // which is what makes `ref_anime_pack` resolve (SPEC §7.1).
    assert!(player.has_pack("Instance"), "the loaded pack did not keep its own name");
    // And the pack it plays from arrived without anyone asking for it.
    assert!(player.has_pack("Instance_SourceAnime"), "the referenced instance pack was not fetched; its parts would not appear");
    assert_ready(&player, &renderer, &["Instance", "Instance_SourceAnime"]);

    // It plays, which is the only claim that matters.
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();
    player.play();
    assert!(player.is_playing());
}

/// The same call, on the export mode with no sibling files at all: everything
/// comes out of the one `.ssab`, and the caller cannot tell the difference.
#[test]
fn an_embedded_pack_needs_no_sibling_files() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    // The fixture directory holds no images at all, so anything that reaches the
    // renderer came out of the `.ssab` itself. Asserted rather than assumed: if
    // `deploy-examples.sh` ever staged the atlases here too, this test would pass
    // by loading them and prove nothing.
    let images = std::fs::read_dir(format!("{}/overall_emb", asset_root())).into_iter().flatten().flatten().filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("png"))).count();
    assert_eq!(images, 0, "the embedded fixture directory has sibling images; this test would not prove they are unused");

    let mut renderer = SSRenderer::new(&device);
    let mut player = SSPlayer::new();
    if player.load(format!("{}/overall_emb/Instance.ssab", asset_root())).is_err() {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    }
    flush(&mut renderer, &device, &queue, &mut [&mut player]);
    assert!(player.has_pack("Instance_SourceAnime"));
    assert_ready(&player, &renderer, &["Instance", "Instance_SourceAnime"]);
}

/// TextureChange binds by texture *file name* hash, so `load` has to have read
/// `external_textures()` as well as the cellmaps (SPEC §8.1) — and to have read
/// one file once even though two hashes name it.
#[test]
fn one_call_covers_both_texture_tables() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);
    let mut player = SSPlayer::new();
    if player.load(format!("{}/overall/TextureChange.ssab", asset_root())).is_err() {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    }
    flush(&mut renderer, &device, &queue, &mut [&mut player]);
    assert_ready(&player, &renderer, &["TextureChange"]);

    // `common.png` is named twice, under a cellmap's hash and its own — two
    // slots, one file. Both slots have to be live (`assert_ready` above), and
    // both resolve to the one key, so the file is read once and uploaded once.
    let refs = player.textures(None).unwrap();
    let both: Vec<u32> = refs.iter().filter(|t| t.path == "common.png").map(|t| t.slot).collect();
    assert_eq!(both.len(), 2);
    assert_ne!(both[0], both[1], "two slots, or the fixture proves nothing");
    assert_eq!(player.texture_key(None, both[0]), player.texture_key(None, both[1]), "one file is one texture, whichever slot asks for it");

    let files: std::collections::HashSet<&str> = refs.iter().map(|t| t.path.as_str()).collect();
    assert_eq!(renderer.texture_upload_count(), files.len(), "one upload per file, not per slot");
}

/// A second player of the same pack uploads nothing: the texture map is
/// device-scoped, so the flush finds every hash already registered and drops what
/// the second load read (SPEC §8.2).
#[test]
fn a_second_player_of_one_pack_loads_nothing() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    let mut renderer = SSRenderer::new(&device);

    let mut first = SSPlayer::new();
    if first.load(format!("{}/overall/Instance.ssab", asset_root())).is_err() {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    }
    flush(&mut renderer, &device, &queue, &mut [&mut first]);
    let after_first = renderer.texture_upload_count();
    assert!(after_first > 0);

    let mut second = SSPlayer::new();
    second.load(format!("{}/overall/Instance.ssab", asset_root())).unwrap();
    flush(&mut renderer, &device, &queue, &mut [&mut second]);
    assert_eq!(renderer.texture_upload_count(), after_first, "the second player re-uploaded atlases the renderer already held");
    assert_ready(&second, &renderer, &["Instance", "Instance_SourceAnime"]);
}

/// Two projects that name their atlas alike are two atlases.
///
/// Adobe Animate writes `spritemap1.png` for every project it exports, so this is
/// the common case rather than a corner one, and a `.sspj` pipeline that converts
/// from elsewhere hits it on the second pack it shows. The bug it pins: a cellmap
/// hash names a slot inside one `.ssab`, the renderer's texture map is scoped to
/// the device, and keying the map by the hash alone made the second pack's atlas
/// look already-registered — its own pixels were dropped at the flush and its
/// parts drew the first pack's atlas.
#[test]
fn two_packs_that_name_one_atlas_alike_still_get_their_own() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    // One pack, staged twice under two directories, each beside a *different*
    // image with the same file name — which is exactly the shape two Animate
    // projects arrive in.
    let Ok(ssab) = std::fs::read(format!("{}/overall/Basic.ssab", asset_root())) else {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let root = std::env::temp_dir().join(format!("ssplayer-wgpu-atlas-clash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut staged = Vec::new();
    for (dir, image) in [("first", "common.png"), ("second", "box_00_00.png")] {
        let dir = root.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Basic.ssab"), &ssab).unwrap();
        // Both copies are named `common.png`; only one of them *is* it.
        std::fs::copy(format!("{}/overall/{image}", asset_root()), dir.join("common.png")).unwrap();
        staged.push(dir.join("Basic.ssab"));
    }

    // One renderer, because that is the whole point: the texture map is scoped to
    // the device and shared by every player on it.
    let mut renderer = SSRenderer::new(&device);
    let mut first = SSPlayer::new();
    let mut second = SSPlayer::new();
    first.load(&staged[0]).unwrap();
    second.load(&staged[1]).unwrap();
    flush(&mut renderer, &device, &queue, &mut [&mut first, &mut second]);

    let slot = first.textures(None).unwrap().iter().find(|t| t.path == "common.png").expect("the fixture stopped naming common.png").slot;
    let first_key = first.texture_key(None, slot);
    let second_key = second.texture_key(None, slot);
    assert_ne!(first_key, second_key, "one slot in two directories resolved to one key; the second pack is drawing the first's atlas");

    // And each is the image that was actually beside it.
    assert_eq!(renderer.texture_size(first_key), Some((256.0, 256.0)));
    assert_eq!(renderer.texture_size(second_key), Some((1024.0, 1024.0)));

    let _ = std::fs::remove_dir_all(&root);
}

/// Re-exporting over a pack and loading it again shows the new atlas.
///
/// The same file, so the same key — which is right, and is why the key alone
/// cannot answer this. What separates "somebody already uploaded exactly this"
/// from "this key holds a different picture now" is the digest the flush keeps
/// beside each player-loaded texture. Without it the viewer's convert / drop /
/// look / convert again round silently keeps showing the old atlas.
#[test]
fn re_loading_a_pack_whose_atlas_changed_picks_up_the_new_one() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    let Ok(ssab) = std::fs::read(format!("{}/overall/Basic.ssab", asset_root())) else {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let dir = std::env::temp_dir().join(format!("ssplayer-wgpu-atlas-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Basic.ssab"), &ssab).unwrap();
    std::fs::copy(format!("{}/overall/common.png", asset_root()), dir.join("common.png")).unwrap();

    let mut renderer = SSRenderer::new(&device);
    let mut player = SSPlayer::new();
    player.load(dir.join("Basic.ssab")).unwrap();
    flush(&mut renderer, &device, &queue, &mut [&mut player]);
    let slot = player.textures(None).unwrap().iter().find(|t| t.path == "common.png").expect("the fixture stopped naming common.png").slot;
    let key = player.texture_key(None, slot);
    assert_eq!(renderer.texture_size(key), Some((256.0, 256.0)));

    // Convert again, drop again.
    std::fs::copy(format!("{}/overall/box_00_00.png", asset_root()), dir.join("common.png")).unwrap();
    let mut reloaded = SSPlayer::new();
    reloaded.load(dir.join("Basic.ssab")).unwrap();
    flush(&mut renderer, &device, &queue, &mut [&mut reloaded]);
    assert_eq!(reloaded.texture_key(None, slot), key, "the same file should still be the same key");
    assert_eq!(renderer.texture_size(key), Some((1024.0, 1024.0)), "the re-exported atlas was dropped as already-registered");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A loader that keys atlases by where they resolve to, like the two that ship.
struct Rooted(std::path::PathBuf, std::path::PathBuf);

impl AssetLoader for Rooted {
    fn reset_base(&mut self) {
        self.1.clone_from(&self.0);
    }
    fn set_base(&mut self, src: &str) {
        self.1 = self.0.join(src).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    }
    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        paths.iter().map(|path| std::fs::read(self.1.join(path)).ok()).collect()
    }
    fn texture_key(&self, path: &str, _cellmap_hash: u32) -> u32 {
        ssruntime::fnv1a_hash_str(&self.1.join(path).to_string_lossy())
    }
    async fn supply_texture(&mut self, _key: u32, _encoded: &[u8]) {}
    fn has_texture(&self, _key: u32) -> bool {
        false
    }
    fn warn(&self, message: &str) {
        eprintln!("{message}");
    }
}

/// Atlases that arrive *after* the tree was built still reach it.
///
/// `load_assets` is for a host that already holds the bytes — a drop handler, a
/// bundler, an archive — and it deliberately does **not** re-register the pack.
/// The context therefore survives, the next `set_animation` takes its reuse path,
/// and what keeps playing is the node built while the player still knew nothing
/// about where the atlases resolved to. Unless that node picks the keys up, its
/// batches ask for slots nothing registered and every part falls back to the 1x1
/// white texture — which only the pixels show, so only the pixels can assert it.
#[test]
fn keys_learned_after_the_tree_was_built_reach_the_tree() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    let dir = std::path::Path::new(asset_root()).join("overall");
    let Ok(bytes) = std::fs::read(dir.join("Basic.ssab")) else {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };

    let shoot = |late: bool| {
        let mut renderer = SSRenderer::new(&device);
        let mut player = SSPlayer::new();
        player.load_ssab("Basic", &bytes).unwrap();
        let anim = player.animation_names().unwrap()[0].clone();
        if late {
            // The tree is built here, before anything knows about atlases.
            player.set_animation(&anim).unwrap();
        }
        let mut loader = Rooted(dir.clone(), dir.clone());
        pollster::block_on(player.load_assets(&mut loader, "Basic"));
        // `load_assets` supplies nothing itself, so the pixels come from here —
        // which is also what a host with its own asset system does.
        for texture in player.textures(None).unwrap().to_vec() {
            let img = image::open(dir.join(&texture.path)).unwrap().to_rgba8();
            let (w, h) = img.dimensions();
            renderer.upload_texture(&device, &queue, player.texture_key(None, texture.slot), &img, (w, h), ssplayer_wgpu::Retention::Retain);
        }
        if !late {
            player.set_animation(&anim).unwrap();
        }
        player.play();
        read_back(&device, &queue, &mut renderer, &mut player)
    };

    let late = shoot(true);
    let fresh = shoot(false);
    let drawn = |px: &[u8]| px.chunks_exact(4).filter(|p| p[0] != 0 || p[1] != 0 || p[2] != 0).count();
    assert!(drawn(&fresh) > 0, "the fixture drew nothing at all; this test would prove nothing");
    let differing = late.chunks_exact(4).zip(fresh.chunks_exact(4)).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "the tree built before the atlases arrived drew differently in {differing} px — it kept the keys it was built with, and its parts fell back to the white texture");
}

/// Render one frame at a fixed size and hand back the pixels.
fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut SSRenderer, player: &mut SSPlayer) -> Vec<u8> {
    const W: u32 = 128;
    let mut scene = SSScene::new(device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
    let (cw, ch) = player.canvas_size().unwrap_or((W as f32, W as f32));
    scene.set_viewport(queue, cw.max(ch), cw.max(ch), false);
    scene.update(renderer, device, queue, 1.0 / 60.0, &mut [player]);

    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: W, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (W * W * 4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store }, depth_slice: None })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        scene.draw(renderer, &mut pass);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(W) } },
        wgpu::Extent3d { width: W, height: W, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    rx.recv().unwrap().unwrap();
    let pixels = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    pixels
}
