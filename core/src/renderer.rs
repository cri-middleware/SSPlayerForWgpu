use crate::player::Player;

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

#[derive(Clone, Debug)]
pub struct BatchDrawCall {
    pub pipeline_idx: usize,
    pub texture_hash: u32,
    pub index_start: u32,
    pub index_count: u32,
}

pub struct Renderer {
    pipelines: [wgpu::RenderPipeline; 4],
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,

    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
    draw_calls: Vec<BatchDrawCall>,
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

impl Renderer {
    pub fn new(device: &wgpu::Device, texture_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("SpriteStudio Shader"), source: wgpu::ShaderSource::Wgsl(WGSL_SHADER.into()) });

        // Uniform Buffer Setup
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

        // Texture Bind Group Layout
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

        // Define blend states for 4 modes
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
                    targets: &[Some(wgpu::ColorTargetState { format: texture_format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
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

        Self { pipelines, uniform_buffer, uniform_bind_group, texture_bind_group_layout, sampler, vertex_buffer: None, index_buffer: None, draw_calls: Vec::new() }
    }

    pub fn update_projection(&self, queue: &wgpu::Queue, width: f32, height: f32, y_down: bool) {
        let scale_y = if y_down { -2.0 / height } else { 2.0 / height };
        let matrix = [2.0 / width, 0.0, 0.0, 0.0, 0.0, scale_y, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&matrix));
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, player: &mut Player, get_texture_size: impl Fn(u32) -> Option<(f32, f32)>) {
        self.draw_calls.clear();

        let frame_no = player.get_frame_no();
        let context = &mut player.runtime_context;

        let frame_data_bytes = match context.get_frame_data(frame_no) {
            Ok(bytes) => unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) },
            Err(_) => return,
        };

        let frame_data = unsafe { ssruntime::runtime::framedata::ss::runtime::root_as_frame_data_unchecked(frame_data_bytes) };

        let draw_order = frame_data.draw_order();
        let draw_batches = frame_data.draw_batches();
        let parts = frame_data.parts();
        let parts_color = frame_data.parts_color();

        let world_matrices_len = context.get_world_matrices_len();
        let world_matrices_ptr = context.get_world_matrices_ptr();
        if world_matrices_ptr.is_null() || world_matrices_len == 0 {
            return;
        }
        let world_matrices = unsafe { std::slice::from_raw_parts(world_matrices_ptr, world_matrices_len) };

        let local_vertices_len = context.get_local_vertices_len();
        let local_vertices_ptr = context.get_local_vertices_ptr();
        if local_vertices_ptr.is_null() || local_vertices_len == 0 {
            return;
        }
        let local_vertices = unsafe { std::slice::from_raw_parts(local_vertices_ptr, local_vertices_len) };

        let local_uvs_len = context.get_local_uvs_len();
        let local_uvs_ptr = context.get_local_uvs_ptr();
        if local_uvs_ptr.is_null() || local_uvs_len == 0 {
            return;
        }
        let local_uvs = unsafe { std::slice::from_raw_parts(local_uvs_ptr, local_uvs_len) };

