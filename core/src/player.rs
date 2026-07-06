//! Layer 3 (operation): SSPlayer — the primary public API.
//!
//! Owns a tree of runtime `Context`s (one per animation instance) and a single
//! shared `SSInternalPlayer`. Borrow model: it holds no `Device`/`Queue`/
//! `Surface`; the host lends them on every `update`/`draw`.
//!
//! Instance parts reference a sub-animation in another `.ssab` pack. Each
//! instance child gets its **own** `Context` (hence its own SoA buffer), which
//! is how the single-buffer overwrite problem is avoided (see SPEC §7). The
//! host registers every pack via [`SSPlayer::add_ssab`]; external references are
//! resolved by pack name.

use crate::frame::FrameView;
use crate::internal::{BatchInfo, RendererConfig, SSInternalPlayer};
use ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked};
use ssruntime::error::SSRuntimeError;
use ssruntime::runtime::framedata::ss::runtime::{DrawBatchKind, root_as_frame_data_unchecked};
use ssruntime::{Context, InstanceSlot, SsabData, SsabResource};
use std::collections::HashMap;
use std::sync::Arc;

/// Column-major 4x4 identity.
const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Guards against cyclic instance references blowing the stack.
const MAX_INSTANCE_DEPTH: u32 = 12;

/// Construction config for [`SSPlayer`], matched to the host's render target.
pub struct SSPlayerConfig {
    /// Target color format. Forwarded to the renderer so pipelines match.
    pub color_format: wgpu::TextureFormat,
}

/// One node in the playback tree: a runtime context plus its instance children.
struct PlaybackNode {
    context: Context<'static>,
    /// Instance children keyed by part index (index into parts/draw_order/SoA).
    instances: HashMap<usize, InstanceChild>,
}

struct InstanceChild {
    slot: InstanceSlot,
    node: Box<PlaybackNode>,
    /// Previous child frame_no, for the slot's independent-loop detection.
    prev_frame: f32,
}

/// The SpriteStudio player. One instance owns one animation-instance tree.
pub struct SSPlayer {
    internal: SSInternalPlayer,
    /// All registered packs, keyed by pack name (matches `ref_anime_pack`).
    resources: HashMap<String, Arc<SsabResource>>,
    main_pack: Option<String>,
    root: Option<PlaybackNode>,
    current_animation: Option<String>,
    /// Root frame_no from the previous `update`, used to detect the loop edge
    /// that instance slots need (`parent_looped`).
    last_root_frame: f32,
}

impl SSPlayer {
    /// Create a player against a borrowed `device`. The device is not retained.
    pub fn new(device: &wgpu::Device, config: &SSPlayerConfig) -> Self {
        let internal = SSInternalPlayer::new(device, &RendererConfig { color_format: config.color_format });
        Self { internal, resources: HashMap::new(), main_pack: None, root: None, current_animation: None, last_root_frame: 0.0 }
    }

    /// Register an `.ssab` pack under `pack_name` (used to resolve external
    /// instance references). Does not change what is playing.
    pub fn add_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError> {
        let ssab_data = SsabData::load_copy(data)?;
        let resource = SsabResource::new(ssab_data);
        if resource.resource_static_ptr().is_null() {
            return Err(SSRuntimeError::NotLoaded);
        }
        self.resources.insert(pack_name.to_string(), resource);
        Ok(())
    }

    /// Register `pack_name` and make it the main pack to play from.
    pub fn load_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError> {
        self.add_ssab(pack_name, data)?;
        self.main_pack = Some(pack_name.to_string());
        self.root = None;
        self.current_animation = None;
        Ok(())
    }

    /// Set up and start the named animation, (re)building the instance tree.
    pub fn play(&mut self, anim_name: &str) -> Result<(), SSRuntimeError> {
        let main_pack = self.main_pack.clone().ok_or(SSRuntimeError::NotLoaded)?;
        let mut root = build_node(&self.resources, &main_pack, anim_name, 0).ok_or(SSRuntimeError::NotLoaded)?;
        root.context.play();
        self.last_root_frame = root.context.get_frame_no();
        self.root = Some(root);
        self.current_animation = Some(anim_name.to_string());
        Ok(())
    }

