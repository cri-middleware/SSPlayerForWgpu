//! The per-render-target half of rendering.
//!
//! Turns the runtime's FrameData / DrawBatch / SoA buffers into draw commands,
//! and owns what belongs to one render target: the projection and the frame's
//! geometry. One `SSScene` draws any number of players into one pass.
//!
//! It owns neither the `Device`/`Queue`, the render target, nor the
//! device-scoped [`SSRenderer`] — all three are lent per frame.

use crate::frame::FrameView;
use crate::mask::{MaskOp, MaskWriter, ScopedWriter, StencilTest};
use crate::metrics::{GpuTimer, SceneStats};
use crate::renderer::{COVERAGE_FORMAT, GeomClass, MESH_DRAW_PRIMS, MESH_PRIM_SHIFT, PipelineKey, PipelineKind, SHADER_PARAM_STRIDE, SHADER_PARAM_STRIDE_F32, SHAPE_FAN_VERTS, SSRenderer, blend_index, ss_shader_entry};

use ssruntime::runtime::effect_draw_plan::ss::runtime::root_as_effect_draw_plan_unchecked;
use ssruntime::runtime::framedata::ss::runtime::{BlendType, DrawBatchKind, PartAttributeShader, root_as_frame_data_unchecked};

/// PartsColor formula selector, matching `varg_of` in the WGSL. Only four exist,
/// so the shader takes two bits per primitive rather than three floats per vertex.
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

/// The identity player tint: white multiplies nothing away. It shares [`WHITE`]'s
/// value and not its meaning — that one is a part's missing PartColor, this one is
/// a player nobody tinted — so the two are named apart.
const NO_TINT: u32 = WHITE;

/// Turn a public `0xRRGGBB` tint into the byte order the shader unpacks
/// ([`pack_rgb`]'s, x = byte 0). Only the outer two bytes move; green is already
/// where it belongs.
pub(crate) fn pack_tint(rgb: u32) -> u32 {
    ((rgb >> 16) & 0xFF) | (rgb & 0x00FF00) | ((rgb & 0xFF) << 16)
}

/// How a scene rasterises mask coverage.
///
/// The two working backends implement the same semantics from opposite ends:
/// [`MaskBackend::Stencil`] is the hardware stencil, and
/// [`MaskBackend::Coverage`] is written for engines that never lend one.
/// Which is available is a property of the *host's render pass*, not of the
/// animation — the same `.ssab` must look the same either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MaskBackend {
    /// No masking. Pure masks draw nothing and masked targets draw unclipped.
    #[default]
    None,
    /// The host's stencil buffer. Needs
    /// [`SceneConfig::stencil_format`] and a matching attachment on every pass.
    Stencil,
    /// A coverage bitmap the player rasterises for itself.
    ///
    /// Needs no stencil from the host, and so is the backend for a pass that has
    /// none to give — Bevy's 2d depth is `Depth32Float`, and a WebGPU engine that
    /// composes its own passes may expose no depth at all. The cost is one extra
    /// pass per masking player per frame, which is why it is not the default.
    ///
    /// **The host must call [`SSScene::encode_mask_coverage`]** between `update`
    /// and `draw`: the bitmap is rasterised into its own render pass, and under
    /// the borrow model only the host has an encoder to open one with.
    Coverage,
}

/// Construction config for SSScene, matched to the host's render target.
#[derive(Clone, Copy, Debug)]
pub struct SceneConfig {
    /// Target color format. Pipelines are baked against this.
    pub color_format: wgpu::TextureFormat,
    /// Depth/stencil format of the host's render target, or `None` when it
    /// attaches neither. When `Some`, the host must attach a matching
    /// depth/stencil buffer to every render pass.
    ///
    /// Independent of [`SceneConfig::mask`]: this describes the *attachment*,
    /// which a host may have for its own reasons whatever it masks with. The one
    /// combination that cannot work is [`MaskBackend::Stencil`] without a format,
    /// which [`SSScene::new`] refuses by falling back to [`MaskBackend::None`].
    pub stencil_format: Option<wgpu::TextureFormat>,
    /// How this scene draws masks.
    pub mask: MaskBackend,
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
    /// The part's place on its player's tree sequence — every part of the
    /// instance tree numbered in drawing order, a sub-animation in its instance
    /// part's place. What the coverage replay scopes writers against; read only
    /// for a masked target (`count == 1`) under [`MaskBackend::Coverage`].
    pub mask_seq: u32,
    /// Player-level alpha, folded into every vertex's colour alpha.
    /// The shader's final coverage is `tex.a * color.a` and the blend table is
    /// premultiplied throughout, so a plain multiply here fades the part
    /// correctly under every blend mode (including Add / Mul, which reach `dst`
    /// unchanged at alpha 0). Mask writers are deliberately not faded — their
    /// `color[0]` carries the cutout threshold, not a colour.
    pub alpha: f32,
    /// Player-level tint, multiplied into the fragment *after* the PartsColor
    /// mix — the place `uColor` lands in the WebGL player, and the reason it
    /// cannot fold into the vertex colour the formula blends towards. Packed in
    /// the `unpack4x8unorm` byte order (red in byte 0); white, `0x00FF_FFFF`,
    /// leaves the colour alone. Mask writers are untinted for the same reason
    /// they are unfaded.
    pub tint: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct BatchDrawCall {
    key: PipelineKey,
    texture_hash: u32,
    /// The `vertex_index` range to issue. Which primitives that covers is the
    /// class's own arithmetic — 6 per quad or particle, 30 per shape,
    /// one stream entry per mesh vertex.
    vertices: std::ops::Range<u32>,
    /// The `instance_index` range to issue: always one instance, the draw's
    /// first primitive for Mesh, which its stream entries count from, and
    /// zero for every other class.
    instances: std::ops::Range<u32>,
    /// Index of this part's block in the shader-params buffer, for
    /// [`PipelineKey::Shader`] draws — the dynamic offset for bind group(2).
    shader_slot: Option<u32>,
    /// Which layer of the frame's coverage array this call rasterises into
    /// (writer draws) or samples from (masked content). `None` when the call has
    /// nothing to do with coverage.
    cov_layer: Option<u32>,
}

/// One drawn primitive: where its geometry sits in the frame arena, and the
/// handful of per-part values that are not in the runtime's buffers.
///
/// This is the record that stands in for a vertex buffer: the CPU does not
/// transform corners, it copies each drawn part's slice of the runtime's SoA
/// arrays into the arena and writes one of these per part.
///
/// Field meanings are per geometry class; see the `Prim` comment in the WGSL,
/// which this must stay in step with.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Prim {
    /// Quad/Shape: world matrix base. Mesh: the part's Y vertices. Effect: unused.
    aux_base: u32,
    /// Local vertices (Quad), the part's X vertices (Mesh), shape vertices
    /// (Shape), particle XY.
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
    /// Arena offset of the owning node's coverage block, meaningful only when
    /// `mask_info` says this primitive takes part in coverage masking. Zero is a
    /// real arena offset, so the flag is what gates it, never this value.
    cov_base: u32,
    /// Coverage masking, packed. Zero — the `Default` — is "no
    /// coverage masking", which is every primitive under the stencil backend.
    ///
    /// | bits | meaning |
    /// |---|---|
    /// | 0 | set when this primitive takes part in coverage masking |
    /// | 1 | 1 = writer (baking a bit), 0 = masked target (replaying) |
    /// | 2 | target polarity: 1 = visible inside the coverage |
    /// | 8..16 | writer: which bit of the bitmap it writes (0..95) |
    /// | 8..32 | target: its place on the tree sequence, for the scope test |
    mask_info: u32,
    /// Player-level tint (see [`BatchInfo::tint`]), in [`pack_rgb`]'s byte order.
    /// Unlike every other field, zero is not the neutral value here — it is
    /// black — which is why [`Prim`] spells its `Default` out.
    tint: u32,
}

