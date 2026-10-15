//! The playback hot path allocates nothing of its own.
//!
//! A frame is `SSScene::update`, `encode_mask_coverage` and `draw`, and once
//! every buffer has grown to what the animations need, none of them may
//! allocate: `FrameChunk::reset` keeps capacity, GPU buffers only grow, and a
//! per-frame buffer lives on the chunk or the player's walk. Nothing else
//! checks that, and an allocation in the walk costs nothing a test of pixels
//! can see.
//!
//! wgpu itself allocates on every frame — a `write_buffer` stages through a
//! fresh buffer, and a render pass records into vectors — so a count alone
//! cannot tell the two apart. Each allocation made on this thread inside a frame
//! is attributed instead, by its backtrace: to wgpu when the innermost frame of
//! either crate is wgpu's, to the player when it is the player's. Only the
//! player's fail the test.
//!
//! Skips itself when no GPU adapter is available, like the rendering tests.

use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;

/// Counts nothing until a thread arms it, so the setup — and every other
/// thread, driver threads included — is left alone.
struct Attributing;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/// What the armed allocations were attributed to.
#[derive(Default)]
struct Seen {
    /// The backtrace of each allocation the player made.
    player: Vec<String>,
    /// How many wgpu made.
    wgpu: usize,
    /// The backtrace of each one that named neither crate.
    unattributed: Vec<String>,
}

static SEEN: Mutex<Seen> = Mutex::new(Seen { player: Vec::new(), wgpu: 0, unattributed: Vec::new() });

fn note() {
    if !ARMED.try_with(Cell::get).unwrap_or(false) {
        return;
    }
    // Disarmed while the backtrace is taken and resolved, which allocates too.
    ARMED.with(|armed| armed.set(false));
    let trace = std::backtrace::Backtrace::force_capture().to_string();
    let mut seen = SEEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match owner(&trace) {
        Some(Owner::Wgpu) => seen.wgpu += 1,
        Some(Owner::Player) => seen.player.push(trace),
        None => seen.unattributed.push(trace),
    }
    drop(seen);
    ARMED.with(|armed| armed.set(true));
}

enum Owner {
    Wgpu,
    Player,
}

/// Whose allocation this is: the innermost frame that is either wgpu's or the
/// player's decides. `None` when the trace names neither, which is what a build
/// without symbols gives for every one.
fn owner(trace: &str) -> Option<Owner> {
    for line in trace.lines().map(str::trim) {
        // Frames read `N: path::to::fn`; the `at file:line` lines between them do not.
        let Some((_, symbol)) = line.split_once(": ") else { continue };
        let symbol = symbol.trim_start_matches('<');
        if ["wgpu::", "wgpu_core::", "wgpu_hal::", "wgpu_types::", "naga::"].iter().any(|krate| symbol.starts_with(krate)) {
            return Some(Owner::Wgpu);
        }
        if symbol.starts_with("ssplayer_wgpu::") {
            return Some(Owner::Player);
        }
    }
    None
}

unsafe impl GlobalAlloc for Attributing {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Attributing = Attributing;

fn armed<R>(f: impl FnOnce() -> R) -> R {
    ARMED.with(|armed| armed.set(true));
    let out = f();
    ARMED.with(|armed| armed.set(false));
    out
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// One of each kind of work the walk does: plain parts, instance children,
/// effect particles, masks, meshes, and a character with all of it at once.
const PACKS: [&str; 6] = ["overall/Basic.ssab", "overall/Instance.ssab", "overall/Effect.ssab", "overall/Mask.ssab", "overall/Mesh.ssab", "Ringo/Ringo.ssab"];

const SIZE: u32 = 256;
const STENCIL: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
/// Long enough for every pack's animation to have looped at least once, so each
/// buffer has grown to the most any frame of it asks for.
const WARM_UP: usize = 600;
const MEASURED: usize = 120;

fn target(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// Play the cast for `WARM_UP + MEASURED` frames under `mask`, arming the
/// allocator around the three calls of each measured frame.
fn play(device: &wgpu::Device, queue: &wgpu::Queue, mask: MaskBackend) {
    let stencil = mask == MaskBackend::Stencil;
    let mut renderer = SSRenderer::new(device);
    let mut scene = SSScene::new(device, &SceneConfig { color_format: wgpu::TextureFormat::Rgba8Unorm, stencil_format: stencil.then_some(STENCIL), mask });
    scene.set_viewport(queue, SIZE as f32, SIZE as f32, false);
    let mut players: Vec<SSPlayer> = PACKS
        .iter()
        .map(|pack| {
            let mut player = SSPlayer::new();
            player.load(format!("{}/../../assets/{pack}", env!("CARGO_MANIFEST_DIR"))).expect("the fixture did not load; run scripts/deploy-examples.sh");
            let name = player.animation_names().and_then(|names| names.first().cloned()).expect("the fixture has no animation");
            player.set_animation(&name).expect("the fixture's first animation did not set up");
            player.set_loop_count(-1);
            player.play();
            player
        })
        .collect();
    let color = target(device, wgpu::TextureFormat::Rgba8Unorm);
    let depth = stencil.then(|| target(device, STENCIL));

    for frame in 0..WARM_UP + MEASURED {
        let measured = frame >= WARM_UP;
        let mut cast: Vec<&mut SSPlayer> = players.iter_mut().collect();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let run = |scene: &mut SSScene, renderer: &mut SSRenderer, cast: &mut [&mut SSPlayer], encoder: &mut wgpu::CommandEncoder| {
            scene.update(renderer, device, queue, 1.0 / 60.0, cast);
            scene.encode_mask_coverage(renderer, device, encoder);
        };
        if measured {
            armed(|| run(&mut scene, &mut renderer, &mut cast, &mut encoder));
        } else {
            run(&mut scene, &mut renderer, &mut cast, &mut encoder);
        }
        drop(cast);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &color, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store }, depth_slice: None })],
                depth_stencil_attachment: depth.as_ref().map(|view| wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if measured {
                armed(|| scene.draw(&renderer, &mut pass));
            } else {
                scene.draw(&renderer, &mut pass);
            }
        }
        queue.submit([encoder.finish()]);
        // Retire the frame's staging buffers, as a host's present would.
        let _ = device.poll(wgpu::PollType::Poll);
    }
}

#[test]
fn a_frame_allocates_nothing_of_the_players_own() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter available, skipping");
        return;
    };
    for mask in [MaskBackend::None, MaskBackend::Stencil, MaskBackend::Coverage] {
        play(&device, &queue, mask);
        let Seen { player, wgpu, unattributed } = std::mem::take(&mut *SEEN.lock().unwrap());
        // An allocation nobody can be blamed for is not a pass: it is a
        // backtrace without symbols, which would hide the player's too.
        assert!(unattributed.is_empty(), "{mask:?}: {} allocation(s) could not be attributed — does this build carry symbols? The first:\n{}", unattributed.len(), unattributed[0]);
        // Without this the attribution could be silently classifying nothing:
        // wgpu allocates on every frame, so it must have been seen to.
        assert!(wgpu > 0, "{mask:?}: no allocation was attributed to wgpu, so the allocator was never armed");
        assert!(player.is_empty(), "{mask:?}: {} allocation(s) over {MEASURED} steady-state frames came from the player, not wgpu. The first:\n{}", player.len(), player[0]);
    }
}
