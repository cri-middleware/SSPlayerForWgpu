//! Layer 2 (rendering / "body"): SSScene.
//!
//! Turns the runtime's FrameData / DrawBatch / SoA buffers into wgpu vertices,
//! indices and draw commands. Borrow model: it never owns the `Device`, `Queue`
//! or the render target — the host (SSPlayer / viewer / Bevy) owns those and
//! lends them by reference.

use crate::frame::FrameView;
use crate::mask::{MaskOp, MaskWriter, StencilTest};
use std::collections::HashMap;

use ssruntime::runtime::effect_draw_plan::ss::runtime::root_as_effect_draw_plan_unchecked;
use ssruntime::runtime::framedata::ss::runtime::{BlendType, DrawBatchKind, PartAttributeShader, root_as_frame_data_unchecked};

/// Per-vertex data uploaded to the GPU: world position, normalized UV, RGBA, and
/// the PartsColor blend control `varg` (`[rate, isMix, isMul, isAddSub]`, SPEC
/// §5.1 / §5.3 — the WebGL `aVarg`).
///
/// For mask-write draws the `color` channel is repurposed: `color[0]` carries
/// the cutout threshold consumed by `fs_mask` (SPEC §6); `varg` is ignored there.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub varg: [f32; 4],
}

/// `varg` for the no-PartColor / effect path: rate 1, mul (a plain
/// `texture * color` modulate; SPEC §5.3, matches WebGL's effect draw).
const VARG_MODULATE: [f32; 4] = [1.0, 0.0, 1.0, 0.0];
/// `varg` for mask-write geometry — consumed by `fs_mask`, which ignores it.
const VARG_NONE: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

impl Vertex {
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { offset: 0, shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32x2 },
                wgpu::VertexAttribute { offset: 20, shader_location: 2, format: wgpu::VertexFormat::Float32x4 },
                wgpu::VertexAttribute { offset: 36, shader_location: 3, format: wgpu::VertexFormat::Float32x4 },
            ],
        }
    }
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

/// What a pipeline does, before the render target is taken into account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PipelineKind {
    /// Ordinary content: blend index (into [`BLEND_ORDER`]) x stencil test.
    Content(usize, StencilTest),
    /// Mask-write geometry: the stencil op it applies. Colour output is masked
    /// off, so blend does not vary.
    Mask(MaskOp),
    /// Integration shader (`ss-*`, SPEC §6): shader id x blend index. Never
    /// stencil-tested — shader parts are not masked in v1.
    Shader(u32, usize),
}

/// What distinguishes one render pipeline from another (SPEC §5.2).
///
/// wgpu bakes blend, depth/stencil state **and the target's formats** into the
/// pipeline, so every combination a frame uses needs its own object. They are
/// built on first use and cached under this key rather than compiled up front:
/// an animation touching one blend mode pays for one pipeline instead of the
/// whole 12 x 3 content matrix.
///
/// The formats are part of the key rather than fixed at construction, which is
/// what lets a single device-scoped [`SSRenderer`] serve render targets that do
/// not agree on them (SPEC §9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PipelineKey {
    kind: PipelineKind,
    color_format: wgpu::TextureFormat,
    stencil_format: Option<wgpu::TextureFormat>,
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

struct TextureEntry {
    bind_group: wgpu::BindGroup,
    size: (f32, f32),
}

/// Layer 2a (SPEC §1.2 / §9.1): everything whose useful lifetime is the
/// **device**, not one player and not one render target.
///
/// Pipelines, bind group layouts, the sampler, the white fallback and the atlas
/// textures are all valid for any target on the device that created them, so
/// exactly one of these serves every [`SSScene`] and every
/// [`crate::SSPlayer`] drawing on that device. The host owns it and lends it,
/// the same way it lends `Device` and `Queue` — putting a copy inside each
/// player is what made a second character on screen duplicate ~39 pipelines and
/// every atlas.
///
/// Nothing here is format-specific: [`PipelineKey`] carries the target formats,
/// so targets that disagree share one renderer.
pub struct SSRenderer {
    /// Every pipeline used so far, built on first use (SPEC §5.2).
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    /// Inputs the lazy builder needs: the sprite/mask WGSL module and its layout.
    shader_module: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    /// Compiled module holding all 13 `fs_ss_*` fragment entries + the shared
    /// vertex stage; shader pipelines are derived from it lazily.
    ss_shader_module: wgpu::ShaderModule,
    /// Pipeline layout for shader pipelines: [globals, texture, ss-params].
    ss_pipeline_layout: wgpu::PipelineLayout,

