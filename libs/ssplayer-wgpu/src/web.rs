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
//! the `.ssab`, so a single fetched file needs no sibling image files. **The
//! decoding of those PNGs is the browser's job here, not the player's** — the
//! `web` feature carries no image decoder, because one is already installed on
//! every page. [`SsPlayerWeb::embedded_textures`] hands out the encoded bytes and
//! [`SsContextWeb::add_texture_from_bytes`] runs them through `createImageBitmap`,
//! which is the same division of labour as `ssplayer-core`'s
//! `extractEmbeddedTextures` + `ssplayer-webgl`'s `loadTextureFromBytes`, and the
//! reason this face is ~30% smaller over the wire than one that bundles `image`.

use crate::pack::SSPack;
use crate::player::{AssetLoader, SSPlayer};
use crate::renderer::{AlphaMode, Retention, SSRenderer};
use crate::roster::{Roster, SlotId};
use crate::scene::{SSScene, SceneConfig};
use std::cell::RefCell;
use std::collections::HashMap;
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

/// Decode encoded image bytes (PNG) with **the browser's** decoder.
///
/// `createImageBitmap` decodes off the main thread and premultiplies on the way,
/// which is why the web face ships no decoder of its own: `image` would cost wasm
/// size to do the same job worse, on the main thread. The premultiply matters as
/// much as the decode — the whole pipeline composites premultiplied (SPEC §5.2),
/// and this is the WebGPU equivalent of the WebGL player's
/// `UNPACK_PREMULTIPLY_ALPHA_WEBGL` (`SSWebGLResourceManager.createTextureFromSource`).
async fn decode_image(bytes: &[u8]) -> Result<web_sys::ImageBitmap, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no `window` (this face needs a document)"))?;
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(parts.as_ref())?;

    let options = web_sys::ImageBitmapOptions::new();
    options.set_premultiply_alpha(web_sys::PremultiplyAlpha::Premultiply);
    let promise = window.create_image_bitmap_with_blob_and_image_bitmap_options(&blob, &options)?;

    wasm_bindgen_futures::JsFuture::from(promise).await?.dyn_into::<web_sys::ImageBitmap>().map_err(|_| JsValue::from_str("createImageBitmap did not return an ImageBitmap"))
}

/// Report a recoverable problem the way the sibling players do — one missing
/// atlas or sub-pack should cost you that one thing, not the character
/// (`SS6PlayerForGodot`'s `ERR_PRINT` + `continue`, `PackResolver`'s
/// `console.warn` + `continue`).
fn warn(message: &str) {
    web_sys::console::warn_1(&JsValue::from_str(message));
}

