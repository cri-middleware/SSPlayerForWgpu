//! The device-scoped half of rendering.
//!
//! Pipelines, bind group layouts, the sampler, the white fallback, the shader
//! modules and the atlas textures are valid for any render target on the device
//! that made them, so the host owns one [`SSRenderer`] and lends it.
//!
//! The per-target, per-frame half is [`crate::SSScene`].

use crate::mask::{MaskOp, StencilTest};
use std::collections::HashMap;

use ssruntime::runtime::framedata::ss::runtime::BlendType;

/// Which vertex stage expands a draw's primitives.
///
/// There is no vertex buffer: the runtime's SoA arrays are copied into the
/// scene's arena verbatim and the vertex stage reads them, so what used to be a
/// CPU expansion loop per geometry kind is now an entry point per geometry kind.
/// They differ only in how `@builtin(vertex_index)` maps to a primitive and where
/// that primitive's position / UV / colour come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GeomClass {
    /// Sprite quad: 6 vertices per primitive, indexless. `vertex_index / 6` is the
    /// primitive, `% 6` picks a corner through the fixed triangle pattern.
    Quad,
    /// Skinned mesh: irregular, so a per-vertex `(primitive, vertex)` stream
    /// drives it — the one class that cannot be addressed by arithmetic alone.
    Mesh,
    /// Shape fan: a fixed [`SHAPE_FAN_VERTS`] vertices per primitive, with the
    /// unused triangles collapsed to zero area.
    Shape,
    /// Effect particle: 6 vertices per primitive, one primitive per particle
    /// quad. Its UVs arrive already normalised and its colour is per particle.
    Effect,
}

impl GeomClass {
    /// The WGSL vertex entry point that expands this class.
    fn entry(self) -> &'static str {
        match self {
            GeomClass::Quad => "vs_quad",
            GeomClass::Mesh => "vs_mesh",
            GeomClass::Shape => "vs_shape",
            GeomClass::Effect => "vs_effect",
        }
    }

    /// The vertex entry that expands this class **into coverage space**: the same
    /// geometry, projected by the node's bbox affine instead of the target's
    /// projection, and stopping short of the player's placement — the bitmap is
    /// pinned to player-local coordinates so it does not move when the player does.
    fn coverage_entry(self) -> &'static str {
        match self {
            GeomClass::Quad => "vs_cov_quad",
            GeomClass::Mesh => "vs_cov_mesh",
            GeomClass::Shape => "vs_cov_shape",
            // Effects never write coverage: the planner only ever hands back
            // Quad / Mesh / Shape writers.
            GeomClass::Effect => "vs_cov_quad",
        }
    }
}

/// The `const` prelude both shader modules are assembled on top of.
///
/// Layout facts the WGSL has to agree with the Rust on live here rather than as
/// literals in the shader text, so a change to one cannot silently leave the
/// other behind.
fn wgsl_consts() -> String {
    format!(
        "const SHAPE_FAN_VERTS: u32 = {SHAPE_FAN_VERTS}u;\n\
         const COVERAGE_HEADER: u32 = {}u;\n\
         const COVERAGE_STRIDE: u32 = {}u;\n\
         const MAX_MASK_WRITERS: u32 = {}u;\n\
         const COVERAGE_LAYER_BITS: u32 = {}u;\n",
        crate::scene::COVERAGE_HEADER_F32,
        crate::scene::COVERAGE_WRITER_F32,
        crate::mask::MAX_MASK_WRITERS,
        crate::mask::COVERAGE_LAYER_BITS,
    )
}

/// The coverage bitmap's format. RGBA8 with one writer per bit gives 24
/// writers, and matches what the reference implementation
/// rasterises, so the two players encode coverage identically.
pub(crate) const COVERAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Vertices a Shape primitive always occupies, whatever its live vertex count.
///
/// The runtime's shape buffer holds at most 12 vertices per part, so a fan is at
/// most 10 triangles. Reserving all 30 vertices keeps `vertex_index / 30` a valid
/// primitive lookup — the alternative is a prefix sum and a search per vertex.
/// Triangles past the live count collapse to a single vertex and rasterise
/// nothing, and shapes are rare enough that the wasted invocations do not matter.
pub(crate) const SHAPE_FAN_VERTS: u32 = 30;

/// What a pipeline does, before the render target is taken into account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PipelineKind {
    /// Ordinary content: blend index (into [`BLEND_ORDER`]) x stencil test x the
    /// geometry class whose vertex stage expands it.
    Content(usize, StencilTest, GeomClass),
    /// Mask-write geometry: the stencil op it applies, and the class. Colour
    /// output is masked off, so blend does not vary.
    Mask(MaskOp, GeomClass),
    /// Content clipped by a coverage bitmap rather than by the stencil: the same
    /// blend and class, plus the polarity the replay compares against. It needs
    /// its own kind because it draws from a different pipeline layout — the one
    /// with the coverage array bound at group(2).
    ContentCoverage(usize, StencilTest, GeomClass),
    /// Coverage-write geometry: the same shapes as
    /// [`PipelineKind::Mask`], rasterised into the coverage bitmap instead of the
    /// stencil. One pipeline per class — the writer's bit travels on the `Prim`,
    /// not in the pipeline, so 24 writers still compile three pipelines between
    /// them. Always targets [`COVERAGE_FORMAT`] and never has a depth-stencil,
    /// so the target's own formats drop out of the key's meaning here.
    Coverage(GeomClass),
    /// Integration shader (`ss-*`): shader id x blend index. Never
    /// stencil-tested — shader parts are not masked in v1 — and always
    /// [`GeomClass::Quad`], because only Normal parts carry a SHDR attribute.
    Shader(u32, usize),
}

/// What distinguishes one render pipeline from another.
///
/// wgpu bakes blend, depth/stencil state **and the target's formats** into the
/// pipeline, so every combination a frame uses needs its own object. They are
/// built on first use and cached under this key rather than compiled up front:
/// an animation touching one blend mode pays for one pipeline instead of the
/// whole 12 x 3 content matrix.
///
/// The formats are part of the key rather than fixed at construction, which is
/// what lets a single device-scoped [`SSRenderer`] serve render targets that do
/// not agree on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PipelineKey {
    pub(crate) kind: PipelineKind,
    pub(crate) color_format: wgpu::TextureFormat,
    pub(crate) stencil_format: Option<wgpu::TextureFormat>,
}

struct TextureEntry {
    bind_group: wgpu::BindGroup,
    size: (f32, f32),
    /// Kept alive alongside the bind group it feeds; also what
    /// [`SSRenderer::texture_alpha_mode`] reports back.
    _info_buffer: wgpu::Buffer,
    alpha: AlphaMode,
}

/// How a texture's colour channels relate to its alpha.
///
/// The whole pipeline composites premultiplied, so the fragment stage has
/// to know which form it is sampling: a premultiplied texel is divided back to
/// straight RGB before the PartsColor maths, a straight one is used as is. The
/// flag rides in the texture's own bind group, so it costs no extra pipeline.
///
/// [`SSRenderer::upload_texture`] premultiplies on the CPU and is always
/// [`AlphaMode::Premultiplied`]; [`SSRenderer::set_texture`] takes whatever the
/// supplier has, which for an engine's asset system is usually
/// [`AlphaMode::Straight`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaMode {
    /// `rgb` is already multiplied by `a`. Correct under linear filtering, and
    /// what SpriteStudio's own atlas path produces.
    Premultiplied,
    /// `rgb` is independent of `a` (the usual form of a decoded PNG, and what an
    /// engine hands over unless it was told otherwise).
    Straight,
}

/// The per-texture uniform bound at group(1) binding(2).
///
/// Small on purpose: it exists so [`AlphaMode`] does not become a pipeline
/// dimension. It also carries the texel size, which is the unit the runtime's UVs
/// arrive in — that is what lets the vertex stage normalise them
/// instead of the CPU.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct TexInfo {
    /// `(width, height, 1/width, 1/height)` in texels.
    size: [f32; 4],
    /// `x`: 1.0 when premultiplied, 0.0 when straight. `y..w` reserved.
    flags: [f32; 4],
}

impl TexInfo {
    fn new((width, height): (f32, f32), alpha: AlphaMode) -> Self {
        let inv = |v: f32| if v > 0.0 { 1.0 / v } else { 0.0 };
        Self { size: [width, height, inv(width), inv(height)], flags: [if alpha == AlphaMode::Premultiplied { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0] }
    }
}

/// Layer 2a: everything whose useful lifetime is the
/// **device**, not one player and not one render target.
///
/// Pipelines, bind group layouts, the sampler, the white fallback and the atlas
/// textures are all valid for any target on the device that created them, so
/// exactly one of these serves every [`crate::SSScene`] and every
/// [`crate::SSPlayer`] drawing on that device. The host owns it and lends it,
/// the same way it lends `Device` and `Queue` — putting a copy inside each
/// player is what made a second character on screen duplicate ~39 pipelines and
/// every atlas.
///
/// Nothing here is format-specific: the pipeline key carries the target formats,
/// so targets that disagree share one renderer.
pub struct SSRenderer {
    /// Every pipeline used so far, built on first use.
    pub(crate) pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    /// Inputs the lazy builder needs: the sprite/mask WGSL module and its layout.
    shader_module: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    /// Compiled module holding all 13 `fs_ss_*` fragment entries + the shared
    /// vertex stage; shader pipelines are derived from it lazily.
    ss_shader_module: wgpu::ShaderModule,
    /// Pipeline layout for shader pipelines: [scene, texture, ss-params].
    ss_pipeline_layout: wgpu::PipelineLayout,
    /// Pipeline layout for coverage-masked content: [scene, texture, coverage].
    coverage_pipeline_layout: wgpu::PipelineLayout,
    /// Layout of the coverage group, kept so a scene can build a bind group for
    /// whatever array it allocated this frame.
    coverage_bind_group_layout: wgpu::BindGroupLayout,