    /// group(0): the projection uniform. The layout is shared; the buffer behind
    /// it belongs to the target ([`SSScene::globals_bind_group`]).
    globals_bind_group_layout: wgpu::BindGroupLayout,
    /// group(1): atlas texture + sampler.
    texture_bind_group_layout: wgpu::BindGroupLayout,
    /// group(2): one dynamic-offset uniform (the per-part `SsParams`).
    ss_params_bind_group_layout: wgpu::BindGroupLayout,

    sampler: wgpu::Sampler,
    /// 1x1 white texture, bound when a draw's texture hash is missing (SPEC §8)
    /// — used by Shape content and textureless mask writers. Filled with white on
    /// the first frame (the constructor has no queue to upload with).
    white_texture: wgpu::Texture,
    white_bind_group: wgpu::BindGroup,
    white_ready: bool,

    /// Atlas textures by fnv1a hash. Shared, so N players showing the same
    /// character upload it once (SPEC §1.2).
    textures: HashMap<u32, TextureEntry>,
    /// Retained pixels for [`Retention::Retain`] uploads, so a device loss can be
    /// recovered without the host re-supplying them (SPEC §14).
    texture_sources: HashMap<u32, (Vec<u8>, u32, u32)>,
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

/// Whether [`SSRenderer::upload_texture`] keeps a copy of the pixels for device
/// loss recovery (SPEC §14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retention {
    /// Keep them. For textures the host supplied and may no longer hold.
    Retain,
    /// Do not: the supplier can produce them again. Embedded atlases use this —
    /// the player re-decodes from the `.ssab` it already owns, so a second
    /// full-size copy of every atlas would be pure waste.
    Reproducible,
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

/// The 12 `BlendType`s in schema discriminant order (SPEC §4.3). Pipelines are
/// built in this order; [`blend_index`] maps a `BlendType` to its slot.
const BLEND_ORDER: [BlendType; 12] =
    [BlendType::Mix, BlendType::Mul, BlendType::Add, BlendType::Sub, BlendType::Mulalpha, BlendType::Screen, BlendType::Exclusion, BlendType::Invert, BlendType::Mul2, BlendType::Div2, BlendType::Screen2, BlendType::Overlay2];

/// Blend mode -> index into the per-blend pipeline groups (schema discriminant,
/// clamped into 0..12).
fn blend_index(blend: BlendType) -> usize {
    (blend.0 as usize).min(BLEND_ORDER.len() - 1)
}

/// Premultiplied blend state for a `BlendType` (SPEC §5.2, matching the WebGL
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
        // Div2 / Overlay2: no GPU-native equivalent -> Mix fallback (SPEC §4.3).
        _ => (One, OneMinusSrcAlpha, Add),
    };
    wgpu::BlendState { color: wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: op }, alpha: wgpu::BlendComponent { src_factor: One, dst_factor: OneMinusSrcAlpha, operation: Add } }
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

/// Stencil comparison a content pipeline uses for a given test (mask doc §2-4).
fn stencil_compare(stencil: StencilTest) -> wgpu::CompareFunction {
    match stencil {
        StencilTest::None => wgpu::CompareFunction::Always,
        StencilTest::Inside => wgpu::CompareFunction::NotEqual,
        StencilTest::Outside => wgpu::CompareFunction::Equal,
    }
}

/// The stencil write a mask writer performs.
fn stencil_write_op(op: MaskOp) -> wgpu::StencilOperation {
    match op {
        MaskOp::Incr => wgpu::StencilOperation::IncrementClamp,
        MaskOp::Decr => wgpu::StencilOperation::DecrementClamp,
        MaskOp::Invert => wgpu::StencilOperation::Invert,
    }
}

