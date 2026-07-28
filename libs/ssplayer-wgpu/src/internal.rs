//! Layer 2 (rendering / "body"): SSInternalPlayer.
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

/// Construction config for SSInternalPlayer, matched to the host's render target.
#[derive(Clone, Copy, Debug)]
pub struct RendererConfig {
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
}

/// An integration-shader (`ss-*`) draw: resolves to a lazily-built shader
/// pipeline and binds the part's params at `slot` in the dynamic-offset buffer.
#[derive(Clone, Copy, Debug)]
struct ShaderDraw {
    id_hash: u32,
    blend_idx: usize,
    /// Index of this part's 256-byte block in the shader-params buffer.
    slot: u32,
}

#[derive(Clone, Debug)]
struct BatchDrawCall {
    /// Index into the flat `pipelines` vec (ignored when `shader` is `Some`).
    pipeline_idx: usize,
    texture_hash: u32,
    index_start: u32,
    index_count: u32,
    /// `Some` for `ss-*` integration-shader parts (SPEC §6): use the shader
    /// pipeline for `(id_hash, blend)` and bind group(2) at `slot`.
    shader: Option<ShaderDraw>,
}

struct TextureEntry {
    bind_group: wgpu::BindGroup,
    size: (f32, f32),
}

/// Layer 2 (rendering): FrameData/DrawBatch/SoA -> wgpu draw commands.
///
/// Borrow model: owns neither the `Device`/`Queue` nor the render target. It
/// only holds GPU resources (textures, pipelines, vertex/index buffers).
pub struct SSInternalPlayer {
    /// Flat pipeline list: content pipelines first (`BLEND_COUNT * content_stride`),
    /// then the mask-write pipelines (`mask_base..`). See [`Self::content_idx`] /
    /// [`Self::mask_idx`].
    pipelines: Vec<wgpu::RenderPipeline>,
    /// Number of stencil variants per blend mode: 3 (None/Inside/Outside) when
    /// masking is enabled, else 1.
    content_stride: usize,
    /// Flat index where the mask-write pipelines begin.
    mask_base: usize,
    /// Whether masking (stencil) is enabled by config.
    stencil_enabled: bool,
    /// Target formats (kept for lazily building shader pipelines in `end_frame`).
    color_format: wgpu::TextureFormat,
    stencil_format: Option<wgpu::TextureFormat>,

    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// 1x1 white texture, bound when a draw's texture hash is missing (SPEC §8)
    /// — used by Shape content and textureless mask writers. Filled with white on
    /// the first `end_frame` (the constructor has no queue to upload with).
    white_texture: wgpu::Texture,
    white_bind_group: wgpu::BindGroup,
    white_ready: bool,

    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
    draw_calls: Vec<BatchDrawCall>,

    // Per-frame CPU accumulation (Unity-style flatten): every node/batch appends
    // here between begin_frame and end_frame, then it is uploaded once.
    vertices: Vec<Vertex>,
    indices: Vec<u32>,

    textures: HashMap<u32, TextureEntry>,

    // --- Integration shaders (`ss-*`, SPEC §6). Only touched when a frame has
    // shader parts; shader-free animations never build any of this. ---
    /// Compiled module holding all 13 `fs_ss_*` fragment entries + the shared
    /// vertex stage; shader pipelines are derived from it lazily.
    ss_shader_module: wgpu::ShaderModule,
    /// Pipeline layout for shader pipelines: [globals, texture, ss-params].
    ss_pipeline_layout: wgpu::PipelineLayout,
    /// group(2) layout: one dynamic-offset uniform (the per-part `SsParams`).
    ss_params_bind_group_layout: wgpu::BindGroupLayout,
    /// Lazily-built shader pipelines, keyed by `(id_hash, blend_idx)`.
    ss_pipelines: HashMap<(u32, usize), wgpu::RenderPipeline>,
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

/// Stencil test -> offset within a blend's pipeline group (0 when disabled).
fn stencil_offset(stencil: StencilTest) -> usize {
    match stencil {
        StencilTest::None => 0,
        StencilTest::Inside => 1,
        StencilTest::Outside => 2,
    }
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

impl SSInternalPlayer {
    /// Build pipelines and GPU resources from the borrowed `device`.
    pub fn new(device: &wgpu::Device, config: &RendererConfig) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("SpriteStudio Shader"), source: wgpu::ShaderSource::Wgsl(WGSL_SHADER.into()) });