/// Fetch every path **once**, in parallel, resolved against `base_url` (or the
/// document's URL).
///
/// Used for both of the things a pack asks the host to go and get: its atlases
/// (SPEC §8.1) and the sibling `.ssab`s its instance parts play from (§7.1).
///
/// One request per path rather than per requester: `external_textures()` can
/// name an image a cellmap already named, under a different hash, and two packs
/// can reference a third — neither must become two downloads. `allSettled`
/// rather than `all` so a single 404 costs one file instead of every file.
async fn fetch_files(paths: &[String], base_url: Option<&str>) -> HashMap<String, Rc<[u8]>> {
    let mut out = HashMap::new();
    let Some(window) = web_sys::window() else {
        warn("[ssplayer-wgpu] no `window`: nothing can be fetched");
        return out;
    };
    let base = base_url.map(str::to_owned).or_else(|| window.document().and_then(|d| d.url().ok()));

    let requests = js_sys::Array::new();
    let mut ordered: Vec<&String> = Vec::new();
    for path in paths {
        let url = match base.as_deref().and_then(|base| web_sys::Url::new_with_base(path, base).ok()) {
            Some(url) => url.href(),
            None => {
                warn(&format!("[ssplayer-wgpu] {path}: cannot resolve against a base URL; pass `baseUrl` to loadSsab"));
                continue;
            }
        };
        requests.push(&window.fetch_with_str(&url));
        ordered.push(path);
    }
    if ordered.is_empty() {
        return out;
    }

    let settled = match wasm_bindgen_futures::JsFuture::from(js_sys::Promise::all_settled(&requests)).await {
        Ok(value) => js_sys::Array::from(&value),
        Err(e) => {
            warn(&format!("[ssplayer-wgpu] fetch failed: {e:?}"));
            return out;
        }
    };

    // Second round: the bodies. `Response` resolving says nothing about the body
    // having arrived, and `allSettled` again so one truncated response is one
    // missing atlas.
    let bodies = js_sys::Array::new();
    let mut with_body: Vec<&String> = Vec::new();
    for (i, path) in ordered.iter().enumerate() {
        let entry = settled.get(i as u32);
        let value = js_sys::Reflect::get(&entry, &JsValue::from_str("value")).unwrap_or(JsValue::UNDEFINED);
        let Ok(response) = value.dyn_into::<web_sys::Response>() else {
            warn(&format!("[ssplayer-wgpu] {path}: fetch rejected"));
            continue;
        };
        if !response.ok() {
            warn(&format!("[ssplayer-wgpu] {path}: HTTP {}", response.status()));
            continue;
        }
        match response.array_buffer() {
            Ok(promise) => {
                bodies.push(&promise);
                with_body.push(path);
            }
            Err(e) => warn(&format!("[ssplayer-wgpu] {path}: {e:?}")),
        }
    }
    if with_body.is_empty() {
        return out;
    }

    let settled = match wasm_bindgen_futures::JsFuture::from(js_sys::Promise::all_settled(&bodies)).await {
        Ok(value) => js_sys::Array::from(&value),
        Err(e) => {
            warn(&format!("[ssplayer-wgpu] body read failed: {e:?}"));
            return out;
        }
    };
    for (i, path) in with_body.iter().enumerate() {
        let entry = settled.get(i as u32);
        let value = js_sys::Reflect::get(&entry, &JsValue::from_str("value")).unwrap_or(JsValue::UNDEFINED);
        match value.dyn_into::<js_sys::ArrayBuffer>() {
            Ok(buffer) => {
                out.insert((*path).clone(), Rc::from(js_sys::Uint8Array::new(&buffer).to_vec()));
            }
            Err(_) => warn(&format!("[ssplayer-wgpu] {path}: body could not be read")),
        }
    }
    out
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
    /// The **encoded** bytes behind every texture supplied through
    /// [`SsContextWeb::add_texture_from_bytes`], kept so a device loss can be
    /// repaired without the page re-fetching or re-reading anything (SPEC §14).
    ///
    /// Affordable precisely because they are encoded: a 2048² atlas is a few
    /// hundred KB here against the 16 MB its pixels would take under
    /// [`Retention::Retain`]. The same trade the WebGL player makes by retaining
    /// its `{ kind: 'bytes' }` source rather than the decoded image.
    encoded_textures: HashMap<u32, Rc<[u8]>>,
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

    /// Upload a decoded `bitmap` and register it under `hash`.
    ///
    /// Goes through [`SSRenderer::set_texture`] rather than `upload_texture`
    /// because the pixels never pass through Rust: `copyExternalImageToTexture`
    /// moves them from the browser's decoded image straight into the texture,
    /// with none of the CPU premultiply pass (and none of its 16 MB temporary,
    /// SPEC §14) that `upload_texture` needs. The renderer only borrows the view
    /// — and since wgpu handles are refcounted, dropping our `Texture` here is
    /// fine: the view it hands over keeps it alive.
    fn set_texture_from_bitmap(&mut self, hash: u32, bitmap: &web_sys::ImageBitmap) {
        let (width, height) = (bitmap.width(), bitmap.height());
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SpriteStudio Atlas (ImageBitmap)"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            // RENDER_ATTACHMENT is not a use of ours: WebGPU requires it of any
            // `copyExternalImageToTexture` destination.
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        self.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo { source: wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()), origin: wgpu::Origin2d::ZERO, flip_y: false },
            wgpu::CopyExternalImageDestInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
                color_space: wgpu::PredefinedColorSpace::Srgb,
                // The bitmap was asked for premultiplied and the destination is
                // declared premultiplied, so the copy converts nothing.
                premultiplied_alpha: true,
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.renderer.set_texture(&self.device, hash, &view, (width, height), AlphaMode::Premultiplied);
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

        Ok(SsContextWeb { inner: Rc::new(RefCell::new(WebContext { renderer, device, queue, surfaces: Roster::default(), adapter, device_lost, encoded_textures: HashMap::new(), instance })) })
    }

    /// Attach a canvas and get the scene that draws into it. Any number of
    /// canvases may share this context.
    #[wasm_bindgen(js_name = createSurface)]
    pub fn create_surface(&self, canvas: web_sys::HtmlCanvasElement) -> Result<SsSurfaceWeb, JsValue> {
        let id = self.inner.borrow_mut().add_surface(canvas)?;
        Ok(SsSurfaceWeb { ctx: Rc::clone(&self.inner), id })
    }

    /// Supply a texture by hash from **raw straight-alpha RGBA** (SPEC §8.1).
    ///
    /// Device-scoped, so it covers every canvas and every player at once — which
    /// is exactly why it is here and not on a surface or a player.
    ///
    /// Prefer [`SsContextWeb::add_texture_from_bytes`] when what you have is an
    /// encoded image: this path premultiplies on the CPU and keeps the pixels for
    /// device-loss recovery, so a 2048² atlas costs two 16 MB passes through wasm
    /// memory that `createImageBitmap` does not.
    #[wasm_bindgen(js_name = addTexture)]
    pub fn add_texture(&self, hash: u32, rgba: &[u8], width: u32, height: u32) {
        let ctx = &mut *self.inner.borrow_mut();
        let WebContext { renderer, device, queue, encoded_textures, .. } = ctx;
        // One hash, one supplier: whichever path registered it last owns the
        // recovery too, so the other's retained source must go.
        encoded_textures.remove(&hash);
        renderer.upload_texture(device, queue, hash, rgba, (width, height), Retention::Retain);
    }

    /// Supply a texture by hash from **encoded image bytes** (PNG) — the main
    /// texture path of this face (SPEC §8.1, §8.2).
    ///
    /// Both `.ssab` forms end here. An embedded pack's atlases come from
    /// [`SsPlayerWeb::embedded_textures`] and need no fetch; a reference pack's
    /// come from fetching each [`SsPlayerWeb::texture_refs`] entry whose
    /// `embedded` is false, resolved against the `.ssab`'s own URL. That is the
    /// same shape as `ssplayer-core`'s `extractEmbeddedTextures` /
    /// `extractTexturePaths` pair, and the reason this face supports both forms
    /// without carrying a decoder.
    ///
    /// The bytes are kept (they are the encoded image, not the pixels) so
    /// [`SsContextWeb::restore_device`] can rebuild this texture on its own.
    #[wasm_bindgen(js_name = addTextureFromBytes)]
    pub async fn add_texture_from_bytes(&self, hash: u32, bytes: Vec<u8>) -> Result<(), JsValue> {
        upload_encoded(&self.inner, hash, Rc::from(bytes)).await
    }

    /// Supply a texture by hash from an `ImageBitmap` the **page** decoded.
    ///
    /// For a host with its own asset pipeline — the analogue of
    /// [`crate::SSRenderer::set_texture`] and of the WebGL player's
    /// `bindTextureSource`. The bitmap must be premultiplied
    /// (`createImageBitmap(blob, { premultiplyAlpha: 'premultiply' })`), which is
    /// what the whole pipeline composites in (SPEC §5.2).
    ///
    /// Device loss is the supplier's to repair on this path, exactly as on the
    /// Rust one: nothing is retained here, so the page registers it again after
    /// [`SsContextWeb::restore_device`]. Use
    /// [`SsContextWeb::add_texture_from_bytes`] to be spared that.
    #[wasm_bindgen(js_name = addTextureFromBitmap)]
    pub fn add_texture_from_bitmap(&self, hash: u32, bitmap: &web_sys::ImageBitmap) {
        let ctx = &mut *self.inner.borrow_mut();
        ctx.encoded_textures.remove(&hash);
        ctx.set_texture_from_bitmap(hash, bitmap);
    }

    /// Whether an atlas is already registered for `hash`.
    ///
    /// The texture map is device-scoped (SPEC §1.2), so this answers for the
    /// whole page: it is how a caller keeps "N players of one pack decode its
    /// atlas once between them" true now that the decode happens in JS. The
    /// player-side flush gets that for free (it filters before the decode,
    /// SPEC §8.2); a page supplying textures itself has to ask.
    #[wasm_bindgen(js_name = hasTexture)]
    pub fn has_texture(&self, hash: u32) -> bool {
        self.inner.borrow().renderer.texture_size(hash).is_some()
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
    /// of any `.ssab` and no re-fetch of any atlas.
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

        let encoded: Vec<(u32, Rc<[u8]>)> = {
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
            ctx.encoded_textures.iter().map(|(&hash, bytes)| (hash, Rc::clone(bytes))).collect()
        };

        // The atlases this context decoded itself went with the old device:
        // `set_texture` is the supplier's to repair (SPEC §8.2), and here the
        // supplier is us. Re-decoding is why the encoded bytes were kept — the
        // page fetches nothing. Last, so that a hash claimed by both an explicit
        // supply and an embedded pack ends up where it was before the loss:
        // `addTextureFrom*` overwrites, on this path as on the first one.
        for (hash, bytes) in encoded {
            upload_encoded(&self.inner, hash, bytes).await?;
        }
        Ok(())
    }

    /// How many canvases this context currently drives (diagnostic).
    #[wasm_bindgen(js_name = surfaceCount)]
    pub fn surface_count(&self) -> usize {
        self.inner.borrow().surfaces.len()
    }
}

