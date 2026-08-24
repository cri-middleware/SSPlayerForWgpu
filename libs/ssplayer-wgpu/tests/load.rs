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

use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

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
            assert!(renderer.texture_size(texture.hash).is_some(), "pack \"{name}\" needs {} ({:#010x}), which never reached the renderer", texture.path, texture.hash);
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

    // `common.png` is named twice, under a cellmap's hash and its own — both keys
    // have to be live, and the file is only worth reading once.
    let both: Vec<u32> = player.textures(None).unwrap().iter().filter(|t| t.path == "common.png").map(|t| t.hash).collect();
    assert_eq!(both.len(), 2);
    assert_eq!(renderer.texture_upload_count(), player.textures(None).unwrap().len(), "one upload per hash asked for, no more");
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