    /// group(0): the render target's projection **and its frame geometry** — the
    /// SoA arena, the primitive table and the mesh stream the vertex stages read
    /// in place of a vertex buffer. The layout is shared; every buffer
    /// behind it belongs to the target ([`crate::SSScene`]).
    pub(crate) scene_bind_group_layout: wgpu::BindGroupLayout,
    /// group(1): atlas texture + sampler + its [`TexInfo`].
    texture_bind_group_layout: wgpu::BindGroupLayout,
    /// group(2): one dynamic-offset uniform (the per-part `SsParams`).
    pub(crate) ss_params_bind_group_layout: wgpu::BindGroupLayout,

    sampler: wgpu::Sampler,
    /// 1x1 white texture, bound when a draw's texture hash is missing
    /// — used by Shape content and textureless mask writers. Filled with white on
    /// the first frame (the constructor has no queue to upload with).
    white_texture: wgpu::Texture,
    white_bind_group: wgpu::BindGroup,
    white_ready: bool,

    /// Atlas textures by fnv1a hash. Shared, so N players showing the same
    /// character upload it once.
    textures: HashMap<u32, TextureEntry>,
    /// Retained pixels for [`Retention::Retain`] uploads, so a device loss can be
    /// recovered without the host re-supplying them.
    ///
    /// **Premultiplied**, not straight: these are the very buffer that was written
    /// to the texture, kept instead of copied, and nothing ever needs the straight
    /// form back — [`SSRenderer::restore_device`] writes them out again as they
    /// are. Keeping straight pixels would cost a second full-size copy per atlas
    /// (16 MB at 2048²) to buy a form no caller asks for.
    texture_sources: HashMap<u32, (Vec<u8>, u32, u32)>,
    /// A digest of the bytes behind each **player-loaded** atlas, for the one
    /// question a key alone cannot answer: is what is registered still what the
    /// loader means by it?
    ///
    /// The key says *which atlas*, so two players of one pack rightly find each
    /// other's upload under it and neither re-uploads. But an authoring loop
    /// re-exports the same file — same path, same key, new pixels — and
    /// "already held" is how the viewer's convert / drop / look / convert again
    /// round keeps showing the old atlas. Comparing digests separates the two.
    ///
    /// Absent for anything a **host** registered: it owns that entry, and a load
    /// walk passing over the same key leaves it alone rather than claiming to
    /// know better.
    texture_digests: HashMap<u32, u32>,
    /// How many times [`SSRenderer::upload_texture`] has actually created a
    /// texture. A diagnostic: the point of a device-scoped texture map
    /// is that N players of one pack upload its atlas once between them, and only
    /// a counter — not the map's length — can tell that from N overwrites.
    texture_uploads: usize,
}

/// How a registration is remembered — the two things that outlive the upload
/// itself, bundled because they are asked and answered together.
pub(crate) struct Provenance {
    /// Whether the renderer keeps the pixels for a device-loss rebuild.
    pub retention: Retention,
    /// What the bytes behind them hashed to, or `None` for an upload a **host**
    /// made: that entry is the host's, and a load walk meeting the same key
    /// leaves it alone. See `texture_digests`.
    pub digest: Option<u32>,
}

/// Whether [`SSRenderer::upload_texture`] keeps a copy of the pixels for device
/// loss recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retention {
    /// Keep them. For textures the host supplied and may no longer hold.
    Retain,
    /// Do not: the supplier can produce them again. Embedded atlases use this —
    /// the player re-decodes from the `.ssab` it already owns, so a second
    /// full-size copy of every atlas would be pure waste.
    Reproducible,
}

/// The 12 `BlendType`s in schema discriminant order. Pipelines are
/// built in this order; [`blend_index`] maps a `BlendType` to its slot.
const BLEND_ORDER: [BlendType; 12] =
    [BlendType::Mix, BlendType::Mul, BlendType::Add, BlendType::Sub, BlendType::Mulalpha, BlendType::Screen, BlendType::Exclusion, BlendType::Invert, BlendType::Mul2, BlendType::Div2, BlendType::Screen2, BlendType::Overlay2];

/// Blend mode -> index into the per-blend pipeline groups (schema discriminant,
/// clamped into 0..12).
pub(crate) fn blend_index(blend: BlendType) -> usize {
    (blend.0 as usize).min(BLEND_ORDER.len() - 1)
}

/// Premultiplied blend state for a `BlendType` (matching the WebGL
/// `applyBlend` table). RGB factors/op per mode; alpha is `One, OneMinusSrcAlpha`
/// (premultiplied coverage) for every mode. Unverified modes mirror ssplayer-pixi:
/// Mulalpha/Mul2 -> Mul, Screen2 -> Screen, Div2/Overlay2 -> Mix.
fn blend_state(blend: BlendType) -> wgpu::BlendState {
    use wgpu::BlendFactor::{Dst, One, OneMinusDst, OneMinusSrc, OneMinusSrcAlpha, Zero};
    use wgpu::BlendOperation::{Add, ReverseSubtract};
    let (src, dst, op) = match blend {
        BlendType::Mix => (One, OneMinusSrcAlpha, Add),
        BlendType::Add => (One, One, Add),
        BlendType::Mul | BlendType::Mulalpha | BlendType::Mul2 => (Dst, OneMinusSrcAlpha, Add),
        BlendType::Sub => (One, One, ReverseSubtract),
        BlendType::Screen | BlendType::Screen2 => (OneMinusDst, One, Add),
        BlendType::Invert => (OneMinusDst, Zero, Add),
        BlendType::Exclusion => (OneMinusDst, OneMinusSrc, Add),
        // Div2 / Overlay2: no GPU-native equivalent -> Mix fallback.
        _ => (One, OneMinusSrcAlpha, Add),
    };
    wgpu::BlendState { color: wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: op }, alpha: wgpu::BlendComponent { src_factor: One, dst_factor: OneMinusSrcAlpha, operation: Add } }
}

/// Stencil comparison a content pipeline uses for a given test.
fn stencil_compare(stencil: StencilTest) -> wgpu::CompareFunction {
    match stencil {
        StencilTest::None => wgpu::CompareFunction::Always,
        StencilTest::Inside => wgpu::CompareFunction::NotEqual,
        StencilTest::Outside => wgpu::CompareFunction::Equal,
    }
}

/// Stencil bit planes, one per OP family. `Invert` and the
/// increment/decrement pair do not commute, so they must never share bits:
/// mixed they make coverage depend on writer order and leave the cancellation
/// short of zero. A stencil write is `new = (op(old) & write_mask) | (old &
/// ~write_mask)`, so restricting the written bits per OP throws the carry/borrow
/// away and keeps each plane closed. The Test side is unaffected — it reads both
/// planes (`read_mask` 0xFF, `value != 0`), which is exactly the union that
/// masks of differing `mask_influence` compose to.
///
/// The counter must be contiguous from bit 0 (increment/decrement are ordinary
/// arithmetic); parity takes one dedicated high bit. Unlike ForUnity — which
/// fences SS masking into URP's low nibble — this player owns the whole 8-bit
/// stencil, so the counter gets seven bits (overlap depth up to 127) and parity
/// the eighth.
const STENCIL_PLANE_COUNTER: u32 = 0x7F;
const STENCIL_PLANE_PARITY: u32 = 0x80;

/// The stencil write a mask writer performs. Increment/decrement wrap *within the
/// counter plane* (`write_mask` discards the carry past bit 6), so a full
/// increment/decrement cycle returns the plane to zero exactly like the coverage
/// backend's counter accumulator.
fn stencil_write_op(op: MaskOp) -> wgpu::StencilOperation {
    match op {
        MaskOp::Incr => wgpu::StencilOperation::IncrementWrap,
        MaskOp::Decr => wgpu::StencilOperation::DecrementWrap,
        MaskOp::Invert => wgpu::StencilOperation::Invert,
    }
}

/// The bit plane a writer's OP writes into.
fn stencil_write_plane(op: MaskOp) -> u32 {
    match op {
        MaskOp::Incr | MaskOp::Decr => STENCIL_PLANE_COUNTER,
        MaskOp::Invert => STENCIL_PLANE_PARITY,
    }
}

/// Baked stencil face for a compare + pass op (front == back; ref is always 0).
fn stencil_face(compare: wgpu::CompareFunction, pass_op: wgpu::StencilOperation) -> wgpu::StencilFaceState {
    wgpu::StencilFaceState { compare, fail_op: wgpu::StencilOperation::Keep, depth_fail_op: wgpu::StencilOperation::Keep, pass_op }
}

/// Depth/stencil state for a pipeline that only *tests* coverage (content), or
/// only *writes* it (mask writers, `write_mask` = the OP's own plane).
/// `read_mask` is always 0xFF so a test reads both planes as a union.
/// The depth aspect is inert throughout: never written, always passing.
fn depth_stencil_state(format: Option<wgpu::TextureFormat>, compare: wgpu::CompareFunction, pass_op: wgpu::StencilOperation, write_mask: u32) -> Option<wgpu::DepthStencilState> {
    format.map(|format| wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: wgpu::StencilState { front: stencil_face(compare, pass_op), back: stencil_face(compare, pass_op), read_mask: 0xFF, write_mask },
        bias: wgpu::DepthBiasState::default(),
    })
}