impl Default for Prim {
    /// Zero everywhere the shader reads a zero as "nothing", and [`NO_TINT`]
    /// where it would read one as black. Written against `zeroed()` rather than
    /// field by field so a new field keeps defaulting to zero on its own.
    fn default() -> Self {
        Self { tint: NO_TINT, ..bytemuck::Zeroable::zeroed() }
    }
}

/// Which backend a mask writer is being appended for, and the one value each
/// needs: the stencil op to apply, or the coverage bit to set.
#[derive(Clone, Copy, Debug)]
enum MaskWriterDest {
    Stencil(MaskOp),
    Coverage(usize),
}

/// Set on [`Prim::mask_info`] when the primitive takes part in coverage masking.
const MASK_INFO_ACTIVE: u32 = 1 << 0;
/// Set when the primitive is a coverage *writer* rather than a masked target.
const MASK_INFO_WRITER: u32 = 1 << 1;
/// Set when a masked target draws *inside* the coverage.
const MASK_INFO_INSIDE: u32 = 1 << 2;

/// Pack a writer's [`Prim::mask_info`].
fn writer_mask_info(bit: usize) -> u32 {
    MASK_INFO_ACTIVE | MASK_INFO_WRITER | ((bit as u32 & 0xFF) << 8)
}

/// Pack a masked target's [`Prim::mask_info`].
///
/// A target has no writer bit, so its sequence number takes all 24 bits above
/// the flags — which is also as far as the replay's f32 comparison is exact.
/// The stencil backend numbers the same tree without a limit, so the two only
/// part ways past 2^24 parts in one tree. Clamped rather than wrapped so the
/// failure, if it ever happened, is a mask that stops scoping rather than one
/// that scopes backwards.
fn target_mask_info(seq: u32, inside: bool) -> u32 {
    let polarity = if inside { MASK_INFO_INSIDE } else { 0 };
    MASK_INFO_ACTIVE | polarity | (seq.min(0xFF_FFFF) << 8)
}

/// Where one node's placement and coverage block sit in the frame arena.
///
/// [`FrameChunk::push_transform`] copies the placement in. A node's parts copy
/// their own geometry in as their primitives are recorded, so only what is
/// drawn reaches the arena.
#[derive(Clone, Copy, Debug)]
pub struct NodeSlot {
    /// The node's placement matrix (16 floats).
    transform: u32,
    /// Arena offset of this node's coverage block, or [`NO_COVERAGE`].
    ///
    /// Written by [`FrameChunk::set_node_coverage`] *after* the writers are
    /// planned, which is why it cannot be filled in by `push_transform`: the block
    /// describes the writer group, and the group is not known until the walk has
    /// looked at the frame.
    coverage: u32,
    /// Which layer of the frame's coverage array this node's bitmap occupies.
    /// Meaningless while `coverage` is [`NO_COVERAGE`].
    coverage_layer: u32,
}

/// One node's claim on the frame's coverage array. The layer index is this
/// entry's position in [`FrameChunk::coverage_nodes`].
#[derive(Clone, Copy, Debug)]
struct CoverageNode {
    /// Player-local width and height of the writer group, which is what decides
    /// the bitmap's size class — a mask twice as wide wants twice the texels.
    extent: (f32, f32),
}

/// [`NodeSlot::coverage`] for a node that writes no coverage this frame.
const NO_COVERAGE: u32 = u32::MAX;

// Written out rather than derived: every other field defaults to arena offset 0,
// which is a real offset, but `coverage` has to default to "none" — and a derived
// `Default` would quietly make every slot claim the first node's coverage block.
impl Default for NodeSlot {
    fn default() -> Self {
        Self { transform: 0, coverage: NO_COVERAGE, coverage_layer: 0 }
    }
}

/// Floats before the writer records in a node's coverage block: the six of the
/// node-local -> coverage-UV affine, the array layer the bitmap lives in, and
/// the writer count.
pub(crate) const COVERAGE_HEADER_F32: usize = 8;

/// Smallest coverage bitmap edge. Below this the quantisation costs more in
/// reallocation than the texels are worth.
const COVERAGE_MIN_SIZE: u32 = 128;
/// Largest coverage bitmap edge — 4 MiB a layer at RGBA8.
const COVERAGE_MAX_SIZE: u32 = 1024;
/// How far below a size class the footprint must fall before the smaller class is
/// taken. The reference Player's value.
const COVERAGE_SHRINK_DEADBAND: f32 = 0.15;

/// The frame's coverage bitmaps: one array texture, one layer per masking node.
///
/// An array rather than a texture apiece so the replay binds once for the whole
/// pass — the layer a primitive reads travels in its coverage block, not in the
/// binding.
struct CoverageTargets {
    /// Held only to keep the views alive; every access goes through one of them.
    _texture: wgpu::Texture,
    /// One single-layer view per layer, for the bake passes to attach to.
    layer_views: Vec<wgpu::TextureView>,
    /// The whole array, as the masked content samples it.
    bind_group: wgpu::BindGroup,
    size: u32,
    layers: u32,
}
/// Floats per writer record in a coverage block: the open and close of its scope
/// on the tree sequence, its bit, and op_invert.
pub(crate) const COVERAGE_WRITER_F32: usize = 4;

/// Layer 2b: the per-target, per-frame half — what one render target's worth of
/// geometry looks like this frame.
///
/// Borrow model: owns neither the `Device`/`Queue`, the render target, nor the
/// [`SSRenderer`]. It holds the projection (which is a property of the target)
/// and the frame's vertex/index/draw-call accumulation.
///
/// One of these serves every player drawing into the same target.
pub struct SSScene {
    /// This frame's coverage bitmaps, kept across frames so a mask that holds its
    /// size class does not reallocate.
    coverage: Option<CoverageTargets>,
    /// How this target draws masks.
    mask: MaskBackend,
    /// The target's formats. Part of every [`PipelineKey`] this builds.
    color_format: wgpu::TextureFormat,
    stencil_format: Option<wgpu::TextureFormat>,

    /// The projection matrix. Part of group(0) along with the frame buffers.
    globals_buffer: wgpu::Buffer,
    /// group(0), rebuilt whenever the frame buffer below is reallocated.
    scene_bind_group: Option<wgpu::BindGroup>,
    /// Last viewport passed to [`SSScene::set_viewport`], re-applied on restore
    /// (the projection lives in a buffer that dies with the device).
    viewport: Option<(f32, f32, bool)>,

