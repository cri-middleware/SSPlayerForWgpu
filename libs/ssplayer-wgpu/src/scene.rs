//! Layer 2b (SPEC §1.2 / §9.2): the per-render-target half of rendering.
//!
//! Turns the runtime's FrameData / DrawBatch / SoA buffers into wgpu vertices,
//! indices and draw commands, and owns what belongs to one render target: the
//! projection and the frame's accumulation. One `SSScene` draws any number of
//! players into one pass.
//!
//! Borrow model: it owns neither the `Device`/`Queue`, the render target, nor
//! the device-scoped [`SSRenderer`] — all three are lent per frame.

use crate::frame::FrameView;
use crate::mask::{MaskOp, MaskWriter, StencilTest};
use crate::renderer::{GeomClass, PipelineKey, PipelineKind, SHADER_PARAM_STRIDE, SHADER_PARAM_STRIDE_F32, SHAPE_FAN_VERTS, SSRenderer, blend_index, ss_shader_entry};

use ssruntime::runtime::effect_draw_plan::ss::runtime::root_as_effect_draw_plan_unchecked;
use ssruntime::runtime::framedata::ss::runtime::{BlendType, DrawBatchKind, PartAttributeShader, root_as_frame_data_unchecked};

/// PartsColor formula selector, matching `varg_of` in the WGSL. Only four exist,
/// so the shader takes two bits per primitive rather than three floats per vertex
/// (SPEC §5.3).
///
/// Deliberately *not* four module-level `pub const`s: cbindgen sweeps those into
/// the C header (see `cbindgen.toml`), and `#define MIX 0` in a public header is
/// a collision waiting to happen.
mod mode {
    /// `mix(straight, colour, rate)` — also what a Shape fill and any unsupported
    /// PartColor blend clamps to.
    pub(super) const MIX: u32 = 0;
    /// `straight * mix(1, colour, rate)`. With rate 1 this is the plain
    /// `texture * colour` modulate the no-PartColor and effect paths use.
    pub(super) const MUL: u32 = 1;
    pub(super) const ADD: u32 = 2;
    pub(super) const SUB: u32 = 3;
}

/// Opaque white, the colour a part without PartColor contributes (its alpha
/// travels separately, at full precision).
const WHITE: u32 = 0x00FF_FFFF;

/// Pack a 0..1 RGB triple into the `unpack4x8unorm` byte order (x = byte 0).
/// Sources are `u8 / 255`, so this round-trips exactly.
fn pack_rgb(r: f32, g: f32, b: f32) -> u32 {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xFF;
    q(r) | (q(g) << 8) | (q(b) << 16)
}

/// Construction config for SSScene, matched to the host's render target.
#[derive(Clone, Copy, Debug)]
pub struct SceneConfig {
    /// Target color format. Pipelines are baked against this.
    pub color_format: wgpu::TextureFormat,
    /// Depth/stencil format of the host's render target, or `None` to run
    /// without masking. When `Some`, the host must attach a matching
    /// depth/stencil buffer to every render pass (SPEC §6); this enables the
    /// stencil-test content pipelines and the mask-write pipelines.
    pub stencil_format: Option<wgpu::TextureFormat>,
}

/// The fields of one `DrawBatch` the renderer needs, extracted by the caller so
/// the operation layer keeps ownership of batch iteration.
#[derive(Clone, Copy, Debug)]
pub struct BatchInfo {
    pub start_rank: usize,
    pub count: usize,
    pub texture_hash: u32,
    pub blend_type: BlendType,
    pub kind: DrawBatchKind,
    /// Stencil test to apply (masked content). `None` for ordinary content.
    pub stencil: StencilTest,
    /// Player-level alpha (SPEC §9.3), folded into every vertex's colour alpha.
    /// The shader's final coverage is `tex.a * color.a` and the blend table is
    /// premultiplied throughout, so a plain multiply here fades the part
    /// correctly under every blend mode (including Add / Mul, which reach `dst`
    /// unchanged at alpha 0). Mask writers are deliberately not faded — their
    /// `color[0]` carries the cutout threshold, not a colour.
    pub alpha: f32,
}

#[derive(Clone, Debug)]
struct BatchDrawCall {
    key: PipelineKey,
    texture_hash: u32,
    /// The `vertex_index` range to issue. Which primitives that covers is the
    /// class's own arithmetic (SPEC §5.5) — 6 per quad or particle, 30 per shape,
    /// one stream entry per mesh vertex.
    vertices: std::ops::Range<u32>,
    /// Index of this part's block in the shader-params buffer, for
    /// [`PipelineKey::Shader`] draws — the dynamic offset for bind group(2).
    shader_slot: Option<u32>,
}