    pub fn pause(&mut self) {
        if let Some(root) = &mut self.root {
            root.context.pause();
        }
    }

    pub fn stop(&mut self) {
        if let Some(root) = &mut self.root {
            root.context.stop();
        }
    }

    pub fn set_speed(&mut self, speed: f32) {
        if let Some(root) = &mut self.root {
            root.context.set_animation_speed(speed);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_playing()).unwrap_or(false)
    }

    /// Names of all animations in the main pack, if loaded.
    pub fn animation_names(&self) -> Option<Vec<String>> {
        let anim_resource = self.resources.get(self.main_pack.as_ref()?)?.resource()?;
        Some(anim_resource.get_animation_names())
    }

    /// Canvas size of the currently playing animation.
    pub fn canvas_size(&self) -> Option<(f32, f32)> {
        let name = self.current_animation.as_ref()?;
        let anim_resource = self.resources.get(self.main_pack.as_ref()?)?.resource()?;
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

    /// Advance the tree by `delta_secs` and build GPU buffers for the frame.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, delta_secs: f32) {
        self.internal.begin_frame();

        if let Some(root) = &mut self.root {
            // The runtime works in milliseconds for the primary context.
            root.context.update(delta_secs * 1000.0);
            let frame_no = root.context.get_frame_no();
            // Detect the loop edge (forward playback wrapping past the end) so
            // instance slots can persist/re-arm their children correctly.
            let looped = frame_no + 0.5 < self.last_root_frame;
            self.last_root_frame = frame_no;
            render_node(root, &mut self.internal, &IDENTITY, frame_no, delta_secs, looped);
        }

        self.internal.end_frame(device, queue);
    }

    /// Record the prepared frame into a RenderPass opened by the host.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> usize {
        self.internal.draw(pass)
    }

    /// Total number of instance child nodes in the current tree. A non-zero
    /// count confirms external references were resolved and the tree was built.
    pub fn instance_node_count(&self) -> usize {
        fn count(node: &PlaybackNode) -> usize {
            node.instances.values().map(|c| 1 + count(&c.node)).sum()
        }
        self.root.as_ref().map(count).unwrap_or(0)
    }
}

/// Recursively build a playback node: create a context bound to `pack`'s
/// resource, set up `anime`, and build an instance child for every instance
/// part found in the pack. Child failures are skipped (not fatal).
fn build_node(resources: &HashMap<String, Arc<SsabResource>>, pack: &str, anime: &str, depth: u32) -> Option<PlaybackNode> {
    if depth > MAX_INSTANCE_DEPTH {
        return None;
    }
    let resource = resources.get(pack)?;
    let res_ptr = resource.resource_static_ptr();
    if res_ptr.is_null() {
        return None;
    }
    // SAFETY: the resource lives in `resources` for the player's lifetime.
    let static_res = unsafe { &*res_ptr };
    let mut context = Context::new();
    context.bind_resource(static_res);
    context.setup_animation(anime).ok()?;

    let mut instances = HashMap::new();
    if let Some(ssab_bytes) = context.get_ssab_data() {
        let ab = unsafe { root_as_ss_anime_binary_unchecked(ssab_bytes) };
        // The pack's part list is shared across its animations; part index i
        // matches the FrameData part index used by draw_order/SoA.
        for (i, part) in ab.parts().iter().enumerate() {
            if part.part_type_type() == PartType::PartTypeInstance
                && let Some(inst) = part.part_type_as_part_type_instance()
                && let Some(child) = build_node(resources, inst.ref_anime_pack(), inst.ref_anime_name(), depth + 1)
            {
                instances.insert(i, InstanceChild { slot: InstanceSlot::new(), node: Box::new(child), prev_frame: 0.0 });
            }
        }
    }

    Some(PlaybackNode { context, instances })
}

