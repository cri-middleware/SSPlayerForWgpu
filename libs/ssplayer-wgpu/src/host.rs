//! The self-contained native host (SPEC §9.4, "③ ffi"): owns a wgpu stack built
//! from a raw window handle and drives **any number of** borrow-model
//! [`SSPlayer`]s with it.
//!
//! This is the native counterpart of `examples/viewer` (winit) and [`crate::web`]
//! (canvas): `Instance`/`Surface`/`Device`/`Queue` live here, and the players
//! borrow device+queue on every frame. All of the wgpu work for the C face lives
//! in this module so that `ffi.rs` stays pure ABI translation.
//!
//! It is plain Rust, so a Rust host that only has a window handle (rather than a
//! winit `Window`) can use it directly instead of assembling wgpu itself.
//!
//! **Players live here, behind [`PlayerId`]** (SPEC §11). The C face cannot hold a
//! `&mut SSPlayer` across calls, so it needs something stable to name a player
//! by; a raw pointer into a `Vec` would dangle the moment the vector grew. A
//! generation-tagged slot index cannot dangle and cannot be confused with the
//! player that later reuses its slot.

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

/// Names one player owned by a [`NativeHost`].
///
/// Stable across everything the host does to its player list, and safe to hold
/// past a `destroy_player`: the generation makes a stale id resolve to `None`
/// rather than to whatever player took over the slot (SPEC §11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerId {
    index: u32,
    generation: u32,
}

/// One slot in the roster. Slots are reused; the generation is what keeps ids
/// from being reused with them.
struct PlayerSlot {
    player: Option<SSPlayer>,
    generation: u32,
}

/// The host's players, addressable by [`PlayerId`].
///
/// Split out from [`NativeHost`] so the id bookkeeping — the part with an actual
/// invariant, and the part a C caller can get wrong — is testable without a
/// window (see the tests at the foot of this file).
#[derive(Default)]
struct PlayerRoster {
    /// Players by slot. Empty slots are reused, so this does not grow with churn.
    slots: Vec<PlayerSlot>,
    /// Indices of empty slots, so `create` is O(1).
    free: Vec<u32>,
}

impl PlayerRoster {
    fn create(&mut self) -> PlayerId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.player = Some(SSPlayer::new());
            return PlayerId { index, generation: slot.generation };
        }
        let index = self.slots.len() as u32;
        self.slots.push(PlayerSlot { player: Some(SSPlayer::new()), generation: 0 });
        PlayerId { index, generation: 0 }
    }

    fn destroy(&mut self, id: PlayerId) -> bool {
        let Some(slot) = self.slots.get_mut(id.index as usize) else { return false };
        if slot.generation != id.generation || slot.player.is_none() {
            return false;
        }
        slot.player = None;
        // Retiring the generation is what stops the next player in this slot from
        // answering to the id just released.
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(id.index);
        true
    }

    fn get_mut(&mut self, id: PlayerId) -> Option<&mut SSPlayer> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation { None } else { slot.player.as_mut() }
    }

    fn live_mut(&mut self) -> Vec<&mut SSPlayer> {
        self.slots.iter_mut().filter_map(|s| s.player.as_mut()).collect()
    }

    fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.player.is_some()).count()
    }
}

/// A window-owning wgpu host and the players drawing into it.
pub struct NativeHost {
    players: PlayerRoster,
    /// Device-scoped renderer (SPEC §1.2). Owned here because the host owns the
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
        // Ask for GPU timing when the adapter has it (SPEC §14.2 item 10). It has
        // to be requested here or never; `SSScene` reports nothing without it,
        // and an adapter that lacks it still works — it just cannot be measured.
        let device_desc = wgpu::DeviceDescriptor { required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY, ..Default::default() };
        let (device, queue) = pollster::block_on(adapter.request_device(&device_desc)).map_err(|e| format!("request_device: {e}"))?;

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
        let scene = SSScene::new(&device, &SceneConfig { color_format: format, stencil_format: config.enable_mask.then_some(STENCIL_FORMAT) });