/// Decode `bytes` in the browser, upload the result under `hash` and retain the
/// bytes for device-loss recovery.
///
/// A free function because both faces need it: the context supplies textures the
/// page hands over, and a player supplies its own pack's (SPEC §8.1). Takes an
/// `Rc` so the restore path can re-run it without copying every retained atlas.
/// Nothing is borrowed across the await — a `RefMut` held over a yield point
/// would still be held when JS re-enters through a rAF callback.
async fn upload_encoded(ctx: &Rc<RefCell<WebContext>>, hash: u32, bytes: Rc<[u8]>) -> Result<(), JsValue> {
    let bitmap = decode_image(&bytes).await?;
    {
        let ctx = &mut *ctx.borrow_mut();
        ctx.set_texture_from_bitmap(hash, &bitmap);
        ctx.encoded_textures.insert(hash, bytes);
    }
    // Ours to decode, ours to release: the copy captured the pixels already.
    bitmap.close();
    Ok(())
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

/// The web half of [`AssetLoader`] (SPEC §8.1): `fetch` for files,
/// `createImageBitmap` for images.
///
/// The counterpart of the filesystem loader behind [`SSPlayer::load`], and the
/// reason the walk itself —
/// both texture tables, deduplication by file, transitive instance packs — is
/// written once in [`SSPlayer::load`] rather than twice. What differs is only
/// what a browser can do that a filesystem cannot, and the other way round.
///
/// Holds the context by `Rc` and takes its `RefMut` for a moment at a time,
/// never across an await: JS re-enters through rAF callbacks, and a borrow held
/// over a yield point would still be held when it does.
struct WebAssetLoader {
    ctx: Rc<RefCell<WebContext>>,
    /// What the pack's relative paths resolve against — the `.ssab`'s own URL.
    /// `None` falls back to the document's.
    base_url: Option<String>,
}

impl AssetLoader for WebAssetLoader {
    /// One `fetch` per path, settled together: this is the face where overlapping
    /// the waiting is the difference between one round trip and N.
    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        let fetched = fetch_files(paths, self.base_url.as_deref()).await;
        paths.iter().map(|path| fetched.get(path).map(|bytes| bytes.to_vec())).collect()
    }

    async fn supply_texture(&mut self, hash: u32, encoded: &[u8]) {
        if let Err(e) = upload_encoded(&self.ctx, hash, Rc::from(encoded)).await {
            self.warn(&format!("an image for {hash:#010x} could not be decoded: {e:?}"));
        }
    }

    fn has_texture(&self, hash: u32) -> bool {
        self.ctx.borrow().renderer.texture_size(hash).is_some()
    }

    fn warn(&self, message: &str) {
        warn(&format!("[ssplayer-wgpu] {message}"));
    }
}

