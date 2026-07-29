//! The "js" face (SPEC §10): a `wasm-bindgen` wrapper that renders SpriteStudio
//! animations to HTML canvases via **WebGPU**.
//!
//! Borrow model, browser edition: this module is the host. It owns the wgpu stack
//! (instance / adapter / device / queue / surfaces) and lends device+queue to the
//! borrow-model [`SSScene`] and [`SSPlayer`] each frame — exactly like the native
//! viewer, but the event loop is JS `requestAnimationFrame` (`examples/web`).
//!
//! **The shape mirrors the redesigned `ssplayer-webgl`** (SPEC §0.2), and the
//! resource scopes of §1.2 are what dictate the three objects:
//!
//! ```text
//! SsContextWeb   one per page    device + queue + SSRenderer  (pipelines, atlases)
//! SsSurfaceWeb   one per canvas  wgpu::Surface + SSScene       (projection, arena)
//! SsPlayerWeb    any number      SSPlayer                      (playback tree only)
//! ```
//!
//! The predecessor requested an adapter and a device **per canvas**, so a page
//! with N elements built N `GPUDevice`s and re-uploaded the same atlas N times —
//! the "1 canvas = 1 player" shape P7 exists to remove. Here the device is asked
//! for once and every canvas shares it, which is also what makes the atlas cache
//! in `SSRenderer` do its job (§8.2).
//!
//! Everything lives in one `Rc<RefCell<WebContext>>` and the exported types are
//! generation-tagged handles into it ([`crate::roster`]), for the reason the C
//! face has the same design (SPEC §11): JS cannot hold a Rust `&mut`, and a
//! stale handle must resolve to "destroyed" rather than to its successor. One
//! cell rather than one per object means no borrow can be held across another.
//!
//! Embedded ssab (SPEC §8.1) is the natural web form: the atlas PNGs ride inside
//! the `.ssab`, so a single fetched file renders with no host texture supply.

use crate::player::SSPlayer;
use crate::renderer::{Retention, SSRenderer};
use crate::roster::{Roster, SlotId};
use crate::scene::{SSScene, SceneConfig};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::prelude::*;

/// Depth/stencil format for masking (matches the native viewer). Masks and the
/// per-part shader gating rely on a stencil buffer being present.
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Map any error into a JS exception value.
fn js_err<E: core::fmt::Debug>(e: E) -> JsValue {
    JsValue::from_str(&format!("{e:?}"))
}

/// The error a handle raises once the thing it names is gone. Not a panic: JS
/// keeps references alive in closures and event listeners, so a call arriving
/// after `destroy()` is ordinary rather than exceptional.
fn gone(what: &str) -> JsValue {
    JsValue::from_str(&format!("this {what} has been destroyed"))
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

/// Request a device and subscribe to its `GPUDevice.lost` (SPEC §14). The returned
/// flag flips when the browser drops the device — which it does on its own schedule
/// (backgrounded tab, GPU process crash, driver reset), so a long-lived ad creative
/// has to expect it. Once set, the host is meant to call [`SsContextWeb::restore_device`].
async fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue, Arc<AtomicBool>), JsValue> {
    // GPU timing when the browser offers it (SPEC §14.2 item 10) — Chrome gates
    // `timestamp-query` behind a flag on some platforms, so this must not be
    // required, only taken when available.
    let device_desc = wgpu::DeviceDescriptor { required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY, ..Default::default() };
    let (device, queue) = adapter.request_device(&device_desc).await.map_err(js_err)?;
    let lost = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&lost);
    device.set_device_lost_callback(move |_reason, _message| flag.store(true, Ordering::Relaxed));
    Ok((device, queue, lost))
}

/// Ask the instance for an adapter.
///
/// No `compatible_surface`: the device is page-scoped and is requested before any
/// canvas exists. A browser exposes one adapter for the page, so pinning it to the
/// first canvas would constrain nothing and would force surface creation to come first.
async fn request_adapter(instance: &wgpu::Instance) -> Result<wgpu::Adapter, JsValue> {
    instance
        .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: None, force_fallback_adapter: false })
        .await
        .map_err(|_| JsValue::from_str("no WebGPU adapter (this browser/GPU may not support WebGPU)"))
}

