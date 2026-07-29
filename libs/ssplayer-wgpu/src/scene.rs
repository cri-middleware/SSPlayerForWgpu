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
use crate::renderer::{PipelineKey, PipelineKind, SHADER_PARAM_STRIDE, SHADER_PARAM_STRIDE_F32, SSRenderer, Vertex, blend_index, ss_shader_entry};

use ssruntime::runtime::effect_draw_plan::ss::runtime::root_as_effect_draw_plan_unchecked;
use ssruntime::runtime::framedata::ss::runtime::{BlendType, DrawBatchKind, PartAttributeShader, root_as_frame_data_unchecked};

/// `varg` for the no-PartColor / effect path: rate 1, mul (a plain
/// `texture * color` modulate; SPEC §5.3, matches WebGL's effect draw).
const VARG_MODULATE: [f32; 4] = [1.0, 0.0, 1.0, 0.0];
/// `varg` for mask-write geometry — consumed by `fs_mask`, which ignores it.
const VARG_NONE: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

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
    index_start: u32,
    index_count: u32,
    /// Index of this part's block in the shader-params buffer, for
    /// [`PipelineKey::Shader`] draws — the dynamic offset for bind group(2).
    shader_slot: Option<u32>,
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

    /// The projection matrix and its bind group (group(0)).
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    /// Last viewport passed to [`SSScene::set_viewport`], re-applied on restore
    /// (the projection lives in a buffer that dies with the device).
    viewport: Option<(f32, f32, bool)>,

    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
    draw_calls: Vec<BatchDrawCall>,

    // Per-frame CPU accumulation (Unity-style flatten): every node/batch appends
    // here between begin_frame and end_frame, then it is uploaded once.
    vertices: Vec<Vertex>,
    indices: Vec<u32>,

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

/// Apply a column-major 4x4 transform to a point.
fn apply_transform(t: &[f32; 16], x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    (t[0] * x + t[4] * y + t[8] * z + t[12], t[1] * x + t[5] * y + t[9] * z + t[13], t[2] * x + t[6] * y + t[10] * z + t[14])
}

/// Bilinearly interpolate `[lt, rt, lb, rb]` RGBA at box coordinate `(fx, fy)`
/// (fx: 0=left/1=right, fy: 0=top/1=bottom).
fn bilerp_color(c: &[[f32; 4]; 4], fx: f32, fy: f32) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for i in 0..4 {
        let top = c[0][i] * (1.0 - fx) + c[1][i] * fx;
        let bot = c[2][i] * (1.0 - fx) + c[3][i] * fx;
        out[i] = top * (1.0 - fy) + bot * fy;
    }
    out
}

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

/// PartsColor blend flags `(isMix, isMul, isAddSub)` for the shader `varg`
/// (SPEC §5.3). Only Mix/Mul/Add/Sub select a PartColor formula; any other
/// `BlendType` clamps to Mix (else no flag matches and the shader outputs black).
fn parts_color_flags(blend: BlendType) -> (f32, f32, f32) {
    match blend {
        BlendType::Mul => (0.0, 1.0, 0.0),
        BlendType::Add => (0.0, 0.0, 1.0),
        BlendType::Sub => (0.0, 0.0, -1.0),
        _ => (1.0, 0.0, 0.0), // Mix (and clamp for all other blend types)
    }
}