    /// The frame's arena, primitive table and mesh stream, in that order, in one
    /// buffer: wgpu stages every write through a new buffer of its own, and
    /// making one costs more than a frame's copy, so one write it is.
    frame_buffer: Option<wgpu::Buffer>,
    /// The bytes each of group(0)'s three storage bindings covers, so the most
    /// a frame may put in each. They only grow, and the buffer holds all three.
    frame_capacity: [u64; 3],
    /// Where this frame's primitive table and mesh stream start in the buffer:
    /// group(0)'s dynamic offsets. The arena starts at zero.
    frame_offsets: [u32; 2],

    /// This frame's geometry, every player's in `order`. This is what reaches
    /// the GPU.
    frame: FrameChunk,

    // --- Integration shaders (`ss-*`). Only touched when a frame has
    // shader parts; shader-free animations never build any of this. ---
    ss_params_buffer: Option<wgpu::Buffer>,
    ss_params_bind_group: Option<wgpu::BindGroup>,

    /// This frame's counts. Free to keep, and the only way
    /// to tell whether a change to how work is issued actually issued less.
    stats: SceneStats,
    /// GPU pass timing, present only when the device was created with
    /// `Features::TIMESTAMP_QUERY`.
    timer: Option<GpuTimer>,
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

/// PartsColor formula for a `BlendType`. Only Mix/Mul/Add/Sub select
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

        // Stencil masking without an attachment to write into cannot work, and
        // silently drawing every mask target unclipped is worse than saying so.
        let mask = match config.mask {
            MaskBackend::Stencil if config.stencil_format.is_none() => MaskBackend::None,
            other => other,
        };
        debug_assert!(mask == config.mask, "MaskBackend::Stencil needs SceneConfig::stencil_format; masking is off for this scene");