/// One canvas: its swapchain and the scene drawn into it (SPEC §9.2).
struct SurfaceEntry {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    stencil_view: wgpu::TextureView,
    scene: SSScene,
    /// The players drawn into this canvas. The scene borrows them per frame
    /// rather than owning them (SPEC §1.2), so the owner is the host — here.
    players: Roster<SSPlayer>,
}

/// Everything the page owns. One instance behind an `Rc<RefCell<_>>`; the
/// exported types are handles into it.
struct WebContext {
    /// Device-scoped renderer (SPEC §1.2): pipelines, sampler and the atlas
    /// cache. **Every surface and player on the page shares this one**, so the
    /// same character in two canvases is uploaded once.
    renderer: SSRenderer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surfaces: Roster<SurfaceEntry>,
    /// Kept so a lost device can be replaced without rebuilding the canvases'
    /// surfaces (a canvas context outlives the device it was configured with).
    adapter: wgpu::Adapter,
    /// Set from `GPUDevice.lost`. While it is set every `frame` is a no-op —
    /// each GPU object on the page is dead until `restoreDevice`.
    device_lost: Arc<AtomicBool>,
    /// Kept last: the instance must outlive the surfaces it created.
    instance: wgpu::Instance,
}

impl WebContext {
    /// Build a surface + scene for `canvas`.
    fn add_surface(&mut self, canvas: web_sys::HtmlCanvasElement) -> Result<SlotId, JsValue> {
        let width = canvas.width().max(1);
        let height = canvas.height().max(1);
        let surface = self.instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas)).map_err(js_err)?;

        let caps = surface.get_capabilities(&self.adapter);
        let &first = caps.formats.first().ok_or_else(|| JsValue::from_str("the adapter cannot present to this canvas"))?;
        // Prefer a non-sRGB format so the premultiplied pipeline composites as authored.
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(first);
        let config = wgpu::SurfaceConfiguration { usage: wgpu::TextureUsages::RENDER_ATTACHMENT, format, width, height, present_mode: wgpu::PresentMode::Fifo, alpha_mode: caps.alpha_modes[0], view_formats: vec![], desired_maximum_frame_latency: 2 };
        surface.configure(&self.device, &config);

        // The scene is format-specific; the renderer is not (SPEC §9.1), which is
        // what lets canvases with different formats share one of it.
        let mut scene = SSScene::new(&self.device, &SceneConfig { color_format: format, stencil_format: Some(STENCIL_FORMAT) });
        scene.set_viewport(&self.queue, width as f32, height as f32, false);
        let stencil_view = create_stencil_view(&self.device, width, height);

        Ok(self.surfaces.insert(SurfaceEntry { surface, config, stencil_view, scene, players: Roster::default() }))
    }
}

/// The page's WebGPU context: **one adapter, one device, one [`SSRenderer`]**,
/// shared by every canvas and every player on the page (SPEC §1.2, §10).
///
/// ```js
/// const ctx    = await SsContextWeb.create();
/// const surf   = ctx.createSurface(canvas);
/// const player = surf.createPlayer();
/// player.loadSsab('Basic', bytes);
/// player.setAnimation('idle');
/// player.play();
/// ```
#[wasm_bindgen]
pub struct SsContextWeb {
    inner: Rc<RefCell<WebContext>>,
}