/// Everything both WGSL modules share: the scene's SoA arena and
/// primitive table, the texture bindings, and the four vertex stages that expand
/// primitives into vertices. Prefixed onto [`WGSL_SHADER`] and [`SS_SHADER_WGSL`],
/// which add only their fragment stages.
///
/// **There is no vertex buffer.** The runtime's `world_matrices` /
/// `local_vertices` / `local_uvs` / `mesh_*` / shape arrays are copied into
/// `f_arena` verbatim, one memcpy per node per frame, and the vertex stage reads
/// them at the offsets a 80-byte `Prim` record points at. WebGL2 cannot do this —
/// no storage buffers — and it is the single largest reason this player exists.
///
/// The position maths is written out scalar by scalar rather than as matrix
/// products, so it performs the same operations in the same order as the CPU path
/// it replaced. That is what lets the two be compared pixel for pixel.
const COMMON_WGSL: &str = r#"
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) varg: vec4<f32>,
    // Where this fragment lands in the owning node's coverage bitmap. Written by
    // every stage but read only by the coverage replay; the stencil path leaves
    // it at the zero a `var` starts from.
    @location(3) cov_uv: vec2<f32>,
    // The primitive's coverage word and block, carried flat because the fragment
    // stage has no primitive index of its own to look them up with.
    @location(4) @interpolate(flat) mask_info: u32,
    @location(5) @interpolate(flat) cov_base: u32,
    // The owning player's tint, flat for the same reason.
    @location(6) @interpolate(flat) tint: u32,
}

struct Globals {
    proj: mat4x4<f32>,
}

// One drawn primitive. Which fields mean what depends on the vertex stage
// reading it — the bases are element offsets into `f_arena`:
//
// | aux_base | pos_base | uv_base | count
// ---------+---------------+-----------------+------------------+-------
// vs_quad | world matrix | local vertices | local UVs (px) | -
// vs_mesh | mesh Y | mesh X | mesh UVs (px) | -
// vs_shape | world matrix | shape vertices | shape box coords | vertices
// vs_effect| - | particle XY | particle UVs | -
//
// `colors` is packed RGBA8 but only RGB is read: alpha is kept in `alphas` at
// full precision, because a part without PartColor takes its alpha straight from
// the runtime as an f32. `rates` is the per-corner PartsColor rate — also f32,
// and for a mask writer it carries the cutout threshold instead.
struct Prim {
    aux_base: u32,
    pos_base: u32,
    uv_base: u32,
    node_base: u32,
    count: u32,
    mode: u32,
    alpha: f32,
    z: f32,
    colors: array<u32, 4>,
    alphas: array<f32, 4>,
    rates: array<f32, 4>,
    // Coverage masking. `cov_base` is the arena offset of the
    // owning node's coverage block; `mask_info` says whether this primitive is a
    // writer baking a bit or a target replaying them, and is zero — meaning "not
    // coverage-masked" — for every primitive under the stencil backend.
    cov_base: u32,
    mask_info: u32,
    // The owning player's tint, packed like `colors`. White (`0x00FFFFFF`) is the
    // identity — zero is black, so this field is never left to a default.
    tint: u32,
}

// `mask_info` bits. Kept in step with the Rust constants of the same names.
const MASK_INFO_ACTIVE: u32 = 1u;
const MASK_INFO_WRITER: u32 = 2u;
const MASK_INFO_INSIDE: u32 = 4u;

@group(0) @binding(0) var<uniform> globals: Globals;
// The frame's SoA arena: runtime buffers and node transforms, copied verbatim.
@group(0) @binding(1) var<storage, read> f_arena: array<f32>;
@group(0) @binding(2) var<storage, read> prims: array<Prim>;
// (primitive, vertex) pairs for `vs_mesh` — indexed directly by vertex_index.
@group(0) @binding(3) var<storage, read> mesh_stream: array<u32>;

@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var s_diffuse: sampler;

// Per-texture facts that must not become pipeline dimensions:
// `size` = (w, h, 1/w, 1/h) in texels, `flags.x` = 1 when premultiplied.
struct TexInfo {
    size: vec4<f32>,
    flags: vec4<f32>,
}
@group(1) @binding(2) var<uniform> tex_info: TexInfo;

// Straight (un-premultiplied) RGB of a sampled texel. Our own uploads are
// premultiplied and need the divide; a texture borrowed from an engine's asset
// system usually is not, and dividing it would blow out the edges.
fn ss_straight_rgb(tex: vec4<f32>) -> vec3<f32> {
    return select(tex.rgb, tex.rgb / max(tex.a, 0.0001), tex_info.flags.x > 0.5);
}

@group(2) @binding(0) var t_coverage: texture_2d_array<f32>;

// --- coverage masking ---------------------------------------------------

// A part-local point in the space the coverage bitmap is pinned to: through the
// part's own world matrix, but *not* through the player's placement, so the
// bitmap does not slide when the player moves.
//
// Deliberately a copy of the first two lines of `world_point` rather than a
// shared helper it calls: the clip path's arithmetic is compared pixel for pixel
// against a golden run, and factoring it out invites the compiler to contract
// the multiply-adds differently there.
fn local_xy(mat_base: u32, lx: f32, ly: f32) -> vec2<f32> {
    return vec2<f32>(
        f_arena[mat_base] * lx + f_arena[mat_base + 4u] * ly + f_arena[mat_base + 12u],
        f_arena[mat_base + 1u] * lx + f_arena[mat_base + 5u] * ly + f_arena[mat_base + 13u],
    );
}

// Player-local -> coverage UV, through the node's bbox affine. Zero for a
// primitive that is not coverage-masked, which never reads the result.
fn cov_uv_of(pi: u32, p: vec2<f32>) -> vec2<f32> {
    if ((prims[pi].mask_info & MASK_INFO_ACTIVE) == 0u) {
        return vec2<f32>(0.0, 0.0);
    }
    let b = prims[pi].cov_base;
    return vec2<f32>(
        f_arena[b] * p.x + f_arena[b + 2u] * p.y + f_arena[b + 4u],
        f_arena[b + 1u] * p.x + f_arena[b + 3u] * p.y + f_arena[b + 5u],
    );
}

// Coverage UV -> clip space, for the bake stages. The bitmap's V runs top-down
// like a texture, so it flips against clip Y.
fn cov_clip(uv: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.5, 1.0);
}

// The colour one writer contributes: its own bit, in its own channel, and
// nothing else. Alpha is 1 because the bake blends additively and a zero alpha
// would scale the bit away on backends that premultiply first.
fn bit_color(info: u32) -> vec4<f32> {
    // The layer was picked by the draw call; this is the bit within it.
    let bit = ((info >> 8u) & 0xFFu) % COVERAGE_LAYER_BITS;
    let value = f32(1u << (bit % 8u)) / 255.0;
    let channel = bit / 8u;
    var c = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    if (channel == 0u) { c.r = value; }
    else if (channel == 1u) { c.g = value; }
    else { c.b = value; }
    return c;
}

// --- vertex stages --------------------------------------------------------

// Corner order of the two triangles making a quad, matching the index pattern
// the CPU path emitted (and the one the runtime emits for effect particles).
fn quad_corner(j: u32) -> u32 {
    var order = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);
    return order[j];
}

// A node's placement applied to a world-space point. Columns are read
// individually so the sum is accumulated in the same order as the CPU's
// `apply_transform`.
fn apply_node(base: u32, x: f32, y: f32, z: f32) -> vec3<f32> {
    return vec3<f32>(
        f_arena[base] * x + f_arena[base + 4u] * y + f_arena[base + 8u] * z + f_arena[base + 12u],
        f_arena[base + 1u] * x + f_arena[base + 5u] * y + f_arena[base + 9u] * z + f_arena[base + 13u],
        f_arena[base + 2u] * x + f_arena[base + 6u] * y + f_arena[base + 10u] * z + f_arena[base + 14u],
    );
}

// A part-local point through the part's world matrix and then its node's
// placement. Local Z is zero, so the matrix's third column drops out.
fn world_point(mat_base: u32, node_base: u32, lx: f32, ly: f32) -> vec3<f32> {
    let wx = f_arena[mat_base] * lx + f_arena[mat_base + 4u] * ly + f_arena[mat_base + 12u];
    let wy = f_arena[mat_base + 1u] * lx + f_arena[mat_base + 5u] * ly + f_arena[mat_base + 13u];
    let wz = f_arena[mat_base + 2u] * lx + f_arena[mat_base + 6u] * ly + f_arena[mat_base + 14u];
    return apply_node(node_base, wx, wy, wz);
}

// Clip position. Z is written as a constant: the projection's third column is
// identity, so the depth the transform produced never reaches x/y.
fn to_clip(p: vec3<f32>) -> vec4<f32> {
    let pos = globals.proj * vec4<f32>(p, 1.0);
    return vec4<f32>(pos.x, pos.y, 0.5, 1.0);
}

// The runtime's UVs are in texture pixels. A missing texture binds
// the 1x1 white fallback, whose size is 1 — the same no-op the CPU path applied.
fn normalize_uv(base: u32, k: u32) -> vec2<f32> {
    return vec2<f32>(f_arena[base + k * 2u], f_arena[base + k * 2u + 1u]) / tex_info.size.xy;
}

// The player's tint, unpacked. It multiplies the finished composite rather than
// the colour the PartsColor formula blends towards, which is why it reaches the
// fragment stage instead of folding into `corner_rgba`.
fn player_tint(packed: u32) -> vec3<f32> {
    return unpack4x8unorm(packed).rgb;
}

fn corner_rgba(pi: u32, k: u32) -> vec4<f32> {
    return vec4<f32>(unpack4x8unorm(prims[pi].colors[k]).rgb, prims[pi].alphas[k] * prims[pi].alpha);
}

// `varg` = [rate, isMix, isMul, isAddSub]. Only four PartsColor
// formulas exist, so the mode is two bits on the primitive rather than three
// floats on every vertex.
fn varg_of(mode: u32, rate: f32) -> vec4<f32> {
    switch mode {
        case 1u: { return vec4<f32>(rate, 0.0, 1.0, 0.0); }
        case 2u: { return vec4<f32>(rate, 0.0, 0.0, 1.0); }
        case 3u: { return vec4<f32>(rate, 0.0, 0.0, -1.0); }
        default: { return vec4<f32>(rate, 1.0, 0.0, 0.0); }
    }
}