        Self {
            coverage: None,
            mask,
            color_format: config.color_format,
            stencil_format: config.stencil_format,
            globals_buffer,
            scene_bind_group: None,
            viewport: None,
            frame_buffer: None,
            frame_capacity: [0; 3],
            frame_offsets: [0; 2],
            frame: FrameChunk::new(*config),
            ss_params_buffer: None,
            ss_params_bind_group: None,
            stats: SceneStats::default(),
            // Deferred to the first frame: `new` has no `Queue` to read the
            // timestamp period from, and the borrow model does not lend one.
            timer: None,
        }
    }

    /// What the last frame cost.
    ///
    /// Counts are always populated. `gpu_pass_time_ns` needs the device to have
    /// been created with `Features::TIMESTAMP_QUERY`, and the host to attach
    /// [`SSScene::timestamp_writes`] to its render pass and call
    /// [`SSScene::resolve_timestamps`] after it.
    pub fn stats(&self) -> SceneStats {
        SceneStats { gpu_pass_time_ns: self.timer.as_ref().and_then(|t| t.last_ns()), ..self.stats }
    }

    /// Timestamp writes for the host's `RenderPassDescriptor`, or `None` when the
    /// device cannot time.
    ///
    /// The host opens the pass (borrow model), so it is the only place these
    /// can be attached — the scene can only hand them over.
    pub fn timestamp_writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.timer.as_ref().map(|t| t.writes())
    }

    /// Copy this frame's timestamps back toward the CPU. Call on the same
    /// encoder as the pass, after it ends. No-op without the feature.
    pub fn resolve_timestamps(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(timer) = self.timer.as_mut() {
            timer.resolve(encoder);
        }
    }

    /// Rasterise this frame's coverage bitmaps.
    ///
    /// Call between [`SSScene::update`] and [`SSScene::draw`], on **the same
    /// encoder** the host will open its own pass with. Passes recorded into one
    /// encoder execute in order, so what this bakes is what the very next pass
    /// samples — which is why this player has none of the one-frame lag that
    /// engines whose offscreen targets are only readable next frame have to work
    /// around. Do not split the two across encoders or frames.
    ///
    /// A no-op under any backend but [`MaskBackend::Coverage`], and on frames
    /// where nothing masks.
    pub fn encode_mask_coverage(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        if self.mask != MaskBackend::Coverage || self.frame.coverage_nodes.is_empty() {
            return;
        }
        if self.scene_bind_group.is_none() {
            return;
        }

        let layers = self.frame.coverage_nodes.len() as u32;
        let size = self.coverage_size(&self.frame.coverage_nodes);
        self.ensure_coverage(renderer, device, size, layers);
        // Re-borrowed after `ensure_coverage`, which may have replaced the array.
        let (Some(scene_bg), Some(targets)) = (self.scene_bind_group.as_ref(), self.coverage.as_ref()) else { return };

        for layer in 0..layers {
            // Cleared per layer: a bitmap is only ever read in the frame it was
            // written, so there is nothing to preserve, and a stale bit left
            // behind would clip content that no writer covers.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("SpriteStudio Mask Coverage"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.layer_views[layer as usize],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, scene_bg, &self.frame_offsets);
            for call in self.frame.coverage_calls.iter().filter(|call| call.cov_layer == Some(layer)) {
                let Some(pipeline) = renderer.pipelines.get(&call.key) else { continue };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(1, renderer.texture_bind_group(call.texture_hash), &[]);
                pass.draw(call.vertices.clone(), call.instances.clone());
            }
        }
        self.stats.coverage_layers = layers;
    }

    /// The edge length every coverage layer gets this frame.
    ///
    /// Quantised to a power of two so a mask that scales smoothly keeps the same
    /// texture instead of reallocating every frame, and shared across layers
    /// because each node's bbox is normalised onto its whole layer — a small mask
    /// does not waste resolution by sharing a size with a large one, it just
    /// spends more texels on itself than it needs.
    fn coverage_size(&self, nodes: &[CoverageNode]) -> u32 {
        let longest = nodes.iter().fold(0.0f32, |acc, node| acc.max(node.extent.0).max(node.extent.1));
        let wanted = (longest.max(1.0).ceil() as u32).next_power_of_two().clamp(COVERAGE_MIN_SIZE, COVERAGE_MAX_SIZE);

        // Shrinking waits for the footprint to fall well inside the smaller class;
        // growing is immediate. The asymmetry is deliberate:
        // holding a size too long costs VRAM nobody sees, while growing late
        // costs visibly ragged mask edges.
        match self.coverage.as_ref().map(|targets| targets.size) {
            Some(current) if wanted < current && longest > (current as f32 / 2.0) * (1.0 - COVERAGE_SHRINK_DEADBAND) => current,
            _ => wanted,
        }
    }

    /// Build or rebuild the coverage array when the frame needs a shape the
    /// current one cannot serve. Layers only ever grow within a frame's life, so
    /// a frame with fewer masking players reuses the larger array.
    fn ensure_coverage(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, size: u32, layers: u32) {
        if self.coverage.as_ref().is_some_and(|targets| targets.size == size && targets.layers >= layers) {
            return;
        }
        let layers = layers.max(self.coverage.as_ref().map_or(0, |targets| targets.layers));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SpriteStudio Mask Coverage"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COVERAGE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let layer_views = (0..layers)
            .map(|layer| texture.create_view(&wgpu::TextureViewDescriptor { label: Some("SpriteStudio Mask Coverage Layer"), dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: layer, array_layer_count: Some(1), ..Default::default() }))
            .collect();
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor { label: Some("SpriteStudio Mask Coverage Array"), dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let bind_group = renderer.coverage_bind_group(device, &array_view);
        self.coverage = Some(CoverageTargets { _texture: texture, layer_views, bind_group, size, layers });
    }

    /// Whether the hardware stencil is in use for this target. Also what decides
    /// whether `draw` sets a stencil reference and whether pipelines carry a
    /// depth-stencil state.
    pub fn stencil_enabled(&self) -> bool {
        self.mask == MaskBackend::Stencil
    }

    /// How this target draws masks.
    pub fn mask_backend(&self) -> MaskBackend {
        self.mask
    }

    /// The config this renderer was built from. Retained so a lost device can be
    /// replaced by a renderer targeting the same formats without the
    /// caller having to remember them.
    pub fn config(&self) -> SceneConfig {
        SceneConfig { color_format: self.color_format, stencil_format: self.stencil_format, mask: self.mask }
    }

    /// Set the projection to the render target's pixel dimensions. Remembered, so
    /// [`SSScene::restore_device`] can re-apply it to a rebuilt buffer.
    pub fn set_viewport(&mut self, queue: &wgpu::Queue, width: f32, height: f32, y_down: bool) {
        self.update_projection(queue, width, height, y_down);
        self.viewport = Some((width, height, y_down));
    }

    /// Rebuild this target's GPU state against a freshly created `device` after
    /// the previous one was lost. The host calls
    /// [`SSRenderer::restore_device`] first — that covers pipelines and textures
    /// for the whole device — then this once per render target, then
    /// [`crate::SSPlayer::restore_textures`] once per player.
    pub fn restore_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let viewport = self.viewport;
        *self = Self::new(device, &SceneConfig { color_format: self.color_format, stencil_format: self.stencil_format, mask: self.mask });
        if let Some((width, height, y_down)) = viewport {
            self.set_viewport(queue, width, height, y_down);
        }
    }

    /// Update the projection matrix to the target's pixel dimensions.
    ///
    /// `y_down` describes the **target**, not the geometry: the players hand up
    /// Y-down world space (the Brain's coordinate system, set in `build_node`),
    /// so a Y-down target passes it straight through and a Y-up one is where the
    /// flip has to happen. That is why the sign reads inverted against the flag.
    fn update_projection(&self, queue: &wgpu::Queue, width: f32, height: f32, y_down: bool) {
        let scale_y = if y_down { 2.0 / height } else { -2.0 / height };
        let matrix = [2.0 / width, 0.0, 0.0, 0.0, 0.0, scale_y, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::cast_slice(&matrix));
    }

    /// Advance every player and build this target's frame.
    ///
    /// This is the frame boundary: one `begin_frame` / `end_frame` pair spans
    /// **all** the players sharing the target, so their geometry lands in one
    /// vertex buffer and their draws can be recorded back to back.
    ///
    /// Players are drawn in ascending [`crate::SSPlayer::order`], ties keeping the
    /// order the caller passed them in (the sort is stable). The scene borrows
    /// them rather than owning them, so they can live wherever the
    /// host keeps them — a `Vec`, an ECS, an engine's scene graph. Each player
    /// appends its geometry straight to the frame, in that order.
    pub fn update(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, delta_secs: f32, players: &mut [&mut crate::SSPlayer]) {
        self.begin_frame();
        players.sort_by_key(|p| p.order());

        // Before the walk: this is the one step that needs `&mut SSRenderer`. It
        // does nothing at all except on the frames right after a pack is loaded.
        for player in players.iter_mut() {
            player.flush_textures(renderer, device, queue);
        }

        for player in players.iter_mut() {
            player.build_frame(&mut self.frame, renderer, delta_secs);
        }
        self.stats.players = players.iter().filter(|p| p.draws()).count() as u32;
        self.end_frame(renderer, device, queue);
    }

    /// Reset per-frame accumulation. Call once before appending any batches.
    pub fn begin_frame(&mut self) {
        if let Some(timer) = self.timer.as_mut() {
            // Both halves of the readback handshake happen here, at the one point
            // in the frame where the previous frame is guaranteed submitted.
            timer.settle();
            timer.poll();
        }
        self.stats = SceneStats::default();
        self.frame.reset(self.config());
    }

    /// Upload the accumulated frame to the GPU. Call once after all batches.
    pub fn end_frame(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        renderer.ensure_white(queue);
        if self.timer.is_none() {
            self.timer = GpuTimer::new(device, queue);
        }

        self.stats.prims = self.frame.prims.len() as u32;
        self.stats.draw_calls = self.frame.draw_calls.len() as u32;
        self.stats.uploaded_bytes = (std::mem::size_of_val(self.frame.arena.as_slice()) + std::mem::size_of_val(self.frame.prims.as_slice()) + std::mem::size_of_val(self.frame.mesh_stream.as_slice())) as u64;

        if self.frame.prims.is_empty() {
            return;
        }

        // Each array starts at the first aligned offset past the one before it.
        let bytes = [std::mem::size_of_val(self.frame.arena.as_slice()), std::mem::size_of_val(self.frame.prims.as_slice()), std::mem::size_of_val(self.frame.mesh_stream.as_slice())].map(|n| n as u64);
        let prims_at = bytes[0].next_multiple_of(FRAME_ALIGN);
        let stream_at = (prims_at + bytes[1]).next_multiple_of(FRAME_ALIGN);
        self.frame_offsets = [prims_at as u32, stream_at as u32];

        // Growing any of the three invalidates group(0), so the buffer is sized
        // for all of them and the bind group rebuilt only when it actually moved.
        if self.ensure_frame_buffer(device, bytes) || self.scene_bind_group.is_none() {
            let buffer = self.frame_buffer.as_ref().expect("ensure_frame_buffer just built it");
            let binding = |binding, size| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer, offset: 0, size: wgpu::BufferSize::new(size) }) };
            self.scene_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("SpriteStudio Scene Bind Group"),
                layout: &renderer.scene_bind_group_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.globals_buffer.as_entire_binding() }, binding(1, self.frame_capacity[0]), binding(2, self.frame_capacity[1]), binding(3, self.frame_capacity[2])],
            }));
        }
        if let Some(buffer) = self.frame_buffer.as_ref() {
            upload_frame(queue, buffer, &self.frame, self.frame_offsets);
        }

        // Compile whatever this frame draws with and has not drawn with before. After the first few frames every key is a cache hit, so
        // this is a walk over the draw list and nothing more. Indexed rather
        // than iterated because `ensure_pipeline` needs `&mut self`.
        //
        // The same walk counts what draw-call merging and bind-group dedup would
        // save. Both are cheap to measure and neither is
        // cheap to implement, so measure first.
        // The bake list is compiled here too. It is drawn from a different pass,
        // by which point there is no `&mut SSRenderer` to build a pipeline with —
        // a writer whose pipeline was missing would simply not rasterise, and an
        // empty bitmap reads as "no coverage", which draws masked content
        // unclipped rather than failing.
        for i in 0..self.frame.coverage_calls.len() {
            let key = self.frame.coverage_calls[i].key;
            renderer.ensure_pipeline(device, key);
        }

        for i in 0..self.frame.draw_calls.len() {
            let key = self.frame.draw_calls[i].key;
            renderer.ensure_pipeline(device, key);

            let Some(previous) = i.checked_sub(1).map(|p| &self.frame.draw_calls[p]) else { continue };
            let call = &self.frame.draw_calls[i];
            if call.texture_hash == previous.texture_hash {
                self.stats.redundant_texture_binds += 1;
            }
            // A shader draw carries its own params offset, so two of them never
            // collapse into one however well their ranges line up.
            if call.key == previous.key && call.texture_hash == previous.texture_hash && call.shader_slot.is_none() && previous.shader_slot.is_none() && call.vertices.start == previous.vertices.end {
                self.stats.mergeable_draws += 1;
            }
        }

        // Integration shaders: upload this frame's per-part params.
        // Nothing here runs for shader-free frames (`ss_params` stays empty).
        if !self.frame.ss_params.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&self.frame.ss_params);
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
    ///
    /// Recorded fresh every frame. The draw list changes on most frames as parts
    /// appear and disappear, so a recording would be invalidated more often than
    /// it is replayed.
    pub fn draw<'a>(&'a self, renderer: &'a SSRenderer, rpass: &mut wgpu::RenderPass<'a>) -> usize {
        if self.frame.draw_calls.is_empty() {
            return 0;
        }

        let Some(scene_bg) = self.scene_bind_group.as_ref() else { return 0 };

        // No vertex or index buffer: group(0) *is* the geometry.
        rpass.set_bind_group(0, scene_bg, &self.frame_offsets);
        if self.stencil_enabled() {
            // Stencil ref is always 0; ops/compare are baked into the pipelines.
            rpass.set_stencil_reference(0);
        }

        let mut issued = 0usize;
        for call in &self.frame.draw_calls {
            // Missing texture -> 1x1 white fallback.
            let texture = renderer.texture_bind_group(call.texture_hash);

            // `end_frame` compiled every key this frame uses, so a miss here means
            // the key was an `ss-*` shader id `build_pipeline` declined to build.
            let (Some(pipeline), params_bg) = (renderer.pipelines.get(&call.key), self.ss_params_bind_group.as_ref()) else {
                continue;
            };
            // Integration-shader part: group(2) carries the part's
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
            // Set per draw rather than once: group(2) is the coverage array under
            // one pipeline layout and the shader params under another, so a shader
            // part drawn in between invalidates whichever was there.
            if matches!(call.key.kind, PipelineKind::ContentCoverage(..)) {
                let Some(coverage) = self.coverage.as_ref() else { continue };
                rpass.set_bind_group(2, &coverage.bind_group, &[]);
            }
            rpass.draw(call.vertices.clone(), call.instances.clone());
            issued += 1;
        }

        issued
    }
}