#[wasm_bindgen]
impl SsContextWeb {
    /// Initialise WebGPU for the page. Rejects if the browser has no WebGPU
    /// adapter/device (the caller should surface a fallback message).
    ///
    /// Call this **once per page** and share the result — that is the whole point
    /// of the type. Calling it twice builds two devices, which is the state P7
    /// removed.
    pub async fn create() -> Result<SsContextWeb, JsValue> {
        console_error_panic_hook::set_once();

        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::BROWSER_WEBGPU;
        let instance = wgpu::Instance::new(instance_desc);
        let adapter = request_adapter(&instance).await?;
        let (device, queue, device_lost) = request_device(&adapter).await?;
        let renderer = SSRenderer::new(&device);

        Ok(SsContextWeb { inner: Rc::new(RefCell::new(WebContext { renderer, device, queue, surfaces: Roster::default(), adapter, device_lost, instance })) })
    }

    /// Attach a canvas and get the scene that draws into it. Any number of
    /// canvases may share this context.
    #[wasm_bindgen(js_name = createSurface)]
    pub fn create_surface(&self, canvas: web_sys::HtmlCanvasElement) -> Result<SsSurfaceWeb, JsValue> {
        let id = self.inner.borrow_mut().add_surface(canvas)?;
        Ok(SsSurfaceWeb { ctx: Rc::clone(&self.inner), id })
    }

    /// Supply a texture for a reference-mode pack by hash (SPEC §8.1).
    ///
    /// Device-scoped, so it covers every canvas and every player at once — which
    /// is exactly why it is here and not on a surface or a player. Embedded packs
    /// need none of this: the player decodes and uploads its own atlases.
    #[wasm_bindgen(js_name = addTexture)]
    pub fn add_texture(&self, hash: u32, rgba: &[u8], width: u32, height: u32) {
        let ctx = &mut *self.inner.borrow_mut();
        let WebContext { renderer, device, queue, .. } = ctx;
        renderer.upload_texture(device, queue, hash, rgba, (width, height), Retention::Retain);
    }

    /// Whether the browser has taken the WebGPU device away (SPEC §14). While
    /// this is true every surface's `frame` is a no-op; call
    /// [`SsContextWeb::restore_device`] to bring the whole page back.
    #[wasm_bindgen(js_name = isDeviceLost)]
    pub fn is_device_lost(&self) -> bool {
        self.inner.borrow().device_lost.load(Ordering::Relaxed)
    }

    /// Rebuild the page's WebGPU stack after a device loss and restore every
    /// surface and player on it (pipelines, buffers, atlases, projections) —
    /// playback continues from the frame it was interrupted on, with no re-fetch
    /// of any `.ssab`.
    ///
    /// **One call for the page**, where the predecessor ran the whole recovery —
    /// including re-decoding the same atlas PNGs — once per canvas.
    ///
    /// Rejects if WebGPU is gone for good, or if the new adapter cannot present a
    /// surface's baked format (then the host should rebuild from `create`).
    #[wasm_bindgen(js_name = restoreDevice)]
    pub async fn restore_device(&self) -> Result<(), JsValue> {
        // Nothing is borrowed across these awaits: a `RefMut` held over a yield
        // point would still be held when JS re-enters through a rAF callback.
        let instance = self.inner.borrow().instance.clone();
        let adapter = request_adapter(&instance).await?;
        let (device, queue, device_lost) = request_device(&adapter).await?;

        let ctx = &mut *self.inner.borrow_mut();
        // Device-scoped first, then each target, then each player's atlases (§14).
        ctx.renderer.restore_device(&device, &queue);
        for surface in ctx.surfaces.iter_mut() {
            let caps = surface.surface.get_capabilities(&adapter);
            if !caps.formats.contains(&surface.config.format) {
                return Err(JsValue::from_str("the restored adapter cannot present a canvas's original surface format; rebuild the context"));
            }
            surface.config.alpha_mode = caps.alpha_modes[0];
            surface.surface.configure(&device, &surface.config);
            surface.stencil_view = create_stencil_view(&device, surface.config.width, surface.config.height);
            surface.scene.restore_device(&device, &queue);
            for player in surface.players.iter_mut() {
                player.restore_textures(&mut ctx.renderer, &device, &queue);
            }
            // `restore_device` re-applies the viewport the scene last saw, which is
            // stale if the canvas was resized while the device was gone (`resize`
            // only records the size then). `config` is authoritative — re-apply from it.
            surface.scene.set_viewport(&queue, surface.config.width as f32, surface.config.height as f32, false);
        }

        ctx.adapter = adapter;
        ctx.device = device;
        ctx.queue = queue;
        ctx.device_lost = device_lost;
        Ok(())
    }