impl SsPlayerWeb {
    /// Parse `bytes` and register the pack, selecting it when `select`.
    ///
    /// A pack keeps the name the exporter wrote unless the caller overrides it,
    /// because that is the name external instance parts are resolved against
    /// (`ref_anime_pack`, SPEC §7.1) and the exporter writes the same string on
    /// both sides. An override is a promise that the caller knows better.
    fn register(&self, bytes: &[u8], pack: Option<String>, select: bool) -> Result<SSPack, JsValue> {
        let pack = match pack.as_deref() {
            Some(name) => SSPack::load_named(name, bytes),
            None => SSPack::load(bytes),
        }
        .map_err(js_err)?;
        // Annotated because the macro's `Ok(out)` leaves the error type to the
        // caller, and here the value is discarded rather than returned.
        let registered: Result<(), JsValue> = with_player!(self, |p| if select { p.set_pack(&pack) } else { p.add_pack(&pack) });
        registered?;
        Ok(pack)
    }

    /// Run `body` on this handle's player, **outside** the context's `RefCell`.
    ///
    /// [`SSPlayer::load`] holds `&mut SSPlayer` across its awaits, and on this
    /// face the player lives inside the one cell everything else borrows — so it
    /// is lifted out for the duration and put back after. The slot keeps a blank
    /// player meanwhile, which draws nothing: it has no animation yet, which is
    /// the only state a load can be in.
    ///
    /// If the handle is destroyed while the load is in flight, the finished
    /// player is dropped rather than written back over its successor — the same
    /// rule the generation-tagged slots exist for (SPEC §11).
    async fn with_player_detached<F, Fut, T>(&self, body: F) -> Result<T, JsValue>
    where
        F: FnOnce(SSPlayer) -> Fut,
        Fut: core::future::Future<Output = (SSPlayer, T)>,
    {
        let taken = {
            let ctx = &mut *self.ctx.borrow_mut();
            let entry = ctx.surfaces.get_mut(self.surface).ok_or_else(|| gone("surface"))?;
            let slot = entry.players.get_mut(self.id).ok_or_else(|| gone("player"))?;
            core::mem::take(slot)
        };

        let (player, out) = body(taken).await;

        let ctx = &mut *self.ctx.borrow_mut();
        if let Some(entry) = ctx.surfaces.get_mut(self.surface)
            && let Some(slot) = entry.players.get_mut(self.id)
        {
            *slot = player;
        }
        Ok(out)
    }
}