/// Baked stencil face for a compare + pass op (front == back; ref is always 0).
fn stencil_face(compare: wgpu::CompareFunction, pass_op: wgpu::StencilOperation) -> wgpu::StencilFaceState {
    wgpu::StencilFaceState { compare, fail_op: wgpu::StencilOperation::Keep, depth_fail_op: wgpu::StencilOperation::Keep, pass_op }
}

/// Depth/stencil state for a pipeline that only *tests* coverage (content), or
/// only *writes* it (mask writers, `write_mask` 0xFF). The depth aspect is
/// inert throughout: never written, always passing (SPEC §6).
fn depth_stencil_state(format: Option<wgpu::TextureFormat>, compare: wgpu::CompareFunction, pass_op: wgpu::StencilOperation, write_mask: u32) -> Option<wgpu::DepthStencilState> {
    format.map(|format| wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: wgpu::StencilState { front: stencil_face(compare, pass_op), back: stencil_face(compare, pass_op), read_mask: 0xFF, write_mask },
        bias: wgpu::DepthBiasState::default(),
    })
}

const WGSL_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) varg: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) varg: vec4<f32>,
}

struct Globals {
    proj: mat4x4<f32>,
}

@group(0) @binding(0)
var<uniform> globals: Globals;

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    var pos = globals.proj * vec4<f32>(model.position, 1.0);
    out.clip_position = vec4<f32>(pos.x, pos.y, 0.5, 1.0);
    out.uv = model.uv;
    out.color = model.color;
    out.varg = model.varg;
    return out;
}

@group(1) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(1) @binding(1)
var s_diffuse: sampler;

// PartsColor blend (SPEC §5.3, direct port of WebGL PartsColorProgram).
// Textures are premultiplied, so recover straight RGB, apply the mix/mul/add-sub
// blend selected by `varg`, then re-premultiply. Blend factors are premul too
// (see the pipeline blend table), keeping the whole path premultiplied.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    if (tex.a == 0.0) {
        return vec4<f32>(0.0);
    }
    let rate = in.varg.x;
    let straight = tex.rgb / max(tex.a, 0.0001);
    let mixC = mix(straight, in.color.rgb, rate);
    let mulC = straight * mix(vec3<f32>(1.0), in.color.rgb, rate);
    let addC = straight + in.color.rgb * rate * sign(in.varg.w);
    let rgb = mixC * in.varg.y + mulC * in.varg.z + addC * abs(in.varg.w);
    let a = tex.a * in.color.a;
    return vec4<f32>(rgb * a, a);
}

// Mask-write fragment: color output is discarded (ColorWrites::empty()); only
// the cutout matters. color.r carries the threshold (< 0 = no cutout).
@fragment
fn fs_mask(in: VertexOutput) -> @location(0) vec4<f32> {
    let a = textureSample(t_diffuse, s_diffuse, in.uv).a;
    let threshold = in.color.r;
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
const SHADER_PARAM_STRIDE: usize = 256;
/// f32 count of one slot (`SHADER_PARAM_STRIDE / 4`).
const SHADER_PARAM_STRIDE_F32: usize = SHADER_PARAM_STRIDE / 4;

/// id_hash (SDK `PartAttributeShader.id_hash()`, `fnv1a_hash_str("ss-*")`) ->
/// WGSL fragment entry point. The 13 SpriteStudio integration shaders (SPEC §6,
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
fn ss_shader_entry(id_hash: u32) -> Option<&'static str> {
    SS_SHADER_ENTRIES.iter().find(|(h, _)| *h == id_hash).map(|(_, e)| *e)
}

/// The 13 SpriteStudio integration shaders (統合シェーダ), ported from the WebGL
/// `SSShaderLib.ts` (itself the SS6-SDK GLSL). Each entry computes a straight-
/// alpha `Pixel`, then `ss_finish` applies the same PartsColor composite as
/// `fs_main` (varg mix/mul/add-sub) and re-premultiplies — except `ss-blur`,
/// which multiplies by the part colour directly (mirrors the SS6 reference).
///
/// `ssTex` un-premultiplies (textures are premultiplied, §5.2) and samples at LOD
/// 0 via `textureSampleLevel` so sampling is legal inside loops / after `discard`
/// (WGSL forbids implicit-derivative `textureSample` in non-uniform control flow).
/// Per-part uniforms live in group(2): `param_a`=param0..3, `param_b`=param4..7,
/// `cell_rect`=(minU,minV,maxU,maxV) over the quad's corners, `pixel_size`=1/texel.
const SS_SHADER_WGSL: &str = r#"
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) varg: vec4<f32>,
}
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) varg: vec4<f32>,
}
struct Globals { proj: mat4x4<f32> }
@group(0) @binding(0) var<uniform> globals: Globals;

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    var pos = globals.proj * vec4<f32>(model.position, 1.0);
    out.clip_position = vec4<f32>(pos.x, pos.y, 0.5, 1.0);
    out.uv = model.uv;
    out.color = model.color;
    out.varg = model.varg;
    return out;
}