    /// How many canvases this context currently drives (diagnostic).
    #[wasm_bindgen(js_name = surfaceCount)]
    pub fn surface_count(&self) -> usize {
        self.inner.borrow().surfaces.len()
    }
}

/// One canvas and the scene drawn into it (SPEC §9.2). Holds **any number of
/// players**, composited in ascending `order` in a single render pass.
#[wasm_bindgen]
pub struct SsSurfaceWeb {
    ctx: Rc<RefCell<WebContext>>,
    id: SlotId,
}

#[wasm_bindgen]
impl SsSurfaceWeb {
    /// Add a player to this canvas. Players hold no GPU resource of their own
    /// (SPEC §1.2), so a canvas with a dozen characters still costs one device,
    /// one renderer and one scene.
    #[wasm_bindgen(js_name = createPlayer)]
    pub fn create_player(&self) -> Result<SsPlayerWeb, JsValue> {
        let ctx = &mut *self.ctx.borrow_mut();
        let entry = ctx.surfaces.get_mut(self.id).ok_or_else(|| gone("surface"))?;
        let id = entry.players.insert(SSPlayer::new());
        Ok(SsPlayerWeb { ctx: Rc::clone(&self.ctx), surface: self.id, id })
    }

    /// Advance every player on this canvas by `dt` seconds and render one frame.
    ///
    /// A no-op while the device is lost — every GPU object it would touch is
    /// dead. The host polls [`SsContextWeb::is_device_lost`] and calls
    /// `restoreDevice` to resume (SPEC §14).
    pub fn frame(&self, dt: f32) -> Result<(), JsValue> {
        let ctx = &mut *self.ctx.borrow_mut();
        if ctx.device_lost.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Split the borrow: the scene needs `&mut` on itself *and* on the
        // renderer, which live in different fields of the same struct (SPEC §1.2).
        let WebContext { renderer, device, queue, surfaces, .. } = ctx;
        let entry = surfaces.get_mut(self.id).ok_or_else(|| gone("surface"))?;

        let mut players: Vec<&mut SSPlayer> = entry.players.iter_mut().collect();
        entry.scene.update(renderer, device, queue, dt, &mut players);

        use wgpu::CurrentSurfaceTexture;
        let frame = match entry.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(f) | CurrentSurfaceTexture::Suboptimal(f) => f,
            // Lost/outdated/timeout (e.g. tab restore): reconfigure and skip this frame.
            _ => {
                entry.surface.configure(device, &entry.config);
                return Ok(());
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ss-web frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store }, depth_slice: None })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &entry.stencil_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: entry.scene.timestamp_writes(),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            entry.scene.draw(renderer, &mut rpass);
        }
        entry.scene.resolve_timestamps(&mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    /// Resize the swapchain, the stencil buffer and the projection to the
    /// canvas's new pixel size.
    pub fn resize(&self, width: u32, height: u32) {
        let ctx = &mut *self.ctx.borrow_mut();
        let lost = ctx.device_lost.load(Ordering::Relaxed);
        let WebContext { device, queue, surfaces, .. } = ctx;
        let Some(entry) = surfaces.get_mut(self.id) else { return };
        entry.config.width = width.max(1);
        entry.config.height = height.max(1);
        // While the device is gone the new size is only recorded; `restoreDevice`
        // configures the surface and the stencil from `config` on the way back.
        if lost {
            return;
        }
        entry.surface.configure(device, &entry.config);
        entry.stencil_view = create_stencil_view(device, entry.config.width, entry.config.height);
        entry.scene.set_viewport(queue, entry.config.width as f32, entry.config.height as f32, false);
    }

    /// What the last frame cost, as a plain JS object (SPEC §14.2 item 10).
    ///
    /// `gpuPassTimeNs` is `undefined` unless the browser granted
    /// `timestamp-query`, and lags one or two frames by design. The counters are
    /// always available: `prims` vs `drawCalls` is the headroom draw-call merging
    /// would have, and `redundantTextureBinds` the headroom bind-group dedup
    /// would — both measured on native, neither yet measured here.
    pub fn stats(&self) -> Result<js_sys::Object, JsValue> {
        let ctx = &mut *self.ctx.borrow_mut();
        let entry = ctx.surfaces.get_mut(self.id).ok_or_else(|| gone("surface"))?;
        let s = entry.scene.stats();
        let out = js_sys::Object::new();
        let set = |key: &str, value: JsValue| {
            let _ = js_sys::Reflect::set(&out, &JsValue::from_str(key), &value);
        };
        set("players", s.players.into());
        set("prims", s.prims.into());
        set("drawCalls", s.draw_calls.into());
        set("uploadedBytes", (s.uploaded_bytes as f64).into());
        set("mergeableDraws", s.mergeable_draws.into());
        set("redundantTextureBinds", s.redundant_texture_binds.into());
        if let Some(ns) = s.gpu_pass_time_ns {
            set("gpuPassTimeNs", (ns as f64).into());
        }
        Ok(out)
    }

    /// How many players draw into this canvas (diagnostic).
    #[wasm_bindgen(js_name = playerCount)]
    pub fn player_count(&self) -> usize {
        self.ctx.borrow_mut().surfaces.get_mut(self.id).map(|e| e.players.len()).unwrap_or(0)
    }

    /// Detach this canvas and release its swapchain, scene and players.
    ///
    /// Explicit because dropping the JS wrapper cannot do it: the surface lives
    /// in the shared context, not in this handle. Calling it twice, or calling
    /// anything else afterwards, is safe — the handle resolves to "destroyed".
    pub fn destroy(&self) {
        self.ctx.borrow_mut().surfaces.remove(self.id);
    }
}