@vertex
fn vs_quad(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let pi = vi / 6u;
    let k = quad_corner(vi % 6u);
    let p = &prims[pi];
    let lx = f_arena[(*p).pos_base + k * 2u];
    let ly = f_arena[(*p).pos_base + k * 2u + 1u];

    var out: VertexOutput;
    out.clip_position = to_clip(world_point((*p).aux_base, (*p).node_base, lx, ly));
    out.uv = normalize_uv((*p).uv_base, k);
    out.color = corner_rgba(pi, k);
    out.varg = varg_of((*p).mode, (*p).rates[k]);
    out.cov_uv = cov_uv_of(pi, local_xy((*p).aux_base, lx, ly));
    out.mask_info = prims[pi].mask_info;
    out.cov_base = prims[pi].cov_base;
    out.tint = prims[pi].tint;
    return out;
}

@vertex
fn vs_mesh(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let pi = mesh_stream[vi * 2u];
    let k = mesh_stream[vi * 2u + 1u];
    let p = &prims[pi];
    // Mesh vertices arrive already in world space (the runtime skins them), so
    // only the node's placement is left to apply.
    let mx = f_arena[(*p).pos_base + k];
    let my = f_arena[(*p).aux_base + k];

    var out: VertexOutput;
    out.clip_position = to_clip(apply_node((*p).node_base, mx, my, (*p).z));
    out.uv = normalize_uv((*p).uv_base, k);
    out.color = corner_rgba(pi, 0u);
    out.varg = varg_of((*p).mode, (*p).rates[0]);
    out.cov_uv = cov_uv_of(pi, vec2<f32>(mx, my));
    out.mask_info = prims[pi].mask_info;
    out.cov_base = prims[pi].cov_base;
    out.tint = prims[pi].tint;
    return out;
}

@vertex
fn vs_shape(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let pi = vi / SHAPE_FAN_VERTS;
    let r = vi % SHAPE_FAN_VERTS;
    let tri = r / 3u;
    let p = &prims[pi];

    // Fan from vertex 0. Triangles past the live vertex count collapse onto
    // vertex 0, so they have no area and rasterise nothing.
    var v = 0u;
    if (tri + 2u < (*p).count) {
        let c = r % 3u;
        if (c == 1u) { v = tri + 1u; }
        if (c == 2u) { v = tri + 2u; }
    }
    let lx = f_arena[(*p).pos_base + v * 2u];
    let ly = f_arena[(*p).pos_base + v * 2u + 1u];

    // Bilinear fill over the part's box coordinates, matching the CPU's
    // `bilerp_color` term for term.
    let fx = f_arena[(*p).uv_base + v * 2u];
    let fy = f_arena[(*p).uv_base + v * 2u + 1u];
    let top = corner_rgba(pi, 0u) * (1.0 - fx) + corner_rgba(pi, 1u) * fx;
    let bot = corner_rgba(pi, 2u) * (1.0 - fx) + corner_rgba(pi, 3u) * fx;

    var out: VertexOutput;
    out.clip_position = to_clip(world_point((*p).aux_base, (*p).node_base, lx, ly));
    // Shapes are textureless (white fallback), so any constant UV samples white.
    out.uv = vec2<f32>(0.5, 0.5);
    out.color = top * (1.0 - fy) + bot * fy;
    out.varg = varg_of((*p).mode, (*p).rates[0]);
    out.cov_uv = cov_uv_of(pi, local_xy((*p).aux_base, lx, ly));
    out.mask_info = prims[pi].mask_info;
    out.cov_base = prims[pi].cov_base;
    out.tint = prims[pi].tint;
    return out;
}

@vertex
fn vs_effect(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let pi = vi / 6u;
    let k = quad_corner(vi % 6u);
    let p = &prims[pi];
    // Particle quads are emitted in world space with vert_stride 2, so Z is zero.
    let px = f_arena[(*p).pos_base + k * 2u];
    let py = f_arena[(*p).pos_base + k * 2u + 1u];

    var out: VertexOutput;
    out.clip_position = to_clip(apply_node((*p).node_base, px, py, 0.0));
    // Effect UVs are already normalised, unlike the sprite/mesh pixel UVs.
    out.uv = vec2<f32>(f_arena[(*p).uv_base + k * 2u], f_arena[(*p).uv_base + k * 2u + 1u]);
    // One colour per particle, and no PartsColor overlay: plain modulate.
    out.color = corner_rgba(pi, 0u);
    out.varg = varg_of((*p).mode, (*p).rates[0]);
    // The particle's own local point: the effect's coverage block already maps
    // the effect part's space onto the player's bitmap.
    out.cov_uv = cov_uv_of(pi, vec2<f32>(px, py));
    out.mask_info = prims[pi].mask_info;
    out.cov_base = prims[pi].cov_base;
    out.tint = (*p).tint;
    return out;
}
"#;

/// The coverage half of the sprite module: the three bake vertex stages, the
/// bit-writing fragment, and the replay every masked part runs.
///
/// Split out only for reading; it is concatenated onto [`COMMON_WGSL`] with
/// [`WGSL_SHADER`] and naga sees one module.
const COVERAGE_WGSL: &str = r#"
// --- bake: rasterise one writer's shape into its bit ----------------------

@vertex
fn vs_cov_quad(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let pi = vi / 6u;
    let k = quad_corner(vi % 6u);
    let p = &prims[pi];
    let lx = f_arena[(*p).pos_base + k * 2u];
    let ly = f_arena[(*p).pos_base + k * 2u + 1u];

    var out: VertexOutput;
    out.cov_uv = cov_uv_of(pi, local_xy((*p).aux_base, lx, ly));
    out.mask_info = prims[pi].mask_info;
    out.cov_base = prims[pi].cov_base;
    out.tint = prims[pi].tint;
    out.clip_position = cov_clip(out.cov_uv);
    out.uv = normalize_uv((*p).uv_base, k);
    out.color = bit_color((*p).mask_info);
    out.varg = vec4<f32>((*p).rates[k], 0.0, 0.0, 0.0);
    return out;
}

@vertex
fn vs_cov_mesh(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let pi = mesh_stream[vi * 2u];
    let k = mesh_stream[vi * 2u + 1u];
    let p = &prims[pi];
    let mx = f_arena[(*p).pos_base + k];
    let my = f_arena[(*p).aux_base + k];

    var out: VertexOutput;
    out.cov_uv = cov_uv_of(pi, vec2<f32>(mx, my));
    out.mask_info = prims[pi].mask_info;
    out.cov_base = prims[pi].cov_base;
    out.tint = prims[pi].tint;
    out.clip_position = cov_clip(out.cov_uv);
    out.uv = normalize_uv((*p).uv_base, k);
    out.color = bit_color((*p).mask_info);
    out.varg = vec4<f32>((*p).rates[0], 0.0, 0.0, 0.0);
    return out;
}

@vertex
fn vs_cov_shape(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let pi = vi / SHAPE_FAN_VERTS;
    let r = vi % SHAPE_FAN_VERTS;
    let tri = r / 3u;
    let p = &prims[pi];

    var v = 0u;
    if (tri + 2u < (*p).count) {
        let c = r % 3u;
        if (c == 1u) { v = tri + 1u; }
        if (c == 2u) { v = tri + 2u; }
    }
    let lx = f_arena[(*p).pos_base + v * 2u];
    let ly = f_arena[(*p).pos_base + v * 2u + 1u];

    var out: VertexOutput;
    out.cov_uv = cov_uv_of(pi, local_xy((*p).aux_base, lx, ly));
    out.mask_info = prims[pi].mask_info;
    out.cov_base = prims[pi].cov_base;
    out.tint = prims[pi].tint;
    out.clip_position = cov_clip(out.cov_uv);
    // Shapes are textureless, so any constant UV samples the white fallback.
    out.uv = vec2<f32>(0.5, 0.5);
    out.color = bit_color((*p).mask_info);
    out.varg = vec4<f32>((*p).rates[0], 0.0, 0.0, 0.0);
    return out;
}

// Writes the writer's bit wherever its cutout says it covers.
// `varg.x` carries the threshold; below zero means "no cutout", which is what a
// textureless pure mask uses to cover its whole shape.
@fragment
fn fs_coverage(in: VertexOutput) -> @location(0) vec4<f32> {
    let a = textureSample(t_diffuse, s_diffuse, in.uv).a;
    let threshold = in.varg.x;
    if (threshold >= 0.0 && a <= threshold) {
        discard;
    }
    return in.color;
}

// --- replay: reconstruct the mask state for one masked fragment -----------

// Re-runs the writers that are in scope for this part and says whether the
// fragment survives. This is the stencil's job done per fragment: the bitmap
// holds every writer of the player's instance tree at once, and the scope test
// picks which of them were "applied" by the time this part drew.
fn mask_passes(in: VertexOutput, info: u32, cov_base: u32) -> bool {
    let layer = i32(f_arena[cov_base + 6u]);
    let count = u32(f_arena[cov_base + 7u]);
    let inside = (info & MASK_INFO_INSIDE) != 0u;
    // No writer covers anything: a part drawn only inside a mask draws nowhere.
    if (count == 0u) {
        return !inside;
    }

    // Outside the bbox there is no writer to be inside of. Answering from the
    // polarity alone keeps the sampler's edge clamp from reading the border
    // texel and masking content that is nowhere near the mask.
    let uv = in.cov_uv;
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return !inside;
    }

    let dims = vec2<f32>(textureDimensions(t_coverage).xy);
    // `textureLoad`, not a sampler: a filtered read would blend one writer's bit
    // into the next and decode as coverage nobody wrote.
    let texel = vec2<i32>(clamp(uv * dims, vec2<f32>(0.0), dims - 1.0));
    // A tree with more writers than one layer holds spills into the layers after
    // it, `COVERAGE_LAYER_BITS` at a time. Records are in bit order, so each
    // layer is loaded once.
    var loaded = 0u;
    var cov = textureLoad(t_coverage, texel, layer, 0);

    // This part's place on the tree sequence — the whole instance tree numbered
    // in drawing order, so a sub-animation's parts sit between its instance part
    // and whatever its caller draws next.
    let this_seq = f32(info >> 8u);
    // Two accumulators, never one: increment and invert are each self-inverse but
    // do not commute, so mixing them makes coverage depend on the order the
    // writers happened to be drawn in.
    var parity = 0u;
    var counter = 0u;
    for (var i = 0u; i < count; i = i + 1u) {
        let rec = cov_base + COVERAGE_HEADER + i * COVERAGE_STRIDE;
        // Scope, open at both ends: a clipping writer holds from itself to the
        // end of the tree, a pure mask from the start of its own animation up to
        // itself. The player worked both out; here it is one comparison.
        if (!(f_arena[rec] < this_seq && this_seq < f_arena[rec + 1u])) {
            continue;
        }
        let tree_bit = u32(f_arena[rec + 2u]);
        let spill = tree_bit / COVERAGE_LAYER_BITS;
        if (spill != loaded) {
            cov = textureLoad(t_coverage, texel, layer + i32(spill), 0);
            loaded = spill;
        }
        let bit = tree_bit % COVERAGE_LAYER_BITS;
        let byte = u32(cov[bit / 8u] * 255.0 + 0.5);
        if (((byte >> (bit % 8u)) & 1u) == 0u) {
            continue;
        }
        if (f_arena[rec + 3u] > 0.5) {
            parity = parity ^ 1u;
        } else {
            counter = counter + 1u;
        }
    }

    let masked = (parity != 0u) || (counter != 0u);
    return masked == inside;
}
"#;

