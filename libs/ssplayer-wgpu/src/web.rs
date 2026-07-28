//! The "js" face (SPEC §12 P1): a `wasm-bindgen` wrapper that renders SpriteStudio
//! animations to an HTML canvas via **WebGPU**.
//!
//! Borrow model, browser edition: this struct is the host. It owns the wgpu stack
//! (instance / surface-from-canvas / device / queue / stencil) and lends the
//! device+queue to the borrow-model [`SSPlayer`] each frame — exactly like the
//! native viewer, but the event loop / rAF is JS (`examples/web`).
//!
//! Embedded ssab (SPEC §8.1) is the natural web form: the atlas PNGs ride inside
//! the `.ssab`, so a single fetched file renders with no host texture supply.

use crate::player::{SSPlayer, SSPlayerConfig};
use wasm_bindgen::prelude::*;

/// Depth/stencil format for masking (matches the native viewer). Masks and the
/// per-part shader gating rely on a stencil buffer being present.
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Map any error into a JS exception value.
fn js_err<E: core::fmt::Debug>(e: E) -> JsValue {
    JsValue::from_str(&format!("{e:?}"))
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

/// A WebGPU SpriteStudio player bound to one `<canvas>`. Construct it with
/// [`SsPlayerWeb::create`] (async — WebGPU adapter/device requests are Promises),
/// load an embedded `.ssab`, `play` an animation, then call [`SsPlayerWeb::frame`]
/// once per `requestAnimationFrame`.
#[wasm_bindgen]
pub struct SsPlayerWeb {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    stencil_view: wgpu::TextureView,
    player: SSPlayer,
}

#[wasm_bindgen]
impl SsPlayerWeb {
    /// Initialise WebGPU on `canvas` and build the player. Rejects if the browser
    /// has no WebGPU adapter/device (the caller should surface a fallback message).
    pub async fn create(canvas: web_sys::HtmlCanvasElement) -> Result<SsPlayerWeb, JsValue> {
        console_error_panic_hook::set_once();

        let width = canvas.width().max(1);
        let height = canvas.height().max(1);

        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::BROWSER_WEBGPU;
        let instance = wgpu::Instance::new(instance_desc);
        let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas)).map_err(js_err)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::default(), compatible_surface: Some(&surface), force_fallback_adapter: false })
            .await
            .map_err(|_| JsValue::from_str("no WebGPU adapter (this browser/GPU may not support WebGPU)"))?;
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.map_err(js_err)?;

        let caps = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB format so the premultiplied pipeline composites as authored.
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration { usage: wgpu::TextureUsages::RENDER_ATTACHMENT, format, width, height, present_mode: wgpu::PresentMode::Fifo, alpha_mode: caps.alpha_modes[0], view_formats: vec![], desired_maximum_frame_latency: 2 };
        surface.configure(&device, &config);
        let stencil_view = create_stencil_view(&device, width, height);

        let player = SSPlayer::new(&device, &SSPlayerConfig { color_format: format, stencil_format: Some(STENCIL_FORMAT) });
        let me = SsPlayerWeb { surface, device, queue, config, stencil_view, player };
        me.player.set_viewport(&me.queue, width as f32, height as f32, false);
        Ok(me)
    }

    /// Register + select an embedded `.ssab` (its atlas textures are decoded and
    /// uploaded automatically on the next `frame`). `pack` names the pack for
    /// external instance references.
    #[wasm_bindgen(js_name = loadSsab)]
    pub fn load_ssab(&mut self, pack: &str, bytes: &[u8]) -> Result<(), JsValue> {
        self.player.load_ssab(pack, bytes).map_err(js_err)
    }

    /// Register an additional pack (for external instance references) without
    /// changing what plays.
    #[wasm_bindgen(js_name = addSsab)]
    pub fn add_ssab(&mut self, pack: &str, bytes: &[u8]) -> Result<(), JsValue> {
        self.player.add_ssab(pack, bytes).map_err(js_err)
    }

    /// The animation names in the loaded pack.
    #[wasm_bindgen(js_name = animationNames)]
    pub fn animation_names(&self) -> Vec<String> {
        self.player.animation_names().unwrap_or_default()
    }

    /// Select `name` and start playing it. Kept as one call because the `<ss-player>`
    /// interface this mirrors has no separate select step (SPEC §10).
    pub fn play(&mut self, name: &str) -> Result<(), JsValue> {
        self.player.set_animation(name).map_err(js_err)?;
        self.player.play();
        Ok(())
    }

    /// Resize the surface + stencil to the canvas's new pixel size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.stencil_view = create_stencil_view(&self.device, self.config.width, self.config.height);
        self.player.set_viewport(&self.queue, self.config.width as f32, self.config.height as f32, false);
    }

    /// Advance by `dt` seconds and render one frame to the canvas.
    pub fn frame(&mut self, dt: f32) -> Result<(), JsValue> {
        self.player.update(&self.device, &self.queue, dt);

        use wgpu::CurrentSurfaceTexture;
        let frame = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(f) | CurrentSurfaceTexture::Suboptimal(f) => f,
            // Lost/outdated/timeout (e.g. tab restore): reconfigure and skip this frame.
            _ => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ss-web frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store }, depth_slice: None })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.stencil_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.player.draw(&mut rpass);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}