#[wasm_bindgen]
impl SsPlayerWeb {
    /// Register + select a `.ssab`, **and load the textures it needs** (SPEC §8.1).
    ///
    /// The shape of `SS6PlayerForGodot`'s `setSSABResource`, which loads the
    /// pack's textures before returning and leaves `setAnimation` a pure
    /// selection. Texture loading belongs to initialisation, not to animation
    /// selection: the porting guide puts it there
    /// (`ssruntime/docs/30_core/10_initialization.ja.md` §3), and switching
    /// motions is a frequent, synchronous operation that must not carry a
    /// `Promise`. [`SsPlayerWeb::set_animation`] therefore stays synchronous.
    ///
    /// This is async because a browser cannot do otherwise — `fetch` and
    /// `createImageBitmap` are asynchronous where Godot's `ResourceLoader::load`
    /// is not. **That is the one place the asynchrony appears.**
    ///
    /// * `pack` names the pack for external instance references (`ref_anime_pack`,
    ///   SPEC §7.1). Pass `null`/`undefined` — the usual case — to use the name
    ///   the exporter wrote into the `.ssab`, which is the name those references
    ///   are resolved against. A name of your own overrides it, and then any
    ///   external instance reference to this pack stops resolving.
    /// * `baseUrl` is where the `.ssab` came from; reference-mode atlas paths are
    ///   relative to it. Defaults to the document's URL. Embedded packs ignore it.
    ///
    /// An atlas the renderer already has is left alone ("registered wins",
    /// SPEC §8.2), so N players of one pack load it once between them. An atlas
    /// that cannot be fetched or decoded is reported on the console and skipped,
    /// as in `_load_external_ssabs`' `ERR_PRINT` and `PackResolver`'s
    /// `console.warn`: one missing image should not cost you the character.
    #[wasm_bindgen(js_name = loadSsab)]
    pub async fn load_ssab(&self, bytes: Vec<u8>, pack: Option<String>, base_url: Option<String>) -> Result<(), JsValue> {
        let registered = self.register(&bytes, pack, true)?;
        let mut loader = WebAssetLoader { ctx: Rc::clone(&self.ctx), base_url };
        let name = registered.name().to_string();
        self.with_player_detached(|mut player| async move {
            // The pack is registered; only the assets around it are left, so the
            // walk starts from what is already loaded rather than re-reading it.
            player.load_assets(&mut loader, &name, "").await;
            (player, ())
        })
        .await
    }

