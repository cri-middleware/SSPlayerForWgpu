//! A self-contained native host: owns a wgpu stack built from a raw window
//! handle and drives **any number of** [`SSPlayer`]s with it.
//!
//! `Instance` / `Surface` / `Device` / `Queue` live here and the players borrow
//! device and queue every frame. All of the wgpu work for the C face is in this
//! module and the swapchain it shares with the web face (`surface.rs`), so
//! `ffi.rs` stays pure ABI translation.
//!
//! It is plain Rust, so a Rust host that has a window handle rather than a winit
//! `Window` can use it directly instead of assembling wgpu itself.
//!
//! **Players live here, behind [`PlayerId`]**, because the C face cannot hold a
//! `&mut SSPlayer` across calls. `roster.rs` does that bookkeeping.

use crate::player::SSPlayer;
use crate::renderer::{Retention, SSRenderer, texture_extent_error};
use crate::roster::Roster;
use crate::scene::{MaskBackend, SSScene, SceneConfig};
use crate::surface::Swapchain;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Names one player owned by a [`NativeHost`].
pub use crate::roster::SlotId as PlayerId;

pub use crate::surface::STENCIL_FORMAT;

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
    /// The window's swapchain, with a stencil buffer unless built with
    /// `enable_mask = false`.
    swapchain: Swapchain,
    device: wgpu::Device,
    queue: wgpu::Queue,
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
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface, force_fallback_adapter: false, apply_limit_buckets: false }))
        .map_err(|e| format!("no GPU adapter for the requested backend: {e}"))
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

        let swapchain = Swapchain::new(&device, &adapter, surface, (config.width, config.height), None, config.enable_mask).ok_or("the adapter cannot present to this surface")?;
        let renderer = SSRenderer::new(&device);
        let mut scene = SSScene::new(&device, &SceneConfig { color_format: swapchain.format(), stencil_format: config.enable_mask.then_some(STENCIL_FORMAT), mask: if config.enable_mask { MaskBackend::Stencil } else { MaskBackend::None } });
        let (width, height) = swapchain.size();
        scene.set_viewport(&queue, width as f32, height as f32, config.y_down);

        let c = config.clear_color;
        Ok(Self { players: Roster::default(), renderer, scene, swapchain, device, queue, clear_color: wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 }, y_down: config.y_down, device_lost, instance })
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

    /// Why this window's device cannot hold a `width` x `height` texture, or
    /// `None` when it can: empty, or past its `max_texture_dimension_2d`.
    ///
    /// What the C face asks before it hands a host's pixels over, so a texture
    /// too large for the device comes back as an error code instead of one
    /// [`NativeHost::upload_texture`] can only report on stderr.
    pub fn texture_extent_error(&self, width: u32, height: u32) -> Option<String> {
        texture_extent_error(&self.device, (width, height))
    }

    /// Register a texture for a reference-mode pack, lending the renderer this
    /// host's device+queue. Shared by every player in the window.
    ///
    /// One the device cannot hold is not uploaded; see
    /// [`SSRenderer::upload_texture`].
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
        if let Some(why) = self.texture_extent_error(width, height) {
            return Err(why);
        }
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
    /// A lost/outdated/timed-out surface is absorbed by reconfiguring, and an
    /// occluded one by waiting, skipping one frame either way; that is reported
    /// as success.
    pub fn render(&mut self) -> Result<(), String> {
        if self.is_device_lost() {
            return Ok(());
        }
        self.swapchain.render(&self.device, &self.queue, &self.renderer, &mut self.scene, self.clear_color, "ss-native frame").map_err(String::from)
    }

    /// Resize the swapchain, the depth/stencil buffer and the projection to a new
    /// drawable size in pixels. Zero in either dimension is ignored, and a size
    /// past the device's `max_texture_dimension_2d` draws at that limit.
    ///
    /// While the device is lost only the size is recorded, and `restore_device`
    /// applies it.
    pub fn resize(&mut self, width: u32, height: u32) {
        // A minimized window reports zero. Its last size stays configured until
        // it comes back, rather than a 1 x 1 swapchain that matches no window;
        // the web face, whose configure sizes the canvas, clamps instead.
        if width == 0 || height == 0 {
            return;
        }
        let lost = self.is_device_lost();
        self.swapchain.resize(&self.device, width, height, lost);
        if !lost {
            let (width, height) = self.swapchain.size();
            self.scene.set_viewport(&self.queue, width as f32, height as f32, self.y_down);
        }
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
        let adapter = request_adapter(&self.instance, self.swapchain.surface())?;
        if !self.swapchain.presents_on(&adapter) {
            return Err("the restored adapter cannot present this window's original surface format; rebuild the host".into());
        }
        let (device, queue, device_lost) = request_device(&adapter)?;

        self.swapchain.restore(&device, &adapter);
        // Widest scope first: a player re-registers its atlases into the renderer.
        self.renderer.restore_device(&device, &queue);
        self.scene.restore_device(&device, &queue);
        for player in self.players.iter_mut() {
            player.restore_textures(&mut self.renderer, &device, &queue);
        }
        // After the scene's own restore, which re-applies the viewport it last saw.
        let (width, height) = self.swapchain.size();
        self.scene.set_viewport(&queue, width as f32, height as f32, self.y_down);

        self.device = device;
        self.queue = queue;
        self.device_lost = device_lost;
        Ok(())
    }
}