/// Where each of the frame's three arrays may start in the frame buffer.
///
/// The largest `min_storage_buffer_offset_alignment` a device may ask for, so
/// every device takes it: the limit is a power of two no larger than this.
const FRAME_ALIGN: u64 = 256;

impl SSScene {
    /// Make sure each of group(0)'s three bindings can hold `bytes` of its array,
    /// reallocating the buffer (with headroom) if not. Returns whether it was
    /// replaced — the bind group is stale if so.
    fn ensure_frame_buffer(&mut self, device: &wgpu::Device, bytes: [u64; 3]) -> bool {
        if self.frame_buffer.is_some() && bytes.iter().zip(&self.frame_capacity).all(|(needed, held)| needed <= held) {
            return false;
        }
        for (held, &needed) in self.frame_capacity.iter_mut().zip(&bytes) {
            // A binding cannot be empty, hence the floor; an array with no
            // entries this frame still needs one to bind.
            if needed > *held || *held == 0 {
                *held = (needed * 2).max(1024).next_multiple_of(FRAME_ALIGN);
            }
        }
        // The binding past each start is a whole capacity long, and a start is
        // never later than the capacities before it add up to: the arena's
        // binding overlaps the arrays after it, which read-only bindings may.
        let size = self.frame_capacity.iter().sum();
        self.frame_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor { label: Some("SpriteStudio Frame"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
        true
    }
}

/// The `f32`s of a FlatBuffers vector's payload, which is little-endian and
/// not necessarily aligned for `f32`.
fn f32s(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes.as_chunks::<4>().0.iter().map(|word| f32::from_le_bytes(*word))
}

/// Write the frame's three arrays where `offsets` put them.
///
/// Natively in one `write_buffer_with`, so the frame goes up through one staging
/// buffer. The browser's `writeBuffer` stages for us and reads straight out of
/// wasm memory, while `write_buffer_with` there fills a zeroed copy on the heap
/// first, so on the web the arrays go up as three writes.
fn upload_frame(queue: &wgpu::Queue, buffer: &wgpu::Buffer, frame: &FrameChunk, offsets: [u32; 2]) {
    let arena: &[u8] = bytemuck::cast_slice(&frame.arena);
    let prims: &[u8] = bytemuck::cast_slice(&frame.prims);
    let stream: &[u8] = bytemuck::cast_slice(&frame.mesh_stream);
    let [prims_at, stream_at] = offsets.map(|at| at as usize);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(size) = wgpu::BufferSize::new((stream_at + stream.len()) as u64) else { return };
        let Some(mut view) = queue.write_buffer_with(buffer, 0, size) else { return };
        view.slice(..arena.len()).copy_from_slice(arena);
        view.slice(arena.len()..prims_at).fill(0);
        view.slice(prims_at..prims_at + prims.len()).copy_from_slice(prims);
        view.slice(prims_at + prims.len()..stream_at).fill(0);
        view.slice(stream_at..).copy_from_slice(stream);
    }
    #[cfg(target_arch = "wasm32")]
    for (at, bytes) in [(0, arena), (prims_at, prims), (stream_at, stream)] {
        if !bytes.is_empty() {
            queue.write_buffer(buffer, at as u64, bytes);
        }
    }
}

/// One frame of one render target: the arena / primitive / draw-call trio every
/// player sharing the target appends to, in its `order`.
pub struct FrameChunk {
    /// The target this frame is being built for — needed for pipeline keys.
    config: SceneConfig,
    /// Runtime SoA buffers and node transforms, copied verbatim.
    arena: Vec<f32>,
    prims: Vec<Prim>,
    /// One entry per mesh vertex drawn: the runtime's own index into its part's
    /// vertices, tagged with the part's primitive (see [`MESH_PRIM_SHIFT`]).
    mesh_stream: Vec<u32>,
    draw_calls: Vec<BatchDrawCall>,
    /// Writer draws for the coverage bitmap, kept apart from `draw_calls` because
    /// they are replayed into a different render pass — the one
    /// [`SSScene::encode_mask_coverage`] opens before the host opens its own.
    /// Empty under every backend but [`MaskBackend::Coverage`].
    coverage_calls: Vec<BatchDrawCall>,
    /// One entry per node that writes coverage this frame, in layer order.
    coverage_nodes: Vec<CoverageNode>,
    /// Per-part integration-shader params, one `SHADER_PARAM_STRIDE_F32` slot each.
    ss_params: Vec<f32>,
}

impl FrameChunk {
    fn new(config: SceneConfig) -> Self {
        Self { config, arena: Vec::new(), prims: Vec::new(), mesh_stream: Vec::new(), draw_calls: Vec::new(), coverage_calls: Vec::new(), coverage_nodes: Vec::new(), ss_params: Vec::new() }
    }

