//! Layer 2 (rendering / "body"): SSInternalPlayer.
//!
//! Turns the runtime's FrameData / DrawBatch / SoA buffers into wgpu vertices,
//! indices and draw commands. Borrow model: it never owns the `Device`, `Queue`
//! or the render target — the host (SSPlayer / viewer / Bevy) owns those and
//! lends them by reference.

use crate::frame::FrameView;
use std::collections::HashMap;

use ssruntime::runtime::framedata::ss::runtime::{BlendType, DrawBatchKind, root_as_frame_data_unchecked};

/// Per-vertex data uploaded to the GPU: world position, normalized UV, and RGBA.
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
}

#[derive(Clone, Debug)]
struct BatchDrawCall {
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
    pipelines: [wgpu::RenderPipeline; 4],
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,

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

        let create_pipeline = |label: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[Vertex::desc()], compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState { format: config.color_format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
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
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let pipelines = [create_pipeline("SpriteStudio Pipeline Mix", blend_mix), create_pipeline("SpriteStudio Pipeline Add", blend_add), create_pipeline("SpriteStudio Pipeline Sub", blend_sub), create_pipeline("SpriteStudio Pipeline Mul", blend_mul)];

        Self { pipelines, uniform_buffer, uniform_bind_group, texture_bind_group_layout, sampler, vertex_buffer: None, index_buffer: None, draw_calls: Vec::new(), vertices: Vec::new(), indices: Vec::new(), textures: HashMap::new() }
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
    /// interleave instance recursion at the right rank.
    pub fn append_batch(&mut self, frame: &FrameView, transform: &[f32; 16], batch: &BatchInfo) {
        let BatchInfo { start_rank, count, texture_hash, blend_type, kind } = *batch;
        if frame.frame_data.is_empty() {
            return;
        }
        let frame_data = unsafe { root_as_frame_data_unchecked(frame.frame_data) };
        let draw_order = frame_data.draw_order();
        let parts = frame_data.parts();
        let parts_color = frame_data.parts_color();

        let world_matrices = frame.world_matrices;
        let local_vertices = frame.local_vertices;
        let local_uvs = frame.local_uvs;
        if world_matrices.is_empty() || local_vertices.is_empty() || local_uvs.is_empty() {
            return;
        }

        let pipeline_idx = match blend_type {
            BlendType::Mix => 0,
            BlendType::Add => 1,
            BlendType::Sub => 2,
            BlendType::Mul => 3,
            _ => 0,
        };
        let tex_size = self.textures.get(&texture_hash).map(|t| t.size);
        let index_start = self.indices.len() as u32;

        match kind {
            DrawBatchKind::Normal => {
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    let base_vert_idx = self.vertices.len() as u32;
                    let m = &world_matrices[part_idx * 16..(part_idx + 1) * 16];
                    let lv = &local_vertices[part_idx * 10..(part_idx + 1) * 10];
                    let l_uv = &local_uvs[part_idx * 10..(part_idx + 1) * 10];

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

                    for k in 0..4 {
                        let lx = lv[k * 2];
                        let ly = lv[k * 2 + 1];

                        let wx = m[0] * lx + m[4] * ly + m[12];
                        let wy = m[1] * lx + m[5] * ly + m[13];
                        let wz = m[2] * lx + m[6] * ly + m[14];
                        let (fx, fy, fz) = apply_transform(transform, wx, wy, wz);

                        let (tu, tv) = if let Some(size) = tex_size { (l_uv[k * 2] / size.0, l_uv[k * 2 + 1] / size.1) } else { (l_uv[k * 2], l_uv[k * 2 + 1]) };

                        self.vertices.push(Vertex { position: [fx, fy, fz], uv: [tu, tv], color: colors_arr[k] });
                    }

                    self.indices.push(base_vert_idx);
                    self.indices.push(base_vert_idx + 1);
                    self.indices.push(base_vert_idx + 2);
                    self.indices.push(base_vert_idx + 2);
                    self.indices.push(base_vert_idx + 1);
                    self.indices.push(base_vert_idx + 3);
                }
            }
            DrawBatchKind::Mesh => {
                let mesh_vertices_x = frame.mesh_vertices_x;
                let mesh_vertices_y = frame.mesh_vertices_y;
                let mesh_vertex_offsets = frame.mesh_vertex_offsets;
                let mesh_uvs = frame.mesh_uvs;
                let mesh_indices = frame.mesh_indices;
                let mesh_index_offsets = frame.mesh_index_offsets;

                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let part = parts.get(part_idx);

                    if mesh_vertex_offsets.is_empty() || part_idx >= mesh_vertex_offsets.len() - 1 {
                        continue;
                    }

                    let start_vert = mesh_vertex_offsets[part_idx] as usize;
                    let end_vert = mesh_vertex_offsets[part_idx + 1] as usize;
                    if end_vert <= start_vert {
                        continue;
                    }

                    let base_vert_idx = self.vertices.len() as u32;

                    let part_color_idx = part.part_color();
                    let mesh_color = parts_color
                        .filter(|_| part_color_idx >= 0)
                        .map(|pc_vec| {
                            let pc = pc_vec.get(part_color_idx as usize);
                            let c_lt = pc.lt().rgba();
                            [c_lt.r() as f32 / 255.0, c_lt.g() as f32 / 255.0, c_lt.b() as f32 / 255.0, c_lt.a() as f32 / 255.0]
                        })
                        .unwrap_or_else(|| [1.0, 1.0, 1.0, part.alpha()]);

                    for k in start_vert..end_vert {
                        let wx = mesh_vertices_x[k];
                        let wy = mesh_vertices_y[k];
                        let wz = part.position_z();
                        let (fx, fy, fz) = apply_transform(transform, wx, wy, wz);

                        let (tu, tv) = if let Some(size) = tex_size { (mesh_uvs[k * 2] / size.0, mesh_uvs[k * 2 + 1] / size.1) } else { (mesh_uvs[k * 2], mesh_uvs[k * 2 + 1]) };

                        self.vertices.push(Vertex { position: [fx, fy, fz], uv: [tu, tv], color: mesh_color });
                    }

                    let start_idx = mesh_index_offsets[part_idx] as usize;
                    let end_idx = mesh_index_offsets[part_idx + 1] as usize;
                    for &rel_idx in &mesh_indices[start_idx..end_idx] {
                        self.indices.push(base_vert_idx + rel_idx as u32);
                    }
                }
            }
            _ => {}
        }

        let index_end = self.indices.len() as u32;
        if index_end > index_start {
            self.draw_calls.push(BatchDrawCall { pipeline_idx, texture_hash, index_start, index_count: index_end - index_start });
        }
    }

    /// Upload the accumulated geometry to the GPU. Call once after all batches.
    pub fn end_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
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

        for call in &self.draw_calls {
            rpass.set_pipeline(&self.pipelines[call.pipeline_idx]);

            if let Some(entry) = self.textures.get(&call.texture_hash) {
                rpass.set_bind_group(1, &entry.bind_group, &[]);
                rpass.draw_indexed(call.index_start..call.index_start + call.index_count, 0, 0..1);
            }
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