/// One character (SPEC §9.3). Holds the playback tree and its placement in the
/// scene — and no GPU resource at all, which is what makes it cheap to have many.
#[wasm_bindgen]
pub struct SsPlayerWeb {
    ctx: Rc<RefCell<WebContext>>,
    surface: SlotId,
    id: SlotId,
}

/// Run `body` on the player this handle names.
///
/// A macro rather than a helper method because the closure would have to borrow
/// the `RefCell` for the caller, and every one of these is a one-liner.
macro_rules! with_player {
    ($self:ident, |$player:ident| $body:expr) => {{
        let ctx = &mut *$self.ctx.borrow_mut();
        let entry = ctx.surfaces.get_mut($self.surface).ok_or_else(|| gone("surface"))?;
        let $player = entry.players.get_mut($self.id).ok_or_else(|| gone("player"))?;
        #[allow(clippy::let_and_return)]
        let out = $body;
        Ok(out)
    }};
}

#[wasm_bindgen]
impl SsPlayerWeb {
    /// Register + select an embedded `.ssab` (its atlas textures are decoded and
    /// uploaded automatically on the next frame). `pack` names the pack for
    /// external instance references.
    #[wasm_bindgen(js_name = loadSsab)]
    pub fn load_ssab(&self, pack: &str, bytes: &[u8]) -> Result<(), JsValue> {
        with_player!(self, |p| p.load_ssab(pack, bytes).map_err(js_err)?)
    }

    /// Register an additional pack (for external instance references) without
    /// changing what plays.
    #[wasm_bindgen(js_name = addSsab)]
    pub fn add_ssab(&self, pack: &str, bytes: &[u8]) -> Result<(), JsValue> {
        with_player!(self, |p| p.add_ssab(pack, bytes).map_err(js_err)?)
    }

    /// The animation names in the loaded pack.
    #[wasm_bindgen(js_name = animationNames)]
    pub fn animation_names(&self) -> Result<Vec<String>, JsValue> {
        with_player!(self, |p| p.animation_names().unwrap_or_default())
    }