    /// Empty it for reuse, keeping the allocations — the frame is rebuilt every
    /// frame, so this is what stops a per-frame allocation storm.
    fn reset(&mut self, config: SceneConfig) {
        self.config = config;
        self.arena.clear();
        self.prims.clear();
        self.mesh_stream.clear();
        self.draw_calls.clear();
        self.coverage_calls.clear();
        self.coverage_nodes.clear();
        self.ss_params.clear();
    }

    /// Whether the hardware stencil is in use for the target this is built for.
    pub fn stencil_enabled(&self) -> bool {
        self.config.mask == MaskBackend::Stencil && self.config.stencil_format.is_some()
    }

    /// How the target this is built for draws masks.
    ///
    /// [`crate::SSPlayer`]'s walk reads this rather than [`Self::stencil_enabled`]:
    /// planning writers, skipping a pure mask's colour and testing a masked
    /// target are the same work for both backends, and only what the batch is
    /// *appended as* differs.
    pub fn mask_backend(&self) -> MaskBackend {
        if self.config.mask == MaskBackend::Stencil && self.config.stencil_format.is_none() { MaskBackend::None } else { self.config.mask }
    }

    /// Cache key for a content draw. Without a stencil buffer every draw uses the
    /// untested variant, so the test is normalised away rather than filling the
    /// cache with three identical pipelines.
    fn content_key(&self, blend: BlendType, stencil: StencilTest, class: GeomClass) -> PipelineKey {
        match self.mask_backend() {
            // The replay reads the polarity off the primitive, so a coverage-masked
            // part needs the coverage pipeline whichever way round it clips.
            MaskBackend::Coverage if stencil != StencilTest::None => self.key(PipelineKind::ContentCoverage(blend_index(blend), stencil, class)),
            MaskBackend::Stencil if self.config.stencil_format.is_some() => self.key(PipelineKind::Content(blend_index(blend), stencil, class)),
            _ => self.key(PipelineKind::Content(blend_index(blend), StencilTest::None, class)),
        }
    }

    /// Attach this target's formats to a pipeline kind.
    fn key(&self, kind: PipelineKind) -> PipelineKey {
        PipelineKey { kind, color_format: self.config.color_format, stencil_format: self.config.stencil_format }
    }

    /// Attach the player's coverage block to its top node, once the whole tree's
    /// writers are planned.
    ///
    /// `uv` is the affine that takes a **player-local** point to coverage UV
    /// `[0,1]` — the bbox normalisation, as
    /// `[a, b, c, d, tx, ty]` for `u = a*x + c*y + tx`, `v = b*x + d*y + ty`.
    /// Both the bake (which turns UV into clip space) and the replay (which
    /// samples with it) read this one transform, so the two cannot disagree about
    /// where the bitmap is — the mismatch that costs Godot's implementation a
    /// frame of lag cannot be represented here.
    pub fn set_node_coverage(&mut self, node: &mut NodeSlot, uv: &[f32; 6], extent: (f32, f32), writers: &[ScopedWriter]) {
        let layer = self.coverage_nodes.len() as u32;
        let base = self.push_coverage_block(uv, layer, writers);
        self.coverage_nodes.push(CoverageNode { extent });
        // One more layer per `COVERAGE_LAYER_BITS` writers, consecutive, so a
        // writer's layer is the block's plus its bit's quotient.
        for _ in 1..writers.len().div_ceil(crate::mask::COVERAGE_LAYER_BITS) {
            self.coverage_nodes.push(CoverageNode { extent });
        }
        node.coverage = base;
        node.coverage_layer = layer;
    }

    /// Point a sub-animation at its player's bitmap.
    ///
    /// No new layer: the child reads the one bitmap the top node baked, and the
    /// same writer records — their scopes are on the tree sequence, which the
    /// whole tree shares. Only the affine differs, composed with the chain of
    /// instance placements so the child's own local coordinates land in the top
    /// node's coverage.
    pub fn inherit_node_coverage(&mut self, node: &mut NodeSlot, uv: &[f32; 6], layer: u32, writers: &[ScopedWriter]) {
        node.coverage = self.push_coverage_block(uv, layer, writers);
        node.coverage_layer = layer;
    }

    /// Which layer a node's coverage went to, for pointing the rest of its tree
    /// at it.
    pub fn node_coverage_layer(&self, node: &NodeSlot) -> u32 {
        node.coverage_layer
    }

    /// Append a coverage block and return where it landed.
    fn push_coverage_block(&mut self, uv: &[f32; 6], layer: u32, writers: &[ScopedWriter]) -> u32 {
        let base = self.arena.len() as u32;
        self.arena.extend_from_slice(uv);
        self.arena.push(layer as f32);
        self.arena.push(writers.len() as f32);
        for (bit, w) in writers.iter().enumerate() {
            self.arena.push(w.open as f32);
            self.arena.push(w.close as f32);
            self.arena.push(bit as f32);
            self.arena.push(if w.writer.op_invert { 1.0 } else { 0.0 });
        }
        base
    }

    /// Copy a placement matrix into the arena: a node's, before any of its
    /// primitives, or an effect's, whose particles the plan already placed
    /// relative to it.
    ///
    /// `transform` is the player's placement for a root, composed with the
    /// world matrix of every instance part above it for a child, and with the
    /// effect part's for an effect.
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
    /// fixed span of `vertex_index` values. Primitives of other
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
        self.draw_calls.push(BatchDrawCall { key, texture_hash, vertices: first * stride..last * stride, instances: 0..1, shader_slot, cov_layer: None });
    }

