//! A borrowed view of one frame's rendering input.
//!
//! The runtime owns the FrameData and SoA buffers; this only references them.
//! The slices are valid immediately after the runtime produces the frame and
//! may be invalidated by the next frame fetch, so do not hold them across
//! frames.

/// Everything [`crate::SSScene`] needs to draw one frame, as borrows into
/// runtime-owned data.
#[derive(Clone, Copy, Default)]
pub struct FrameView<'a> {
    /// FlatBuffers-encoded `FrameData` (draw_order / draw_batches / parts / parts_color).
    pub frame_data: &'a [u8],
    /// `part_index * 16`, column-major 4x4 world matrix per part.
    pub world_matrices: &'a [f32],
    /// `part_index * 10` = `[lt, rt, lb, rb, center]` * xy (pivot/flip/deform/coordinate-flip applied).
    pub local_vertices: &'a [f32],
    /// `part_index * 10` (same layout as vertices; UVs in texture pixel space).
    pub local_uvs: &'a [f32],
    /// Skinned mesh vertex X, flat (sliced by `mesh_vertex_offsets`).
    pub mesh_vertices_x: &'a [f32],
    /// Skinned mesh vertex Y.
    pub mesh_vertices_y: &'a [f32],
    /// Prefix-sum of length parts+1; part `i` spans `[offsets[i], offsets[i+1])`.
    pub mesh_vertex_offsets: &'a [u32],
    /// Mesh UVs (vertex index * 2, texture pixel space).
    pub mesh_uvs: &'a [f32],
    /// Mesh indices (part-relative; rebased with `base_vert_idx + rel_idx`).
    pub mesh_indices: &'a [i32],
    /// Prefix-sum of length parts+1 for mesh index ranges.
    pub mesh_index_offsets: &'a [u32],
    /// Shape vertices (local xy), `part_index * SHAPE_VERTICES_STRIDE` (=24);
    /// each part's live vertex count is in `shape_vertex_counts`.
    pub shape_vertices: &'a [f32],
    /// Per-part shape vertex box coordinates (fx, fy in 0..1), same stride as
    /// `shape_vertices`. Used to bilinearly interpolate the shape's fill color.
    pub shape_vertex_box_coords: &'a [f32],
    /// Per-part shape vertex count (0 when the part is not a shape).
    pub shape_vertex_counts: &'a [i32],
}