/// One drawn primitive: where its geometry sits in the frame arena, and the
/// handful of per-part values that are not in the runtime's buffers.
///
/// This is the record SPEC §5.5 trades the vertex buffer for. The CPU no longer
/// transforms corners — it copies the runtime's SoA arrays into the arena and
/// writes one of these (80 bytes) per part, against 4 x 52 bytes of vertices plus
/// 24 bytes of indices before.
///
/// Field meanings are per geometry class; see the `Prim` comment in the WGSL,
/// which this must stay in step with.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Prim {
    /// Quad/Shape: world matrix base. Mesh: the Y vertex array. Effect: unused.
    aux_base: u32,
    /// Local vertices (Quad), mesh X (Mesh), shape vertices (Shape), particle XY.
    pos_base: u32,
    /// Local UVs (Quad/Mesh, texture pixels), box coords (Shape), particle UVs.
    uv_base: u32,
    /// The owning node's placement matrix.
    node_base: u32,
    /// Shape: live vertex count. Unused elsewhere.
    count: u32,
    /// PartsColor formula (see [`mode`]).
    mode: u32,
    /// Player-level fade, applied in the vertex stage so it multiplies the corner
    /// alphas before interpolation — the order the CPU path used.
    alpha: f32,
    /// Mesh: the part's Z. Unused elsewhere (local Z is zero).
    z: f32,
    /// Per-corner RGB, packed. Alpha is deliberately *not* in here.
    colors: [u32; 4],
    /// Per-corner alpha, kept as f32: a part without PartColor takes its alpha
    /// straight from the runtime and quantising it would shift the output.
    alphas: [f32; 4],
    /// Per-corner PartsColor rate — or, for a mask writer, the cutout threshold
    /// (`fs_mask` reads `varg.x`), which needs the full range including -1.
    rates: [f32; 4],
}

/// Where one node's slice of the frame arena starts (SPEC §5.5).
///
/// [`SSScene::begin_node`] copies the runtime's SoA buffers in — once per node
/// per frame, as a handful of memcpys — and hands back the offsets. Every
/// primitive that node contributes then refers to the arena rather than carrying
/// its own expanded geometry.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeSlot {
    /// The node's placement matrix (16 floats).
    transform: u32,
    world_matrices: u32,
    local_vertices: u32,
    local_uvs: u32,
    mesh_x: u32,
    mesh_y: u32,
    mesh_uvs: u32,
    shape_vertices: u32,
    shape_box_coords: u32,
}

/// Layer 2b: the per-target, per-frame half — what one render target's worth of
/// geometry looks like this frame.
///
/// Borrow model: owns neither the `Device`/`Queue`, the render target, nor the
/// [`SSRenderer`]. It holds the projection (which is a property of the target)
/// and the frame's vertex/index/draw-call accumulation.
///
/// P7-4 renames this to `SSScene` and gives it the list of players drawing into
/// it; today it is still one per player.
pub struct SSScene {
    /// Whether masking (stencil) is enabled for this target.
    stencil_enabled: bool,
    /// The target's formats. Part of every [`PipelineKey`] this builds.
    color_format: wgpu::TextureFormat,
    stencil_format: Option<wgpu::TextureFormat>,

    /// The projection matrix. Part of group(0) along with the frame buffers.
    globals_buffer: wgpu::Buffer,
    /// group(0), rebuilt whenever one of the frame buffers below is reallocated.
    scene_bind_group: Option<wgpu::BindGroup>,
    /// Last viewport passed to [`SSScene::set_viewport`], re-applied on restore
    /// (the projection lives in a buffer that dies with the device).
    viewport: Option<(f32, f32, bool)>,

    arena_buffer: Option<wgpu::Buffer>,
    prim_buffer: Option<wgpu::Buffer>,
    mesh_stream_buffer: Option<wgpu::Buffer>,
    draw_calls: Vec<BatchDrawCall>,

    // Per-frame CPU accumulation: every node copies its runtime SoA buffers into
    // `arena` and appends a `Prim` per part; nothing here is per vertex except
    // the mesh stream, whose geometry is genuinely irregular (SPEC §5.5).
    arena: Vec<f32>,
    prims: Vec<Prim>,
    /// `(primitive, vertex)` pairs, one per mesh vertex.
    mesh_stream: Vec<u32>,

    // --- Integration shaders (`ss-*`, SPEC §6). Only touched when a frame has
    // shader parts; shader-free animations never build any of this. ---
    /// Per-frame params blocks (one 256-byte / `SHADER_PARAM_STRIDE_F32`-float
    /// slot per shader draw), uploaded to `ss_params_buffer` in `end_frame`.
    ss_params: Vec<f32>,
    ss_params_buffer: Option<wgpu::Buffer>,
    ss_params_bind_group: Option<wgpu::BindGroup>,
}