        let mesh_vertices_x_len = context.get_mesh_vertices_x_len();
        let mesh_vertices_x_ptr = context.get_mesh_vertices_x_ptr();
        let mesh_vertices_x = if mesh_vertices_x_ptr.is_null() || mesh_vertices_x_len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(mesh_vertices_x_ptr, mesh_vertices_x_len) } };

        let mesh_vertices_y_len = context.get_mesh_vertices_y_len();
        let mesh_vertices_y_ptr = context.get_mesh_vertices_y_ptr();
        let mesh_vertices_y = if mesh_vertices_y_ptr.is_null() || mesh_vertices_y_len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(mesh_vertices_y_ptr, mesh_vertices_y_len) } };

        let mesh_vertex_offsets_len = context.get_mesh_vertex_offsets_len();
        let mesh_vertex_offsets_ptr = context.get_mesh_vertex_offsets_ptr();
        let mesh_vertex_offsets = if mesh_vertex_offsets_ptr.is_null() || mesh_vertex_offsets_len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(mesh_vertex_offsets_ptr, mesh_vertex_offsets_len) } };

        let mesh_uvs_len = context.get_mesh_uvs_len();
        let mesh_uvs_ptr = context.get_mesh_uvs_ptr();
        let mesh_uvs = if mesh_uvs_ptr.is_null() || mesh_uvs_len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(mesh_uvs_ptr, mesh_uvs_len) } };

        let mesh_indices_len = context.get_mesh_indices_len();
        let mesh_indices_ptr = context.get_mesh_indices_ptr();
        let mesh_indices = if mesh_indices_ptr.is_null() || mesh_indices_len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(mesh_indices_ptr, mesh_indices_len) } };

        let mesh_index_offsets_len = context.get_mesh_index_offsets_len();
        let mesh_index_offsets_ptr = context.get_mesh_index_offsets_ptr();
        let mesh_index_offsets = if mesh_index_offsets_ptr.is_null() || mesh_index_offsets_len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(mesh_index_offsets_ptr, mesh_index_offsets_len) } };

        let mut vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        for batch in draw_batches {
            let start_rank = batch.start_rank() as usize;
            let count = batch.count() as usize;
            let texture_hash = batch.texture_hash();
            let blend_type = batch.blend_type();
            let kind = batch.kind();

            let pipeline_idx = match blend_type {
                ssruntime::runtime::framedata::ss::runtime::BlendType::Mix => 0,
                ssruntime::runtime::framedata::ss::runtime::BlendType::Add => 1,
                ssruntime::runtime::framedata::ss::runtime::BlendType::Sub => 2,
                ssruntime::runtime::framedata::ss::runtime::BlendType::Mul => 3,
                _ => 0,
            };

            let index_start = indices.len() as u32;

            match kind {
                ssruntime::runtime::framedata::ss::runtime::DrawBatchKind::Normal => {
                    for rank in start_rank..start_rank + count {
                        let part_idx = draw_order.get(rank) as usize;
                        let part = parts.get(part_idx);

                        let base_vert_idx = vertices.len() as u32;
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

                            let (tu, tv) = if let Some(tex_size) = get_texture_size(texture_hash) {
                                (l_uv[k * 2] / tex_size.0, l_uv[k * 2 + 1] / tex_size.1)
                            } else {
                                (l_uv[k * 2], l_uv[k * 2 + 1])
                            };

                            vertices.push(Vertex { position: [wx, wy, wz], uv: [tu, tv], color: colors_arr[k] });
                        }

                        indices.push(base_vert_idx);
                        indices.push(base_vert_idx + 1);
                        indices.push(base_vert_idx + 2);
                        indices.push(base_vert_idx + 2);
                        indices.push(base_vert_idx + 1);
                        indices.push(base_vert_idx + 3);
                    }
                }
                ssruntime::runtime::framedata::ss::runtime::DrawBatchKind::Mesh => {
                    for rank in start_rank..start_rank + count {
                        let part_idx = draw_order.get(rank) as usize;
                        let part = parts.get(part_idx);

                        if part_idx >= mesh_vertex_offsets.len() - 1 {
                            continue;
                        }

                        let start_vert = mesh_vertex_offsets[part_idx] as usize;
                        let end_vert = mesh_vertex_offsets[part_idx + 1] as usize;
                        if end_vert <= start_vert {
                            continue;
                        }

                        let base_vert_idx = vertices.len() as u32;

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

                            let (tu, tv) = if let Some(tex_size) = get_texture_size(texture_hash) { (mesh_uvs[k * 2] / tex_size.0, mesh_uvs[k * 2 + 1] / tex_size.1) } else { (mesh_uvs[k * 2], mesh_uvs[k * 2 + 1]) };

                            vertices.push(Vertex { position: [wx, wy, wz], uv: [tu, tv], color: mesh_color });
                        }

                        let start_idx = mesh_index_offsets[part_idx] as usize;
                        let end_idx = mesh_index_offsets[part_idx + 1] as usize;
                        for &rel_idx in &mesh_indices[start_idx..end_idx] {
                            indices.push(base_vert_idx + rel_idx as u32);
                        }
                    }
                }
                _ => {}
            }

            let index_end = indices.len() as u32;
            if index_end > index_start {
                self.draw_calls.push(BatchDrawCall { pipeline_idx, texture_hash, index_start, index_count: index_end - index_start });
            }
        }

        if vertices.is_empty() || indices.is_empty() {
            return;
        }

        let vertex_bytes = bytemuck::cast_slice(&vertices);
        self.ensure_vertex_buffer(device, vertex_bytes.len());
        if let Some(ref buffer) = self.vertex_buffer {
            queue.write_buffer(buffer, 0, vertex_bytes);
        }

        let index_bytes = bytemuck::cast_slice(&indices);
        self.ensure_index_buffer(device, index_bytes.len());
        if let Some(ref buffer) = self.index_buffer {
            queue.write_buffer(buffer, 0, index_bytes);
        }
    }

    pub fn render<'a>(&'a self, rpass: &mut wgpu::RenderPass<'a>, get_texture_bind_group: impl Fn(u32) -> Option<&'a wgpu::BindGroup>) -> usize {
        if self.draw_calls.is_empty() {
            return 0;
        }

        let v_buf = match &self.vertex_buffer {
            Some(b) => b,
            None => return 0,
        };
        let i_buf = match &self.index_buffer {
            Some(b) => b,
            None => return 0,
        };

        rpass.set_vertex_buffer(0, v_buf.slice(..));
        rpass.set_index_buffer(i_buf.slice(..), wgpu::IndexFormat::Uint32);
        rpass.set_bind_group(0, &self.uniform_bind_group, &[]);

        for call in &self.draw_calls {
            let pipeline = &self.pipelines[call.pipeline_idx];
            rpass.set_pipeline(pipeline);

            if let Some(tex_bg) = get_texture_bind_group(call.texture_hash) {
                rpass.set_bind_group(1, tex_bg, &[]);
                rpass.draw_indexed(call.index_start..call.index_start + call.index_count, 0, 0..1);
            }
        }

        self.draw_calls.len()
    }

    pub fn create_texture_bind_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
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