    /// Fetch a `.ssab` **and everything it needs**, then select it (SPEC §8.1, §7.1).
    ///
    /// The shortest form, and the one `ssplayer-core`'s `load(ssabUrl)` takes: a
    /// URL is the one input that answers both questions a pack raises — what the
    /// bytes are, and what its sibling files resolve against. `src` may be
    /// absolute or relative (resolved against the document, like any URL a page
    /// uses), and the pack keeps the name the exporter gave it.
    ///
    /// This is [`SSPlayer::load`] with the browser's loader plugged in — the same
    /// call, the same walk, as a native host makes with
    /// [`SSPlayer::load`]. Use [`SsPlayerWeb::load_ssab`] when the bytes
    /// come from somewhere that is not URL-addressable — a drop, a bundler, an
    /// archive — and pass `baseUrl` if that pack still has siblings to find.
    pub async fn load(&self, src: &str) -> Result<(), JsValue> {
        // No base of our own: `src` is already what the document resolves, and the
        // walk joins the pack's siblings onto its directory before asking. Giving
        // the loader the `.ssab`'s own URL instead would resolve `src` against
        // itself — `assets/assets/Basic.ssab` — which is what
        // `SSPlayer::load` avoids by rooting its loader at the same place the path
        // is relative to.
        let mut loader = WebAssetLoader { ctx: Rc::clone(&self.ctx), base_url: None };
        let src = src.to_owned();
        self.with_player_detached(|mut player| async move {
            let out = player.load_with(&mut loader, &src).await;
            (player, out)
        })
        .await?
        .map_err(js_err)
    }