const WGSL_SHADER: &str = r#"
// PartsColor blend (direct port of WebGL PartsColorProgram).
// Recover straight RGB, apply the mix/mul/add-sub blend selected by `varg`, then
// re-premultiply. Blend factors are premul too (see the pipeline blend table),
// keeping the whole path premultiplied.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    if (tex.a == 0.0) {
        return vec4<f32>(0.0);
    }
    let rate = in.varg.x;
    let straight = ss_straight_rgb(tex);
    let mixC = mix(straight, in.color.rgb, rate);
    let mulC = straight * mix(vec3<f32>(1.0), in.color.rgb, rate);
    let addC = straight + in.color.rgb * rate * sign(in.varg.w);
    let rgb = mixC * in.varg.y + mulC * in.varg.z + addC * abs(in.varg.w);
    let a = tex.a * in.color.a;
    return vec4<f32>(rgb * player_tint(in.tint) * a, a);
}

// The same composite as `fs_main`, clipped by the coverage bitmap.
//
// The texture is sampled *before* the clip decides: `textureSample` needs
// implicit derivatives, and WGSL forbids those once control flow has stopped
// being uniform — which a `discard` above it would make it.
@fragment
fn fs_main_coverage(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    let info = in.mask_info;
    if ((info & MASK_INFO_ACTIVE) != 0u && !mask_passes(in, info, in.cov_base)) {
        discard;
    }
    if (tex.a == 0.0) {
        return vec4<f32>(0.0);
    }
    let rate = in.varg.x;
    let straight = ss_straight_rgb(tex);
    let mixC = mix(straight, in.color.rgb, rate);
    let mulC = straight * mix(vec3<f32>(1.0), in.color.rgb, rate);
    let addC = straight + in.color.rgb * rate * sign(in.varg.w);
    let rgb = mixC * in.varg.y + mulC * in.varg.z + addC * abs(in.varg.w);
    let a = tex.a * in.color.a;
    return vec4<f32>(rgb * player_tint(in.tint) * a, a);
}

// Mask-write fragment: color output is discarded (ColorWrites::empty()); only
// the cutout matters. `varg.x` carries the threshold (< 0 = no cutout) — the
// rate slot rather than a colour channel, because it has to survive at full f32
// precision and the colours are packed to bytes.
@fragment
fn fs_mask(in: VertexOutput) -> @location(0) vec4<f32> {
    let a = textureSample(t_diffuse, s_diffuse, in.uv).a;
    let threshold = in.varg.x;
    if (threshold >= 0.0 && a <= threshold) {
        discard;
    }
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}
"#;

/// Bytes per shader-param slot in the dynamic-offset uniform buffer. Dynamic
/// uniform offsets must be a multiple of `min_uniform_buffer_offset_alignment`
/// (256 on all backends we target), so each part's params occupy one 256-byte
/// slot even though only the first 64 bytes are used.
pub(crate) const SHADER_PARAM_STRIDE: usize = 256;
/// f32 count of one slot (`SHADER_PARAM_STRIDE / 4`).
pub(crate) const SHADER_PARAM_STRIDE_F32: usize = SHADER_PARAM_STRIDE / 4;

/// id_hash (SDK `PartAttributeShader.id_hash()`, `fnv1a_hash_str("ss-*")`) ->
/// WGSL fragment entry point. The 13 SpriteStudio integration shaders (,
/// ports of the WebGL `SSShaderLib.ts`). Hashes are the SDK's precomputed values.
const SS_SHADER_ENTRIES: [(u32, &str); 13] = [
    (0x36629dfc, "fs_ss_sepia"),
    (0x8a5c3037, "fs_ss_hsb"),
    (0x1b721788, "fs_ss_step"),
    (0xc488a527, "fs_ss_pix"),
    (0x80d6d514, "fs_ss_noise"),
    (0x64e07450, "fs_ss_bmask"),
    (0x8f8307d4, "fs_ss_outline"),
    (0xb59c8d1b, "fs_ss_blur"),
    (0xe1df9a2b, "fs_ss_wave"),
    (0xe1c1ee07, "fs_ss_move"),
    (0x45e35578, "fs_ss_scatter"),
    (0x9b90e282, "fs_ss_circle"),
    (0x7140896a, "fs_ss_spot"),
];

/// WGSL entry point for an integration-shader `id_hash`, or `None` when it is not
/// a built-in `ss-*` (the caller then draws the part with the plain shader).
pub(crate) fn ss_shader_entry(id_hash: u32) -> Option<&'static str> {
    SS_SHADER_ENTRIES.iter().find(|(h, _)| *h == id_hash).map(|(_, e)| *e)
}

/// The 13 SpriteStudio integration shaders (統合シェーダ), ported from the WebGL
/// `SSShaderLib.ts` (itself the SS6-SDK GLSL). Each entry computes a straight-
/// alpha `Pixel`, then `ss_finish` applies the same PartsColor composite as
/// `fs_main` (varg mix/mul/add-sub) and re-premultiplies — except `ss-blur`,
/// which multiplies by the part colour directly (mirrors the SS6 reference).
///
/// `ssTex` un-premultiplies (textures are premultiplied) and samples at LOD
/// 0 via `textureSampleLevel` so sampling is legal inside loops / after `discard`
/// (WGSL forbids implicit-derivative `textureSample` in non-uniform control flow).
/// Per-part uniforms live in group(2): `param_a`=param0..3, `param_b`=param4..7,
/// `cell_rect`=(minU,minV,maxU,maxV) over the quad's corners, `pixel_size`=1/texel.
const SS_SHADER_WGSL: &str = r#"
struct SsParams {
    param_a: vec4<f32>, // param0..3
    param_b: vec4<f32>, // param4..7
    cell_rect: vec4<f32>, // minU, minV, maxU, maxV
    pixel_size: vec4<f32>, // 1/texW, 1/texH, 0, 0
}
@group(2) @binding(0) var<uniform> ss: SsParams;

const PI: f32 = 3.141592653589793;

fn ssTex(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSampleLevel(t_diffuse, s_diffuse, uv, 0.0);
    // Straight-alpha sample; only premultiplied sources need the divide.
    let rgb = select(c.rgb, c.rgb / max(c.a, 0.0001), tex_info.flags.x > 0.5);
    return vec4<f32>(rgb, c.a);
}

// PartColor composite (straight space) + re-premultiply — same math as fs_main,
// player tint included.
fn ss_finish(pixel: vec4<f32>, color: vec4<f32>, varg: vec4<f32>, tint: u32) -> vec4<f32> {
    let rate = varg.x;
    let p = pixel.rgb;
    let mix_c = mix(p, color.rgb, rate);
    let mul_c = p * mix(vec3<f32>(1.0), color.rgb, rate);
    let add_c = p + color.rgb * rate * sign(varg.w);
    let rgb = mix_c * varg.y + mul_c * varg.z + add_c * abs(varg.w);
    let a = pixel.a * color.a;
    return vec4<f32>(rgb * player_tint(tint) * a, a);
}

