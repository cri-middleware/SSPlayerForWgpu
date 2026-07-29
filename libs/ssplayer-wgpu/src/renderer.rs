//! Layer 2a (SPEC §1.2 / §9.1): the device-scoped half of rendering.
//!
//! Pipelines, bind group layouts, the sampler, the white fallback and the atlas
//! textures stay valid for any render target on the device that made them, so
//! the host owns one [`SSRenderer`] and lends it — the same borrow model it
//! already applies to `Device` and `Queue`. The shader modules and the vertex
//! format live here too: they are the contract the pipelines are built against.
//!
//! The per-target, per-frame half is [`crate::SSScene`].

use crate::mask::{MaskOp, StencilTest};
use std::collections::HashMap;

use ssruntime::runtime::framedata::ss::runtime::BlendType;

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

/// What a pipeline does, before the render target is taken into account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PipelineKind {
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
pub(crate) struct PipelineKey {
    pub(crate) kind: PipelineKind,
    pub(crate) color_format: wgpu::TextureFormat,
    pub(crate) stencil_format: Option<wgpu::TextureFormat>,
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
/// exactly one of these serves every [`crate::SSScene`] and every
/// [`crate::SSPlayer`] drawing on that device. The host owns it and lends it,
/// the same way it lends `Device` and `Queue` — putting a copy inside each
/// player is what made a second character on screen duplicate ~39 pipelines and
/// every atlas.
///
/// Nothing here is format-specific: the pipeline key carries the target formats,
/// so targets that disagree share one renderer.
pub struct SSRenderer {
    /// Every pipeline used so far, built on first use (SPEC §5.2).
    pub(crate) pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
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
    pub(crate) globals_bind_group_layout: wgpu::BindGroupLayout,
    /// group(1): atlas texture + sampler.
    texture_bind_group_layout: wgpu::BindGroupLayout,
    /// group(2): one dynamic-offset uniform (the per-part `SsParams`).
    pub(crate) ss_params_bind_group_layout: wgpu::BindGroupLayout,

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

/// The 12 `BlendType`s in schema discriminant order (SPEC §4.3). Pipelines are
/// built in this order; [`blend_index`] maps a `BlendType` to its slot.
const BLEND_ORDER: [BlendType; 12] =
    [BlendType::Mix, BlendType::Mul, BlendType::Add, BlendType::Sub, BlendType::Mulalpha, BlendType::Screen, BlendType::Exclusion, BlendType::Invert, BlendType::Mul2, BlendType::Div2, BlendType::Screen2, BlendType::Overlay2];

/// Blend mode -> index into the per-blend pipeline groups (schema discriminant,
/// clamped into 0..12).
pub(crate) fn blend_index(blend: BlendType) -> usize {
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
pub(crate) const SHADER_PARAM_STRIDE: usize = 256;
/// f32 count of one slot (`SHADER_PARAM_STRIDE / 4`).
pub(crate) const SHADER_PARAM_STRIDE_F32: usize = SHADER_PARAM_STRIDE / 4;

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
pub(crate) fn ss_shader_entry(id_hash: u32) -> Option<&'static str> {
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
    /// Takes no target formats — those ride in the pipeline key, which is what
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
    /// them (for embedded packs that is `SSPlayer::restore_textures`, which
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
