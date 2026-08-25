//! Instance / effect **slot stepping** (SPEC §7.1, `ssruntime` docs
//! `40_components/10_instance.md` §Supporting Independent Playback).
//!
//! Whose clock moves and what gets drawn are different questions, and the player
//! answers them in two separate walks for a reason: the runtime drops `Hide`-ed
//! parts from `draw_order` / `draw_batches` entirely. A player that steps its
//! slots from the draw walk therefore stops stepping a slot the moment its part
//! is hidden — and an independently-played child, which runs on its own elapsed
//! time rather than the parent's frame, then freezes and resumes late by exactly
//! the time it spent hidden. The doc calls this out as the mistake to avoid, and
//! `Instance.ssab` ships `INDEP_MID_INFINITE_HIDE` — an independent infinite
//! child hidden through the middle of the parent's timeline — for the check.
//!
//! The player's other loop-edge rule — read the pulse from `Context::is_looped`
//! rather than inferring it from "the frame went backwards" — has **no test
//! here on purpose**: the two agree on every path today's public API can reach.
//! `set_speed_scale` cannot make the frame run backwards (the runtime clamps a
//! non-positive rate to 1.0) and neither reverse nor ping-pong direction is
//! exposed, so nothing distinguishes them until one of those lands. The reason
//! to read the pulse anyway is that it is the runtime's own answer, and the
//! inference silently becomes wrong the moment the frame can move backwards for
//! a reason other than a loop.
//!
//! Skips cleanly when no GPU adapter is available: stepping happens inside
//! `SSScene::update`, which borrows a device.

use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

const W: u32 = 256;
const H: u32 = 256;

fn asset_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall")
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue))
}

/// `Instance.ssab` with its referenced pack registered, playing `anime_1` — the
/// animation whose instance parts carry the authored hide windows.
fn instance_player() -> Option<SSPlayer> {
    let parent = std::fs::read(format!("{}/Instance.ssab", asset_dir())).ok()?;
    let source = std::fs::read(format!("{}/Instance_SourceAnime.ssab", asset_dir())).ok()?;
    let mut player = SSPlayer::new();
    player.add_ssab("Instance_SourceAnime", &source).ok()?;
    player.load_ssab("Instance", &parent).ok()?;
    player.set_animation("anime_1").ok()?;
    player.play();
    Some(player)
}

/// Part index of `name` in `Instance.ssab`, so the tests name the authored part
/// rather than hard-coding an index the fixture is free to renumber.
fn part_index(name: &str) -> Option<usize> {
    let bytes = std::fs::read(format!("{}/Instance.ssab", asset_dir())).ok()?;
    // SAFETY: the file is the converter's own output, loaded the same way the
    // player loads it.
    let ab = unsafe { ssab::ssab::ss::format::root_as_ss_anime_binary_unchecked(&bytes) };
    ab.parts().iter().position(|p| p.name() == name)
}

fn child_frame(player: &SSPlayer, part_idx: usize) -> f32 {
    player.instance_child_frames().iter().find(|&&(i, _)| i == part_idx).map(|&(_, f)| f).expect("no instance child at that part index")
}

/// Step `player` one tick at a time, sampling one child's frame after each.
fn sample_child_frames(player: &mut SSPlayer, part_idx: usize, ticks: usize) -> Option<Vec<f32>> {
    let (device, queue) = gpu()?;
    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: None, mask: MaskBackend::None });
    scene.set_viewport(&queue, W as f32, H as f32, false);

    let mut frames = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        scene.update(&mut renderer, &device, &queue, 1.0 / 60.0, &mut [player]);
        frames.push(child_frame(player, part_idx));
    }
    Some(frames)
}

/// The clock of an independently-played child is not the draw order's business:
/// it keeps advancing while the part is hidden, so the child is where it should
/// be when it comes back rather than resuming late.
#[test]
fn a_hidden_independent_child_keeps_its_clock() {
    let Some(mut player) = instance_player() else {
        eprintln!("Instance fixtures missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let Some(part_idx) = part_index("INDEP_MID_INFINITE_HIDE") else {
        eprintln!("the fixture no longer carries INDEP_MID_INFINITE_HIDE — this test would prove nothing. skipping.");
        return;
    };
    // The parent runs 0..60; the part is hidden for 51 of those frames.
    let Some(frames) = sample_child_frames(&mut player, part_idx, 61) else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };

    // Start measuring once the child is actually running: before its playback
    // key fires it sits still for legitimate reasons.
    let Some(start) = frames.windows(2).position(|w| w[0] != w[1]) else {
        panic!("the independent child never advanced at all — the fixture or the slot wiring changed");
    };

    // At 60 fps with a 1/60 s tick the child moves a frame per tick, so the only
    // repeats should be the wrap at the loop edge.
    let mut run = 1usize;
    let mut worst = (1usize, 0usize); // (length, tick where it ended)
    for (i, w) in frames[start..].windows(2).enumerate() {
        run = if w[0] == w[1] { run + 1 } else { 1 };
        if run > worst.0 {
            worst = (run, start + i + 1);
        }
    }
    assert!(worst.0 <= 2, "the independent child's frame stood still for {} ticks (up to tick {}): its clock stopped while the part was hidden, which is what stepping from the draw order does. frames={frames:?}", worst.0, worst.1);
}
