// Native development viewer for `ssplayer-wgpu` (SPEC §13).
//
// The viewer is a host: it owns the whole wgpu stack (instance / device / queue
// / surface) and lends it to the borrow-model player each frame (SPEC §1).
//
// **It shows a cast, not a character.** One `SSRenderer` (device-scoped), one
// `SSScene` (this window), and N `SSPlayer`s built into a single render pass —
// which is what P7 rebuilt the drawing layer to allow. The multi-player faults
// that only the eye catches live here: one player's mask clipping the next (the
// `Stack` layout piles them on top of each other on purpose), a blend reading
// through a neighbour, and who ends up in front when `order` is reversed.
// `tests/scene.rs` pins those headless as pixel counts; this is where you look
// at them.
//
//   Space       pause / resume            +/-     add / remove a player
//   Left/Right  previous / next .ssab     Tab     cycle focus (all / #1 / #2 …)
//   Up/Down     previous / next animation V       toggle visibility
//   R           restart animation         [ / ]   alpha down / up
//   L           layout: Row / Stack       O       reverse draw order
//   F           toggle Y-axis flip        Escape  quit
//   drop        play a dropped .ssab, or every .ssab in a dropped folder
//
// The pack, animation, visibility and alpha keys act on the **focused** member;
// focus starts at "all", so they drive the whole cast until you press Tab.

use ssplayer_wgpu::{SSPack, SSPlayer, SSRenderer, SSScene, SceneConfig};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wgpu::{Device, Queue, Surface, SurfaceConfiguration};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes};

/// Depth/stencil format the host attaches so the player can rasterise masks
/// (SPEC §6). The depth aspect is unused (compare Always, no write).
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Cast size at startup. Two, because one is the case that worked before P7 —
/// starting there means a regression in player isolation is on screen from the
/// first frame rather than behind a keypress.
const INITIAL_CAST: usize = 2;
/// Most players `+` will go to. Throughput is `tests/scale.rs`'s job; this is
/// for looking at interference, and past a handful there is nothing to see.
const MAX_CAST: usize = 8;

/// Canvas size assumed for a member with no animation selected yet, so the
/// layout has something to fit.
const FALLBACK_CANVAS: (f32, f32) = (320.0, 320.0);

/// How the cast is arranged on screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Side by side, each scaled to its own slot. For reading one character.
    Row,
    /// Piled at the centre with a small stagger, so they overlap almost
    /// completely. Interference between players only shows where they overlap.
    Stack,
}

impl Layout {
    fn name(self) -> &'static str {
        match self {
            Layout::Row => "Row",
            Layout::Stack => "Stack (overlapping)",
        }
    }
}

/// One character in the scene: a player plus which pack and animation it is on.
///
/// A player holds no GPU resource at all (SPEC §1.2), so the cast is a plain
/// `Vec` and the whole of it is lent to the scene once per frame.
struct Member {
    player: SSPlayer,
    /// Index into [`AppState::packs`].
    pack_idx: usize,
    /// Index into [`AppState::pack_anims`]`[pack_idx]`.
    anim_idx: usize,
}