        // Uniform buffer setup.
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SpriteStudio Globals UB"),
            size: 64, // 16 * 4 bytes for mat4x4
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SpriteStudio Uniform Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }],
        });

        let uniform_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("SpriteStudio Uniform Bind Group"), layout: &uniform_bind_group_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() }] });

        // Texture bind group layout.
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("SpriteStudio Pipeline Layout"), bind_group_layouts: &[Some(&uniform_bind_group_layout), Some(&texture_bind_group_layout)], immediate_size: 0 });

        // Integration-shader (`ss-*`) resources (SPEC §6). The module is compiled
        // now; the pipelines are built lazily per (id_hash, blend) on first use.
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
            bind_group_layouts: &[Some(&uniform_bind_group_layout), Some(&texture_bind_group_layout), Some(&ss_params_bind_group_layout)],
            immediate_size: 0,
        });

        let stencil_enabled = config.stencil_format.is_some();

        // Baked stencil face for a given compare + op (front == back; ref is
        // always 0). Content tests never write (write_mask 0); mask writers
        // always pass the test (compare Always) and apply their op on pass.
        let stencil_face = |compare: wgpu::CompareFunction, pass_op: wgpu::StencilOperation| wgpu::StencilFaceState { compare, fail_op: wgpu::StencilOperation::Keep, depth_fail_op: wgpu::StencilOperation::Keep, pass_op };

        // depth_stencil state for content pipelines with a given stencil compare.
        let content_ds = |compare: wgpu::CompareFunction| {
            config.stencil_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState { front: stencil_face(compare, wgpu::StencilOperation::Keep), back: stencil_face(compare, wgpu::StencilOperation::Keep), read_mask: 0xFF, write_mask: 0x00 },
                bias: wgpu::DepthBiasState::default(),
            })
        };

        let make_pipeline = |label: &str, entry: &str, blend: Option<wgpu::BlendState>, write_mask: wgpu::ColorWrites, depth_stencil: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[Vertex::desc()], compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), targets: &[Some(wgpu::ColorTargetState { format: config.color_format, blend, write_mask })], compilation_options: Default::default() }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        // --- Content pipelines: blend x stencil test ---
        // Stencil variants: None (Always), Inside (NotEqual,0), Outside (Equal,0).
        // When disabled, only the None variant is built (content_stride = 1).
        let content_stride = if stencil_enabled { 3 } else { 1 };
        let mut pipelines: Vec<wgpu::RenderPipeline> = Vec::new();
        for blend in BLEND_ORDER {
            let name = blend.variant_name().unwrap_or("Blend");
            let state = blend_state(blend);
            // Order within a blend group must match `stencil_offset`.
            pipelines.push(make_pipeline(&format!("Content {name} None"), "fs_main", Some(state), wgpu::ColorWrites::ALL, content_ds(wgpu::CompareFunction::Always)));
            if stencil_enabled {
                pipelines.push(make_pipeline(&format!("Content {name} Inside"), "fs_main", Some(state), wgpu::ColorWrites::ALL, content_ds(wgpu::CompareFunction::NotEqual)));
                pipelines.push(make_pipeline(&format!("Content {name} Outside"), "fs_main", Some(state), wgpu::ColorWrites::ALL, content_ds(wgpu::CompareFunction::Equal)));
            }
        }

        // --- Mask-write pipelines: Incr / Decr / Invert (order matches mask_idx) ---
        let mask_base = pipelines.len();
        if stencil_enabled {
            let write_ds = |op: wgpu::StencilOperation| {
                config.stencil_format.map(|format| wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState { front: stencil_face(wgpu::CompareFunction::Always, op), back: stencil_face(wgpu::CompareFunction::Always, op), read_mask: 0xFF, write_mask: 0xFF },
                    bias: wgpu::DepthBiasState::default(),
                })
            };
            pipelines.push(make_pipeline("Mask Write Incr", "fs_mask", None, wgpu::ColorWrites::empty(), write_ds(wgpu::StencilOperation::IncrementClamp)));
            pipelines.push(make_pipeline("Mask Write Decr", "fs_mask", None, wgpu::ColorWrites::empty(), write_ds(wgpu::StencilOperation::DecrementClamp)));
            pipelines.push(make_pipeline("Mask Write Invert", "fs_mask", None, wgpu::ColorWrites::empty(), write_ds(wgpu::StencilOperation::Invert)));
        }

        // 1x1 white fallback texture (SPEC §8): used when a draw's texture hash
        // is missing and by textureless mask writers.
        let white = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SpriteStudio White 1x1"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let white_view = white.create_view(&wgpu::TextureViewDescriptor::default());
        let white_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SpriteStudio White Bind Group"),
            layout: &texture_bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&white_view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }],
        });

        Self {
            pipelines,
            content_stride,
            mask_base,
            stencil_enabled,
            color_format: config.color_format,
            stencil_format: config.stencil_format,
            uniform_buffer,
            uniform_bind_group,
            texture_bind_group_layout,
            sampler,
            white_texture: white,
            white_bind_group,
            white_ready: false,
            vertex_buffer: None,
            index_buffer: None,
            draw_calls: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            textures: HashMap::new(),
            ss_shader_module,
            ss_pipeline_layout,
            ss_params_bind_group_layout,
            ss_pipelines: HashMap::new(),
            ss_params: Vec::new(),
            ss_params_buffer: None,
            ss_params_bind_group: None,
        }
    }

    /// Whether masking (stencil) is enabled for this renderer.
    pub fn stencil_enabled(&self) -> bool {
        self.stencil_enabled
    }

    /// Flat index of a content pipeline for `(blend, stencil)`.
    fn content_idx(&self, blend: BlendType, stencil: StencilTest) -> usize {
        let off = if self.stencil_enabled { stencil_offset(stencil) } else { 0 };
        blend_index(blend) * self.content_stride + off
    }

    /// Flat index of a mask-write pipeline for `op`.
    fn mask_idx(&self, op: MaskOp) -> usize {
        let off = match op {
            MaskOp::Incr => 0,
            MaskOp::Decr => 1,
            MaskOp::Invert => 2,
        };
        self.mask_base + off
    }

    /// Update the projection matrix to the target's pixel dimensions.
    pub fn update_projection(&self, queue: &wgpu::Queue, width: f32, height: f32, y_down: bool) {
        let scale_y = if y_down { -2.0 / height } else { 2.0 / height };
        let matrix = [2.0 / width, 0.0, 0.0, 0.0, 0.0, scale_y, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&matrix));
    }

    /// Provide a texture (SPEC §8). `hash` is the fnv1a hash of the cellmap image
    /// path. Both embedded and reference paths funnel into this single API.
    ///
    /// `rgba` is straight-alpha (as decoded from PNG); it is premultiplied here so
    /// the whole pipeline stays premultiplied (SPEC §5.2, matching WebGL's
    /// `UNPACK_PREMULTIPLY_ALPHA_WEBGL`). The premul shader recovers straight RGB
    /// before applying PartsColor.
    pub fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hash: u32, rgba: &[u8], width: u32, height: u32) {
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
    pub fn append_batch(&mut self, frame: &FrameView, transform: &[f32; 16], batch: &BatchInfo) {
        let BatchInfo { start_rank, count, texture_hash, blend_type, kind, stencil } = *batch;
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

        let pipeline_idx = self.content_idx(blend_type, stencil);
        let tex_size = self.textures.get(&texture_hash).map(|t| t.size);
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
                    let (colors_arr, vargs_arr) = parts_color
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
                    let shader = has_shader(rank).map(|attr| {
                        let slot = self.push_shader_params(&attr, frame, part_idx, tex_size);
                        ShaderDraw { id_hash: attr.id_hash(), blend_idx: blend_index(blend_type), slot }
                    });
                    self.draw_calls.push(BatchDrawCall { pipeline_idx, texture_hash, index_start: call_start, index_count: call_end - call_start, shader });
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
                    let (mesh_color, mesh_varg) = parts_color
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
                    let corners = parts_color
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
                    let shape_varg = [if has_pc { 1.0 } else { 0.0 }, 1.0, 0.0, 0.0];

                    self.push_shape_content_part(frame, transform, part_idx, &corners, shape_varg);
                }
            }
            _ => {}
        }

        let index_end = self.indices.len() as u32;
        if index_end > index_start {
            self.draw_calls.push(BatchDrawCall { pipeline_idx, texture_hash, index_start, index_count: index_end - index_start, shader: None });
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

    /// Lazily build (and cache) the shader pipeline for `(id_hash, blend_idx)`.
    /// No-op if `id_hash` is not a built-in `ss-*` shader. Stencil-test variants
    /// are unnecessary: shader parts are never masked (v1), so the pipeline uses
    /// a keep/Always depth-stencil state when the target has one.
    fn ensure_shader_pipeline(&mut self, device: &wgpu::Device, id_hash: u32, blend_idx: usize) {
        if self.ss_pipelines.contains_key(&(id_hash, blend_idx)) {
            return;
        }
        let Some(entry) = ss_shader_entry(id_hash) else {
            return;
        };
        let blend = blend_state(BLEND_ORDER[blend_idx.min(BLEND_ORDER.len() - 1)]);
        let depth_stencil =
            self.stencil_format.map(|format| wgpu::DepthStencilState { format, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: wgpu::StencilState::default(), bias: wgpu::DepthBiasState::default() });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("SpriteStudio SS Pipeline"),
            layout: Some(&self.ss_pipeline_layout),
            vertex: wgpu::VertexState { module: &self.ss_shader_module, entry_point: Some("vs_main"), buffers: &[Vertex::desc()], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &self.ss_shader_module,
                entry_point: Some(entry),
                targets: &[Some(wgpu::ColorTargetState { format: self.color_format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
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
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.ss_pipelines.insert((id_hash, blend_idx), pipeline);
    }

    /// Append one mask writer's geometry into the frame accumulation, routed to
    /// the mask-write pipeline for `op`. The cutout threshold rides in the
    /// vertex `color[0]` (consumed by `fs_mask`). No-op if masking is disabled.
    pub fn append_mask_writer(&mut self, frame: &FrameView, transform: &[f32; 16], writer: &MaskWriter, op: MaskOp) {
        use crate::mask::GeomKind;
        if !self.stencil_enabled || frame.frame_data.is_empty() {
            return;
        }
        if frame.world_matrices.is_empty() {
            return;
        }
        let tex_size = self.textures.get(&writer.texture_hash).map(|t| t.size);
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
            self.draw_calls.push(BatchDrawCall { pipeline_idx: self.mask_idx(op), texture_hash: writer.texture_hash, index_start, index_count: index_end - index_start, shader: None });
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
    /// (inherited from a parent mask); `None` for ordinary effects.
    pub fn append_effect(&mut self, plan_buf: &[u8], transform: &[f32; 16], stencil: StencilTest) {
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
            let pipeline_idx = self.content_idx(blend, stencil);

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

                let color = [colors.get(vi * 4), colors.get(vi * 4 + 1), colors.get(vi * 4 + 2), colors.get(vi * 4 + 3)];
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
                self.draw_calls.push(BatchDrawCall { pipeline_idx, texture_hash, index_start, index_count: index_end - index_start, shader: None });
            }
        }
    }

    /// Upload the accumulated geometry to the GPU. Call once after all batches.
    pub fn end_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        // One-time fill of the 1x1 white fallback (the constructor has no queue).
        if !self.white_ready {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.white_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &[255u8, 255, 255, 255],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
            self.white_ready = true;
        }

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

        // Integration shaders (SPEC §6): build any missing (id_hash, blend)
        // pipeline lazily, then upload this frame's per-part params. Nothing here
        // runs for shader-free frames (`ss_params` stays empty).
        if !self.ss_params.is_empty() {
            let needed: Vec<(u32, usize)> = self.draw_calls.iter().filter_map(|c| c.shader.map(|s| (s.id_hash, s.blend_idx))).collect();
            for (id_hash, blend_idx) in needed {
                self.ensure_shader_pipeline(device, id_hash, blend_idx);
            }

            let bytes: &[u8] = bytemuck::cast_slice(&self.ss_params);
            let needed_size = bytes.len() as u64;
            let current = self.ss_params_buffer.as_ref().map(|b| b.size()).unwrap_or(0);
            if current < needed_size {
                let new_size = (needed_size * 2).max(SHADER_PARAM_STRIDE as u64);
                let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("SpriteStudio SS Params Buffer"), size: new_size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                // The binding window is one `SsParams` (64 B); the dynamic offset picks the slot.
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("SpriteStudio SS Params Bind Group"),
                    layout: &self.ss_params_bind_group_layout,
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
    pub fn draw<'a>(&'a self, rpass: &mut wgpu::RenderPass<'a>) -> usize {
        if self.draw_calls.is_empty() {
            return 0;
        }

        let (v_buf, i_buf) = match (&self.vertex_buffer, &self.index_buffer) {
            (Some(v), Some(i)) => (v, i),
            _ => return 0,
        };

        rpass.set_vertex_buffer(0, v_buf.slice(..));
        rpass.set_index_buffer(i_buf.slice(..), wgpu::IndexFormat::Uint32);
        rpass.set_bind_group(0, &self.uniform_bind_group, &[]);
        if self.stencil_enabled {
            // Stencil ref is always 0; ops/compare are baked into the pipelines.
            rpass.set_stencil_reference(0);
        }

        for call in &self.draw_calls {
            // Missing texture -> 1x1 white fallback (SPEC §8).
            let texture = self.textures.get(&call.texture_hash).map(|e| &e.bind_group).unwrap_or(&self.white_bind_group);

            match call.shader {
                // Integration-shader part (SPEC §6): shader pipeline + group(2)
                // params at the part's dynamic offset. Skip if the pipeline failed
                // to build (unknown id_hash filtered earlier, so normally present).
                Some(sd) => {
                    let (Some(pipeline), Some(params_bg)) = (self.ss_pipelines.get(&(sd.id_hash, sd.blend_idx)), self.ss_params_bind_group.as_ref()) else {
                        continue;
                    };
                    rpass.set_pipeline(pipeline);
                    rpass.set_bind_group(1, texture, &[]);
                    rpass.set_bind_group(2, params_bg, &[sd.slot * SHADER_PARAM_STRIDE as u32]);
                }
                None => {
                    rpass.set_pipeline(&self.pipelines[call.pipeline_idx]);
                    rpass.set_bind_group(1, texture, &[]);
                }
            }
            rpass.draw_indexed(call.index_start..call.index_start + call.index_count, 0, 0..1);
        }

        self.draw_calls.len()
    }

    fn create_texture_bind_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SpriteStudio Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
        })
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
