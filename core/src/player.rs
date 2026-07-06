//! Layer 3 (operation): SSPlayer — the primary public API.
//!
//! Owns a runtime `Context`, its bound `.ssab` resource, and an
//! `SSInternalPlayer`. Borrow model: it holds no `Device`/`Queue`/`Surface`;
//! the host lends them on every `update`/`draw`.

use crate::frame::FrameView;
use crate::internal::{RendererConfig, SSInternalPlayer};
use ssruntime::error::SSRuntimeError;
use ssruntime::{Context, SsabData, SsabResource};
use std::sync::Arc;

/// Construction config for [`SSPlayer`], matched to the host's render target.
pub struct SSPlayerConfig {
    /// Target color format. Forwarded to the renderer so pipelines match.
    pub color_format: wgpu::TextureFormat,
}

/// The SpriteStudio player. One instance owns exactly one runtime context.
pub struct SSPlayer {
    context: Context<'static>,
    resource: Option<Arc<SsabResource>>,
    current_animation: Option<String>,
    internal: SSInternalPlayer,
}

impl SSPlayer {
    /// Create a player against a borrowed `device`. The device is not retained.
    pub fn new(device: &wgpu::Device, config: &SSPlayerConfig) -> Self {
        let internal = SSInternalPlayer::new(device, &RendererConfig { color_format: config.color_format });
        Self { context: Context::new(), resource: None, current_animation: None, internal }
    }

    /// Load an `.ssab` resource and bind it to the runtime.
    ///
    /// Ownership contract (SPEC §1.1): the player owns the resource and lends it
    /// to the runtime by borrow. The resource must outlive the bound context.
    pub fn load_ssab(&mut self, data: &[u8]) -> Result<(), SSRuntimeError> {
        let ssab_data = SsabData::load_copy(data)?;
        let resource = SsabResource::new(ssab_data);

        let res_ptr = resource.resource_static_ptr();
        if res_ptr.is_null() {
            return Err(SSRuntimeError::NotLoaded);
        }

        self.resource = Some(resource);
        let static_res = unsafe { &*res_ptr };
        self.context.bind_resource(static_res);
        self.current_animation = None;
        Ok(())
    }

    /// Set up and start the named animation.
    pub fn play(&mut self, anim_name: &str) -> Result<(), SSRuntimeError> {
        self.context.setup_animation(anim_name)?;
        self.context.play();
        self.current_animation = Some(anim_name.to_string());
        Ok(())
    }

    pub fn pause(&mut self) {
        self.context.pause();
    }

    pub fn stop(&mut self) {
        self.context.stop();
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.context.set_animation_speed(speed);
    }

    pub fn is_playing(&self) -> bool {
        self.context.is_playing()
    }

    /// Names of all animations in the loaded resource, if any.
    pub fn animation_names(&self) -> Option<Vec<String>> {
        let anim_resource = self.resource.as_ref()?.resource()?;
        Some(anim_resource.get_animation_names())
    }

    /// Canvas size of the currently playing animation.
    pub fn canvas_size(&self) -> Option<(f32, f32)> {
        let name = self.current_animation.as_ref()?;
        let anim_resource = self.resource.as_ref()?.resource()?;
        let idx = anim_resource.find_animation_index(name)?;
        let anim_meta = &anim_resource.animations()[idx];
        Some((anim_meta.canvas_width, anim_meta.canvas_height))
    }

    /// Provide a decoded texture keyed by fnv1a hash. Forwarded to the renderer.
    pub fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hash: u32, rgba: &[u8], width: u32, height: u32) {
        self.internal.upload_texture(device, queue, hash, rgba, width, height);
    }

    /// Set the projection to the render target's pixel dimensions.
    pub fn set_viewport(&self, queue: &wgpu::Queue, width: f32, height: f32, y_down: bool) {
        self.internal.update_projection(queue, width, height, y_down);
    }

    /// Advance the animation by `delta_secs` and prepare GPU buffers for the frame.
    ///
    /// Borrows the runtime-owned FrameData/SoA into a [`FrameView`] and hands it
    /// to the renderer, which copies what it needs into GPU buffers.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, delta_secs: f32) {
        // The runtime works in milliseconds.
        self.context.update(delta_secs * 1000.0);
        let frame_no = self.context.get_frame_no();

        // `get_frame_data` writes the runtime's internal SoA buffers, then hands
        // back a borrow. We detach it to a raw slice (valid until the next fetch)
        // so it does not alias the subsequent accessor calls.
        let frame_data = match self.context.get_frame_data(frame_no) {
            Ok(bytes) => unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) },
            Err(_) => return,
        };

        // Gather the SoA pointers/lengths (each accessor returns owned values).
        let wm = (self.context.get_world_matrices_ptr(), self.context.get_world_matrices_len());
        let lv = (self.context.get_local_vertices_ptr(), self.context.get_local_vertices_len());
        let lu = (self.context.get_local_uvs_ptr(), self.context.get_local_uvs_len());
        let mvx = (self.context.get_mesh_vertices_x_ptr(), self.context.get_mesh_vertices_x_len());
        let mvy = (self.context.get_mesh_vertices_y_ptr(), self.context.get_mesh_vertices_y_len());
        let mvo = (self.context.get_mesh_vertex_offsets_ptr(), self.context.get_mesh_vertex_offsets_len());
        let mu = (self.context.get_mesh_uvs_ptr(), self.context.get_mesh_uvs_len());
        let mi = (self.context.get_mesh_indices_ptr(), self.context.get_mesh_indices_len());
        let mio = (self.context.get_mesh_index_offsets_ptr(), self.context.get_mesh_index_offsets_len());

        let frame = FrameView {
            frame_data,
            world_matrices: unsafe { slice_f32(wm.0, wm.1) },
            local_vertices: unsafe { slice_f32(lv.0, lv.1) },
            local_uvs: unsafe { slice_f32(lu.0, lu.1) },
            mesh_vertices_x: unsafe { slice_f32(mvx.0, mvx.1) },
            mesh_vertices_y: unsafe { slice_f32(mvy.0, mvy.1) },
            mesh_vertex_offsets: unsafe { slice_u32(mvo.0, mvo.1) },
            mesh_uvs: unsafe { slice_f32(mu.0, mu.1) },
            mesh_indices: unsafe { slice_i32(mi.0, mi.1) },
            mesh_index_offsets: unsafe { slice_u32(mio.0, mio.1) },
        };

        self.internal.prepare(device, queue, &frame);
    }

    /// Record the prepared frame into a RenderPass opened by the host.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> usize {
        self.internal.draw(pass)
    }
}

/// Build a slice from a runtime pointer/length, or empty if null/zero.
///
/// # Safety
/// `ptr` must be valid for `len` `f32`s and stay alive for `'a` (guaranteed
/// until the next `get_frame_data` call on the same context).
unsafe fn slice_f32<'a>(ptr: *const f32, len: usize) -> &'a [f32] {
    if ptr.is_null() || len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len) } }
}

/// See [`slice_f32`]. `ptr` must be valid for `len` `u32`s.
unsafe fn slice_u32<'a>(ptr: *const u32, len: usize) -> &'a [u32] {
    if ptr.is_null() || len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len) } }
}

/// See [`slice_f32`]. `ptr` must be valid for `len` `i32`s.
unsafe fn slice_i32<'a>(ptr: *const i32, len: usize) -> &'a [i32] {
    if ptr.is_null() || len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len) } }
}