/// Column-major 4x4 identity. Used as the root transform (children compose their
/// parent instance part's world matrix onto this).
pub const IDENTITY_TRANSFORM: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Min/max of a Normal part's 4 corner UVs (normalized by `tex_size`), as
/// `[minU, minV, maxU, maxV]` — the cell UV rect an integration shader receives
/// as `cell_rect` (ss-circle / ss-spot). Falls back to the full `[0,0,1,1]` when
/// the part has no UV data.
fn quad_uv_bounds(frame: &FrameView, part_idx: usize, tex_size: Option<(f32, f32)>) -> [f32; 4] {
    let luv = &frame.local_uvs;
    if (part_idx + 1) * 10 > luv.len() {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let l_uv = &luv[part_idx * 10..(part_idx + 1) * 10];
    let (mut min_u, mut min_v, mut max_u, mut max_v) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for k in 0..4 {
        let (u, v) = if let Some(s) = tex_size { (l_uv[k * 2] / s.0, l_uv[k * 2 + 1] / s.1) } else { (l_uv[k * 2], l_uv[k * 2 + 1]) };
        min_u = min_u.min(u);
        min_v = min_v.min(v);
        max_u = max_u.max(u);
        max_v = max_v.max(v);
    }
    [min_u, min_v, max_u, max_v]
}

/// PartsColor formula for a `BlendType` (SPEC §5.3). Only Mix/Mul/Add/Sub select
/// one; any other `BlendType` clamps to Mix (else no branch matches and the
/// shader outputs black).
fn parts_color_mode(blend: BlendType) -> u32 {
    match blend {
        BlendType::Mul => mode::MUL,
        BlendType::Add => mode::ADD,
        BlendType::Sub => mode::SUB,
        _ => mode::MIX, // Mix (and clamp for all other blend types)
    }
}

impl SSScene {
    /// Build the target-scoped resources: the projection uniform. The rest of
    /// group(0) is this frame's geometry, so its bind group is built in
    /// `end_frame` — against whichever renderer is drawing the frame — once the
    /// buffers are sized. That is why no `SSRenderer` is needed here.
    pub fn new(device: &wgpu::Device, config: &SceneConfig) -> Self {
        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SpriteStudio Globals UB"),
            size: 64, // 16 * 4 bytes for mat4x4
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            stencil_enabled: config.stencil_format.is_some(),
            color_format: config.color_format,
            stencil_format: config.stencil_format,
            globals_buffer,
            scene_bind_group: None,
            viewport: None,
            arena_buffer: None,
            prim_buffer: None,
            mesh_stream_buffer: None,
            draw_calls: Vec::new(),
            arena: Vec::new(),
            prims: Vec::new(),
            mesh_stream: Vec::new(),
            ss_params: Vec::new(),
            ss_params_buffer: None,
            ss_params_bind_group: None,
        }
    }

    /// Whether masking (stencil) is enabled for this renderer.
    pub fn stencil_enabled(&self) -> bool {
        self.stencil_enabled
    }

    /// The config this renderer was built from. Retained so a lost device can be
    /// replaced by a renderer targeting the same formats (SPEC §14) without the
    /// caller having to remember them.
    pub fn config(&self) -> SceneConfig {
        SceneConfig { color_format: self.color_format, stencil_format: self.stencil_format }
    }

    /// Cache key for a content draw. Without a stencil buffer every draw uses the
    /// untested variant, so the test is normalised away rather than filling the
    /// cache with three identical pipelines.
    fn content_key(&self, blend: BlendType, stencil: StencilTest, class: GeomClass) -> PipelineKey {
        let stencil = if self.stencil_enabled { stencil } else { StencilTest::None };
        self.key(PipelineKind::Content(blend_index(blend), stencil, class))
    }

    /// Attach this target's formats to a pipeline kind (SPEC §9.1).
    fn key(&self, kind: PipelineKind) -> PipelineKey {
        PipelineKey { kind, color_format: self.color_format, stencil_format: self.stencil_format }
    }

    /// Set the projection to the render target's pixel dimensions. Remembered, so
    /// [`SSScene::restore_device`] can re-apply it to a rebuilt buffer.
    pub fn set_viewport(&mut self, queue: &wgpu::Queue, width: f32, height: f32, y_down: bool) {
        self.update_projection(queue, width, height, y_down);
        self.viewport = Some((width, height, y_down));
    }

    /// Rebuild this target's GPU state against a freshly created `device` after
    /// the previous one was lost (SPEC §14). The host calls
    /// [`SSRenderer::restore_device`] first — that covers pipelines and textures
    /// for the whole device — then this once per render target, then
    /// [`crate::SSPlayer::restore_textures`] once per player.
    pub fn restore_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let viewport = self.viewport;
        *self = Self::new(device, &SceneConfig { color_format: self.color_format, stencil_format: self.stencil_format });
        if let Some((width, height, y_down)) = viewport {
            self.set_viewport(queue, width, height, y_down);
        }
    }

    /// Update the projection matrix to the target's pixel dimensions.
    fn update_projection(&self, queue: &wgpu::Queue, width: f32, height: f32, y_down: bool) {
        let scale_y = if y_down { -2.0 / height } else { 2.0 / height };
        let matrix = [2.0 / width, 0.0, 0.0, 0.0, 0.0, scale_y, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::cast_slice(&matrix));
    }

    /// Advance every player and build this target's frame (SPEC §9.2).
    ///
    /// This is the frame boundary: one `begin_frame` / `end_frame` pair spans
    /// **all** the players sharing the target, so their geometry lands in one
    /// vertex buffer and their draws can be recorded back to back.
    ///
    /// Players are drawn in ascending [`crate::SSPlayer::order`], ties keeping the
    /// order the caller passed them in (the sort is stable). The scene borrows
    /// them rather than owning them (SPEC §1.2), so they can live wherever the
    /// host keeps them — a `Vec`, an ECS, an engine's scene graph.
    pub fn update(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, delta_secs: f32, players: &mut [&mut crate::SSPlayer]) {
        self.begin_frame();
        players.sort_by_key(|p| p.order());
        for player in players.iter_mut() {
            player.build_frame(self, renderer, device, queue, delta_secs);
        }
        self.end_frame(renderer, device, queue);
    }

    /// Reset per-frame accumulation. Call once before appending any batches.
    pub fn begin_frame(&mut self) {
        self.arena.clear();
        self.prims.clear();
        self.mesh_stream.clear();
        self.draw_calls.clear();
        self.ss_params.clear();
    }

    /// Copy one node's runtime buffers into the frame arena and return where they
    /// landed (SPEC §5.5). Call once per node per frame, before appending any of
    /// its batches.
    ///
    /// This is the whole CPU side of the vertex path: a handful of
    /// `extend_from_slice` calls over buffers `ssruntime` already laid out
    /// contiguously. Nothing is transformed here — the vertex stage does that,
    /// reading the same numbers the runtime produced.
    ///
    /// `transform` is the node's placement: the player's for a root, the
    /// composition with the parent instance part's world matrix for a child.
    pub fn begin_node(&mut self, frame: &FrameView, transform: &[f32; 16]) -> NodeSlot {
        let mut push = |src: &[f32]| -> u32 {
            let base = self.arena.len() as u32;
            self.arena.extend_from_slice(src);
            base
        };
        NodeSlot {
            transform: push(transform),
            world_matrices: push(frame.world_matrices),
            local_vertices: push(frame.local_vertices),
            local_uvs: push(frame.local_uvs),
            mesh_x: push(frame.mesh_vertices_x),
            mesh_y: push(frame.mesh_vertices_y),
            mesh_uvs: push(frame.mesh_uvs),
            shape_vertices: push(frame.shape_vertices),
            shape_box_coords: push(frame.shape_vertex_box_coords),
        }
    }

    /// Copy a standalone placement matrix into the arena.
    ///
    /// An effect draws from its own `EffectDrawPlan`, not from the node's runtime
    /// buffers, so the only arena slot it needs is its composed transform — the
    /// rest of the returned [`NodeSlot`] is unused and must stay so.
    pub fn push_transform(&mut self, transform: &[f32; 16]) -> NodeSlot {
        let base = self.arena.len() as u32;
        self.arena.extend_from_slice(transform);
        NodeSlot { transform: base, ..Default::default() }
    }

    /// Append `prim`, returning its index in the frame's primitive table.
    fn push_prim(&mut self, prim: Prim) -> u32 {
        let index = self.prims.len() as u32;
        self.prims.push(prim);
        index
    }

    /// Record a draw over the primitives `[first, self.prims.len())`.
    ///
    /// Primitive indices are global to the frame, and each class turns one into a
    /// fixed span of `vertex_index` values (SPEC §5.5). Primitives of other
    /// classes in between simply leave gaps in a span nothing draws, which is why
    /// one primitive table serves all four.
    fn push_draw(&mut self, key: PipelineKey, class: GeomClass, texture_hash: u32, first: u32, shader_slot: Option<u32>) {
        let last = self.prims.len() as u32;
        if last == first {
            return;
        }
        let stride = match class {
            GeomClass::Quad | GeomClass::Effect => 6,
            GeomClass::Shape => SHAPE_FAN_VERTS,
            // Mesh draws are driven by the stream, not by primitive arithmetic.
            GeomClass::Mesh => return,
        };
        self.draw_calls.push(BatchDrawCall { key, texture_hash, vertices: first * stride..last * stride, shader_slot });
    }

    /// Append one Normal/Mesh DrawBatch's geometry into the frame accumulation,
    /// pre-multiplying every vertex by `transform` (identity for the root; the
    /// composed parent instance world matrix for instance children). `frame` is
    /// re-parsed for its draw_order/parts/parts_color; the batch fields are
    /// passed in so the caller (SSPlayer) drives batch iteration and can
    /// interleave instance recursion at the right rank. `batch.stencil` selects
    /// the stencil-test content pipeline for masked parts.
    pub fn append_batch(&mut self, renderer: &SSRenderer, frame: &FrameView, node: &NodeSlot, batch: &BatchInfo) {
        let BatchInfo { start_rank, count, texture_hash, blend_type, kind, stencil, alpha } = *batch;
        if frame.frame_data.is_empty() {
            return;
        }
        let frame_data = unsafe { root_as_frame_data_unchecked(frame.frame_data) };
        let draw_order = frame_data.draw_order();
        let parts = frame_data.parts();
        let parts_color = frame_data.parts_color();

        if frame.world_matrices.is_empty() || frame.local_vertices.is_empty() || frame.local_uvs.is_empty() {
            return;
        }

        let first_prim = self.prims.len() as u32;

        match kind {
            DrawBatchKind::Normal => {
                let key = self.content_key(blend_type, stencil, GeomClass::Quad);
                // Integration-shader dispatch (SPEC §6): when the frame carries
                // `SHDR` attributes and this non-masked batch has ≥1 shader part,
                // draw per-part so each shader part gets its own pipeline + params.
                // Masked parts (stencil != None) use the plain path (shader × mask
                // is not combined in v1, matching WebGL).
                let shaders = frame_data.shaders();
                let has_shader = |rank: usize| -> Option<PartAttributeShader> {
                    let si = parts.get(draw_order.get(rank) as usize).shader();
                    if si < 0 {
                        return None;
                    }
                    let attr = *shaders?.get(si as usize);
                    ss_shader_entry(attr.id_hash()).map(|_| attr)
                };
                let shader_dispatch = stencil == StencilTest::None && (start_rank..start_rank + count).any(|r| has_shader(r).is_some());

                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    let part_color_idx = part.part_color();
                    let (colors, alphas, rates, mode) = parts_color
                        .filter(|_| part_color_idx >= 0)
                        .map(|pc_vec| {
                            let pc = pc_vec.get(part_color_idx as usize);
                            let corners = [pc.lt(), pc.rt(), pc.lb(), pc.rb()];
                            let mut colors = [0u32; 4];
                            let mut alphas = [0.0f32; 4];
                            let mut rates = [0.0f32; 4];
                            for (i, corner) in corners.iter().enumerate() {
                                let c = corner.rgba();
                                colors[i] = pack_rgb(c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
                                alphas[i] = c.a() as f32 / 255.0;
                                rates[i] = corner.rate();
                            }
                            (colors, alphas, rates, parts_color_mode(pc.blend_type()))
                        })
                        // No PartColor: white modulated by the part's own alpha.
                        .unwrap_or(([WHITE; 4], [part.alpha(); 4], [1.0; 4], mode::MUL));

                    let call_start = self.prims.len() as u32;
                    self.push_quad_prim(frame, node, part_idx, Prim { mode, alpha, colors, alphas, rates, ..Default::default() });
                    if !shader_dispatch {
                        continue;
                    }
                    // Per-part draw call so shader / non-shader parts don't merge.
                    if self.prims.len() as u32 == call_start {
                        continue;
                    }
                    // A shader part gets its own pipeline and params slot; a plain
                    // part sharing the batch falls back to the content pipeline.
                    let (call_key, shader_slot) = match has_shader(rank) {
                        Some(attr) => {
                            let slot = self.push_shader_params(&attr, frame, part_idx, renderer.texture_size(texture_hash));
                            (self.key(PipelineKind::Shader(attr.id_hash(), blend_index(blend_type))), Some(slot))
                        }
                        None => (key, None),
                    };
                    self.push_draw(call_key, GeomClass::Quad, texture_hash, call_start, shader_slot);
                }
                if !shader_dispatch {
                    self.push_draw(key, GeomClass::Quad, texture_hash, first_prim, None);
                }
            }
            DrawBatchKind::Mesh => {
                let key = self.content_key(blend_type, stencil, GeomClass::Mesh);
                let stream_start = (self.mesh_stream.len() / 2) as u32;
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    let part_color_idx = part.part_color();
                    let (color, part_alpha, rate, mode) = parts_color
                        .filter(|_| part_color_idx >= 0)
                        .map(|pc_vec| {
                            let pc = pc_vec.get(part_color_idx as usize);
                            let lt = pc.lt();
                            let c = lt.rgba();
                            // Mesh uses a single (corner-0) PartColor sample, like WebGL.
                            (pack_rgb(c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0), c.a() as f32 / 255.0, lt.rate(), parts_color_mode(pc.blend_type()))
                        })
                        .unwrap_or((WHITE, part.alpha(), 1.0, mode::MUL));

                    self.push_mesh_prim(frame, node, part_idx, Prim { mode, alpha, z: part.position_z(), colors: [color; 4], alphas: [part_alpha; 4], rates: [rate; 4], ..Default::default() });
                }
                let stream_end = (self.mesh_stream.len() / 2) as u32;
                if stream_end > stream_start {
                    self.draw_calls.push(BatchDrawCall { key, texture_hash, vertices: stream_start..stream_end, shader_slot: None });
                }
            }
            DrawBatchKind::Shape => {
                let key = self.content_key(blend_type, stencil, GeomClass::Shape);
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    // Shape fill: the interpolated PartColor corners, or white *
                    // alpha when the part has no PartColor (matches WebGL). Shapes
                    // are textureless (white fallback), so this colour shows as-is.
                    // Shape bypasses the PartColor blend formula: it is forced to
                    // the Mix path with rate 1 (hasPc) / 0 (no PartColor), so the
                    // fill is the bilinear PartColor itself (10_parts_color §1-2).
                    let part_color_idx = part.part_color();
                    let has_pc = part_color_idx >= 0 && parts_color.is_some();
                    let (colors, alphas) = parts_color
                        .filter(|_| part_color_idx >= 0)
                        .map(|pc_vec| {
                            let pc = pc_vec.get(part_color_idx as usize);
                            let corners = [pc.lt().rgba(), pc.rt().rgba(), pc.lb().rgba(), pc.rb().rgba()];
                            let mut colors = [0u32; 4];
                            let mut alphas = [0.0f32; 4];
                            for (i, c) in corners.iter().enumerate() {
                                colors[i] = pack_rgb(c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
                                alphas[i] = c.a() as f32 / 255.0;
                            }
                            (colors, alphas)
                        })
                        .unwrap_or(([WHITE; 4], [part.alpha(); 4]));

                    self.push_shape_prim(frame, node, part_idx, Prim { mode: mode::MIX, alpha, colors, alphas, rates: [if has_pc { 1.0 } else { 0.0 }; 4], ..Default::default() });
                }
                self.push_draw(key, GeomClass::Shape, texture_hash, first_prim, None);
            }
            _ => {}
        }
    }

    /// Append one part's integration-shader (`ss-*`) params as a 256-byte slot in
    /// `ss_params` and return its slot index (the dynamic uniform offset unit).
    /// `cell_rect` is the min/max of the quad's 4 normalized corner UVs (for
    /// ss-circle / ss-spot); `pixel_size` is 1/texel (for the neighbour-sampling
    /// shaders). MUST be called after the part's quad is built.
    fn push_shader_params(&mut self, attr: &PartAttributeShader, frame: &FrameView, part_idx: usize, tex_size: Option<(f32, f32)>) -> u32 {
        let slot = (self.ss_params.len() / SHADER_PARAM_STRIDE_F32) as u32;
        let cell = quad_uv_bounds(frame, part_idx, tex_size);
        let (tw, th) = tex_size.unwrap_or((1.0, 1.0));
        let px = if tw > 0.0 { 1.0 / tw } else { 0.0 };
        let py = if th > 0.0 { 1.0 / th } else { 0.0 };

        let mut block = [0.0f32; SHADER_PARAM_STRIDE_F32];
        block[0] = attr.param0();
        block[1] = attr.param1();
        block[2] = attr.param2();
        block[3] = attr.param3();
        block[4] = attr.param4();
        block[5] = attr.param5();
        block[6] = attr.param6();
        block[7] = attr.param7();
        block[8..12].copy_from_slice(&cell);
        block[12] = px;
        block[13] = py;
        self.ss_params.extend_from_slice(&block);
        slot
    }

    /// Append one mask writer's geometry, routed to the mask-write pipeline for
    /// `op`. The cutout threshold rides in the primitive's rate slot, which
    /// `fs_mask` reads as `varg.x` (SPEC §6): it needs the full f32 range,
    /// including the -1 that means "no cutout". No-op if masking is disabled.
    pub fn append_mask_writer(&mut self, frame: &FrameView, node: &NodeSlot, writer: &MaskWriter, op: MaskOp) {
        use crate::mask::GeomKind;
        if !self.stencil_enabled || frame.frame_data.is_empty() {
            return;
        }
        if frame.world_matrices.is_empty() {
            return;
        }
        // Colour output is masked off for these pipelines, so only the threshold
        // in `rates` matters.
        let prim = Prim { rates: [writer.threshold; 4], alpha: 1.0, ..Default::default() };
        let first_prim = self.prims.len() as u32;

        let class = match writer.geom {
            GeomKind::Quad => {
                if frame.local_vertices.is_empty() || frame.local_uvs.is_empty() {
                    return;
                }
                self.push_quad_prim(frame, node, writer.part_idx, prim);
                GeomClass::Quad
            }
            GeomKind::Mesh => {
                let stream_start = (self.mesh_stream.len() / 2) as u32;
                self.push_mesh_prim(frame, node, writer.part_idx, prim);
                let stream_end = (self.mesh_stream.len() / 2) as u32;
                if stream_end > stream_start {
                    self.draw_calls.push(BatchDrawCall { key: self.key(PipelineKind::Mask(op, GeomClass::Mesh)), texture_hash: writer.texture_hash, vertices: stream_start..stream_end, shader_slot: None });
                }
                return;
            }
            GeomKind::Shape => {
                self.push_shape_prim(frame, node, writer.part_idx, prim);
                GeomClass::Shape
            }
            GeomKind::Unsupported => return,
        };

        self.push_draw(self.key(PipelineKind::Mask(op, class)), class, writer.texture_hash, first_prim, None);
    }

    /// Record one sprite quad for `part_idx`: point `prim` at the part's slice of
    /// this node's arena copy and append it. The corners themselves are expanded
    /// by `vs_quad` (SPEC §5.5).
    fn push_quad_prim(&mut self, frame: &FrameView, node: &NodeSlot, part_idx: usize, prim: Prim) {
        // Bounds are still checked here: the vertex stage indexes the arena
        // unconditionally, so a part the runtime has no geometry for must not
        // produce a primitive at all.
        if (part_idx + 1) * 16 > frame.world_matrices.len() || (part_idx + 1) * 10 > frame.local_vertices.len() || (part_idx + 1) * 10 > frame.local_uvs.len() {
            return;
        }
        self.push_prim(Prim { aux_base: node.world_matrices + (part_idx * 16) as u32, pos_base: node.local_vertices + (part_idx * 10) as u32, uv_base: node.local_uvs + (part_idx * 10) as u32, node_base: node.transform, ..prim });
    }

    /// Record one skinned-mesh part. Mesh geometry is the one irregular case, so
    /// besides the primitive this appends a `(primitive, vertex)` pair per index —
    /// the runtime's own triangle list, rebased onto the arena.
    fn push_mesh_prim(&mut self, frame: &FrameView, node: &NodeSlot, part_idx: usize, prim: Prim) {
        let offsets = &frame.mesh_vertex_offsets;
        if offsets.is_empty() || part_idx >= offsets.len() - 1 {
            return;
        }
        let start_vert = offsets[part_idx] as usize;
        let end_vert = offsets[part_idx + 1] as usize;
        if end_vert <= start_vert {
            return;
        }

        let index = self.push_prim(Prim {
            // Mesh vertices come as two parallel world-space arrays.
            aux_base: node.mesh_y,
            pos_base: node.mesh_x,
            uv_base: node.mesh_uvs,
            node_base: node.transform,
            ..prim
        });

        let start_idx = frame.mesh_index_offsets[part_idx] as usize;
        let end_idx = frame.mesh_index_offsets[part_idx + 1] as usize;
        for &rel_idx in &frame.mesh_indices[start_idx..end_idx] {
            self.mesh_stream.push(index);
            self.mesh_stream.push(start_vert as u32 + rel_idx as u32);
        }
    }

    /// Record one Shape part, drawn as a fan from vertex 0 by `vs_shape`.
    /// No-op if the part is not a shape or has fewer than 3 live vertices.
    fn push_shape_prim(&mut self, frame: &FrameView, node: &NodeSlot, part_idx: usize, prim: Prim) {
        // Layout: `shape_vertices[part_idx * SHAPE_STRIDE + 2*v]` = local xy.
        const SHAPE_STRIDE: usize = 24;
        let counts = &frame.shape_vertex_counts;
        if part_idx >= counts.len() || (part_idx + 1) * 16 > frame.world_matrices.len() {
            return;
        }
        let count = counts[part_idx].max(0) as usize;
        // The fan pattern is baked into the vertex stage at a fixed stride, so a
        // shape with more vertices than that would silently lose triangles.
        if count < 3 || count > (SHAPE_FAN_VERTS as usize / 3) + 2 {
            return;
        }
        let base = part_idx * SHAPE_STRIDE;
        if base + count * 2 > frame.shape_vertices.len() || base + count * 2 > frame.shape_vertex_box_coords.len() {
            return;
        }
        self.push_prim(Prim { aux_base: node.world_matrices + (part_idx * 16) as u32, pos_base: node.shape_vertices + base as u32, uv_base: node.shape_box_coords + base as u32, node_base: node.transform, count: count as u32, ..prim });
    }

    /// Append an effect slot's `EffectDrawPlan` (a raw FlatBuffer) into the
    /// frame accumulation. `transform` is the effect part's composed world
    /// matrix (verts are slot-relative). Blend is per-command; color is taken
    /// from the plan and modulates the cellmap texture via the sprite shader.
    /// `stencil` selects the stencil test when the effect is a masked target
    /// (inherited from a parent mask); `None` for ordinary effects. `alpha` is
    /// the player-level fade (see [`BatchInfo::alpha`]).
    pub fn append_effect(&mut self, plan_buf: &[u8], node: &NodeSlot, stencil: StencilTest, alpha: f32) {
        if plan_buf.is_empty() {
            return;
        }
        let plan = unsafe { root_as_effect_draw_plan_unchecked(plan_buf) };
        let stride = plan.vert_stride() as usize;
        let verts = plan.verts();
        let uvs = plan.uvs();
        let colors = plan.colors();

        for cmd in plan.commands().iter() {
            let quad_count = cmd.quad_count() as usize;
            if quad_count == 0 {
                continue;
            }
            let texture_hash = cmd.cellmap_hash();
            let particle_offset = cmd.particle_offset() as usize;
            // 0 = Mix pipeline, 1 = Add pipeline (per-command, not per-batch).
            let blend = if cmd.blend() == 1 { BlendType::Add } else { BlendType::Mix };
            let key = self.content_key(blend, stencil, GeomClass::Effect);

            let vert_first = particle_offset * 4;
            let vert_last = (particle_offset + quad_count) * 4;

            // The particle quads go into the arena as they are — the runtime
            // already put them in world space. Only the effect part's own
            // placement is left, and the vertex stage applies that.
            let pos_base = self.arena.len() as u32;
            for vi in vert_first..vert_last {
                self.arena.push(verts.get(vi * stride));
                self.arena.push(verts.get(vi * stride + 1));
            }
            let uv_base = self.arena.len() as u32;
            for vi in vert_first..vert_last {
                self.arena.push(uvs.get(vi * 2));
                self.arena.push(uvs.get(vi * 2 + 1));
            }

            // One primitive per particle. The runtime emits every particle as a
            // quad with the same corner order this player uses (see
            // `quad_corner`), so no index list has to travel with them.
            let first_prim = self.prims.len() as u32;
            for q in 0..quad_count {
                let vi = vert_first + q * 4;
                let color = pack_rgb(colors.get(vi * 4), colors.get(vi * 4 + 1), colors.get(vi * 4 + 2));
                self.push_prim(Prim {
                    pos_base: pos_base + (q * 8) as u32,
                    uv_base: uv_base + (q * 8) as u32,
                    node_base: node.transform,
                    // Effects carry no PartsColor overlay: plain texture*color modulate.
                    mode: mode::MUL,
                    alpha,
                    colors: [color; 4],
                    alphas: [colors.get(vi * 4 + 3); 4],
                    rates: [1.0; 4],
                    ..Default::default()
                });
            }
            self.push_draw(key, GeomClass::Effect, texture_hash, first_prim, None);
        }
    }

    /// Upload the accumulated frame to the GPU. Call once after all batches.
    pub fn end_frame(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        renderer.ensure_white(queue);

        if self.prims.is_empty() {
            return;
        }

        // Growing any of the three invalidates group(0), so they are sized
        // together and the bind group is rebuilt only when one actually moved.
        let mut rebuild = self.scene_bind_group.is_none();
        rebuild |= ensure_storage(&mut self.arena_buffer, device, "SpriteStudio Arena", std::mem::size_of_val(self.arena.as_slice()));
        rebuild |= ensure_storage(&mut self.prim_buffer, device, "SpriteStudio Prims", std::mem::size_of_val(self.prims.as_slice()));
        rebuild |= ensure_storage(&mut self.mesh_stream_buffer, device, "SpriteStudio Mesh Stream", std::mem::size_of_val(self.mesh_stream.as_slice()));
        if rebuild {
            self.scene_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("SpriteStudio Scene Bind Group"),
                layout: &renderer.scene_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.globals_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.arena_buffer.as_ref().unwrap().as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.prim_buffer.as_ref().unwrap().as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: self.mesh_stream_buffer.as_ref().unwrap().as_entire_binding() },
                ],
            }));
        }
        write_slice(queue, self.arena_buffer.as_ref(), &self.arena);
        write_slice(queue, self.prim_buffer.as_ref(), &self.prims);
        write_slice(queue, self.mesh_stream_buffer.as_ref(), &self.mesh_stream);

        // Compile whatever this frame draws with and has not drawn with before
        // (SPEC §5.2). After the first few frames every key is a cache hit, so
        // this is a walk over the draw list and nothing more. Indexed rather
        // than iterated because `ensure_pipeline` needs `&mut self`.
        for i in 0..self.draw_calls.len() {
            let key = self.draw_calls[i].key;
            renderer.ensure_pipeline(device, key);
        }

        // Integration shaders (SPEC §6): upload this frame's per-part params.
        // Nothing here runs for shader-free frames (`ss_params` stays empty).
        if !self.ss_params.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&self.ss_params);
            let needed_size = bytes.len() as u64;
            let current = self.ss_params_buffer.as_ref().map(|b| b.size()).unwrap_or(0);
            if current < needed_size {
                let new_size = (needed_size * 2).max(SHADER_PARAM_STRIDE as u64);
                let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("SpriteStudio SS Params Buffer"), size: new_size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                // The binding window is one `SsParams` (64 B); the dynamic offset picks the slot.
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("SpriteStudio SS Params Bind Group"),
                    layout: &renderer.ss_params_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buffer, offset: 0, size: wgpu::BufferSize::new(64) }) }],
                });
                self.ss_params_buffer = Some(buffer);
                self.ss_params_bind_group = Some(bind_group);
            }
            if let Some(ref buffer) = self.ss_params_buffer {
                queue.write_buffer(buffer, 0, bytes);
            }
        }
    }

    /// Record draw commands into a RenderPass opened by the host.
    /// Returns the number of draw calls issued.
    pub fn draw<'a>(&'a self, renderer: &'a SSRenderer, rpass: &mut wgpu::RenderPass<'a>) -> usize {
        if self.draw_calls.is_empty() {
            return 0;
        }

        let Some(scene_bg) = self.scene_bind_group.as_ref() else { return 0 };

        // No vertex or index buffer: group(0) *is* the geometry (SPEC §5.5).
        rpass.set_bind_group(0, scene_bg, &[]);
        if self.stencil_enabled {
            // Stencil ref is always 0; ops/compare are baked into the pipelines.
            rpass.set_stencil_reference(0);
        }

        let mut issued = 0usize;
        for call in &self.draw_calls {
            // Missing texture -> 1x1 white fallback (SPEC §8).
            let texture = renderer.texture_bind_group(call.texture_hash);

            // `end_frame` compiled every key this frame uses, so a miss here means
            // the key was an `ss-*` shader id `build_pipeline` declined to build.
            let (Some(pipeline), params_bg) = (renderer.pipelines.get(&call.key), self.ss_params_bind_group.as_ref()) else {
                continue;
            };
            // Integration-shader part (SPEC §6): group(2) carries the part's
            // params at its dynamic offset.
            let params = match (call.shader_slot, params_bg) {
                (Some(slot), Some(bg)) => Some((slot, bg)),
                (Some(_), None) => continue, // params buffer missing — cannot draw
                (None, _) => None,
            };

            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(1, texture, &[]);
            if let Some((slot, bg)) = params {
                rpass.set_bind_group(2, bg, &[slot * SHADER_PARAM_STRIDE as u32]);
            }
            rpass.draw(call.vertices.clone(), 0..1);
            issued += 1;
        }

        issued
    }
}

/// Make sure `buffer` can hold `size` bytes, reallocating (with headroom) if not.
/// Returns whether it was replaced — the caller's bind group is stale if so.
fn ensure_storage(buffer: &mut Option<wgpu::Buffer>, device: &wgpu::Device, label: &str, size: usize) -> bool {
    if buffer.as_ref().is_some_and(|b| b.size() >= size as u64) {
        return false;
    }
    // A zero-length frame array still needs a bindable buffer, hence the floor.
    let new_size = (size * 2).max(1024) as u64;
    *buffer = Some(device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: new_size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
    true
}

fn write_slice<T: bytemuck::Pod>(queue: &wgpu::Queue, buffer: Option<&wgpu::Buffer>, data: &[T]) {
    if let (Some(buffer), false) = (buffer, data.is_empty()) {
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(data));
    }
}
