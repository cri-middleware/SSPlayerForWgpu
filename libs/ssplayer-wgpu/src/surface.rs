//! The swapchain the two faces that own their surface keep alike: the C face's
//! [`crate::host::NativeHost`] and each canvas of the web face.
//!
//! Configuring it, resizing it together with its stencil buffer, acquiring a
//! frame and drawing a scene into it are the same steps on both, and a fix
//! that reached one of two copies and not the other is what this module stops.
//! What does differ is passed in: the alpha mode, the clear colour, and whether
//! a zero size is ignored, which each face decides before calling
//! [`Swapchain::resize`].

use crate::renderer::SSRenderer;
use crate::scene::SSScene;

/// Depth/stencil format allocated when masking is enabled; the depth aspect is
/// unused. Matches the native viewer.
pub const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// `width` x `height`, brought inside what `device` can configure a surface or a
/// stencil buffer at: at least 1, at most `max_texture_dimension_2d`.
///
/// A window or canvas past the limit then draws at the largest size the device
/// has, stretched by the compositor; configuring it at its own size is a
/// validation error and a panic.
pub(crate) fn surface_extent(device: &wgpu::Device, width: u32, height: u32) -> (u32, u32) {
    let max = device.limits().max_texture_dimension_2d;
    (width.clamp(1, max), height.clamp(1, max))
}

/// A surface, its configuration, and the stencil buffer sized with it.
pub(crate) struct Swapchain {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// `None` when built without masking.
    stencil_view: Option<wgpu::TextureView>,
    /// The alpha mode to configure with, or `None` for the first the surface
    /// reports — kept so a restore picks it the same way.
    alpha_mode: Option<wgpu::CompositeAlphaMode>,
}

impl Swapchain {
    /// Configure `surface` for `device` at `width` x `height`, brought inside
    /// what the device can hold, with a stencil buffer when `stencil` is set.
    /// `None` when `adapter` cannot present to it at all.
    ///
    /// A non-sRGB format is preferred: the atlases are sRGB-encoded bytes
    /// sampled as UNORM and the pipeline composites premultiplied as authored,
    /// so an sRGB target would encode them a second time.
    pub fn new(device: &wgpu::Device, adapter: &wgpu::Adapter, surface: wgpu::Surface<'static>, (width, height): (u32, u32), alpha_mode: Option<wgpu::CompositeAlphaMode>, stencil: bool) -> Option<Self> {
        let caps = surface.get_capabilities(adapter);
        let &first = caps.formats.first()?;
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(first);
        let (width, height) = surface_extent(device, width, height);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: alpha_mode.unwrap_or(caps.alpha_modes[0]),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(device, &config);
        let stencil_view = stencil.then(|| create_stencil_view(device, width, height));
        Some(Self { surface, config, stencil_view, alpha_mode })
    }

    /// The surface itself, for the native host to ask for an adapter that can
    /// present to it; the web face asks for one before it has a canvas.
    #[cfg(feature = "ffi")]
    pub fn surface(&self) -> &wgpu::Surface<'static> {
        &self.surface
    }

    /// The colour format the scene drawing into this must be built for.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// The configured size in pixels — or, while the device is lost, the size
    /// the next [`Swapchain::restore`] configures.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Take a new drawable size, brought inside what `device` can hold. While
    /// the device is `lost` the size is only recorded, and
    /// [`Swapchain::restore`] applies it.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32, lost: bool) {
        (self.config.width, self.config.height) = surface_extent(device, width, height);
        if !lost {
            self.configure(device);
        }
    }

    /// Whether `adapter` can present the format this was configured with. The
    /// scene's pipelines are baked against that format, so one that cannot is
    /// a rebuild, not a restore.
    pub fn presents_on(&self, adapter: &wgpu::Adapter) -> bool {
        self.surface.get_capabilities(adapter).formats.contains(&self.config.format)
    }

    /// Configure again on the device that replaced a lost one, at the size last
    /// recorded. Ask [`Swapchain::presents_on`] first.
    pub fn restore(&mut self, device: &wgpu::Device, adapter: &wgpu::Adapter) {
        self.config.alpha_mode = self.alpha_mode.unwrap_or_else(|| self.surface.get_capabilities(adapter).alpha_modes[0]);
        self.configure(device);
    }

    fn configure(&mut self, device: &wgpu::Device) {
        self.surface.configure(device, &self.config);
        if self.stencil_view.is_some() {
            self.stencil_view = Some(create_stencil_view(device, self.config.width, self.config.height));
        }
    }

    /// Acquire this frame's texture, draw `scene` into it over `clear`, and
    /// present it. What the scene draws offscreen goes first, on the same
    /// encoder.
    ///
    /// A lost, outdated or timed-out surface is configured again and the frame
    /// skipped; an occluded one, which is still valid, skips the frame alone.
    /// Both are success, since the next frame draws. Only a configuration the
    /// surface rejects is an error.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut SSRenderer, scene: &mut SSScene, clear: wgpu::Color, label: &'static str) -> Result<(), &'static str> {
        use wgpu::CurrentSurfaceTexture;
        let frame = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => frame,
            CurrentSurfaceTexture::Occluded => return Ok(()),
            CurrentSurfaceTexture::Validation => return Err("surface configuration is invalid"),
            // The size has not changed, so the stencil buffer still fits.
            _ => {
                self.surface.configure(device, &self.config);
                return Ok(());
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        scene.encode_offscreen(renderer, device, &mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store }, depth_slice: None })],
                depth_stencil_attachment: self.stencil_view.as_ref().map(|stencil| wgpu::RenderPassDepthStencilAttachment {
                    view: stencil,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: scene.timestamp_writes(),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            scene.draw(renderer, &mut pass);
        }
        scene.resolve_timestamps(&mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        queue.present(frame);
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
