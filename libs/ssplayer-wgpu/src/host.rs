//! A self-contained native host: owns a wgpu stack built from a raw window
//! handle and drives **any number of** [`SSPlayer`]s with it.
//!
//! `Instance` / `Surface` / `Device` / `Queue` live here and the players borrow
//! device and queue every frame. All of the wgpu work for the C face is in this
//! module, so `ffi.rs` stays pure ABI translation.
//!
//! It is plain Rust, so a Rust host that has a window handle rather than a winit
//! `Window` can use it directly instead of assembling wgpu itself.
//!
//! **Players live here, behind [`PlayerId`]**, because the C face cannot hold a
//! `&mut SSPlayer` across calls. [`crate::roster`] does that bookkeeping.

use crate::player::SSPlayer;
use crate::renderer::{Retention, SSRenderer};
use crate::roster::Roster;
use crate::scene::{MaskBackend, SSScene, SceneConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Names one player owned by a [`NativeHost`].
pub use crate::roster::SlotId as PlayerId;

/// Depth/stencil format allocated when masking is enabled. Matches the native
/// viewer and the web host; the depth aspect is unused.
pub const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// How to build a [`NativeHost`].
pub struct NativeHostConfig {
    /// Backends the instance may use. `Backends::PRIMARY` lets wgpu choose.
    pub backends: wgpu::Backends,
    /// Drawable width in pixels.
    pub width: u32,
    /// Drawable height in pixels.
    pub height: u32,
    /// Clear colour, straight RGBA in 0..1.
    pub clear_color: [f32; 4],
    /// Allocate a depth/stencil buffer so mask parts rasterise.
    pub enable_mask: bool,
    /// Flip the projection to Y-down (screen coordinates).
    pub y_down: bool,
}

/// A window-owning wgpu host and the players drawing into it.
pub struct NativeHost {
    players: Roster<SSPlayer>,
    /// Device-scoped renderer. Owned here because the host owns the
    /// device; **every player above shares this one** — one atlas upload and one
    /// set of pipelines no matter how many characters are on screen.
    renderer: SSRenderer,
    /// One scene per window, drawing every player in `order`.
    scene: SSScene,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// `None` when built with `enable_mask = false`.
    stencil_view: Option<wgpu::TextureView>,
    clear_color: wgpu::Color,
    y_down: bool,
    /// Set by wgpu when the device is taken away.
    device_lost: Arc<AtomicBool>,
    /// Kept last: the instance must outlive the surface it created. It survives a
    /// device loss, which is what lets `restore_device` ask it for a new adapter.
    instance: wgpu::Instance,
}

/// Ask `instance` for an adapter able to present to `surface`.
///
/// macOS: `compatible_surface` deadlocks against AppKit on the main thread, so ask
/// without it there and rely on the capability query that follows.
fn request_adapter(instance: &wgpu::Instance, surface: &wgpu::Surface<'static>) -> Result<wgpu::Adapter, String> {
    let compatible_surface = if cfg!(target_vendor = "apple") { None } else { Some(surface) };
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface, force_fallback_adapter: false })).map_err(|e| format!("no GPU adapter for the requested backend: {e}"))
}

/// Build a device+queue, and the flag wgpu raises if it is ever lost.
///
/// GPU timing has to be asked for here or never; an adapter
/// without it still draws, only `stats()` goes quiet.
fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue, Arc<AtomicBool>), String> {
    let device_desc = wgpu::DeviceDescriptor { required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY, ..Default::default() };
    let (device, queue) = pollster::block_on(adapter.request_device(&device_desc)).map_err(|e| format!("request_device: {e}"))?;
    let lost = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&lost);
    device.set_device_lost_callback(move |_reason, _message| flag.store(true, Ordering::Relaxed));
    Ok((device, queue, lost))
}