/// (Re)create the depth/stencil render target at the given size.
fn create_stencil_view(device: &Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SpriteStudio Stencil"),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: STENCIL_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The viewer is a host: it owns the whole wgpu stack (instance/device/queue/
/// surface) and drives the borrow-model players each frame.
struct AppState {
    window: Arc<Window>,
    device: Device,
    queue: Queue,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    /// Depth/stencil view attached to every render pass, resized with the window.
    stencil_view: wgpu::TextureView,

    /// Device-scoped and host-owned (SPEC §1.2): one renderer serves every
    /// player on this device, so putting more characters on screen costs no
    /// extra pipelines or atlas uploads.
    renderer: SSRenderer,
    /// One scene per render target; it owns the frame arena and draws every
    /// player into a single pass (SPEC §9.2).
    scene: SSScene,

    /// Every `.ssab` in the asset directory, parsed once. Members share them by
    /// `Arc` (SPEC §8.2), so a cast of N on one pack costs one parse and one
    /// atlas upload between them, not N.
    packs: Vec<SSPack>,
    /// Where each pack was read from. `SSPlayer::load` takes the path, not the
    /// bytes — it is what lets the player find the pack's atlases and the packs
    /// its instance parts play from (SPEC §8.1, §7.1).
    pack_paths: Vec<PathBuf>,
    /// Animation names per pack, in `packs` order.
    pack_anims: Vec<Vec<String>>,
    /// The characters in the scene, in the order they were added. Draw order is
    /// `set_order`, not this (see `layout_cast`).
    cast: Vec<Member>,
    /// Which member the pack / animation / appearance keys drive. `None` is
    /// "all of them", which keeps the single-character key bindings behaving as
    /// they always did.
    focus: Option<usize>,
    layout: Layout,
    /// Draw the cast back to front instead of front to back (SPEC §9.3). Only
    /// visible where they overlap, which is what `Layout::Stack` is for.
    reverse_order: bool,

    last_update: Instant,
    is_paused: bool,
    y_down: bool,
    logged_first_draw: bool,
    /// Wall time of the last `SSScene::update`, in milliseconds.
    last_update_ms: f64,
    /// When the status block was last drawn. It refreshes on a timer as well as
    /// on keypresses, so the frame counters on it stay live (SPEC §14.2 item 10
    /// — these numbers are the only evidence a change made the frame cheaper).
    last_status: Instant,
}

impl AppState {
    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.stencil_view = create_stencil_view(&self.device, new_size.width, new_size.height);
            // Placement is in projection units, which are the target's pixels.
            self.layout_cast();
        }
    }

    fn update(&mut self) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;

        if !self.is_paused {
            // The whole cast goes into one `update`, so one `begin_frame` /
            // `end_frame` pair spans it and the draws end up back to back in a
            // single pass (SPEC §9.2).
            let mut players: Vec<&mut SSPlayer> = self.cast.iter_mut().map(|m| &mut m.player).collect();
            // The CPU half of the frame. The scene deliberately does not time
            // itself (SPEC §14.2 item 10) — `Instant` is not portable to wasm,
            // and a host needs one line to do it.
            let started = Instant::now();
            self.scene.update(&mut self.renderer, &self.device, &self.queue, delta, &mut players);
            self.last_update_ms = started.elapsed().as_secs_f64() * 1.0e3;
        }
    }

    fn render(&mut self) -> Result<(), &'static str> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Lost => return Err("Lost"),
            wgpu::CurrentSurfaceTexture::Outdated => return Err("Outdated"),
            wgpu::CurrentSurfaceTexture::Timeout => return Err("Timeout"),
            wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            wgpu::CurrentSurfaceTexture::Validation => return Err("Validation"),
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Render Encoder") });

        // Match the projection to the current window size.
        let size = self.window.inner_size();
        self.scene.set_viewport(&self.queue, size.width as f32, size.height as f32, self.y_down);

        {
            // One pass for the whole cast. Clearing the stencil here would hide
            // a player failing to unwind its own mask coverage, which is the
            // regression `tests/scene.rs::stencil_unwind_isolates_players`
            // guards headless — but the eye catches it here first (SPEC §6).
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.08, g: 0.08, b: 0.12, a: 1.0 }), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.stencil_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: self.scene.timestamp_writes(),
                occlusion_query_set: None,
                multiview_mask: None,
            });

            let drawn = self.scene.draw(&self.renderer, &mut rpass);
            if drawn > 0 && !self.logged_first_draw {
                println!("[DIAG] First rendered frame with draw calls. Submitted: {}", drawn);
                self.logged_first_draw = true;
            }
        }
        self.scene.resolve_timestamps(&mut encoder);

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    // --- cast ------------------------------------------------------------

    /// Add one character to the scene, playing the same thing as the last one.
    fn add_member(&mut self) {
        if self.cast.len() >= MAX_CAST {
            return;
        }
        let (pack_idx, anim_idx) = self.cast.last().map(|m| (m.pack_idx, m.anim_idx)).unwrap_or((0, 0));
        self.spawn_member(pack_idx, anim_idx);
    }

    /// Build a player and put it on screen. It arrives empty: `apply_pack` below
    /// loads what it plays, and that call brings the pack's atlases and the packs
    /// its instance parts reference with it (SPEC §8.1, §7.1).
    fn spawn_member(&mut self, pack_idx: usize, anim_idx: usize) {
        self.cast.push(Member { player: SSPlayer::new(), pack_idx, anim_idx });

        let idx = self.cast.len() - 1;
        self.apply_pack(idx, pack_idx);
    }

    /// Drop the last character. One always stays, or there is nothing to look at.
    fn remove_member(&mut self) {
        if self.cast.len() <= 1 {
            return;
        }
        self.cast.pop();
        if self.focus.is_some_and(|i| i >= self.cast.len()) {
            self.focus = None;
        }
        self.layout_cast();
    }

    /// Play a `.ssab` dropped on the window, or every `.ssab` directly inside a
    /// dropped folder.
    ///
    /// **Atlases resolve against the directory the drop came from**, not against
    /// the viewer's asset directory: each pack names its images relative to
    /// itself (SPEC §8.2), so a pack from anywhere on disk brings its textures
    /// with it, embedded or not. Drop the folder rather than the file when the
    /// pack has external instance references — those resolve by pack name, so
    /// every referenced pack has to be registered too (SPEC §7.1).
    ///
    /// Dropped packs join the list Right/Left walks; a pack dropped twice
    /// replaces its own entry rather than growing the list, which is what makes
    /// "convert, drop, look, convert again" work.
    fn adopt_dropped(&mut self, path: &Path) {
        let is_ssab = |p: &Path| p.extension().and_then(|s| s.to_str()) == Some("ssab");
        let mut paths = Vec::new();
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                paths.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_ssab(p)));
            }
            paths.sort();
        } else if is_ssab(path) {
            paths.push(path.to_path_buf());
        }

        let mut first = None;
        for path in paths {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    eprintln!("Failed to read {:?}: {:?}", path, e);
                    continue;
                }
            };
            // Parsed for the picker only, and never renamed — the name inside the
            // `.ssab` is what instance references resolve against (SPEC §7.1).
            let pack = match SSPack::load(&bytes) {
                Ok(pack) => pack,
                Err(e) => {
                    eprintln!("Failed to parse {:?}: {:?}", path, e);
                    continue;
                }
            };
            println!("Dropped SSAB: {:?}", path);

            let idx = self.packs.iter().position(|p| p.name() == pack.name()).unwrap_or(self.packs.len());
            if idx == self.packs.len() {
                self.packs.push(pack);
                self.pack_anims.push(self.packs[idx].animation_names());
                self.pack_paths.push(path);
            } else {
                self.pack_anims[idx] = pack.animation_names();
                self.packs[idx] = pack;
                self.pack_paths[idx] = path;
            }
            first.get_or_insert(idx);
        }

        // Show the first of them, on whoever the pack keys drive.
        let Some(idx) = first else { return };
        for member_idx in self.targets() {
            self.apply_pack(member_idx, idx);
        }
        self.refresh_status();
    }

    /// The members the pack / animation / appearance keys act on.
    fn targets(&self) -> Vec<usize> {
        match self.focus {
            Some(i) if i < self.cast.len() => vec![i],
            _ => (0..self.cast.len()).collect(),
        }
    }

    /// Point one member at a pack and start playing from it.
    fn apply_pack(&mut self, idx: usize, pack_idx: usize) {
        if pack_idx >= self.packs.len() {
            return;
        }
        {
            let path = self.pack_paths[pack_idx].clone();
            let member = &mut self.cast[idx];
            member.pack_idx = pack_idx;
            // One call: the pack, its atlases (either export mode) and the packs
            // its instance parts play from. `load` clears the selection —
            // selecting an animation and starting it are separate calls
            // (SPEC §9.3) — so `apply_anim` below is not optional. Atlases the
            // renderer already holds are dropped at the flush, so switching packs
            // on a cast of eight does not re-upload anything.
            if let Err(e) = member.player.load(&path) {
                eprintln!("Failed to load {:?}: {:?}", path, e);
                return;
            }
        }
        let anim_idx = self.cast[idx].anim_idx.min(self.pack_anims[pack_idx].len().saturating_sub(1));
        self.apply_anim(idx, anim_idx);
        // Packs differ in canvas size, so the fit has to be recomputed.
        self.layout_cast();
    }

    /// Select and play an animation on one member. `anim_idx` wraps, so callers
    /// can hand it `current + 1` without knowing how many there are.
    fn apply_anim(&mut self, idx: usize, anim_idx: usize) {
        let pack_idx = self.cast[idx].pack_idx;
        let names = self.pack_anims.get(pack_idx).map(Vec::as_slice).unwrap_or(&[]);
        if names.is_empty() {
            return;
        }
        let anim_idx = anim_idx % names.len();
        let name = names[anim_idx].clone();

        let member = &mut self.cast[idx];
        member.anim_idx = anim_idx;
        if let Err(e) = member.player.set_animation(&name) {
            eprintln!("Failed to select animation {name:?}: {e:?}");
            return;
        }
        member.player.play();
    }

    /// Place the cast and stack it front to back.
    ///
    /// Placement is per player (SPEC §5.4) — the projection belongs to the
    /// scene — so this is only a root matrix per member, and re-running it every
    /// time the window or the cast changes costs nothing.
    fn layout_cast(&mut self) {
        let count = self.cast.len();
        if count == 0 {
            return;
        }
        let (width, height) = (self.config.width as f32, self.config.height as f32);
        let (layout, reverse) = (self.layout, self.reverse_order);
        let last = (count - 1) as f32;

        for (i, member) in self.cast.iter_mut().enumerate() {
            // Fit each member to the space it has: the demo packs disagree about
            // canvas size, and showing them all at authored scale puts some of
            // them off-screen.
            let (cw, ch) = member.player.canvas_size().unwrap_or(FALLBACK_CANVAS);
            let (cw, ch) = (cw.max(1.0), ch.max(1.0));
            let offset = i as f32 - last * 0.5;

            let (x, y, scale) = match layout {
                Layout::Row => {
                    let slot = width / count as f32;
                    (offset * slot, 0.0, (slot * 0.9 / cw).min(height * 0.9 / ch).clamp(0.05, 1.0))
                }
                Layout::Stack => {
                    // Enough stagger to tell them apart, little enough that they
                    // still cover each other.
                    let step = (width * 0.04).min(48.0);
                    (offset * step, -offset * step, (width * 0.5 / cw).min(height * 0.8 / ch).clamp(0.05, 1.5))
                }
            };
            member.player.set_transform(x, y, scale, scale, 0.0);
            member.player.set_order(if reverse { (count - 1 - i) as i32 } else { i as i32 });
        }
    }

    // --- status ----------------------------------------------------------

    fn refresh_status(&mut self) {
        self.last_status = Instant::now();
        self.print_status();
    }

    fn print_status(&self) {
        // Clear screen using ANSI escape code.
        print!("\x1b[2J\x1b[H");
        println!("====================================================");
        println!("         SpriteStudio 7 wgpu Player Viewer");
        println!("====================================================");

        let focus = match self.focus {
            Some(i) => format!("#{}", i + 1),
            None => "all".to_string(),
        };
        println!("  Cast:             {} player(s) in 1 pass, focus {focus}", self.cast.len());
        println!("  Layout:           {}{}", self.layout.name(), if self.reverse_order { ", draw order reversed" } else { "" });
        for (i, member) in self.cast.iter().enumerate() {
            let marker = if self.focus == Some(i) { '>' } else { ' ' };
            let pack = self.packs.get(member.pack_idx).map(SSPack::name).unwrap_or("none");
            let anim = self.pack_anims.get(member.pack_idx).and_then(|names| names.get(member.anim_idx)).map(String::as_str).unwrap_or("none");
            let hidden = if member.player.is_visible() { "" } else { "  [hidden]" };
            println!("  {marker} #{}  {pack} / {anim}   order {}  alpha {:.2}  nodes {}{hidden}", i + 1, member.player.order(), member.player.alpha(), member.player.instance_node_count());
        }

        // SPEC §14.2 item 10: what the frame cost. `prims` vs `draw_calls` is the
        // headroom draw-call merging has; `gpu` is absent when the adapter has no
        // timestamp query, and lags a frame or two when it does.
        let stats = self.scene.stats();
        let gpu = match stats.gpu_pass_time_ns {
            Some(ns) => format!("{:.3} ms", ns as f64 / 1.0e6),
            None => "n/a (no timestamp query)".to_string(),
        };
        println!("----------------------------------------------------");
        println!("  Frame:            {} drawn / {} prims / {} draws / {:.1} KiB uploaded", stats.players, stats.prims, stats.draw_calls, stats.uploaded_bytes as f64 / 1024.0);
        println!("  Draw list:        {} mergeable / {} redundant texture binds", stats.mergeable_draws, stats.redundant_texture_binds);
        println!("  GPU pass:         {gpu}  (cpu update {:.3} ms)", self.last_update_ms);
        println!("  Playback:         {}", if self.is_paused { "Paused" } else { "Playing" });
        println!("  Y-Axis Mode:      {}", if self.y_down { "Y-Down (flipped)" } else { "Y-Up (standard)" });
        println!("----------------------------------------------------");
        println!("  Controls (pack / animation / V / [ ] act on the focus):");
        println!("    Space       - Pause / Resume        +/-   - Add / Remove player");
        println!("    Right/Left  - Next / Prev SSAB      Tab   - Cycle focus");
        println!("    Up/Down     - Next / Prev animation V     - Toggle visibility");
        println!("    R           - Restart animation     [ / ] - Alpha down / up");
        println!("    L           - Row / Stack layout    O     - Reverse draw order");
        println!("    F           - Toggle Y-Axis Flip    Esc   - Exit");
        println!("    Drop a .ssab or a folder on the window to play it");
        println!("====================================================");
    }
}

