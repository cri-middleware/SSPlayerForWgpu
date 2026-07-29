//! The self-contained native host (SPEC §9.3, "③ ffi"): owns a wgpu stack built
//! from a raw window handle and drives the borrow-model [`SSPlayer`] with it.
//!
//! This is the native counterpart of `examples/viewer` (winit) and [`crate::web`]
//! (canvas): `Instance`/`Surface`/`Device`/`Queue` live here, and the player
//! borrows device+queue on every frame. All of the wgpu work for the C face lives
//! in this module so that `ffi.rs` stays pure ABI translation.
//!
//! It is plain Rust, so a Rust host that only has a window handle (rather than a
//! winit `Window`) can use it directly instead of assembling wgpu itself.

use crate::player::SSPlayer;
use crate::renderer::{Retention, SSRenderer};
use crate::scene::{SSScene, SceneConfig};

/// Depth/stencil format allocated when masking is enabled. Matches the native
/// viewer and the web host; the depth aspect is unused (SPEC §6).
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
    /// Allocate a depth/stencil buffer so mask parts rasterise (SPEC §6).
    pub enable_mask: bool,
    /// Flip the projection to Y-down (screen coordinates).
    pub y_down: bool,
}

/// A window-owning wgpu host plus the one player drawing into it.
pub struct NativeHost {
    /// The player. Public because the host adds nothing to playback control —
    /// callers drive `load_ssab` / `play` / … on it directly.
    pub player: SSPlayer,
    /// Device-scoped renderer (SPEC §1.2). Owned here because the host owns the
    /// device; every player this host gains at P7-7 will share this one.
    renderer: SSRenderer,
    /// One scene per window. P7-7 turns `player` into a `Vec` drawn through it.
    scene: SSScene,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// `None` when built with `enable_mask = false`.
    stencil_view: Option<wgpu::TextureView>,
    clear_color: wgpu::Color,
    y_down: bool,
    /// Kept last: the instance must outlive the surface it created.
    _instance: wgpu::Instance,
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

        // macOS: requesting an adapter with `compatible_surface` set deadlocks
        // against AppKit when called from the main thread, so ask without it (the
        // surface is still queried for capabilities below). Elsewhere, pin the
        // adapter to the surface so it is guaranteed presentable.
        #[cfg(target_vendor = "apple")]
        let compatible_surface = None;
        #[cfg(not(target_vendor = "apple"))]
        let compatible_surface = Some(&surface);

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface, force_fallback_adapter: false }))
            .map_err(|e| format!("no GPU adapter for the requested backend: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("request_device: {e}"))?;

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
        let scene = SSScene::new(&device, &renderer, &SceneConfig { color_format: format, stencil_format: config.enable_mask.then_some(STENCIL_FORMAT) });
        let player = SSPlayer::new();

        let c = config.clear_color;
        let mut host = Self { player, renderer, scene, surface, device, queue, config: surface_config, stencil_view, clear_color: wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 }, y_down: config.y_down, _instance: instance };
        host.scene.set_viewport(&host.queue, host.config.width as f32, host.config.height as f32, host.y_down);
        Ok(host)
    }

    /// Hand the player a texture for a reference-mode pack, lending it this
    /// host's device+queue (SPEC §8.1).
    pub fn upload_texture(&mut self, hash: u32, rgba: &[u8], width: u32, height: u32) {
        self.renderer.upload_texture(&self.device, &self.queue, hash, rgba, (width, height), Retention::Retain);
    }

    /// Advance the playback tree and build this frame's GPU buffers.
    pub fn update(&mut self, delta_secs: f32) {
        self.scene.update(&mut self.renderer, &self.device, &self.queue, delta_secs, &mut [&mut self.player]);
    }

    /// Acquire the swapchain image, draw the prepared frame, and present.
    ///
    /// A lost/outdated/timed-out surface is absorbed by reconfiguring (SPEC §14),
    /// skipping one frame; that is reported as success.
    pub fn render(&mut self) -> Result<(), String> {
        use wgpu::CurrentSurfaceTexture;
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
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.scene.draw(&self.renderer, &mut pass);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }

    /// Resize the swapchain, the depth/stencil buffer and the projection to a new
    /// drawable size in pixels. Zero in either dimension is ignored.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        if self.stencil_view.is_some() {
            self.stencil_view = Some(create_stencil_view(&self.device, width, height));
        }
        self.scene.set_viewport(&self.queue, width as f32, height as f32, self.y_down);
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