    /// Append one Normal/Mesh DrawBatch's geometry into the frame accumulation,
    /// pre-multiplying every vertex by `transform` (identity for the root; the
    /// composed parent instance world matrix for instance children). `frame` is
    /// re-parsed for its draw_order/parts/parts_color; the batch fields are
    /// passed in so the caller (SSPlayer) drives batch iteration and can
    /// interleave instance recursion at the right rank. `batch.stencil` selects
    /// the stencil-test content pipeline for masked parts.
    pub fn append_batch(&mut self, renderer: &SSRenderer, frame: &FrameView, node: &NodeSlot, batch: &BatchInfo) {
        let BatchInfo { start_rank, count, texture_hash, blend_type, kind, stencil, mask_seq, alpha, tint } = *batch;
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

        // Under the coverage backend a masked target carries its own scope test:
        // which bitmap to read, its place on the tree sequence, and its polarity.
        // The stencil backend leaves this zero because the pipeline state does
        // the testing for it.
        //
        // `mask_seq` is the part's own — the walk splits masked batches to one
        // part each precisely so this is true.
        let (cov_base, mask_info) = match stencil {
            _ if self.mask_backend() != MaskBackend::Coverage || node.coverage == NO_COVERAGE => (0, 0),
            StencilTest::Inside => (node.coverage, target_mask_info(mask_seq, true)),
            StencilTest::Outside => (node.coverage, target_mask_info(mask_seq, false)),
            StencilTest::None => (0, 0),
        };

        let first_prim = self.prims.len() as u32;

        match kind {
            DrawBatchKind::Normal => {
                let key = self.content_key(blend_type, stencil, GeomClass::Quad);
                // Integration-shader dispatch: when the frame carries
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
                    self.push_quad_prim(frame, node, part_idx, Prim { mode, alpha, tint, colors, alphas, rates, cov_base, mask_info, ..Default::default() });
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
                let (mut stream_start, mut draw_prim) = (self.mesh_stream.len() as u32, first_prim);
                for rank in start_rank..start_rank + count {
                    if self.prims.len() as u32 - draw_prim == MESH_DRAW_PRIMS {
                        let call = self.mesh_draw(key, texture_hash, stream_start, draw_prim, None);
                        self.draw_calls.extend(call);
                        (stream_start, draw_prim) = (self.mesh_stream.len() as u32, self.prims.len() as u32);
                    }
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

                    self.push_mesh_prim(frame, part_idx, draw_prim, Prim { mode, alpha, tint, node_base: node.transform, z: part.position_z(), colors: [color; 4], alphas: [part_alpha; 4], rates: [rate; 4], cov_base, mask_info, ..Default::default() });
                }
                let call = self.mesh_draw(key, texture_hash, stream_start, draw_prim, None);
                self.draw_calls.extend(call);
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
                    // fill is the bilinear PartColor itself.
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

                    self.push_shape_prim(frame, node, part_idx, Prim { mode: mode::MIX, alpha, tint, colors, alphas, rates: [if has_pc { 1.0 } else { 0.0 }; 4], cov_base, mask_info, ..Default::default() });
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
    /// `fs_mask` reads as `varg.x`: it needs the full f32 range,
    /// including the -1 that means "no cutout". No-op if masking is disabled.
    pub fn append_mask_writer(&mut self, frame: &FrameView, node: &NodeSlot, writer: &MaskWriter, op: MaskOp) {
        self.append_writer(frame, node, writer, MaskWriterDest::Stencil(op));
    }

    /// Rasterise one writer's shape into the coverage bitmap, setting `bit`.
    ///
    /// Called once per writer per frame — there is no add/remove interleave here,
    /// because the coverage backend does not carry a running state through the
    /// draw walk. What the stencil path spends passes replaying, the replay
    /// shader recomputes per fragment from the writer's rank.
    pub fn append_coverage_writer(&mut self, frame: &FrameView, node: &NodeSlot, writer: &MaskWriter, bit: usize) {
        self.append_writer(frame, node, writer, MaskWriterDest::Coverage(bit));
    }

    /// The geometry half both writer paths share. Only the primitive's coverage
    /// word and which list the draw call lands in differ.
    fn append_writer(&mut self, frame: &FrameView, node: &NodeSlot, writer: &MaskWriter, dest: MaskWriterDest) {
        use crate::mask::GeomKind;
        if self.mask_backend() == MaskBackend::None || frame.frame_data.is_empty() {
            return;
        }
        if frame.world_matrices.is_empty() {
            return;
        }
        // Colour output is masked off for the stencil pipelines and carries the
        // writer's bit for the coverage ones, so only the threshold in `rates`
        // matters to both.
        let mut prim = Prim { rates: [writer.threshold; 4], alpha: 1.0, ..Default::default() };
        let kind_of = |class| match dest {
            MaskWriterDest::Stencil(op) => PipelineKind::Mask(op, class),
            MaskWriterDest::Coverage(_) => PipelineKind::Coverage(class),
        };
        if let MaskWriterDest::Coverage(bit) = dest {
            if node.coverage == NO_COVERAGE {
                return;
            }
            prim.cov_base = node.coverage;
            prim.mask_info = writer_mask_info(bit);
        }
        // Only a bake call picks a layer; masked content reads the layer its
        // primitive's coverage block names, so one bind group serves them all.
        // Past the first layer's bits a writer lands in the next layer along.
        let cov_layer = match dest {
            MaskWriterDest::Coverage(bit) => Some(node.coverage_layer + (bit / crate::mask::COVERAGE_LAYER_BITS) as u32),
            MaskWriterDest::Stencil(_) => None,
        };
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
                let stream_start = self.mesh_stream.len() as u32;
                self.push_mesh_prim(frame, writer.part_idx, first_prim, Prim { node_base: node.transform, ..prim });
                let call = self.mesh_draw(self.key(kind_of(GeomClass::Mesh)), writer.texture_hash, stream_start, first_prim, cov_layer);
                self.writer_calls(dest).extend(call);
                return;
            }
            GeomKind::Shape => {
                self.push_shape_prim(frame, node, writer.part_idx, prim);
                GeomClass::Shape
            }
            GeomKind::Unsupported => return,
        };

        self.push_writer_draw(self.key(kind_of(class)), class, writer.texture_hash, first_prim, dest, cov_layer);
    }

    /// Which list a writer's draw call belongs in.
    fn writer_calls(&mut self, dest: MaskWriterDest) -> &mut Vec<BatchDrawCall> {
        match dest {
            MaskWriterDest::Stencil(_) => &mut self.draw_calls,
            MaskWriterDest::Coverage(_) => &mut self.coverage_calls,
        }
    }

    /// [`FrameChunk::push_draw`] against whichever list `dest` names.
    fn push_writer_draw(&mut self, key: PipelineKey, class: GeomClass, texture_hash: u32, first: u32, dest: MaskWriterDest, cov_layer: Option<u32>) {
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
        let call = BatchDrawCall { key, texture_hash, vertices: first * stride..last * stride, instances: 0..1, shader_slot: None, cov_layer };
        self.writer_calls(dest).push(call);
    }

    /// Record one sprite quad for `part_idx`: copy the part's world matrix, local
    /// vertices and local UVs into the arena, point `prim` at them and append it.
    /// The corners themselves are expanded by `vs_quad`.
    fn push_quad_prim(&mut self, frame: &FrameView, node: &NodeSlot, part_idx: usize, prim: Prim) {
        // A part the runtime has no geometry for must not produce a primitive at
        // all: the vertex stage reads whatever its offsets point at.
        let (Some(matrix), Some(vertices), Some(uvs)) = (frame.world_matrices.get(part_idx * 16..part_idx * 16 + 16), frame.local_vertices.get(part_idx * 10..part_idx * 10 + 10), frame.local_uvs.get(part_idx * 10..part_idx * 10 + 10)) else {
            return;
        };
        let aux_base = self.arena.len() as u32;
        self.arena.extend_from_slice(matrix);
        self.arena.extend_from_slice(vertices);
        self.arena.extend_from_slice(uvs);
        self.push_prim(Prim { aux_base, pos_base: aux_base + 16, uv_base: aux_base + 26, node_base: node.transform, ..prim });
    }

    /// Record one skinned-mesh part, the primitive after `draw_prim` in its draw.
    ///
    /// A mesh's vertex count varies by part, so its geometry is copied per drawn
    /// part rather than per node: its vertices into the arena as the runtime laid
    /// them out, and its triangle list into the mesh stream, each index tagged
    /// with the part's place in its draw.
    fn push_mesh_prim(&mut self, frame: &FrameView, part_idx: usize, draw_prim: u32, prim: Prim) {
        let (vertices, indices) = (&frame.mesh_vertex_offsets, &frame.mesh_index_offsets);
        if part_idx + 1 >= vertices.len() || part_idx + 1 >= indices.len() {
            return;
        }
        let (start, end) = (vertices[part_idx] as usize, vertices[part_idx + 1] as usize);
        let (Some(x), Some(y), Some(uvs), Some(triangles)) =
            (frame.mesh_vertices_x.get(start..end), frame.mesh_vertices_y.get(start..end), frame.mesh_uvs.get(start * 2..end * 2), frame.mesh_indices.get(indices[part_idx] as usize..indices[part_idx + 1] as usize))
        else {
            return;
        };
        if x.is_empty() || triangles.is_empty() {
            return;
        }

        let pos_base = self.arena.len() as u32;
        self.arena.extend_from_slice(x);
        let aux_base = self.arena.len() as u32;
        self.arena.extend_from_slice(y);
        let uv_base = self.arena.len() as u32;
        self.arena.extend_from_slice(uvs);
        let tag = (self.prims.len() as u32 - draw_prim) << MESH_PRIM_SHIFT;
        self.mesh_stream.extend(triangles.iter().map(|&index| tag | index as u32));
        self.push_prim(Prim { aux_base, pos_base, uv_base, ..prim });
    }

    /// The draw over the stream from `stream_start` to its end, whose first
    /// primitive is `draw_prim`, or `None` when that is empty.
    fn mesh_draw(&self, key: PipelineKey, texture_hash: u32, stream_start: u32, draw_prim: u32, cov_layer: Option<u32>) -> Option<BatchDrawCall> {
        let stream_end = self.mesh_stream.len() as u32;
        (stream_end > stream_start).then(|| BatchDrawCall { key, texture_hash, vertices: stream_start..stream_end, instances: draw_prim..draw_prim + 1, shader_slot: None, cov_layer })
    }

    /// Record one Shape part, drawn as a fan from vertex 0 by `vs_shape`: copy
    /// its world matrix and its live vertices and box coordinates into the arena.
    /// No-op if the part is not a shape or has fewer than 3 live vertices.
    fn push_shape_prim(&mut self, frame: &FrameView, node: &NodeSlot, part_idx: usize, prim: Prim) {
        // Layout: `shape_vertices[part_idx * SHAPE_VERTICES_STRIDE + 2*v]` = local xy.
        let count = frame.shape_vertex_counts.get(part_idx).copied().unwrap_or(0).max(0) as usize;
        // The fan pattern is baked into the vertex stage at a fixed stride, so a
        // shape with more vertices than that would silently lose triangles.
        if count < 3 || count > (SHAPE_FAN_VERTS as usize / 3) + 2 {
            return;
        }
        let base = part_idx * crate::frame::SHAPE_VERTICES_STRIDE;
        let (Some(matrix), Some(vertices), Some(box_coords)) = (frame.world_matrices.get(part_idx * 16..part_idx * 16 + 16), frame.shape_vertices.get(base..base + count * 2), frame.shape_vertex_box_coords.get(base..base + count * 2)) else {
            return;
        };
        let aux_base = self.arena.len() as u32;
        self.arena.extend_from_slice(matrix);
        self.arena.extend_from_slice(vertices);
        self.arena.extend_from_slice(box_coords);
        self.push_prim(Prim { aux_base, pos_base: aux_base + 16, uv_base: aux_base + 16 + (count * 2) as u32, node_base: node.transform, count: count as u32, ..prim });
    }

    /// Append an effect slot's `EffectDrawPlan` (a raw FlatBuffer) into the
    /// frame accumulation. `transform` is the effect part's composed world
    /// matrix (verts are slot-relative). Blend is per-command; color is taken
    /// from the plan and modulates the cellmap texture via the sprite shader.
    /// `stencil` selects the stencil test when the effect is a masked target
    /// (inherited from a parent mask); `None` for ordinary effects. `alpha` and
    /// `tint` are the player-level fade and colour (see [`BatchInfo::alpha`] and
    /// [`BatchInfo::tint`]).
    ///
    /// Under the coverage backend a masked effect tests like any other part:
    /// `node` carries the coverage block, pulled back to the effect's own local
    /// space, and `mask_seq` is the emitter's place on the tree sequence.
    #[allow(clippy::too_many_arguments)]
    pub fn append_effect(&mut self, plan_buf: &[u8], node: &NodeSlot, stencil: StencilTest, mask_seq: u32, alpha: f32, tint: u32, keys: &crate::AtlasKeys) {
        if plan_buf.is_empty() {
            return;
        }
        let (cov_base, mask_info) = match stencil {
            _ if self.mask_backend() != MaskBackend::Coverage || node.coverage == NO_COVERAGE => (0, 0),
            StencilTest::Inside => (node.coverage, target_mask_info(mask_seq, true)),
            StencilTest::Outside => (node.coverage, target_mask_info(mask_seq, false)),
            StencilTest::None => (0, 0),
        };
        let plan = unsafe { root_as_effect_draw_plan_unchecked(plan_buf) };
        // XY pairs, four to a particle: `step_effects` asks every slot for a
        // vertex stride of 2.
        let (verts, uvs) = (plan.verts().bytes(), plan.uvs().bytes());
        let colors = plan.colors();

        for cmd in plan.commands().iter() {
            let quad_count = cmd.quad_count() as usize;
            if quad_count == 0 {
                continue;
            }
            // Cellmap-hash terms, like a draw batch's: translated to the key the
            // owning pack's atlas actually registered under.
            let texture_hash = keys.resolve(cmd.cellmap_hash());
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
            self.arena.extend(f32s(&verts[vert_first * 8..vert_last * 8]));
            let uv_base = self.arena.len() as u32;
            self.arena.extend(f32s(&uvs[vert_first * 8..vert_last * 8]));

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
                    tint,
                    colors: [color; 4],
                    alphas: [colors.get(vi * 4 + 3); 4],
                    rates: [1.0; 4],
                    cov_base,
                    mask_info,
                    ..Default::default()
                });
            }
            self.push_draw(key, GeomClass::Effect, texture_hash, first_prim, None);
        }
    }
}
