//! The smallest program that puts a SpriteStudio animation on screen: one winit
//! window, one character, fitted into the window.
//!
//! It is the [Rust quick start](../../docs/en/start/rust.md) as a program, step
//! for step. `examples/viewer` is where the rest of the surface is exercised.
//!
//! ```text
//! ./scripts/deploy-examples.sh   # once — converts the SDK's sample projects into assets/
//! cargo run -p ssplayer-wgpu-hello
//! ```

use std::sync::Arc;
use std::time::Instant;

use ssplayer_wgpu::{MaskBackend, Placement, SSPlayer, SSRenderer, SSScene, SceneConfig};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

const PACK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/Ringo/Ringo.ssab");
const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

struct Hello {
    window: Arc<Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    stencil: wgpu::TextureView,
    renderer: SSRenderer,
    scene: SSScene,
    player: SSPlayer,
    last_frame: Instant,
}

impl Hello {
    fn new(window: Arc<Window>) -> Self {
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        // Vulkan / Metal / DX12. The player has no GL path, so GL is left out.
        instance_desc.backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(instance_desc);
        let surface = instance.create_surface(window.clone()).expect("no surface for this window");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("no GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("no device");

        let capabilities = surface.get_capabilities(&adapter);
        // A non-sRGB target: the player writes atlas bytes as they are, and an sRGB
        // target would encode them a second time.
        let format = capabilities.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(capabilities.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
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

        // One renderer per device, one scene per render target.
        let renderer = SSRenderer::new(&device);
        let scene = SSScene::new(&device, &SceneConfig { color_format: format, stencil_format: Some(STENCIL_FORMAT), mask: MaskBackend::Stencil });

        // One character. `load` brings the pack, its atlas and the packs its
        // instance parts play, in one call.
        let mut player = SSPlayer::new();
        if let Err(err) = player.load(PACK) {
            panic!("could not read {PACK} ({err:?}) — run scripts/deploy-examples.sh first");
        }
        let first = player.animation_names().and_then(|names| names.into_iter().next()).expect("the pack has no animation");
        player.set_animation(&first).expect("an animation the pack itself listed");
        player.play();

        let stencil = stencil_view(&device, config.width, config.height);
        let mut hello = Self { window, device, queue, surface, config, stencil, renderer, scene, player, last_frame: Instant::now() };
        hello.place();
        hello
    }

    /// Match the projection to the window, and fit the character into it.
    fn place(&mut self) {
        let (width, height) = (self.config.width as f32, self.config.height as f32);
        self.scene.set_viewport(&self.queue, width, height, false);
        self.player.fit_into(width, height, &Placement::default());
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.stencil = stencil_view(&self.device, width, height);
        self.place();
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let delta_secs = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        // 1. Advance the character and build this target's geometry.
        self.scene.update(&mut self.renderer, &self.device, &self.queue, delta_secs, &mut [&mut self.player]);

        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

        // 2. Bake mask coverage. A no-op under the stencil backend; it has to go on
        //    the same encoder as the pass below if the scene ever switches.
        self.scene.encode_mask_coverage(&mut self.renderer, &self.device, &mut encoder);

        // 3. Draw into a pass this program opened.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hello"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.12, g: 0.12, b: 0.16, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.stencil, depth_ops: None, stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }) }),
                ..Default::default()
            });
            self.scene.draw(&self.renderer, &mut rpass);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}

/// The stencil attachment `MaskBackend::Stencil` draws masks with, sized to the window.
fn stencil_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("hello stencil"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: STENCIL_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

#[derive(Default)]
struct App {
    hello: Option<Hello>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.hello.is_none() {
            let attributes = WindowAttributes::default().with_title("SpriteStudio Player for wgpu — hello").with_inner_size(winit::dpi::LogicalSize::new(640.0, 640.0));
            let window = Arc::new(event_loop.create_window(attributes).expect("no window"));
            self.hello = Some(Hello::new(window));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(hello) = &mut self.hello else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => hello.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                hello.frame();
                hello.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("no event loop");
    event_loop.run_app(&mut App::default()).expect("event loop failed");
}
