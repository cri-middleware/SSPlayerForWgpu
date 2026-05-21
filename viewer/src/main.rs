use ssplayer_wgpu::player::Player;
use ssplayer_wgpu::renderer::Renderer;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wgpu::{BindGroup, Device, Queue, Surface, SurfaceConfiguration};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes};

struct AppState {
    window: Arc<Window>,
    device: Device,
    queue: Queue,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    renderer: Renderer,
    textures: HashMap<u32, (BindGroup, (f32, f32))>,

    player: Player,
    ssab_paths: Vec<PathBuf>,
    current_ssab_idx: usize,
    current_anim_idx: usize,
    anim_names: Vec<String>,

    last_update: std::time::Instant,
    is_paused: bool,
    y_down: bool,
    logged_first_draw: bool,
}

impl AppState {
    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn update(&mut self) {
        let now = std::time::Instant::now();
        let delta = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;

        if !self.is_paused {
            self.player.update(delta);
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

        // 1. Update projection matrix based on window size
        let size = self.window.inner_size();
        self.renderer.update_projection(&self.queue, size.width as f32, size.height as f32, self.y_down);

        // 2. Prepare the vertex/index buffers and draw calls
        let textures_ref = &self.textures;
        let get_texture_size = |hash: u32| textures_ref.get(&hash).map(|(_, size)| *size);
        self.renderer.prepare(&self.device, &self.queue, &mut self.player, get_texture_size);

        // 3. Perform render pass
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.08, g: 0.08, b: 0.12, a: 1.0 }), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            let get_texture_bind_group = |hash: u32| textures_ref.get(&hash).map(|(bg, _)| bg);
            let drawn = self.renderer.render(&mut rpass, get_texture_bind_group);
            if drawn > 0 && !self.logged_first_draw {
                println!("[DIAG] First rendered frame with draw calls. Submitted: {}", drawn);
                self.logged_first_draw = true;
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    fn load_ssab_file(&mut self, path: &Path) {
        println!("Loading SSAB: {:?}", path);
        let bytes = std::fs::read(path).expect("Failed to read SSAB file");
        self.player.load_ssab(&bytes).expect("Failed to load SSAB in Player");

        let anims = self.player.animation_names().unwrap_or_default();
        println!("Animations found: {:?}", anims);
        if !anims.is_empty() {
            self.player.play(&anims[0]).expect("Failed to play animation");
            self.anim_names = anims;
            self.current_anim_idx = 0;
        } else {
            self.anim_names = Vec::new();
            self.current_anim_idx = 0;
        }
    }

    fn print_status(&self) {
        // Clear screen using ANSI escape code
        print!("\x1b[2J\x1b[H");
        println!("====================================================");
        println!("         SpriteStudio 7 wgpu Player Viewer");
        println!("====================================================");
        if !self.ssab_paths.is_empty() {
            println!("  Active File:      {:?} ({}/{})", self.ssab_paths[self.current_ssab_idx].file_name().unwrap_or_default(), self.current_ssab_idx + 1, self.ssab_paths.len());
        } else {
            println!("  Active File:      None");
        }
        if !self.anim_names.is_empty() {
            println!("  Animation:        {} ({}/{})", self.anim_names[self.current_anim_idx], self.current_anim_idx + 1, self.anim_names.len());
        } else {
            println!("  Animation:        None");
        }
        println!("  Playback:         {}", if self.is_paused { "Paused" } else { "Playing" });
        println!("  Y-Axis Mode:      {}", if self.y_down { "Y-Down (flipped)" } else { "Y-Up (standard)" });
        println!("----------------------------------------------------");
        println!("  Controls:");
        println!("    Space       - Pause / Resume");
        println!("    Right/Left  - Next / Prev Animation");
        println!("    Up/Down     - Next / Prev SSAB File");
        println!("    R           - Restart Animation");
        println!("    F           - Toggle Y-Axis Flip");
        println!("    Escape      - Exit");
        println!("====================================================");
    }
}

struct App {
    state: Option<AppState>,
    asset_dir: PathBuf,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        println!("[DIAG] resumed() entered!");
        if self.state.is_some() {
            println!("[DIAG] resumed: state already exists, returning");
            return;
        }

        // 1. Create window
        println!("[DIAG] Creating window...");
        let window = Arc::new(event_loop.create_window(WindowAttributes::default().with_title("SpriteStudio 7 wgpu Viewer")).expect("Failed to create window"));
        println!("[DIAG] Window created.");

        // 2. Create surface from window and initialize wgpu instance, adapter, device, queue
        println!("[DIAG] Initializing wgpu instance (METAL)...");
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::METAL;
        let instance = wgpu::Instance::new(instance_desc);

        println!("[DIAG] Creating surface...");
        let surface = instance.create_surface(window.clone()).expect("Failed to create surface");