    /// [`SsPlayerWeb::load_ssab`] without changing what plays: registers an
    /// additional pack for external instance references, and loads its assets
    /// too — a sub-animation's cells come out of its own pack's atlases.
    ///
    /// Rarely needed now that [`SsPlayerWeb::load`] resolves those references
    /// itself; it remains for packs that are not URL-addressable, the case
    /// `ssplayer-core` covers with its `InstancePackProvider` hook.
    #[wasm_bindgen(js_name = addSsab)]
    pub async fn add_ssab(&self, bytes: Vec<u8>, pack: Option<String>, base_url: Option<String>) -> Result<(), JsValue> {
        let registered = self.register(&bytes, pack, false)?;
        let mut loader = WebAssetLoader { ctx: Rc::clone(&self.ctx), base_url };
        let name = registered.name().to_string();
        self.with_player_detached(|mut player| async move {
            player.load_assets(&mut loader, &name, "").await;
            (player, ())
        })
        .await
    }

    /// The animation names in the loaded pack.
    #[wasm_bindgen(js_name = animationNames)]
    pub fn animation_names(&self) -> Result<Vec<String>, JsValue> {
        with_player!(self, |p| p.animation_names().unwrap_or_default())
    }

    /// The atlases a registered pack asks for, as `{ hash, path, embedded }[]`
    /// in pack order (SPEC §8.1). `pack` undefined selects the main pack.
    ///
    /// The counterpart of [`SsContextWeb::add_texture`], and the same shape the
    /// reference implementation exposes (`ssplayer-core`'s `extractTexturePaths`):
    /// only the `.ssab` knows where its atlases were exported to, so a page
    /// showing a reference-mode pack fetches `new URL(path, ssabUrl)` for every
    /// entry whose `embedded` is false and registers the pixels by `hash`.
    /// Embedded entries need no fetch — they arrive with the pack.
    #[wasm_bindgen(js_name = textureRefs)]
    pub fn texture_refs(&self, pack: Option<String>) -> Result<js_sys::Array, JsValue> {
        with_player!(self, |p| {
            let out = js_sys::Array::new();
            for texture in p.textures(pack.as_deref()).unwrap_or_default() {
                let entry = js_sys::Object::new();
                let set = |key: &str, value: JsValue| {
                    let _ = js_sys::Reflect::set(&entry, &JsValue::from_str(key), &value);
                };
                set("hash", texture.hash.into());
                set("path", texture.path.as_str().into());
                set("embedded", texture.embedded.into());
                out.push(&entry);
            }
            out
        })
    }

    /// The atlases a registered pack **carries**, as `{ hash, bytes }[]` — the
    /// encoded images, not pixels (SPEC §8.1). Empty for a reference-mode pack.
    /// `pack` undefined selects the main pack.
    ///
    /// The embedded half of texture supply, and the counterpart of
    /// [`SsPlayerWeb::texture_refs`]: there is nothing to fetch, but there is
    /// still something to decode, and on this face the decoder is the browser.
    /// Hand each entry to [`SsContextWeb::add_texture_from_bytes`].
    ///
    /// Same shape as `ssplayer-core`'s `extractEmbeddedTextures`, and same
    /// reason: it is what lets a player support embedded packs without carrying
    /// an image decoder into the bundle.
    #[wasm_bindgen(js_name = embeddedTextures)]
    pub fn embedded_textures(&self, pack: Option<String>) -> Result<js_sys::Array, JsValue> {
        with_player!(self, |p| {
            let out = js_sys::Array::new();
            for image in p.embedded_images(pack.as_deref()).unwrap_or_default() {
                let entry = js_sys::Object::new();
                let set = |key: &str, value: JsValue| {
                    let _ = js_sys::Reflect::set(&entry, &JsValue::from_str(key), &value);
                };
                set("hash", image.hash.into());
                set("bytes", js_sys::Uint8Array::from(image.bytes).into());
                out.push(&entry);
            }
            out
        })
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