    /// Select an animation: builds the instance tree, and the only call here that
    /// allocates. Separate from [`SsPlayerWeb::play`] on purpose (SPEC §9.3) —
    /// the reference implementation split the same way, so the old single
    /// `play(name)` is deliberately not carried over.
    #[wasm_bindgen(js_name = setAnimation)]
    pub fn set_animation(&self, name: &str) -> Result<(), JsValue> {
        with_player!(self, |p| p.set_animation(name).map_err(js_err)?)
    }

    /// Start playing, or resume after [`SsPlayerWeb::pause`].
    pub fn play(&self) -> Result<(), JsValue> {
        with_player!(self, |p| p.play())
    }

    /// Hold the current frame. [`SsPlayerWeb::play`] resumes from it.
    pub fn pause(&self) -> Result<(), JsValue> {
        with_player!(self, |p| p.pause())
    }

    /// Stop and rewind to the first frame.
    pub fn stop(&self) -> Result<(), JsValue> {
        with_player!(self, |p| p.stop())
    }

    /// Playback rate multiplier (1.0 = as authored).
    #[wasm_bindgen(js_name = setSpeed)]
    pub fn set_speed(&self, speed: f32) -> Result<(), JsValue> {
        with_player!(self, |p| p.set_speed(speed))
    }

    /// Whether this player is currently advancing.
    #[wasm_bindgen(js_name = isPlaying)]
    pub fn is_playing(&self) -> Result<bool, JsValue> {
        with_player!(self, |p| p.is_playing())
    }

    /// Place this player in the scene, in pixels from the canvas centre.
    /// `rotation` is in radians.
    #[wasm_bindgen(js_name = setTransform)]
    pub fn set_transform(&self, x: f32, y: f32, scale_x: f32, scale_y: f32, rotation: f32) -> Result<(), JsValue> {
        with_player!(self, |p| p.set_transform(x, y, scale_x, scale_y, rotation))
    }

    /// Back to the identity placement (centred, unscaled, unrotated).
    #[wasm_bindgen(js_name = resetTransform)]
    pub fn reset_transform(&self) -> Result<(), JsValue> {
        with_player!(self, |p| p.reset_transform())
    }

    /// Draw order within the canvas; lower draws first (SPEC §9.2).
    #[wasm_bindgen(js_name = setOrder)]
    pub fn set_order(&self, order: i32) -> Result<(), JsValue> {
        with_player!(self, |p| p.set_order(order))
    }

    /// Hide this player. A hidden player is skipped by both update and draw.
    #[wasm_bindgen(js_name = setVisible)]
    pub fn set_visible(&self, visible: bool) -> Result<(), JsValue> {
        with_player!(self, |p| p.set_visible(visible))
    }

    /// Player-level fade in 0..1, multiplied into every part.
    #[wasm_bindgen(js_name = setAlpha)]
    pub fn set_alpha(&self, alpha: f32) -> Result<(), JsValue> {
        with_player!(self, |p| p.set_alpha(alpha))
    }

    /// The authored canvas size of the selected animation, as `[width, height]`,
    /// or an empty array when nothing is loaded. Useful for scaling a character
    /// to fit the element it was dropped into.
    #[wasm_bindgen(js_name = canvasSize)]
    pub fn canvas_size(&self) -> Result<Vec<f32>, JsValue> {
        with_player!(self, |p| p.canvas_size().map(|(w, h)| vec![w, h]).unwrap_or_default())
    }

    /// Remove this player from its canvas. Safe to call twice; afterwards every
    /// method on this handle reports that it is destroyed rather than acting on
    /// whichever player later reuses the slot (SPEC §11).
    pub fn destroy(&self) {
        let ctx = &mut *self.ctx.borrow_mut();
        if let Some(entry) = ctx.surfaces.get_mut(self.surface) {
            entry.players.remove(self.id);
        }
    }
}