@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var s_diffuse: sampler;

struct SsParams {
    param_a: vec4<f32>,    // param0..3
    param_b: vec4<f32>,    // param4..7
    cell_rect: vec4<f32>,  // minU, minV, maxU, maxV
    pixel_size: vec4<f32>, // 1/texW, 1/texH, 0, 0
}
@group(2) @binding(0) var<uniform> ss: SsParams;

const PI: f32 = 3.141592653589793;

fn ssTex(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSampleLevel(t_diffuse, s_diffuse, uv, 0.0);
    return vec4<f32>(c.rgb / max(c.a, 0.0001), c.a);
}

// PartColor composite (straight space) + re-premultiply — same math as fs_main.
fn ss_finish(pixel: vec4<f32>, color: vec4<f32>, varg: vec4<f32>) -> vec4<f32> {
    let rate = varg.x;
    let p = pixel.rgb;
    let mix_c = mix(p, color.rgb, rate);
    let mul_c = p * mix(vec3<f32>(1.0), color.rgb, rate);
    let add_c = p + color.rgb * rate * sign(varg.w);
    let rgb = mix_c * varg.y + mul_c * varg.z + add_c * abs(varg.w);
    let a = pixel.a * color.a;
    return vec4<f32>(rgb * a, a);
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
    return ss_finish(vec4<f32>(mixed_rgb, pixel.a), in.color, in.varg);
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
    return ss_finish(vec4<f32>(shifted.rgb, shifted.a), in.color, in.varg);
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
    return ss_finish(vec4<f32>(v, pixel.a), in.color, in.varg);
}

@fragment
fn fs_ss_pix(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_power = ss.param_a.x;
    let px = ss.pixel_size.xy;
    let v = 1.0 + 96.0 * f_power;
    let step_uv = vec2<f32>(v * px.x, v * px.y);
    let coord = floor(in.uv / step_uv) * step_uv;
    return ss_finish(ssTex(coord), in.color, in.varg);
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
    return ss_finish(vec4<f32>(rgb, pixel.a), in.color, in.varg);
}

@fragment
fn fs_ss_bmask(in: VertexOutput) -> @location(0) vec4<f32> {
    let f_brightness = ss.param_a.x;
    let pixel = ssTex(in.uv);
    let b = abs(step(f_brightness, 0.0) - max(pixel.r, max(pixel.g, pixel.b)));
    if (b <= abs(f_brightness)) { discard; }
    return ss_finish(pixel, in.color, in.varg);
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
    return ss_finish(vec4<f32>(vec3<f32>(v), v), in.color, in.varg);
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
    return vec4<f32>(c.rgb * c.a, c.a);
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
    return ss_finish(ssTex(vec2<f32>(coord.x + s, coord.y)), in.color, in.varg);
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
    return ss_finish(pixel, in.color, in.varg);
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
    return ss_finish(ssTex(coord + v), in.color, in.varg);
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
    return ss_finish(ssTex(lt_uv + st), in.color, in.varg);
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
    return ss_finish(pixel, in.color, in.varg);
}
"#;