/// Recursively append a node's geometry to the renderer, driving instance slots
/// and composing transforms. The node's context must already be advanced to
/// `frame_no` (root by `update`, children by the parent's `InstanceSlot::step`).
fn render_node(node: &mut PlaybackNode, internal: &mut SSInternalPlayer, transform: &[f32; 16], frame_no: f32, delta_secs: f32, node_looped: bool) {
    let frame_data = match node.context.get_frame_data(frame_no) {
        Ok(bytes) => unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) },
        Err(_) => return,
    };
    let frame = frame_view(&mut node.context, frame_data);

    let fd = unsafe { root_as_frame_data_unchecked(frame.frame_data) };
    let draw_order = fd.draw_order();
    for batch in fd.draw_batches() {
        let start_rank = batch.start_rank() as usize;
        let count = batch.count() as usize;
        match batch.kind() {
            DrawBatchKind::Normal | DrawBatchKind::Mesh => {
                let info = BatchInfo { start_rank, count, texture_hash: batch.texture_hash(), blend_type: batch.blend_type(), kind: batch.kind() };
                internal.append_batch(&frame, transform, &info);
            }
            DrawBatchKind::Instance => {
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let Some(child) = node.instances.get_mut(&part_idx) else { continue };

                    let ev = node.context.get_active_event_instance(part_idx as u32);
                    let result = child.slot.step(ev, &mut child.node.context, child.prev_frame, frame_no, delta_secs, node_looped);

                    // On a transition edge the slot configures the child's section
                    // but leaves the frame snap to the host: play() moves the child
                    // to its start frame so independent playback actually advances.
                    if result.transitioned {
                        child.node.context.play();
                    }
                    let child_frame_no = child.node.context.get_frame_no();
                    child.prev_frame = child_frame_no;

                    if result.visible {
                        // Compose the instance part's world matrix onto the parent transform.
                        let m: [f32; 16] = frame.world_matrices[part_idx * 16..part_idx * 16 + 16].try_into().unwrap();
                        let child_transform = mat4_mul(transform, &m);
                        render_node(&mut child.node, internal, &child_transform, child_frame_no, delta_secs, result.child_looped);
                    }
                }
            }
            // Effect / Shape / Mask / Text / Nines: not yet handled.
            _ => {}
        }
    }
}

/// Build a [`FrameView`] from the context's current (already-evaluated) SoA.
fn frame_view<'a>(ctx: &mut Context<'static>, frame_data: &'a [u8]) -> FrameView<'a> {
    FrameView {
        frame_data,
        world_matrices: unsafe { slice_f32(ctx.get_world_matrices_ptr(), ctx.get_world_matrices_len()) },
        local_vertices: unsafe { slice_f32(ctx.get_local_vertices_ptr(), ctx.get_local_vertices_len()) },
        local_uvs: unsafe { slice_f32(ctx.get_local_uvs_ptr(), ctx.get_local_uvs_len()) },
        mesh_vertices_x: unsafe { slice_f32(ctx.get_mesh_vertices_x_ptr(), ctx.get_mesh_vertices_x_len()) },
        mesh_vertices_y: unsafe { slice_f32(ctx.get_mesh_vertices_y_ptr(), ctx.get_mesh_vertices_y_len()) },
        mesh_vertex_offsets: unsafe { slice_u32(ctx.get_mesh_vertex_offsets_ptr(), ctx.get_mesh_vertex_offsets_len()) },
        mesh_uvs: unsafe { slice_f32(ctx.get_mesh_uvs_ptr(), ctx.get_mesh_uvs_len()) },
        mesh_indices: unsafe { slice_i32(ctx.get_mesh_indices_ptr(), ctx.get_mesh_indices_len()) },
        mesh_index_offsets: unsafe { slice_u32(ctx.get_mesh_index_offsets_ptr(), ctx.get_mesh_index_offsets_len()) },
    }
}

/// Multiply two column-major 4x4 matrices: `a * b`.
fn mat4_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[col * 4 + k];
            }
            r[col * 4 + row] = s;
        }
    }
    r
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