struct App {
    state: Option<AppState>,
    asset_dir: PathBuf,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let attributes = WindowAttributes::default().with_title("SpriteStudio 7 wgpu Viewer").with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
        let window = Arc::new(event_loop.create_window(attributes).expect("Failed to create window"));

        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        // `PRIMARY` is Metal here, DX12 on Windows, Vulkan on Linux. Naming one
        // backend leaves the instance with none of them on the other two, and the
        // failure is a surface that cannot be created "for any backend" with an
        // empty list of what was tried — nothing was. `WGPU_BACKEND` still wins,
        // so isolating a backend stays an environment variable rather than an
        // edit, which is what this viewer is for.
        instance_desc.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
        let instance = wgpu::Instance::new(instance_desc);

        let surface = instance.create_surface(window.clone()).expect("Failed to create surface");

        // Keep compatible_surface: None to prevent an AppKit deadlock on macOS.
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })).expect("Failed to request adapter");
        println!("[DIAG] Adapter: {:?}", adapter.get_info().name);

        let (device, queue) =
            // Take GPU timing when the adapter has it (SPEC §14.2 item 10). The
            // development viewer is where the numbers behind §14.2's proposals
            // get read, so it asks; a machine without the feature still runs.
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                ..Default::default()
            }))
                .expect("Failed to request device");

        let size = window.inner_size();
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(capabilities.formats[0]);

        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let stencil_view = create_stencil_view(&device, config.width, config.height);

        let renderer = SSRenderer::new(&device);
        let scene = SSScene::new(&device, &SceneConfig { color_format: format, stencil_format: Some(STENCIL_FORMAT) });

        // Only the catalogue: every player loads what it plays from its path, and
        // that one call brings the atlases and the referenced packs with it
        // (SPEC §8.1). There is no separate texture pass here any more.
        let (packs, pack_anims, pack_paths) = load_packs(&self.asset_dir);

        // Prefer "Basic" as the initial pack, otherwise the first one.
        let initial_pack = packs.iter().position(|p| p.name() == "Basic").unwrap_or(0);

        let mut state = AppState {
            window,
            device,
            queue,
            surface,
            config,
            stencil_view,
            renderer,
            scene,
            packs,
            pack_paths,
            pack_anims,
            cast: Vec::new(),
            focus: None,
            layout: Layout::Row,
            reverse_order: false,
            last_update: Instant::now(),
            is_paused: false,
            y_down: false,
            logged_first_draw: false,
            last_update_ms: 0.0,
            last_status: Instant::now(),
        };

        for _ in 0..INITIAL_CAST {
            state.spawn_member(initial_pack, 0);
        }

        state.refresh_status();
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: winit::window::WindowId, event: WindowEvent) {
        let state = match &mut self.state {
            Some(s) => s,
            None => return,
        };

        if window_id != state.window.id() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(physical_size) => {
                state.resize(physical_size);
            }
            WindowEvent::DroppedFile(path) => {
                state.adopt_dropped(&path);
            }
            WindowEvent::RedrawRequested => {
                state.update();
                match state.render() {
                    Ok(_) => {}
                    Err("Lost") | Err("Outdated") => {
                        let size = state.window.inner_size();
                        state.resize(size);
                    }
                    Err("Timeout") => {
                        eprintln!("Render timeout");
                    }
                    Err(e) => {
                        eprintln!("Render error: {:?}", e);
                        event_loop.exit();
                    }
                }
                // Keep the frame counters on screen current without redrawing
                // the block 60 times a second.
                if state.last_status.elapsed() >= Duration::from_millis(500) {
                    state.refresh_status();
                }
            }
            WindowEvent::KeyboardInput { event: winit::event::KeyEvent { logical_key, state: winit::event::ElementState::Pressed, .. }, .. } => match logical_key {
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) => {
                    event_loop.exit();
                }
                winit::keyboard::Key::Character(ref c) if c == " " => {
                    state.is_paused = !state.is_paused;
                    state.refresh_status();
                }
                // Focus picks who the pack/animation/appearance keys drive:
                // all → #1 → … → #N → all.
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::Tab) => {
                    state.focus = match state.focus {
                        None if !state.cast.is_empty() => Some(0),
                        Some(i) if i + 1 < state.cast.len() => Some(i + 1),
                        _ => None,
                    };
                    state.refresh_status();
                }
                // Horizontal: switch SSAB file (matches Ren'Py's key mapping).
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowRight) if !state.packs.is_empty() => {
                    for idx in state.targets() {
                        let next = (state.cast[idx].pack_idx + 1) % state.packs.len();
                        state.apply_pack(idx, next);
                    }
                    state.refresh_status();
                }
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowLeft) if !state.packs.is_empty() => {
                    for idx in state.targets() {
                        let next = (state.cast[idx].pack_idx + state.packs.len() - 1) % state.packs.len();
                        state.apply_pack(idx, next);
                    }
                    state.refresh_status();
                }
                // Vertical: switch animation within the current file.
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowDown) => {
                    for idx in state.targets() {
                        let next = state.cast[idx].anim_idx + 1;
                        state.apply_anim(idx, next);
                    }
                    state.refresh_status();
                }
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowUp) => {
                    for idx in state.targets() {
                        let member = &state.cast[idx];
                        let count = state.pack_anims.get(member.pack_idx).map(Vec::len).unwrap_or(0);
                        if count == 0 {
                            continue;
                        }
                        let next = member.anim_idx + count - 1;
                        state.apply_anim(idx, next);
                    }
                    state.refresh_status();
                }
                winit::keyboard::Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                    for idx in state.targets() {
                        let current = state.cast[idx].anim_idx;
                        state.apply_anim(idx, current);
                    }
                    state.refresh_status();
                }
                // "=" as well as "+", so the key works without Shift.
                winit::keyboard::Key::Character(ref c) if c == "+" || c == "=" => {
                    state.add_member();
                    state.refresh_status();
                }
                winit::keyboard::Key::Character(ref c) if c == "-" => {
                    state.remove_member();
                    state.refresh_status();
                }
                winit::keyboard::Key::Character(ref c) if c.eq_ignore_ascii_case("l") => {
                    state.layout = if state.layout == Layout::Row { Layout::Stack } else { Layout::Row };
                    state.layout_cast();
                    state.refresh_status();
                }
                winit::keyboard::Key::Character(ref c) if c.eq_ignore_ascii_case("o") => {
                    state.reverse_order = !state.reverse_order;
                    state.layout_cast();
                    state.refresh_status();
                }
                winit::keyboard::Key::Character(ref c) if c.eq_ignore_ascii_case("v") => {
                    for idx in state.targets() {
                        let member = &mut state.cast[idx];
                        let visible = member.player.is_visible();
                        member.player.set_visible(!visible);
                    }
                    state.refresh_status();
                }
                winit::keyboard::Key::Character(ref c) if c == "]" || c == "[" => {
                    let step = if c == "]" { 0.1 } else { -0.1 };
                    for idx in state.targets() {
                        let member = &mut state.cast[idx];
                        let alpha = member.player.alpha();
                        member.player.set_alpha(alpha + step);
                    }
                    state.refresh_status();
                }
                winit::keyboard::Key::Character(ref c) if c.eq_ignore_ascii_case("f") => {
                    state.y_down = !state.y_down;
                    state.refresh_status();
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

/// Parse every `.ssab` in `dir` once, and collect the animation names of each.
///
/// One [`SSPack`] per file, shared by the whole cast: cloning a pack is an `Arc`
/// bump, so N characters on one pack hold one parse and one copy of the bytes
/// between them (SPEC §8.2).
fn load_packs(dir: &Path) -> (Vec<SSPack>, Vec<Vec<String>>, Vec<PathBuf>) {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("ssab") {
                paths.push(path);
            }
        }
    }
    paths.sort();

    // Parsed here only to fill the picker — what the pack is called and what it
    // can play. The players load from the path (`SSPlayer::load`), which is also
    // what brings the atlases and the referenced packs with it.
    let mut packs = Vec::new();
    let mut kept = Vec::new();
    for path in paths {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("Failed to read {:?}: {:?}", path, e);
                continue;
            }
        };
        // Never renamed: the name inside the `.ssab` is what `ref_anime_pack`
        // resolves against (SPEC §7.1).
        match SSPack::load(&bytes) {
            Ok(pack) => {
                println!("Found SSAB: {:?}", path);
                packs.push(pack);
                kept.push(path);
            }
            Err(e) => eprintln!("Failed to parse {:?}: {:?}", path, e),
        }
    }

    let pack_anims = packs.iter().map(SSPack::animation_names).collect();
    (packs, pack_anims, kept)
}

/// Locate the converted demo assets, which `scripts/deploy-examples.sh` writes to
/// `assets/` at the repository root.
fn find_asset_dir() -> PathBuf {
    let candidates = ["assets/overall", "../assets/overall", "../../assets/overall"];
    for c in &candidates {
        let path = PathBuf::from(c);
        if path.exists() && path.is_dir() {
            println!("Found asset directory at: {:?}", path.canonicalize().unwrap());
            return path;
        }
    }
    panic!("Could not find assets/overall — run scripts/deploy-examples.sh first. Candidates: {:?}", candidates);
}

fn main() {
    let asset_dir = find_asset_dir();

    let event_loop = EventLoop::new().unwrap();
    let mut app = App { state: None, asset_dir };
    event_loop.run_app(&mut app).unwrap();
}