impl SSRenderer {
    /// Build the device-scoped resources. Cheap: no pipeline is compiled until a
    /// frame asks for one (SPEC §5.2), so this is two shader modules, three bind
    /// group layouts, a sampler and a 1x1 texture.
    ///
    /// Takes no target formats — those ride in [`PipelineKey`], which is what
    /// lets one renderer serve every target on the device (SPEC §9.1).
    pub fn new(device: &wgpu::Device) -> Self {
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("SpriteStudio Shader"), source: wgpu::ShaderSource::Wgsl(WGSL_SHADER.into()) });

        // group(0): the projection uniform. Only the layout is shared — each
        // render target owns the buffer behind it.
        let globals_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SpriteStudio Globals Layout"),
            entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }],
        });

        // group(1): atlas texture + sampler.
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
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("SpriteStudio Pipeline Layout"), bind_group_layouts: &[Some(&globals_bind_group_layout), Some(&texture_bind_group_layout)], immediate_size: 0 });

        // Integration-shader (`ss-*`) resources (SPEC §6). The module is compiled
        // now; the pipelines are built lazily per key on first use.
        let ss_shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("SpriteStudio SS Shaders"), source: wgpu::ShaderSource::Wgsl(SS_SHADER_WGSL.into()) });
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
            bind_group_layouts: &[Some(&globals_bind_group_layout), Some(&texture_bind_group_layout), Some(&ss_params_bind_group_layout)],
            immediate_size: 0,
        });

        // 1x1 white fallback texture (SPEC §8): used when a draw's texture hash
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
        let white_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SpriteStudio White Bind Group"),
            layout: &texture_bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&white_view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }],
        });

        Self {
            pipelines: HashMap::new(),
            shader_module,
            pipeline_layout,
            ss_shader_module,
            ss_pipeline_layout,
            globals_bind_group_layout,
            texture_bind_group_layout,
            ss_params_bind_group_layout,
            sampler,
            white_texture,
            white_bind_group,
            white_ready: false,
            textures: HashMap::new(),
            texture_sources: HashMap::new(),
        }
    }

    /// How many pipelines have been compiled so far (SPEC §5.2). A diagnostic:
    /// it counts what the animation actually asked for, against the 39 that
    /// eager construction used to build before the first frame.
    pub fn pipeline_count(&self) -> usize {
        self.pipelines.len()
    }

    /// Compile `key`'s pipeline if this is the first frame that needs it. A
    /// [`PipelineKey::Shader`] whose id is not a built-in `ss-*` shader stays
    /// absent; [`Self::draw`] skips draws whose pipeline is missing.
    fn ensure_pipeline(&mut self, device: &wgpu::Device, key: PipelineKey) {
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
            entry: &'a str,
            blend: Option<wgpu::BlendState>,
            write_mask: wgpu::ColorWrites,
            depth_stencil: Option<wgpu::DepthStencilState>,
        }
        let blend_of = |idx: usize| BLEND_ORDER[idx.min(BLEND_ORDER.len() - 1)];

        let recipe = match key.kind {
            PipelineKind::Content(blend_idx, stencil) => {
                let blend = blend_of(blend_idx);
                Recipe {
                    label: format!("Content {} {stencil:?}", blend.variant_name().unwrap_or("Blend")),
                    module: &self.shader_module,
                    layout: &self.pipeline_layout,
                    entry: "fs_main",
                    blend: Some(blend_state(blend)),
                    write_mask: wgpu::ColorWrites::ALL,
                    // Content tests coverage; it never writes it.
                    depth_stencil: depth_stencil_state(key.stencil_format, stencil_compare(stencil), wgpu::StencilOperation::Keep, 0x00),
                }
            }
            PipelineKind::Mask(op) => Recipe {
                label: format!("Mask Write {op:?}"),
                module: &self.shader_module,
                layout: &self.pipeline_layout,
                entry: "fs_mask",
                blend: None,
                // Coverage only — the fragment's colour is thrown away.
                write_mask: wgpu::ColorWrites::empty(),
                depth_stencil: depth_stencil_state(key.stencil_format, wgpu::CompareFunction::Always, stencil_write_op(op), 0xFF),
            },
            PipelineKind::Shader(id_hash, blend_idx) => {
                let entry = ss_shader_entry(id_hash)?;
                Recipe {
                    label: format!("SS Shader {entry}"),
                    module: &self.ss_shader_module,
                    layout: &self.ss_pipeline_layout,
                    entry,
                    blend: Some(blend_state(blend_of(blend_idx))),
                    write_mask: wgpu::ColorWrites::ALL,
                    // Shader parts are never masked in v1, so the stencil is inert.
                    depth_stencil: depth_stencil_state(key.stencil_format, wgpu::CompareFunction::Always, wgpu::StencilOperation::Keep, 0x00),
                }
            }
        };

        Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&recipe.label),
            layout: Some(recipe.layout),
            vertex: wgpu::VertexState { module: recipe.module, entry_point: Some("vs_main"), buffers: &[Vertex::desc()], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: recipe.module,
                entry_point: Some(recipe.entry),
                targets: &[Some(wgpu::ColorTargetState { format: key.color_format, blend: recipe.blend, write_mask: recipe.write_mask })],
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
    /// Provide a texture (SPEC §8). `hash` is the fnv1a hash of the cellmap image
    /// path. Both embedded and reference paths funnel into this single API.
    ///
    /// `rgba` is straight-alpha (as decoded from PNG); it is premultiplied here so
    /// the whole pipeline stays premultiplied (SPEC §5.2, matching WebGL's
    /// `UNPACK_PREMULTIPLY_ALPHA_WEBGL`). The premul shader recovers straight RGB
    /// before applying PartsColor.
    pub fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hash: u32, rgba: &[u8], (width, height): (u32, u32), retention: Retention) {
        if retention == Retention::Retain {
            self.texture_sources.insert(hash, (rgba.to_vec(), width, height));
        }
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };

        // Premultiply straight-alpha RGBA in place (rgb *= a/255, rounded).
        let mut premul = rgba.to_vec();
        for px in premul.chunks_exact_mut(4) {
            let a = px[3] as u32;
            px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
            px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
            px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
        }

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
            &premul,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * width), rows_per_image: Some(height) },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.create_texture_bind_group(device, &view);
        self.textures.insert(hash, TextureEntry { bind_group, size: (width as f32, height as f32) });
    }
    fn create_texture_bind_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SpriteStudio Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
        })
    }
    /// Pixel size of an uploaded atlas, for normalising the runtime's UVs (which
    /// arrive in texture pixel space, SPEC §3.2).
    pub fn texture_size(&self, hash: u32) -> Option<(f32, f32)> {
        self.textures.get(&hash).map(|t| t.size)
    }

    /// The bind group for `hash`, or the 1x1 white fallback when it is missing
    /// (SPEC §8) — Shape content and textureless mask writers rely on this.
    fn texture_bind_group(&self, hash: u32) -> &wgpu::BindGroup {
        self.textures.get(&hash).map(|e| &e.bind_group).unwrap_or(&self.white_bind_group)
    }

    /// Fill the white fallback. Deferred to the first frame because the
    /// constructor has no `Queue`; a `create_texture` alone leaves it
    /// transparent black, which would collapse `tex * colour` to zero.
    fn ensure_white(&mut self, queue: &wgpu::Queue) {
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

    /// Drop the retained copies of [`Retention::Retain`] uploads (SPEC §14).
    /// Saves the duplicated pixels when the host keeps its own and would rather
    /// re-supply them itself; after this, [`SSRenderer::restore_device`] restores
    /// nothing and every texture must be uploaded again before drawing.
    pub fn forget_texture_sources(&mut self) {
        self.texture_sources.clear();
    }

    /// Rebuild every GPU resource against a freshly created `device` after the
    /// previous one was lost (SPEC §14).
    ///
    /// Because this is device-scoped, **one call covers every scene and player on
    /// the device** — the old design repeated the whole job, atlas decode
    /// included, once per player.
    ///
    /// Textures uploaded with [`Retention::Retain`] come back from their retained
    /// pixels. [`Retention::Reproducible`] ones do not: their supplier re-registers
    /// them (for embedded packs that is [`crate::SSPlayer::restore_device`], which
    /// re-decodes from the `.ssab` it already owns). Anything holding a
    /// target-scoped resource — the projection buffer — restores itself.
    pub fn restore_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        // Lift the sources out *before* replacing self — everything else in here
        // belonged to the dead device, but these are plain CPU pixels.
        let sources = std::mem::take(&mut self.texture_sources);
        *self = Self::new(device);
        for (hash, (rgba, w, h)) in &sources {
            // Already held below, so re-uploading must not retain a second copy.
            self.upload_texture(device, queue, *hash, rgba, (*w, *h), Retention::Reproducible);
        }
        self.texture_sources = sources;
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
