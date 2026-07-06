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
use ssruntime::runtime::framedata::ss::runtime::{BlendType, DrawBatchKind, root_as_frame_data_unchecked};

/// Per-vertex data uploaded to the GPU: world position, normalized UV, and RGBA.
///
/// For mask-write draws the `color` channel is repurposed: `color[0]` carries
/// the cutout threshold consumed by `fs_mask` (SPEC §6).
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
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

#[derive(Clone, Debug)]
struct BatchDrawCall {
    /// Index into the flat `pipelines` vec.
    pipeline_idx: usize,
    texture_hash: u32,
    index_start: u32,
    index_count: u32,
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

/// Blend mode -> index into the per-blend pipeline groups.
fn blend_index(blend: BlendType) -> usize {
    match blend {
        BlendType::Mix => 0,
        BlendType::Add => 1,
        BlendType::Sub => 2,
        BlendType::Mul => 3,
        _ => 0,
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
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
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
    return out;
}

@group(1) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(1) @binding(1)
var s_diffuse: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(t_diffuse, s_diffuse, in.uv);
    return tex_color * in.color;
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

        // Blend states for the 4 supported modes.
        let blend_mix = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        };
        let blend_add = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let blend_sub = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::ReverseSubtract },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let blend_mul = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Dst, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let blends = [("Mix", blend_mix), ("Add", blend_add), ("Sub", blend_sub), ("Mul", blend_mul)];

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
        for (name, blend) in blends {
            // Order within a blend group must match `stencil_offset`.
            pipelines.push(make_pipeline(&format!("Content {name} None"), "fs_main", Some(blend), wgpu::ColorWrites::ALL, content_ds(wgpu::CompareFunction::Always)));
            if stencil_enabled {
                pipelines.push(make_pipeline(&format!("Content {name} Inside"), "fs_main", Some(blend), wgpu::ColorWrites::ALL, content_ds(wgpu::CompareFunction::NotEqual)));
                pipelines.push(make_pipeline(&format!("Content {name} Outside"), "fs_main", Some(blend), wgpu::ColorWrites::ALL, content_ds(wgpu::CompareFunction::Equal)));
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
    pub fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hash: u32, rgba: &[u8], width: u32, height: u32) {
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
            rgba,
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
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    let part_color_idx = part.part_color();
                    let colors_arr = parts_color
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
                        .unwrap_or_else(|| {
                            let a = part.alpha();
                            [[1.0, 1.0, 1.0, a]; 4]
                        });

                    self.push_quad_part(frame, transform, part_idx, tex_size, &colors_arr);
                }
            }
            DrawBatchKind::Mesh => {
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    let part_color_idx = part.part_color();
                    let mesh_color = parts_color
                        .filter(|_| part_color_idx >= 0)
                        .map(|pc_vec| {
                            let pc = pc_vec.get(part_color_idx as usize);
                            let c_lt = pc.lt().rgba();
                            [c_lt.r() as f32 / 255.0, c_lt.g() as f32 / 255.0, c_lt.b() as f32 / 255.0, c_lt.a() as f32 / 255.0]
                        })
                        .unwrap_or_else(|| [1.0, 1.0, 1.0, part.alpha()]);

                    self.push_mesh_part(frame, transform, part_idx, part.position_z(), tex_size, mesh_color);
                }
            }
            DrawBatchKind::Shape => {
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    // Shape fill: the interpolated PartColor corners, or white *
                    // alpha when the part has no PartColor (matches WebGL). Shapes
                    // are textureless (white fallback), so this colour shows as-is.
                    let part_color_idx = part.part_color();
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

                    self.push_shape_content_part(frame, transform, part_idx, &corners);
                }
            }
            _ => {}
        }

        let index_end = self.indices.len() as u32;
        if index_end > index_start {
            self.draw_calls.push(BatchDrawCall { pipeline_idx, texture_hash, index_start, index_count: index_end - index_start });
        }
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
                self.push_quad_part(frame, transform, writer.part_idx, tex_size, &[color; 4]);
            }
            GeomKind::Mesh => {
                self.push_mesh_part(frame, transform, writer.part_idx, 0.0, tex_size, color);
            }
            GeomKind::Shape => {
                self.push_shape_part(frame, transform, writer.part_idx, color);
            }
            GeomKind::Unsupported => return,
        }

        let index_end = self.indices.len() as u32;
        if index_end > index_start {
            self.draw_calls.push(BatchDrawCall { pipeline_idx: self.mask_idx(op), texture_hash: writer.texture_hash, index_start, index_count: index_end - index_start });
        }
    }

    /// Build one sprite quad (4 verts / 2 tris) for `part_idx` into the frame
    /// accumulation. `colors` is per-corner `[lt, rt, lb, rb]`.
    fn push_quad_part(&mut self, frame: &FrameView, transform: &[f32; 16], part_idx: usize, tex_size: Option<(f32, f32)>, colors: &[[f32; 4]; 4]) {
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

            self.vertices.push(Vertex { position: [fx, fy, fz], uv: [tu, tv], color: colors[k] });
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
    fn push_mesh_part(&mut self, frame: &FrameView, transform: &[f32; 16], part_idx: usize, pos_z: f32, tex_size: Option<(f32, f32)>, color: [f32; 4]) {
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

            self.vertices.push(Vertex { position: [fx, fy, fz], uv: [tu, tv], color });
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
            self.vertices.push(Vertex { position: [fx, fy, fz], uv: [0.0, 0.0], color });
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
    fn push_shape_content_part(&mut self, frame: &FrameView, transform: &[f32; 16], part_idx: usize, corners: &[[f32; 4]; 4]) {
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

            self.vertices.push(Vertex { position: [fx, fy, fz], uv: [0.5, 0.5], color });
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
                self.vertices.push(Vertex { position: [fx, fy, fz], uv: [tu, tv], color });
            }

            // Indices are quad-local (0..quad_count*4), rebased onto base_vert.
            let idx_first = particle_offset * 6;
            let idx_last = (particle_offset + quad_count) * 6;
            for ii in idx_first..idx_last {
                self.indices.push(base_vert + indices.get(ii) as u32);
            }

            let index_end = self.indices.len() as u32;
            if index_end > index_start {
                self.draw_calls.push(BatchDrawCall { pipeline_idx, texture_hash, index_start, index_count: index_end - index_start });
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
            rpass.set_pipeline(&self.pipelines[call.pipeline_idx]);

            // Missing texture -> 1x1 white fallback (SPEC §8).
            let bind_group = self.textures.get(&call.texture_hash).map(|e| &e.bind_group).unwrap_or(&self.white_bind_group);
            rpass.set_bind_group(1, bind_group, &[]);
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