        println!("[DIAG] Requesting adapter (METAL)...");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None, // Keep compatible_surface: None to prevent AppKit deadlock
            force_fallback_adapter: false,
        }))
        .expect("Failed to request adapter");
        println!("[DIAG] Adapter retrieved: {:?}", adapter.get_info().name);

        println!("[DIAG] Requesting device & queue...");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: None, required_features: wgpu::Features::empty(), required_limits: wgpu::Limits::default(), memory_hints: wgpu::MemoryHints::default(), ..Default::default() }))
                .expect("Failed to request device");
        println!("[DIAG] Device and Queue retrieved.");

        let size = window.inner_size();
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(capabilities.formats[0]);

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

        let renderer = Renderer::new(&device, format);

        // Load textures
        println!("[DIAG] Loading textures...");
        let textures = load_textures(&device, &queue, &renderer, &self.asset_dir);

        // Find all SSAB files
        let mut ssab_paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.asset_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("ssab") {
                    ssab_paths.push(path);
                }
            }
        }
        // Sort to have a predictable order
        ssab_paths.sort();

        let mut player = Player::new();
        let mut current_ssab_idx = 0;
        let mut anim_names = Vec::new();

        // Find "Basic.ssab" and load it first if available, otherwise first file
        if !ssab_paths.is_empty() {
            if let Some(idx) = ssab_paths.iter().position(|p| p.file_name().and_then(|s| s.to_str()) == Some("Basic.ssab")) {
                current_ssab_idx = idx;
            }

            let path = &ssab_paths[current_ssab_idx];
            println!("Loading initial SSAB: {:?}", path);
            let bytes = std::fs::read(path).expect("Failed to read SSAB file");
            player.load_ssab(&bytes).expect("Failed to load SSAB in Player");

            anim_names = player.animation_names().unwrap_or_default();
            if !anim_names.is_empty() {
                player.play(&anim_names[0]).expect("Failed to play animation");
            }
        }

        let state = AppState { window, device, queue, surface, config, renderer, textures, player, ssab_paths, current_ssab_idx, current_anim_idx: 0, anim_names, last_update: std::time::Instant::now(), is_paused: false, y_down: false, logged_first_draw: false };

        state.print_status();
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
            }
            WindowEvent::KeyboardInput { event: winit::event::KeyEvent { logical_key, state: winit::event::ElementState::Pressed, .. }, .. } => match logical_key {
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) => {
                    event_loop.exit();
                }
                winit::keyboard::Key::Character(ref c) if c == " " => {
                    state.is_paused = !state.is_paused;
                    state.print_status();
                }
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowRight) if !state.anim_names.is_empty() => {
                    state.current_anim_idx = (state.current_anim_idx + 1) % state.anim_names.len();
                    let name = state.anim_names[state.current_anim_idx].clone();
                    state.player.play(&name).unwrap();
                    state.print_status();
                }
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowLeft) if !state.anim_names.is_empty() => {
                    if state.current_anim_idx == 0 {
                        state.current_anim_idx = state.anim_names.len() - 1;
                    } else {
                        state.current_anim_idx -= 1;
                    }
                    let name = state.anim_names[state.current_anim_idx].clone();
                    state.player.play(&name).unwrap();
                    state.print_status();
                }
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowUp) if !state.ssab_paths.is_empty() => {
                    state.current_ssab_idx = (state.current_ssab_idx + 1) % state.ssab_paths.len();
                    let path = state.ssab_paths[state.current_ssab_idx].clone();
                    state.load_ssab_file(&path);
                    state.print_status();
                }
                winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowDown) if !state.ssab_paths.is_empty() => {
                    if state.current_ssab_idx == 0 {
                        state.current_ssab_idx = state.ssab_paths.len() - 1;
                    } else {
                        state.current_ssab_idx -= 1;
                    }
                    let path = state.ssab_paths[state.current_ssab_idx].clone();
                    state.load_ssab_file(&path);
                    state.print_status();
                }
                winit::keyboard::Key::Character(ref c) if c.eq_ignore_ascii_case("r") && !state.anim_names.is_empty() => {
                    let name = state.anim_names[state.current_anim_idx].clone();
                    state.player.play(&name).unwrap();
                    state.print_status();
                }
                winit::keyboard::Key::Character(ref c) if c.eq_ignore_ascii_case("f") => {
                    state.y_down = !state.y_down;
                    state.print_status();
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

fn load_textures(device: &Device, queue: &Queue, renderer: &Renderer, dir: &Path) -> HashMap<u32, (BindGroup, (f32, f32))> {
    let mut textures = HashMap::new();

    let entries = std::fs::read_dir(dir).expect("Failed to read assets directory");
    for entry in entries {
        let entry = entry.expect("Failed to read entry");
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("png") {
            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let hash = ssruntime::fnv1a_hash_str(file_stem);

            println!("Loading texture: {:?} (stem: {}, hash: {})", path, file_stem, hash);

            let img = image::open(&path).expect("Failed to open texture image");
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();

            let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };

            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(file_stem),
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
                &rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * width), rows_per_image: Some(height) },
                size,
            );

            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = renderer.create_texture_bind_group(device, &view);

            textures.insert(hash, (bind_group, (width as f32, height as f32)));
        }
    }

    textures
}

fn find_asset_dir() -> PathBuf {
    let candidates = ["tests/overall/overall_ssab", "../tests/overall/overall_ssab", "../../tests/overall/overall_ssab"];
    for c in &candidates {
        let path = PathBuf::from(c);
        if path.exists() && path.is_dir() {
            println!("Found asset directory at: {:?}", path.canonicalize().unwrap());
            return path;
        }
    }
    panic!("Could not find tests/overall/overall_ssab directory! Candidates: {:?}", candidates);
}

fn main() {
    println!("[DIAG] main() started");
    let asset_dir = find_asset_dir();

    println!("[DIAG] Initializing EventLoop...");
    let event_loop = EventLoop::new().unwrap();
    println!("[DIAG] EventLoop initialized.");

    let mut app = App { state: None, asset_dir };
    println!("[DIAG] Starting event loop via run_app...");
    event_loop.run_app(&mut app).unwrap();
}