impl SSScene {
    /// Build the target-scoped resources: the projection uniform and its bind
    /// group, cut against `renderer`'s shared layout.
    pub fn new(device: &wgpu::Device, renderer: &SSRenderer, config: &SceneConfig) -> Self {
        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SpriteStudio Globals UB"),
            size: 64, // 16 * 4 bytes for mat4x4
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("SpriteStudio Globals Bind Group"), layout: &renderer.globals_bind_group_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buffer.as_entire_binding() }] });

        Self {
            stencil_enabled: config.stencil_format.is_some(),
            color_format: config.color_format,
            stencil_format: config.stencil_format,
            globals_buffer,
            globals_bind_group,
            viewport: None,
            vertex_buffer: None,
            index_buffer: None,
            draw_calls: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
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
    fn content_key(&self, blend: BlendType, stencil: StencilTest) -> PipelineKey {
        let stencil = if self.stencil_enabled { stencil } else { StencilTest::None };
        self.key(PipelineKind::Content(blend_index(blend), stencil))
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
    pub fn restore_device(&mut self, renderer: &SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        let viewport = self.viewport;
        *self = Self::new(device, renderer, &SceneConfig { color_format: self.color_format, stencil_format: self.stencil_format });
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
        self.vertices.clear();
        self.indices.clear();
        self.draw_calls.clear();
        self.ss_params.clear();
    }

    /// Append one Normal/Mesh DrawBatch's geometry into the frame accumulation,
    /// pre-multiplying every vertex by `transform` (identity for the root; the
    /// composed parent instance world matrix for instance children). `frame` is
    /// re-parsed for its draw_order/parts/parts_color; the batch fields are
    /// passed in so the caller (SSPlayer) drives batch iteration and can
    /// interleave instance recursion at the right rank. `batch.stencil` selects
    /// the stencil-test content pipeline for masked parts.
    pub fn append_batch(&mut self, renderer: &SSRenderer, frame: &FrameView, transform: &[f32; 16], batch: &BatchInfo) {
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

        let key = self.content_key(blend_type, stencil);
        let tex_size = renderer.texture_size(texture_hash);
        let index_start = self.indices.len() as u32;

        match kind {
            DrawBatchKind::Normal => {
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
                    let (mut colors_arr, vargs_arr) = parts_color
                        .filter(|_| part_color_idx >= 0)
                        .map(|pc_vec| {
                            let pc = pc_vec.get(part_color_idx as usize);
                            let (mix_f, mul_f, add_f) = parts_color_flags(pc.blend_type());
                            let corners = [pc.lt(), pc.rt(), pc.lb(), pc.rb()];
                            let mut colors = [[0.0f32; 4]; 4];
                            let mut vargs = [[0.0f32; 4]; 4];
                            for (i, corner) in corners.iter().enumerate() {
                                let c = corner.rgba();
                                colors[i] = [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0];
                                vargs[i] = [corner.rate(), mix_f, mul_f, add_f];
                            }
                            (colors, vargs)
                        })
                        .unwrap_or_else(|| ([[1.0, 1.0, 1.0, part.alpha()]; 4], [VARG_MODULATE; 4]));
                    for c in &mut colors_arr {
                        c[3] *= alpha;
                    }

                    if !shader_dispatch {
                        self.push_quad_part(frame, transform, part_idx, tex_size, &colors_arr, &vargs_arr);
                        continue;
                    }

                    // Per-part draw call so shader / non-shader parts don't merge.
                    let call_start = self.indices.len() as u32;
                    self.push_quad_part(frame, transform, part_idx, tex_size, &colors_arr, &vargs_arr);
                    let call_end = self.indices.len() as u32;
                    if call_end == call_start {
                        continue;
                    }
                    // A shader part gets its own pipeline and params slot; a plain
                    // part sharing the batch falls back to the content pipeline.
                    let (call_key, shader_slot) = match has_shader(rank) {
                        Some(attr) => {
                            let slot = self.push_shader_params(&attr, frame, part_idx, tex_size);
                            (self.key(PipelineKind::Shader(attr.id_hash(), blend_index(blend_type))), Some(slot))
                        }
                        None => (key, None),
                    };
                    self.draw_calls.push(BatchDrawCall { key: call_key, texture_hash, index_start: call_start, index_count: call_end - call_start, shader_slot });
                }
                if shader_dispatch {
                    // Per-part draw calls already emitted; skip the aggregate push.
                    return;
                }
            }
            DrawBatchKind::Mesh => {
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    let part_color_idx = part.part_color();
                    let (mut mesh_color, mesh_varg) = parts_color
                        .filter(|_| part_color_idx >= 0)
                        .map(|pc_vec| {
                            let pc = pc_vec.get(part_color_idx as usize);
                            let lt = pc.lt();
                            let c = lt.rgba();
                            let (mix_f, mul_f, add_f) = parts_color_flags(pc.blend_type());
                            // Mesh uses a single (corner-0) PartColor sample, like WebGL.
                            ([c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0], [lt.rate(), mix_f, mul_f, add_f])
                        })
                        .unwrap_or_else(|| ([1.0, 1.0, 1.0, part.alpha()], VARG_MODULATE));
                    mesh_color[3] *= alpha;

                    self.push_mesh_part(frame, transform, part_idx, part.position_z(), tex_size, mesh_color, mesh_varg);
                }
            }
            DrawBatchKind::Shape => {
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
                    let mut corners = parts_color
                        .filter(|_| part_color_idx >= 0)
                        .map(|pc_vec| {
                            let pc = pc_vec.get(part_color_idx as usize);
                            let c_lt = pc.lt().rgba();
                            let c_rt = pc.rt().rgba();
                            let c_lb = pc.lb().rgba();
                            let c_rb = pc.rb().rgba();
                            [
                                [c_lt.r() as f32 / 255.0, c_lt.g() as f32 / 255.0, c_lt.b() as f32 / 255.0, c_lt.a() as f32 / 255.0],
                                [c_rt.r() as f32 / 255.0, c_rt.g() as f32 / 255.0, c_rt.b() as f32 / 255.0, c_rt.a() as f32 / 255.0],
                                [c_lb.r() as f32 / 255.0, c_lb.g() as f32 / 255.0, c_lb.b() as f32 / 255.0, c_lb.a() as f32 / 255.0],
                                [c_rb.r() as f32 / 255.0, c_rb.g() as f32 / 255.0, c_rb.b() as f32 / 255.0, c_rb.a() as f32 / 255.0],
                            ]
                        })
                        .unwrap_or_else(|| [[1.0, 1.0, 1.0, part.alpha()]; 4]);
                    for c in &mut corners {
                        c[3] *= alpha;
                    }
                    let shape_varg = [if has_pc { 1.0 } else { 0.0 }, 1.0, 0.0, 0.0];

                    self.push_shape_content_part(frame, transform, part_idx, &corners, shape_varg);
                }
            }
            _ => {}
        }

        let index_end = self.indices.len() as u32;
        if index_end > index_start {
            self.draw_calls.push(BatchDrawCall { key, texture_hash, index_start, index_count: index_end - index_start, shader_slot: None });
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

    /// Append one mask writer's geometry into the frame accumulation, routed to
    /// the mask-write pipeline for `op`. The cutout threshold rides in the
    /// vertex `color[0]` (consumed by `fs_mask`). No-op if masking is disabled.
    pub fn append_mask_writer(&mut self, renderer: &SSRenderer, frame: &FrameView, transform: &[f32; 16], writer: &MaskWriter, op: MaskOp) {
        use crate::mask::GeomKind;
        if !self.stencil_enabled || frame.frame_data.is_empty() {
            return;
        }
        if frame.world_matrices.is_empty() {
            return;
        }
        let tex_size = renderer.texture_size(writer.texture_hash);
        let color = [writer.threshold, 0.0, 0.0, 0.0];
        let index_start = self.indices.len() as u32;

        match writer.geom {
            GeomKind::Quad => {
                if frame.local_vertices.is_empty() || frame.local_uvs.is_empty() {
                    return;
                }
                // `fs_mask` ignores varg; color[0] carries the cutout threshold.
                self.push_quad_part(frame, transform, writer.part_idx, tex_size, &[color; 4], &[VARG_NONE; 4]);
            }
            GeomKind::Mesh => {
                self.push_mesh_part(frame, transform, writer.part_idx, 0.0, tex_size, color, VARG_NONE);
            }
            GeomKind::Shape => {
                self.push_shape_part(frame, transform, writer.part_idx, color);
            }
            GeomKind::Unsupported => return,
        }

        let index_end = self.indices.len() as u32;
        if index_end > index_start {
            self.draw_calls.push(BatchDrawCall { key: self.key(PipelineKind::Mask(op)), texture_hash: writer.texture_hash, index_start, index_count: index_end - index_start, shader_slot: None });
        }
    }

    /// Build one sprite quad (4 verts / 2 tris) for `part_idx` into the frame
    /// accumulation. `colors` / `vargs` are per-corner `[lt, rt, lb, rb]`.
    fn push_quad_part(&mut self, frame: &FrameView, transform: &[f32; 16], part_idx: usize, tex_size: Option<(f32, f32)>, colors: &[[f32; 4]; 4], vargs: &[[f32; 4]; 4]) {
        let wm = &frame.world_matrices;
        let lv = &frame.local_vertices;
        let luv = &frame.local_uvs;
        if (part_idx + 1) * 16 > wm.len() || (part_idx + 1) * 10 > lv.len() || (part_idx + 1) * 10 > luv.len() {
            return;
        }
        let m = &wm[part_idx * 16..(part_idx + 1) * 16];
        let lv = &lv[part_idx * 10..(part_idx + 1) * 10];
        let l_uv = &luv[part_idx * 10..(part_idx + 1) * 10];

        let base_vert_idx = self.vertices.len() as u32;
        for k in 0..4 {
            let lx = lv[k * 2];
            let ly = lv[k * 2 + 1];

            let wx = m[0] * lx + m[4] * ly + m[12];
            let wy = m[1] * lx + m[5] * ly + m[13];
            let wz = m[2] * lx + m[6] * ly + m[14];
            let (fx, fy, fz) = apply_transform(transform, wx, wy, wz);

            let (tu, tv) = if let Some(size) = tex_size { (l_uv[k * 2] / size.0, l_uv[k * 2 + 1] / size.1) } else { (l_uv[k * 2], l_uv[k * 2 + 1]) };

            self.vertices.push(Vertex { position: [fx, fy, fz], uv: [tu, tv], color: colors[k], varg: vargs[k] });
        }

        self.indices.push(base_vert_idx);
        self.indices.push(base_vert_idx + 1);
        self.indices.push(base_vert_idx + 2);
        self.indices.push(base_vert_idx + 2);
        self.indices.push(base_vert_idx + 1);
        self.indices.push(base_vert_idx + 3);
    }

    /// Build one skinned-mesh part for `part_idx` into the frame accumulation
    /// with a single uniform `color`. `pos_z` is the part's Z. Returns silently
    /// if the part has no mesh geometry this frame.
    #[allow(clippy::too_many_arguments)]
    fn push_mesh_part(&mut self, frame: &FrameView, transform: &[f32; 16], part_idx: usize, pos_z: f32, tex_size: Option<(f32, f32)>, color: [f32; 4], varg: [f32; 4]) {
        let offsets = &frame.mesh_vertex_offsets;
        if offsets.is_empty() || part_idx >= offsets.len() - 1 {
            return;
        }
        let start_vert = offsets[part_idx] as usize;
        let end_vert = offsets[part_idx + 1] as usize;
        if end_vert <= start_vert {
            return;
        }

        let base_vert_idx = self.vertices.len() as u32;
        for k in start_vert..end_vert {
            let wx = frame.mesh_vertices_x[k];
            let wy = frame.mesh_vertices_y[k];
            let (fx, fy, fz) = apply_transform(transform, wx, wy, pos_z);

            let (tu, tv) = if let Some(size) = tex_size { (frame.mesh_uvs[k * 2] / size.0, frame.mesh_uvs[k * 2 + 1] / size.1) } else { (frame.mesh_uvs[k * 2], frame.mesh_uvs[k * 2 + 1]) };

            self.vertices.push(Vertex { position: [fx, fy, fz], uv: [tu, tv], color, varg });
        }

        let start_idx = frame.mesh_index_offsets[part_idx] as usize;
        let end_idx = frame.mesh_index_offsets[part_idx + 1] as usize;
        for &rel_idx in &frame.mesh_indices[start_idx..end_idx] {
            self.indices.push(base_vert_idx + rel_idx as u32);
        }
    }

    /// Build one Shape part for `part_idx` as a triangle fan into the frame
    /// accumulation with a single uniform `color`. Vertices are the runtime's
    /// local shape vertices transformed by the part's world matrix; UV is unused
    /// (Shape mask writers are textureless). No-op if the part is not a shape or
    /// has fewer than 3 live vertices.
    fn push_shape_part(&mut self, frame: &FrameView, transform: &[f32; 16], part_idx: usize, color: [f32; 4]) {
        // Layout: `shape_vertices[part_idx * SHAPE_STRIDE + 2*v]` = local xy.
        const SHAPE_STRIDE: usize = 24;
        let counts = &frame.shape_vertex_counts;
        let sv = &frame.shape_vertices;
        let wm = &frame.world_matrices;
        if part_idx >= counts.len() || (part_idx + 1) * 16 > wm.len() {
            return;
        }
        let count = counts[part_idx].max(0) as usize;
        if count < 3 || part_idx * SHAPE_STRIDE + count * 2 > sv.len() {
            return;
        }
        let m = &wm[part_idx * 16..(part_idx + 1) * 16];
        let base = part_idx * SHAPE_STRIDE;

        let base_vert_idx = self.vertices.len() as u32;
        for v in 0..count {
            let lx = sv[base + 2 * v];
            let ly = sv[base + 2 * v + 1];
            let wx = m[0] * lx + m[4] * ly + m[12];
            let wy = m[1] * lx + m[5] * ly + m[13];
            let wz = m[2] * lx + m[6] * ly + m[14];
            let (fx, fy, fz) = apply_transform(transform, wx, wy, wz);
            self.vertices.push(Vertex { position: [fx, fy, fz], uv: [0.0, 0.0], color, varg: VARG_NONE });
        }

        // Triangle fan from vertex 0 (cull is disabled, so winding is irrelevant).
        for k in 1..count - 1 {
            self.indices.push(base_vert_idx);
            self.indices.push(base_vert_idx + k as u32);
            self.indices.push(base_vert_idx + (k + 1) as u32);
        }
    }

    /// Build one Shape *content* part as a triangle fan with a bilinearly
    /// interpolated fill colour. `corners` is `[lt, rt, lb, rb]` RGBA; each
    /// vertex samples it by its box coordinate (fx, fy). Textureless: UV is
    /// constant and the white fallback is bound, so the colour shows as-is
    /// (matches WebGL `buildShape`).
    fn push_shape_content_part(&mut self, frame: &FrameView, transform: &[f32; 16], part_idx: usize, corners: &[[f32; 4]; 4], varg: [f32; 4]) {
        const SHAPE_STRIDE: usize = 24;
        let counts = &frame.shape_vertex_counts;
        let sv = &frame.shape_vertices;
        let bc = &frame.shape_vertex_box_coords;
        let wm = &frame.world_matrices;
        if part_idx >= counts.len() || (part_idx + 1) * 16 > wm.len() {
            return;
        }
        let count = counts[part_idx].max(0) as usize;
        if count < 3 || part_idx * SHAPE_STRIDE + count * 2 > sv.len() || part_idx * SHAPE_STRIDE + count * 2 > bc.len() {
            return;
        }
        let m = &wm[part_idx * 16..(part_idx + 1) * 16];
        let base = part_idx * SHAPE_STRIDE;

        let base_vert_idx = self.vertices.len() as u32;
        for v in 0..count {
            let lx = sv[base + 2 * v];
            let ly = sv[base + 2 * v + 1];
            let wx = m[0] * lx + m[4] * ly + m[12];
            let wy = m[1] * lx + m[5] * ly + m[13];
            let wz = m[2] * lx + m[6] * ly + m[14];
            let (fx, fy, fz) = apply_transform(transform, wx, wy, wz);

            // Bilinear over box coords: [lt,rt] on top, [lb,rb] on bottom.
            let (bx, by) = (bc[base + 2 * v], bc[base + 2 * v + 1]);
            let color = bilerp_color(corners, bx, by);

            self.vertices.push(Vertex { position: [fx, fy, fz], uv: [0.5, 0.5], color, varg });
        }

        for k in 1..count - 1 {
            self.indices.push(base_vert_idx);
            self.indices.push(base_vert_idx + k as u32);
            self.indices.push(base_vert_idx + (k + 1) as u32);
        }
    }

    /// Append an effect slot's `EffectDrawPlan` (a raw FlatBuffer) into the
    /// frame accumulation. `transform` is the effect part's composed world
    /// matrix (verts are slot-relative). Blend is per-command; color is taken
    /// from the plan and modulates the cellmap texture via the sprite shader.
    /// `stencil` selects the stencil test when the effect is a masked target
    /// (inherited from a parent mask); `None` for ordinary effects. `alpha` is
    /// the player-level fade (see [`BatchInfo::alpha`]).
    pub fn append_effect(&mut self, plan_buf: &[u8], transform: &[f32; 16], stencil: StencilTest, alpha: f32) {
        if plan_buf.is_empty() {
            return;
        }
        let plan = unsafe { root_as_effect_draw_plan_unchecked(plan_buf) };
        let stride = plan.vert_stride() as usize;
        let verts = plan.verts();
        let uvs = plan.uvs();
        let colors = plan.colors();
        let indices = plan.indices();

        for cmd in plan.commands().iter() {
            let quad_count = cmd.quad_count() as usize;
            if quad_count == 0 {
                continue;
            }
            let texture_hash = cmd.cellmap_hash();
            let particle_offset = cmd.particle_offset() as usize;
            // 0 = Mix pipeline, 1 = Add pipeline (per-command, not per-batch).
            let blend = if cmd.blend() == 1 { BlendType::Add } else { BlendType::Mix };
            let key = self.content_key(blend, stencil);

            let index_start = self.indices.len() as u32;
            let base_vert = self.vertices.len() as u32;

            let vert_first = particle_offset * 4;
            let vert_last = (particle_offset + quad_count) * 4;
            for vi in vert_first..vert_last {
                let px = verts.get(vi * stride);
                let py = verts.get(vi * stride + 1);
                let pz = if stride >= 3 { verts.get(vi * stride + 2) } else { 0.0 };
                let (fx, fy, fz) = apply_transform(transform, px, py, pz);

                // Effect UVs are already normalized 0..1 (unlike sprite/mesh pixel UVs).
                let tu = uvs.get(vi * 2);
                let tv = uvs.get(vi * 2 + 1);

                let color = [colors.get(vi * 4), colors.get(vi * 4 + 1), colors.get(vi * 4 + 2), colors.get(vi * 4 + 3) * alpha];
                // Effects carry no PartsColor overlay: plain texture*color modulate.
                self.vertices.push(Vertex { position: [fx, fy, fz], uv: [tu, tv], color, varg: VARG_MODULATE });
            }

            // Indices are quad-local (0..quad_count*4), rebased onto base_vert.
            let idx_first = particle_offset * 6;
            let idx_last = (particle_offset + quad_count) * 6;
            for ii in idx_first..idx_last {
                self.indices.push(base_vert + indices.get(ii) as u32);
            }

            let index_end = self.indices.len() as u32;
            if index_end > index_start {
                self.draw_calls.push(BatchDrawCall { key, texture_hash, index_start, index_count: index_end - index_start, shader_slot: None });
            }
        }
    }

    /// Upload the accumulated geometry to the GPU. Call once after all batches.
    pub fn end_frame(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        renderer.ensure_white(queue);

        if self.vertices.is_empty() || self.indices.is_empty() {
            return;
        }

        let vertex_len = std::mem::size_of_val(self.vertices.as_slice());
        self.ensure_vertex_buffer(device, vertex_len);
        if let Some(ref buffer) = self.vertex_buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.vertices));
        }

        let index_len = std::mem::size_of_val(self.indices.as_slice());
        self.ensure_index_buffer(device, index_len);
        if let Some(ref buffer) = self.index_buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.indices));
        }

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

        let (v_buf, i_buf) = match (&self.vertex_buffer, &self.index_buffer) {
            (Some(v), Some(i)) => (v, i),
            _ => return 0,
        };

        rpass.set_vertex_buffer(0, v_buf.slice(..));
        rpass.set_index_buffer(i_buf.slice(..), wgpu::IndexFormat::Uint32);
        rpass.set_bind_group(0, &self.globals_bind_group, &[]);
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
            rpass.draw_indexed(call.index_start..call.index_start + call.index_count, 0, 0..1);
            issued += 1;
        }

        issued
    }

    fn ensure_vertex_buffer(&mut self, device: &wgpu::Device, size: usize) {
        let current_size = self.vertex_buffer.as_ref().map(|b| b.size()).unwrap_or(0);
        if current_size < size as u64 {
            let new_size = (size * 2).max(1024);
            self.vertex_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor { label: Some("SpriteStudio Vertex Buffer"), size: new_size as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
        }
    }

    fn ensure_index_buffer(&mut self, device: &wgpu::Device, size: usize) {
        let current_size = self.index_buffer.as_ref().map(|b| b.size()).unwrap_or(0);
        if current_size < size as u64 {
            let new_size = (size * 2).max(1024);
            self.index_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor { label: Some("SpriteStudio Index Buffer"), size: new_size as u64, usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
        }
    }
}