        let c = config.clear_color;
        let mut host = Self {
            players: PlayerRoster::default(),
            renderer,
            scene,
            surface,
            device,
            queue,
            config: surface_config,
            stencil_view,
            clear_color: wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 },
            y_down: config.y_down,
            _instance: instance,
        };
        host.scene.set_viewport(&host.queue, host.config.width as f32, host.config.height as f32, host.y_down);
        Ok(host)
    }

    /// Add a player to this window and return its id.
    ///
    /// Players are cheap — no GPU resource of their own (SPEC §1.2) — so a window
    /// holding a dozen characters costs one device, one renderer and one scene.
    pub fn create_player(&mut self) -> PlayerId {
        self.players.create()
    }

    /// Remove a player. Returns whether `id` named a live one; a stale id is a
    /// no-op rather than a mistake made silently.
    pub fn destroy_player(&mut self, id: PlayerId) -> bool {
        self.players.destroy(id)
    }

    /// The player `id` names, or `None` if it has been destroyed.
    pub fn player_mut(&mut self, id: PlayerId) -> Option<&mut SSPlayer> {
        self.players.get_mut(id)
    }

    /// How many players this window currently holds.
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// What the last frame cost (SPEC §14.2 item 10).
    pub fn stats(&self) -> crate::SceneStats {
        self.scene.stats()
    }

    /// Register a texture for a reference-mode pack, lending the renderer this
    /// host's device+queue (SPEC §8.1). Shared by every player in the window.
    pub fn upload_texture(&mut self, hash: u32, rgba: &[u8], width: u32, height: u32) {
        self.renderer.upload_texture(&self.device, &self.queue, hash, rgba, (width, height), Retention::Retain);
    }

    /// Advance every player and build this frame's GPU buffers. Players are drawn
    /// in ascending `order` (SPEC §9.2), whatever order the slots happen to be in.
    pub fn update(&mut self, delta_secs: f32) {
        let mut players = self.players.live_mut();
        self.scene.update(&mut self.renderer, &self.device, &self.queue, delta_secs, &mut players);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC §11: a `ss_player_t` outliving its player must resolve to "gone",
    /// not to whatever took over the slot. Slots *are* reused — a program that
    /// spawns and drops characters must not grow a slot per spawn — so the
    /// generation is the only thing separating the two.
    #[test]
    fn a_destroyed_id_never_names_its_successor() {
        let mut roster = PlayerRoster::default();

        let first = roster.create();
        assert!(roster.get_mut(first).is_some());
        assert_eq!(roster.len(), 1);

        assert!(roster.destroy(first), "destroying a live player reported failure");
        assert!(roster.get_mut(first).is_none(), "a destroyed id still resolved");
        assert_eq!(roster.len(), 0);
        // Double destroy is the easy C mistake; it has to be inert, not a panic
        // and not a second slot release.
        assert!(!roster.destroy(first), "destroying the same id twice reported success");

        // The slot comes back, but the id does not.
        let second = roster.create();
        assert_eq!(second.index, first.index, "the empty slot was not reused");
        assert_ne!(second.generation, first.generation);
        assert!(roster.get_mut(first).is_none(), "the old id resolved to the player that reused its slot");
        assert!(roster.get_mut(second).is_some());
        assert_eq!(roster.len(), 1);
    }

    /// Live players are handed to the scene in slot order; holes left by
    /// destroyed ones are skipped rather than drawn as blanks.
    #[test]
    fn only_live_players_are_drawn() {
        let mut roster = PlayerRoster::default();
        let ids: Vec<PlayerId> = (0..4).map(|_| roster.create()).collect();
        assert_eq!(roster.live_mut().len(), 4);

        roster.destroy(ids[1]);
        roster.destroy(ids[2]);
        assert_eq!(roster.len(), 2);
        assert_eq!(roster.live_mut().len(), 2);

        // Refilling the holes must not produce duplicates or lose anyone.
        roster.create();
        roster.create();
        roster.create();
        assert_eq!(roster.len(), 5);
        assert_eq!(roster.live_mut().len(), 5);
    }
}