impl NativeHost {
    /// Build the wgpu stack for `target` and a player matched to its surface.
    ///
    /// # Safety
    /// `target` must describe a live window, view or layer that outlives the
    /// returned host. On macOS this must be called from the main thread.
    pub unsafe fn new(target: wgpu::SurfaceTargetUnsafe, config: &NativeHostConfig) -> Result<Self, String> {
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = config.backends;
        let instance = wgpu::Instance::new(instance_desc);

        // SAFETY: forwarded from this function's contract.
        let surface = unsafe { instance.create_surface_unsafe(target) }.map_err(|e| format!("create_surface: {e}"))?;

        let adapter = request_adapter(&instance, &surface)?;
        let (device, queue, device_lost) = request_device(&adapter)?;

        let caps = surface.get_capabilities(&adapter);
        let &first = caps.formats.first().ok_or("the adapter cannot present to this surface")?;
        // Prefer a non-sRGB format: the atlases are sRGB-encoded bytes sampled as
        // UNORM, so an sRGB target would encode them a second time (see web.rs).
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(first);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: config.width.max(1),
            height: config.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let stencil_view = config.enable_mask.then(|| create_stencil_view(&device, surface_config.width, surface_config.height));
        let renderer = SSRenderer::new(&device);
        let scene = SSScene::new(&device, &SceneConfig { color_format: format, stencil_format: config.enable_mask.then_some(STENCIL_FORMAT), mask: if config.enable_mask { MaskBackend::Stencil } else { MaskBackend::None } });

        let c = config.clear_color;
        let mut host = Self {
            players: Roster::default(),
            renderer,
            scene,
            surface,
            device,
            queue,
            config: surface_config,
            stencil_view,
            clear_color: wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 },
            y_down: config.y_down,
            device_lost,
            instance,
        };
        host.scene.set_viewport(&host.queue, host.config.width as f32, host.config.height as f32, host.y_down);
        Ok(host)
    }

    /// Add a player to this window and return its id.
    ///
    /// Players are cheap — no GPU resource of their own — so a window
    /// holding a dozen characters costs one device, one renderer and one scene.
    pub fn create_player(&mut self) -> PlayerId {
        self.players.insert(SSPlayer::new())
    }

    /// Remove a player. Returns whether `id` named a live one; a stale id is a
    /// no-op rather than a mistake made silently.
    pub fn destroy_player(&mut self, id: PlayerId) -> bool {
        self.players.remove(id)
    }

    /// The player `id` names, or `None` if it has been destroyed.
    pub fn player_mut(&mut self, id: PlayerId) -> Option<&mut SSPlayer> {
        self.players.get_mut(id)
    }

    /// How many players this window currently holds.
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// What the last frame cost.
    pub fn stats(&self) -> crate::SceneStats {
        self.scene.stats()
    }

    /// Register a texture for a reference-mode pack, lending the renderer this
    /// host's device+queue. Shared by every player in the window.
    pub fn upload_texture(&mut self, hash: u32, rgba: &[u8], width: u32, height: u32) {
        self.renderer.upload_texture(&self.device, &self.queue, hash, rgba, (width, height), Retention::Retain);
    }

    /// Register a texture from **encoded** image bytes (PNG), for hosts that have
    /// no image decoder of their own — the C face's reference-mode path.
    ///
    /// The library already carries a PNG decoder for embedded packs,
    /// so lending it out costs nothing and saves every C viewer from bringing
    /// ImageIO / WIC / libpng along just to show a reference-mode `.ssab`. The
    /// host still does the reading: nothing here opens a file.
    pub fn upload_texture_encoded(&mut self, hash: u32, encoded: &[u8]) -> Result<(), String> {
        let image = image::load_from_memory(encoded).map_err(|e| format!("decode image: {e}"))?;
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        self.renderer.upload_texture(&self.device, &self.queue, hash, &rgba, (width, height), Retention::Retain);
        Ok(())
    }

    /// Advance every player and build this frame's GPU buffers. Players are drawn
    /// in ascending `order`, whatever order the slots happen to be in.
    pub fn update(&mut self, delta_secs: f32) {
        if self.is_device_lost() {
            return;
        }
        let mut players: Vec<&mut SSPlayer> = self.players.iter_mut().collect();
        self.scene.update(&mut self.renderer, &self.device, &self.queue, delta_secs, &mut players);
    }

    /// Acquire the swapchain image, draw the prepared frame, and present.
    ///
    /// A lost/outdated/timed-out surface is absorbed by reconfiguring,
    /// skipping one frame; that is reported as success.
    pub fn render(&mut self) -> Result<(), String> {
        use wgpu::CurrentSurfaceTexture;
        if self.is_device_lost() {
            return Ok(());
        }
        let output = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(t) | CurrentSurfaceTexture::Suboptimal(t) => t,
            // Occluded: nothing to present, and the surface is still valid.
            CurrentSurfaceTexture::Occluded => return Ok(()),
            CurrentSurfaceTexture::Validation => return Err("surface configuration is invalid".into()),
            // Lost / Outdated / Timeout: rebuild the swapchain and skip this frame.
            _ => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ss-native frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ss-native pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(self.clear_color), store: wgpu::StoreOp::Store }, depth_slice: None })],
                depth_stencil_attachment: self.stencil_view.as_ref().map(|stencil| wgpu::RenderPassDepthStencilAttachment {
                    view: stencil,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: self.scene.timestamp_writes(),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.scene.draw(&self.renderer, &mut pass);
        }
        self.scene.resolve_timestamps(&mut encoder);
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }

    /// Resize the swapchain, the depth/stencil buffer and the projection to a new
    /// drawable size in pixels. Zero in either dimension is ignored.
    ///
    /// While the device is lost only the size is recorded, and `restore_device`
    /// applies it.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        if self.is_device_lost() {
            return;
        }
        self.surface.configure(&self.device, &self.config);
        if self.stencil_view.is_some() {
            self.stencil_view = Some(create_stencil_view(&self.device, width, height));
        }
        self.scene.set_viewport(&self.queue, width as f32, height as f32, self.y_down);
    }

    /// Whether the driver has taken the device away. While this is true
    /// `update` / `render` / `resize` keep off the GPU.
    pub fn is_device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Relaxed)
    }

    /// Rebuild the wgpu stack after a device loss and restore everything drawing
    /// through it, resuming playback where it was interrupted.
    ///
    /// Errors if the GPU is gone for good, or if the replacement adapter cannot
    /// present this window's format — the scene's pipelines are baked against it, so that case is a rebuild from [`NativeHost::new`].
    pub fn restore_device(&mut self) -> Result<(), String> {
        let adapter = request_adapter(&self.instance, &self.surface)?;
        let caps = self.surface.get_capabilities(&adapter);
        if !caps.formats.contains(&self.config.format) {
            return Err("the restored adapter cannot present this window's original surface format; rebuild the host".into());
        }
        let (device, queue, device_lost) = request_device(&adapter)?;

        self.config.alpha_mode = caps.alpha_modes[0];
        self.surface.configure(&device, &self.config);
        if self.stencil_view.is_some() {
            self.stencil_view = Some(create_stencil_view(&device, self.config.width, self.config.height));
        }
        // Widest scope first: a player re-registers its atlases into the renderer.
        self.renderer.restore_device(&device, &queue);
        self.scene.restore_device(&device, &queue);
        for player in self.players.iter_mut() {
            player.restore_textures(&mut self.renderer, &device, &queue);
        }
        // After the scene's own restore, which re-applies the viewport it last saw.
        self.scene.set_viewport(&queue, self.config.width as f32, self.config.height as f32, self.y_down);

        self.device = device;
        self.queue = queue;
        self.device_lost = device_lost;
        Ok(())
    }
}

fn create_stencil_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
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