fn ss_rand(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn ss_rgb_to_hsb(color: vec4<f32>) -> vec4<f32> {
    let k = vec4<f32>(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = mix(vec4<f32>(color.bg, k.wz), vec4<f32>(color.gb, k.xy), step(color.b, color.g));
    let q = mix(vec4<f32>(p.xyw, color.r), vec4<f32>(color.r, p.yzx), step(p.x, color.r));
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec4<f32>(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x, color.a);
}
fn ss_hsb_to_rgb(hsb: vec4<f32>) -> vec4<f32> {
    let k = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(hsb.xxx + k.xyz) * 6.0 - k.www);
    return vec4<f32>(hsb.z * mix(k.xxx, clamp(p - k.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), hsb.y), hsb.w);
}

@fragment
fn fs_ss_sepia(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_power = ss.param_a.x;
    let pixel = ssTex(in.uv);
    let gray = dot(pixel.rgb, vec3<f32>(0.298912, 0.586611, 0.114478));
    let gray_rgb = vec3<f32>(gray);
    let sepia_rgb = gray_rgb * vec3<f32>(1.07, 0.74, 0.43);
    let toned = mix(sepia_rgb, gray_rgb, step(f_power, 0.0));
    let mixed_rgb = mix(pixel.rgb, toned, abs(f_power));
    return ss_finish(vec4<f32>(mixed_rgb, pixel.a), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_hsb(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_hue = ss.param_a.x;
    let f_sat = ss.param_a.y;
    let f_bri = ss.param_a.z;
    let pixel = ssTex(in.uv);
    var hsb = ss_rgb_to_hsb(pixel);
    hsb.x = hsb.x + f_hue;
    hsb.y = clamp(hsb.y + f_sat, 0.0, 1.0);
    hsb.z = clamp(hsb.z + f_bri, 0.0, 1.0);
    if (hsb.x < 0.0) { hsb.x = hsb.x + 1.0; }
    if (hsb.x > 1.0) { hsb.x = hsb.x - 1.0; }
    let shifted = ss_hsb_to_rgb(hsb);
    return ss_finish(vec4<f32>(shifted.rgb, shifted.a), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_step(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_threshold = ss.param_a.x;
    let f_stage = ss.param_a.y;
    let f_color = ss.param_a.z;
    let pixel = ssTex(in.uv);
    let d = 1.0 + 255.0 * abs(f_stage);
    let e = 1.0e-10;
    let t = abs(f_threshold);
    let b = abs(step(f_threshold, 0.0) - max(pixel.r, max(pixel.g, pixel.b)));
    let r = clamp(floor((b - t) / (t + e) * 256.0 / d) * d / 256.0, 0.0, 1.0);
    let c = step(t, b) * r;
    let v = mix(vec3<f32>(c), pixel.rgb * c, f_color);
    return ss_finish(vec4<f32>(v, pixel.a), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_pix(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_power = ss.param_a.x;
    let px = ss.pixel_size.xy;
    let v = 1.0 + 96.0 * f_power;
    let step_uv = vec2<f32>(v * px.x, v * px.y);
    let coord = floor(in.uv / step_uv) * step_uv;
    return ss_finish(ssTex(coord), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_noise(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_power = ss.param_a.x;
    let f_color = ss.param_a.y;
    let f_phase = ss.param_a.z;
    let px = ss.pixel_size.xy;
    let u = floor(in.uv / px) * px;
    let pixel = ssTex(in.uv);
    let t = u + vec2<f32>(f_phase);
    var c = vec3<f32>(0.0);
    c.r = ss_rand(t);
    c.g = mix(c.r, ss_rand(t * c.r), f_color);
    c.b = mix(c.g, ss_rand(t * c.g), f_color);
    let rgb = clamp(mix(pixel.rgb, c, f_power), vec3<f32>(0.0), vec3<f32>(1.0));
    return ss_finish(vec4<f32>(rgb, pixel.a), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_bmask(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_brightness = ss.param_a.x;
    let pixel = ssTex(in.uv);
    let b = abs(step(f_brightness, 0.0) - max(pixel.r, max(pixel.g, pixel.b)));
    if (b <= abs(f_brightness)) { discard; }
    return ss_finish(pixel, in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_outline(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_ratio = abs(ss.param_a.x);
    let e = 1.0e-5;
    let px = ss.pixel_size.xy;
    var lo = 0.0;
    lo = lo + step(ssTex(in.uv + vec2<f32>(-px.x, 0.0)).a, f_ratio);
    lo = lo + step(ssTex(in.uv + vec2<f32>(px.x, 0.0)).a, f_ratio);
    lo = lo + step(ssTex(in.uv + vec2<f32>(0.0, -px.y)).a, f_ratio);
    lo = lo + step(ssTex(in.uv + vec2<f32>(0.0, px.y)).a, f_ratio);
    let center = ssTex(in.uv);
    let hi = step(f_ratio + e, center.a);
    let v = min(hi * lo, 1.0);
    if (v <= 0.0) { discard; }
    return ss_finish(vec4<f32>(vec3<f32>(v), v), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_blur(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_focus = ss.param_a.x;
    let px = ss.pixel_size.xy;
    let d = vec2<f32>(f_focus * px.x * 16.0, f_focus * px.y * 16.0);
    let tc = in.uv;
    var sum = ssTex(tc);
    sum = sum + ssTex(vec2<f32>(tc.x + d.x, tc.y + d.y));
    sum = sum + ssTex(vec2<f32>(tc.x + d.x, tc.y));
    sum = sum + ssTex(vec2<f32>(tc.x, tc.y + d.y));
    sum = sum + ssTex(vec2<f32>(tc.x - d.x, tc.y - d.y));
    sum = sum + ssTex(vec2<f32>(tc.x + d.x, tc.y - d.y));
    sum = sum + ssTex(vec2<f32>(tc.x - d.x, tc.y + d.y));
    sum = sum + ssTex(vec2<f32>(tc.x - d.x, tc.y));
    sum = sum + ssTex(vec2<f32>(tc.x, tc.y - d.y));
    let pixel = sum / 9.0;
    // ss-blur skips the PartColor composite: straight multiply by part colour.
    let c = pixel * in.color;
    return vec4<f32>(c.rgb * player_tint(in.tint) * c.a, c.a);
}

@fragment
fn fs_ss_wave(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_width = ss.param_a.x;
    let f_height = ss.param_a.y;
    let f_phase = ss.param_a.z;
    let coord = in.uv;
    let px = ss.pixel_size.xy;
    let l = 384.0 * f_height * 0.5;
    let s = l * sin(coord.y * 384.0 * f_width * 0.5 + PI * 2.0 * f_phase) * px.x;
    return ss_finish(ssTex(vec2<f32>(coord.x + s, coord.y)), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_move(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_distance = ss.param_a.x;
    let f_direction = ss.param_a.y;
    let f_power = ss.param_a.z;
    var coord = in.uv;
    let px = ss.pixel_size.xy;
    var i_count = i32(floor(abs(f_distance) * 96.0));
    if (i_count > 128) { i_count = 128; }
    let vel = vec2<f32>(sin(f_direction * PI) * px.x, cos(f_direction * PI) * px.y) * sign(f_distance);
    coord = coord + vel * f32(i_count);
    coord = coord - vel;
    var pixel = ssTex(coord);
    for (var i: i32 = 1; i < 128; i = i + 1) {
        if (i >= i_count) { break; }
        coord = coord - vel;
        pixel = mix(ssTex(coord), pixel, 0.96 * abs(f_power));
    }
    return ss_finish(pixel, in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_scatter(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_power = ss.param_a.x;
    let f_ratio = ss.param_a.y;
    let f_phase = ss.param_a.z;
    let coord = in.uv;
    let px = ss.pixel_size.xy;
    let u = floor(coord / px) * px;
    let t = u + vec2<f32>(f_phase);
    let r = ss_rand(t) * PI * 2.0 - PI;
    let r2 = step(ss_rand(t * r), f_ratio);
    let v = vec2<f32>(sin(r), cos(r)) * px * 96.0 * f_power * r2;
    return ss_finish(ssTex(coord + v), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_circle(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_phase = ss.param_a.x;
    let f_direction = ss.param_a.y;
    let e = 1.0e-10;
    let px = ss.pixel_size.xy;
    let uv1 = vec2<f32>(px.x + e, px.y + e);
    let lt_uv = ss.cell_rect.xy;
    let rb_uv = ss.cell_rect.zw;
    let center_uv = (lt_uv + rb_uv) * 0.5;
    let tx = in.uv / uv1;
    let c = center_uv / uv1;
    let lt = lt_uv / uv1;
    let rb = rb_uv / uv1;
    let dd = rb - lt;
    let v = tx - c;
    let r = min(abs(rb.x - c.x), abs(rb.y - c.y)) + e;
    let dis = length(v);
    if (dis > r) { discard; }
    let nv = select(vec2<f32>(1.0, 0.0), v / dis, dis > e);
    let uu = 1.0 - dis / r;
    var vv = (atan2(nv.y, nv.x) / PI + 1.0) * 0.5 + f_phase;
    if (vv < 0.0) { vv = vv + 1.0; }
    if (vv > 1.0) { vv = vv - 1.0; }
    if (f_direction <= 0.0) { vv = 1.0 - vv; }
    let st = dd * vec2<f32>(uu, vv) * uv1;
    return ss_finish(ssTex(lt_uv + st), in.color, in.varg, in.tint);
}

@fragment
fn fs_ss_spot(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_power = ss.param_a.x;
    let f_decay = ss.param_a.y;
    let f_color = ss.param_a.z;
    let e = 1.0e-10;
    let p = abs(f_power);
    let d_abs = abs(f_decay);
    let px = ss.pixel_size.xy;
    let uv1 = vec2<f32>(px.x + e, px.y + e);
    let lt_uv = ss.cell_rect.xy;
    let rb_uv = ss.cell_rect.zw;
    let center_uv = (lt_uv + rb_uv) * 0.5;
    let c = center_uv / uv1;
    let rb = rb_uv / uv1;
    let l = min(abs(rb.x - c.x), abs(rb.y - c.y)) + e;
    let t = in.uv / uv1;
    let v = c - t;
    var f_r = max(p / (length(v / l) + e) - p, 0.0);
    let f_l = max(p - length(v / l), 0.0);
    f_r = clamp(mix(f_r, f_l, d_abs), 0.0, 1.0);
    let sampled = ssTex(in.uv).rgb;
    let pixel = vec4<f32>(mix(vec3<f32>(1.0), sampled, f_color), f_r);
    return ss_finish(pixel, in.color, in.varg, in.tint);
}
"#;

impl SSRenderer {
    /// Build the device-scoped resources. Cheap: no pipeline is compiled until a
    /// frame asks for one, so this is two shader modules, three bind
    /// group layouts, a sampler and a 1x1 texture.
    ///
    /// Takes no target formats — those ride in the pipeline key, which is what
    /// lets one renderer serve every target on the device.
    pub fn new(device: &wgpu::Device) -> Self {
        // WGSL source for a module is the shared vertex half plus its own
        // fragment half; naga sees one string either way.
        let sprite_wgsl = format!("{}{COMMON_WGSL}{COVERAGE_WGSL}{WGSL_SHADER}", wgsl_consts());
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("SpriteStudio Shader"), source: wgpu::ShaderSource::Wgsl(sprite_wgsl.into()) });

        // group(0) is the scene: the projection, plus the frame's SoA
        // arena, primitive table and mesh stream — everything the vertex stages
        // read instead of a vertex buffer. Only the layout is shared; the
        // buffer behind it belongs to the render target. The three share one
        // buffer, the arena first, and the other two start wherever the frame's
        // arena ended: hence their dynamic offsets.
        let storage = |binding: u32, has_dynamic_offset: bool| wgpu::BindGroupLayoutEntry {
            binding,
            // The fragment stage reads the arena too: the coverage replay walks
            // the node's writer records there, per fragment.
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset, min_binding_size: None },
            count: None,
        };
        let scene_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SpriteStudio Scene Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                storage(1, false),
                storage(2, true),
                storage(3, true),
            ],
        });

        // group(1): atlas texture + sampler + the per-texture facts.
        // The uniform is visible to the vertex stage too, so UV normalisation can
        // move there without reshaping the layout.
        let texture_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SpriteStudio Texture Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<TexInfo>() as u64) },
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("SpriteStudio Pipeline Layout"), bind_group_layouts: &[Some(&scene_bind_group_layout), Some(&texture_bind_group_layout)], immediate_size: 0 });

        // Coverage masking adds a third group holding the frame's bitmap array.
        // No sampler: the replay reads exact texels with `textureLoad`, because a
        // filtered read would blend neighbouring writers' bits into each other and
        // decode as coverage that nobody wrote.
        let coverage_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SpriteStudio Coverage Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                count: None,
            }],
        });
        let coverage_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SpriteStudio Coverage Pipeline Layout"),
            bind_group_layouts: &[Some(&scene_bind_group_layout), Some(&texture_bind_group_layout), Some(&coverage_bind_group_layout)],
            immediate_size: 0,
        });

        // Integration-shader (`ss-*`) resources. The module is compiled
        // now; the pipelines are built lazily per key on first use.
        let ss_wgsl = format!("{}{COMMON_WGSL}{SS_SHADER_WGSL}", wgsl_consts());
        let ss_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("SpriteStudio SS Shaders"), source: wgpu::ShaderSource::Wgsl(ss_wgsl.into()) });
        let ss_params_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SpriteStudio SS Params Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(64) },
                count: None,
            }],
        });
        let ss_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SpriteStudio SS Pipeline Layout"),
            bind_group_layouts: &[Some(&scene_bind_group_layout), Some(&texture_bind_group_layout), Some(&ss_params_bind_group_layout)],
            immediate_size: 0,
        });

        // 1x1 white fallback texture: used when a draw's texture hash
        // is missing and by textureless mask writers.
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SpriteStudio White 1x1"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let white_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Opaque white reads the same either way, so the mode is arbitrary; the
        // 1x1 size matters only to keep the reciprocals finite.
        let white_info = tex_info_buffer(device, TexInfo::new((1.0, 1.0), AlphaMode::Premultiplied));
        let white_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SpriteStudio White Bind Group"),
            layout: &texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&white_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: white_info.as_entire_binding() },
            ],
        });

        Self {
            pipelines: HashMap::new(),
            shader_module,
            pipeline_layout,
            ss_shader_module,
            ss_pipeline_layout,
            coverage_pipeline_layout,
            coverage_bind_group_layout,
            scene_bind_group_layout,
            texture_bind_group_layout,
            ss_params_bind_group_layout,
            sampler,
            white_texture,
            white_bind_group,
            white_ready: false,
            textures: HashMap::new(),
            texture_sources: HashMap::new(),
            texture_digests: HashMap::new(),
            texture_uploads: 0,
        }
    }

    /// How many pipelines have been compiled so far — that is, how many distinct
    /// combinations the animation has actually asked for.
    pub fn pipeline_count(&self) -> usize {
        self.pipelines.len()
    }

    /// Compile `key`'s pipeline if this is the first frame that needs it. A
    /// [`PipelineKey::Shader`] whose id is not a built-in `ss-*` shader stays
    /// absent; [`Self::draw`] skips draws whose pipeline is missing.
    pub(crate) fn ensure_pipeline(&mut self, device: &wgpu::Device, key: PipelineKey) {
        if self.pipelines.contains_key(&key) {
            return;
        }
        if let Some(pipeline) = self.build_pipeline(device, key) {
            self.pipelines.insert(key, pipeline);
        }
    }
    /// Build one pipeline. Everything that varies between them — module, fragment
    /// entry, blend, colour write mask, depth/stencil state — is chosen here; the
    /// vertex layout and primitive state are shared by all of them.
    fn build_pipeline(&self, device: &wgpu::Device, key: PipelineKey) -> Option<wgpu::RenderPipeline> {
        struct Recipe<'a> {
            label: String,
            module: &'a wgpu::ShaderModule,
            layout: &'a wgpu::PipelineLayout,
            /// Vertex entry: which geometry class expands this draw.
            vertex_entry: &'static str,
            entry: &'a str,
            blend: Option<wgpu::BlendState>,
            write_mask: wgpu::ColorWrites,
            depth_stencil: Option<wgpu::DepthStencilState>,
            /// Usually the target's own format; the coverage bitmap is its own
            /// target and so brings its own.
            color_format: wgpu::TextureFormat,
        }
        let blend_of = |idx: usize| BLEND_ORDER[idx.min(BLEND_ORDER.len() - 1)];

        let recipe = match key.kind {
            PipelineKind::Content(blend_idx, stencil, class) => {
                let blend = blend_of(blend_idx);
                Recipe {
                    label: format!("Content {} {stencil:?} {class:?}", blend.variant_name().unwrap_or("Blend")),
                    module: &self.shader_module,
                    layout: &self.pipeline_layout,
                    vertex_entry: class.entry(),
                    entry: "fs_main",
                    blend: Some(blend_state(blend)),
                    write_mask: wgpu::ColorWrites::ALL,
                    // Content tests coverage; it never writes it.
                    depth_stencil: depth_stencil_state(key.stencil_format, stencil_compare(stencil), wgpu::StencilOperation::Keep, 0x00),
                    color_format: key.color_format,
                }
            }
            PipelineKind::Mask(op, class) => Recipe {
                label: format!("Mask Write {op:?} {class:?}"),
                module: &self.shader_module,
                layout: &self.pipeline_layout,
                vertex_entry: class.entry(),
                entry: "fs_mask",
                blend: None,
                // Coverage only — the fragment's colour is thrown away.
                write_mask: wgpu::ColorWrites::empty(),
                depth_stencil: depth_stencil_state(key.stencil_format, wgpu::CompareFunction::Always, stencil_write_op(op), stencil_write_plane(op)),
                color_format: key.color_format,
            },
            PipelineKind::ContentCoverage(blend_idx, stencil, class) => {
                let blend = blend_of(blend_idx);
                Recipe {
                    label: format!("Content Coverage {} {stencil:?} {class:?}", blend.variant_name().unwrap_or("Blend")),
                    module: &self.shader_module,
                    layout: &self.coverage_pipeline_layout,
                    vertex_entry: class.entry(),
                    entry: "fs_main_coverage",
                    blend: Some(blend_state(blend)),
                    write_mask: wgpu::ColorWrites::ALL,
                    // The clip happens in the fragment stage, so the stencil is
                    // inert even where the host happens to have attached one.
                    depth_stencil: depth_stencil_state(key.stencil_format, wgpu::CompareFunction::Always, wgpu::StencilOperation::Keep, 0x00),
                    color_format: key.color_format,
                }
            }
            PipelineKind::Coverage(class) => Recipe {
                label: format!("Coverage Write {class:?}"),
                module: &self.shader_module,
                layout: &self.pipeline_layout,
                vertex_entry: class.coverage_entry(),
                entry: "fs_coverage",
                // Additive, so writers whose bits are disjoint OR together into
                // one bitmap however they overlap.
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
                    alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
                }),
                write_mask: wgpu::ColorWrites::ALL,
                // The bitmap has no depth-stencil of its own; the whole point of
                // this backend is not needing one.
                depth_stencil: None,
                color_format: COVERAGE_FORMAT,
            },
            PipelineKind::Shader(id_hash, blend_idx) => {
                let entry = ss_shader_entry(id_hash)?;
                Recipe {
                    label: format!("SS Shader {entry}"),
                    module: &self.ss_shader_module,
                    layout: &self.ss_pipeline_layout,
                    // SHDR is a Normal-part attribute, so shader draws are quads.
                    vertex_entry: GeomClass::Quad.entry(),
                    entry,
                    blend: Some(blend_state(blend_of(blend_idx))),
                    write_mask: wgpu::ColorWrites::ALL,
                    // Shader parts are never masked in v1, so the stencil is inert.
                    depth_stencil: depth_stencil_state(key.stencil_format, wgpu::CompareFunction::Always, wgpu::StencilOperation::Keep, 0x00),
                    color_format: key.color_format,
                }
            }
        };

        Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&recipe.label),
            layout: Some(recipe.layout),
            // No vertex buffers: every attribute is read from the scene's storage
            // buffers by `vertex_index` arithmetic.
            vertex: wgpu::VertexState { module: recipe.module, entry_point: Some(recipe.vertex_entry), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: recipe.module,
                entry_point: Some(recipe.entry),
                targets: &[Some(wgpu::ColorTargetState { format: recipe.color_format, blend: recipe.blend, write_mask: recipe.write_mask })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: recipe.depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        }))
    }
    /// A bind group over one frame's coverage array.
    ///
    /// The layout is the renderer's (device-scoped, like every other layout); the
    /// array itself belongs to the scene that allocated it, which is why this
    /// builds a group rather than storing one.
    pub(crate) fn coverage_bind_group(&self, device: &wgpu::Device, array_view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SpriteStudio Coverage Bind Group"),
            layout: &self.coverage_bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(array_view) }],
        })
    }

    /// Register a texture the **supplier** owns: the renderer builds a
    /// bind group around `view` and nothing else.
    ///
    /// This is the channel an engine integration uses. Bevy hands over the
    /// `GpuImage`'s view for its `Handle<Image>`, keeps the lifecycle it already
    /// has (hot reload, unload, format and mip policy), and no atlas is uploaded
    /// twice. `alpha` says which form the pixels are in — engine textures are
    /// normally [`AlphaMode::Straight`], ours are premultiplied — and costs no
    /// pipeline, because it rides in the texture's own bind group.
    ///
    /// wgpu's handles are refcounted internally, so the clone kept here cannot
    /// dangle; it does hold the texture alive, so a supplier that has finished
    /// with a hash calls [`SSRenderer::forget_texture`].
    ///
    /// Device loss is the supplier's to repair on this path: the
    /// renderer keeps no pixels it could rebuild from, so after
    /// [`SSRenderer::restore_device`] the supplier re-creates its texture and
    /// calls this again. Taking over a hash that was previously uploaded drops the
    /// retained copy, so the two paths cannot both claim to own one entry.
    pub fn set_texture(&mut self, device: &wgpu::Device, hash: u32, view: &wgpu::TextureView, (width, height): (u32, u32), alpha: AlphaMode) {
        self.texture_sources.remove(&hash);
        let size = (width as f32, height as f32);
        let info = tex_info_buffer(device, TexInfo::new(size, alpha));
        let bind_group = self.create_texture_bind_group(device, view, &info);
        self.texture_digests.remove(&hash);
        self.textures.insert(hash, TextureEntry { bind_group, size, _info_buffer: info, alpha });
    }

    /// Drop a texture registration (and any retained pixels for it).
    ///
    /// The counterpart of [`SSRenderer::set_texture`]: until this is called the
    /// renderer's clone of the view keeps the supplier's texture alive, which for
    /// an engine means an unload that does not actually free anything. Draws that
    /// still reference `hash` fall back to the 1x1 white texture.
    pub fn forget_texture(&mut self, hash: u32) {
        self.textures.remove(&hash);
        self.texture_sources.remove(&hash);
        self.texture_digests.remove(&hash);
    }

    /// Which form [`SSRenderer::set_texture`] / [`SSRenderer::upload_texture`]
    /// registered `hash` in, or `None` if it is not registered.
    pub fn texture_alpha_mode(&self, hash: u32) -> Option<AlphaMode> {
        self.textures.get(&hash).map(|t| t.alpha)
    }

    /// Provide a texture the **renderer** creates. `hash` is the fnv1a
    /// hash of the cellmap image path. This is the convenience path used by the
    /// viewer, the web host and the C API; an engine integration wants
    /// [`SSRenderer::set_texture`] instead.
    ///
    /// `rgba` is straight-alpha (as decoded from PNG); it is premultiplied here so
    /// the whole pipeline stays premultiplied (matching WebGL's
    /// `UNPACK_PREMULTIPLY_ALPHA_WEBGL`). The premul shader recovers straight RGB
    /// before applying PartsColor.
    ///
    /// This copies `rgba` so it can premultiply without touching the caller's
    /// buffer. A caller that can give the buffer up — anything that just decoded it
    /// — wants [`SSRenderer::upload_texture_owned`] instead, which premultiplies in
    /// place and copies nothing.
    pub fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hash: u32, rgba: &[u8], (width, height): (u32, u32), retention: Retention) {
        self.upload_texture_owned(device, queue, hash, rgba.to_vec(), (width, height), retention);
    }

    /// [`SSRenderer::upload_texture`], taking the pixels rather than borrowing them.
    ///
    /// The whole point is that no full-size copy happens: `rgba` is premultiplied in
    /// place, written to the texture, and — for [`Retention::Retain`] — retained as
    /// itself. A 4096² atlas is 64 MB, so the two copies the borrowing form makes
    /// are worth a second entry point.
    pub fn upload_texture_owned(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hash: u32, mut rgba: Vec<u8>, (width, height): (u32, u32), retention: Retention) {
        premultiply(&mut rgba);
        // No digest: a host owns what it uploads, and a load walk that meets
        // this key leaves it alone rather than claiming to know better.
        self.register_premultiplied(device, queue, hash, rgba, (width, height), Provenance { retention, digest: None });
    }

    /// [`SSRenderer::upload_texture_owned`] for pixels that are premultiplied
    /// already — what the loading path hands over, having premultiplied off the
    /// host's thread while it was decoding.
    pub(crate) fn register_premultiplied(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hash: u32, premul: Vec<u8>, (width, height): (u32, u32), how: Provenance) {
        self.write_and_register(device, queue, hash, &premul, (width, height));
        match how.digest {
            Some(digest) => self.texture_digests.insert(hash, digest),
            None => self.texture_digests.remove(&hash),
        };
        if how.retention == Retention::Retain {
            self.texture_sources.insert(hash, (premul, width, height));
        }
    }

    /// What the bytes behind a player-loaded atlas hashed to, or `None` when this
    /// key holds nothing or holds something a host registered.
    ///
    /// The load walk asks so it can tell "somebody already uploaded exactly this"
    /// from "this key holds a different picture now" — see `texture_digests`.
    pub fn texture_digest(&self, hash: u32) -> Option<u32> {
        self.texture_digests.get(&hash).copied()
    }

    /// Create the texture, write `premul` into it and register the bind group.
    ///
    /// Everything that varies between the upload paths (premultiplying, retaining)
    /// has happened by the time this runs, so this is the only part that needs the
    /// device and the queue — and the only part a device-loss restore has to repeat.
    fn write_and_register(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hash: u32, premul: &[u8], (width, height): (u32, u32)) {
        self.texture_uploads += 1;
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SpriteStudio Texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            premul,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * width), rows_per_image: Some(height) },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let size = (width as f32, height as f32);
        let info = tex_info_buffer(device, TexInfo::new(size, AlphaMode::Premultiplied));
        let bind_group = self.create_texture_bind_group(device, &view, &info);
        self.textures.insert(hash, TextureEntry { bind_group, size, _info_buffer: info, alpha: AlphaMode::Premultiplied });
    }
    fn create_texture_bind_group(&self, device: &wgpu::Device, view: &wgpu::TextureView, info: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SpriteStudio Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: info.as_entire_binding() },
            ],
        })
    }
    /// Pixel size of a registered atlas, for normalising the runtime's UVs (which
    /// arrive in texture pixel space).
    pub fn texture_size(&self, hash: u32) -> Option<(f32, f32)> {
        self.textures.get(&hash).map(|t| t.size)
    }

    /// How many distinct texture hashes are registered.
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// How many textures [`SSRenderer::upload_texture`] has created since this
    /// renderer was built (device restores included).
    ///
    /// A diagnostic, and the only way to tell "N players of one pack uploaded the
    /// atlas once between them" from "they each uploaded it over the last one" —
    /// [`SSRenderer::texture_count`] reads the same either way.
    pub fn texture_upload_count(&self) -> usize {
        self.texture_uploads
    }

    /// The bind group for `hash`, or the 1x1 white fallback when it is missing
    /// — Shape content and textureless mask writers rely on this.
    pub(crate) fn texture_bind_group(&self, hash: u32) -> &wgpu::BindGroup {
        self.textures.get(&hash).map(|e| &e.bind_group).unwrap_or(&self.white_bind_group)
    }

    /// Fill the white fallback. Deferred to the first frame because the
    /// constructor has no `Queue`; a `create_texture` alone leaves it
    /// transparent black, which would collapse `tex * colour` to zero.
    pub(crate) fn ensure_white(&mut self, queue: &wgpu::Queue) {
        if self.white_ready {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.white_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        self.white_ready = true;
    }

    /// Drop the retained copies of [`Retention::Retain`] uploads.
    /// Saves the duplicated pixels when the host keeps its own and would rather
    /// re-supply them itself; after this, [`SSRenderer::restore_device`] restores
    /// nothing and every texture must be uploaded again before drawing.
    pub fn forget_texture_sources(&mut self) {
        self.texture_sources.clear();
    }

    /// Rebuild every GPU resource against a freshly created `device` after the
    /// previous one was lost.
    ///
    /// Because this is device-scoped, **one call covers every scene and player on
    /// the device**.
    ///
    /// Textures uploaded with [`Retention::Retain`] come back from their retained
    /// pixels. [`Retention::Reproducible`] ones do not: their supplier re-registers
    /// them (for embedded packs that is `SSPlayer::restore_textures`, which
    /// re-decodes from the `.ssab` it already owns). Anything holding a
    /// target-scoped resource — the projection buffer — restores itself.
    pub fn restore_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        // Lift the sources out *before* replacing self — everything else in here
        // belonged to the dead device, but these are plain CPU pixels.
        let sources = std::mem::take(&mut self.texture_sources);
        let uploads = self.texture_uploads;
        *self = Self::new(device);
        self.texture_uploads = uploads;
        for (hash, (premul, w, h)) in &sources {
            // Straight back out to the new device: these were retained *after*
            // premultiplying, so putting them through it again would darken every
            // partially transparent texel a second time.
            self.write_and_register(device, queue, *hash, premul, (*w, *h));
        }
        self.texture_sources = sources;
    }
}

/// Premultiply straight-alpha RGBA in place (rgb *= a/255, rounded).
///
/// The rounding is `(c * a + 127) / 255` and not something cheaper: it is what the
/// WebGL player's `UNPACK_PREMULTIPLY_ALPHA_WEBGL` produces, and the fixtures that
/// compare an uploaded atlas against a host-premultiplied one are byte-for-byte.
pub(crate) fn premultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
        px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
        px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
    }
}

/// A write-once uniform holding one texture's [`TexInfo`].
///
/// Mapped at creation rather than written through a `Queue`, because
/// [`SSRenderer::new`] builds the white fallback's copy and the borrow model does
/// not lend it a queue that early.
fn tex_info_buffer(device: &wgpu::Device, info: TexInfo) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("SpriteStudio Texture Info"), size: std::mem::size_of::<TexInfo>() as u64, usage: wgpu::BufferUsages::UNIFORM, mapped_at_creation: true });
    buffer.slice(..).get_mapped_range_mut().expect("a buffer created mapped, viewed once").copy_from_slice(bytemuck::bytes_of(&info));
    buffer.unmap();
    buffer
}
