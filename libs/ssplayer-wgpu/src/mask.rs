//! Mask planning (stencil) — a Rust port of the WebGL `MaskPlanner`.
//!
//! SpriteStudio's "mask" and "clipping mask" both resolve to the same stencil
//! mechanism. The
//! runtime only emits `DrawBatchKind::Mask` batches, `mask_indices` and the
//! per-batch `FLAG_MASK_INFLUENCE` bit; rasterising coverage into a stencil
//! buffer is the Player's job. This module classifies, per frame, which parts
//! write coverage (writers) and how, so [`crate::player`] can drive the
//! pre-write + scoped-replay interleave that a single hardware stencil needs.
//!
//! Data sources:
//! * **Static per-part flags** ([`PartMaskFlags`]) come from ssab `PartData` and
//!   are read once at node-build time.
//! * **Per-frame values** (`PartState.mask`, draw order, batches) come from
//!   `FrameData` and are re-read every frame here.

use crate::frame::{FrameView, SHAPE_VERTICES_STRIDE};
use ssruntime::runtime::framedata::ss::runtime::{DrawBatchKind, root_as_frame_data_unchecked};

/// Upper bound on writers per frame (matches the WebGL planner). A single 8-bit
/// stencil / 24-bit coverage encoding can only distinguish so many writers.
pub const MAX_MASK_WRITERS: usize = 24;

/// Stencil test mode baked into a content pipeline. Selected per masked part
/// from the target's `visible_inside_mask` polarity.
///
/// Hashable because it is half of the renderer's pipeline cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StencilTest {
    /// Not masked — compare `Always` (draw everywhere, ignore stencil).
    None,
    /// `visible_inside_mask == true`: draw where coverage is set (`NotEqual`, 0).
    Inside,
    /// `visible_inside_mask == false`: draw where coverage is clear (`Equal`, 0).
    Outside,
}

/// Stencil write operation, resolved by the caller from a writer's op and
/// whether it is being added or removed from coverage. Increment
/// pairs with decrement (its inverse); invert is its own inverse.
///
/// Hashable because it keys a mask-write pipeline in the renderer cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaskOp {
    /// `stencil + 1` (increment, clamped).
    Incr,
    /// `stencil - 1` (decrement, clamped) — undoes a prior increment.
    Decr,
    /// `~stencil` (bitwise invert) — its own inverse.
    Invert,
}

/// Geometry a part contributes as a mask writer. Text/Nines are not yet
/// rendered, so they cannot write coverage yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GeomKind {
    /// Sprite quad (Normal parts and `PartTypeMask` cells).
    Quad,
    /// Skinned mesh.
    Mesh,
    /// Shape part (rect/circle/…), rasterised from its runtime shape vertices as
    /// a triangle fan. Coverage-only (no colour draw yet).
    Shape,
    /// Not renderable as a writer yet (Text/Nines/Null/etc.).
    #[default]
    Unsupported,
}

/// Static, per-part mask flags read once from ssab `PartData`.
///
/// `mask_influence` is deliberately dual-purpose: for a writer
/// it is the stencil OP (0 = increment, 1 = invert); for a drawn target it is
/// the mask opt-in. Clipping parts (writer *and* target) use the same value for
/// both roles.
#[derive(Clone, Copy, Debug, Default)]
pub struct PartMaskFlags {
    /// Part type is `PartTypeMask` — always a pure (colourless) mask.
    pub is_mask_part: bool,
    /// A drawn part that also writes coverage (a clipping writer).
    pub mask_write: bool,
    /// Dual meaning: OP for writers, mask opt-in for targets.
    pub mask_influence: bool,
    /// Polarity: draw inside (true) or outside (false) the coverage.
    pub visible_inside_mask: bool,
    /// Shape/Text/Nines promoted to a pure mask.
    pub draw_as_mask: bool,
    /// Geometry this part builds when it writes coverage.
    pub geom: GeomKind,
}

impl PartMaskFlags {
    /// A pure (colourless) mask writer.
    pub fn pure_mask(&self) -> bool {
        self.is_mask_part || self.draw_as_mask
    }

    /// Writes coverage into the stencil (pure mask or clipping).
    pub fn is_writer(&self) -> bool {
        self.pure_mask() || self.mask_write
    }

    /// A clipping writer: draws its own colour *and* writes coverage.
    pub fn is_clipping(&self) -> bool {
        self.mask_write && !self.pure_mask()
    }

    /// Stencil test to apply to this part when it is a masked target.
    pub fn polarity(&self) -> StencilTest {
        if self.visible_inside_mask { StencilTest::Inside } else { StencilTest::Outside }
    }
}

/// One planned stencil writer for the current frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct MaskWriter {
    /// Part index (into parts / draw_order / SoA).
    pub part_idx: usize,
    /// Rank in `draw_order`; drives the scope replay (pure = backward scope,
    /// clipping = forward scope).
    pub rank: usize,
    /// Stencil OP: `false` = increment, `true` = invert.
    pub op_invert: bool,
    /// `false` = pure mask (pre-written, removed when the cursor passes its
    /// rank); `true` = clipping (added when the cursor reaches its rank).
    pub is_clipping: bool,
    /// Cutout threshold `(255 - mask)/255`, or `-1.0` for no cutout (textureless
    /// pure masks — the whole quad writes coverage).
    pub threshold: f32,
    /// Cellmap texture for the cutout (0 = none → white fallback, no cutout).
    pub texture_hash: u32,
    /// Geometry to rasterise.
    pub geom: GeomKind,
}

/// A frame's planned writers, held inline.
///
/// The plan is capped at [`MAX_MASK_WRITERS`] and [`plan_writers`] runs once per
/// node per frame, so a `Vec` here spent its life growing from empty to a bound
/// it already knew — six reallocations to reach a buffer under a kilobyte, on
/// every masking node of every frame. A fixed array costs the same bytes and
/// none of the allocator. Everything that reads a plan reads it as a slice
/// (`Deref`), which is what it read when this was a `Vec`.
#[derive(Clone, Copy, Debug)]
pub struct MaskWriters {
    writers: [MaskWriter; MAX_MASK_WRITERS],
    len: usize,
}

// Written out rather than derived: the array `Default` impls stop at 32, and the
// cap is this module's to choose.
impl Default for MaskWriters {
    fn default() -> Self {
        Self { writers: [MaskWriter::default(); MAX_MASK_WRITERS], len: 0 }
    }
}

impl MaskWriters {
    /// Append a writer.
    ///
    /// Panics past [`MAX_MASK_WRITERS`]. Respecting the cap is the planner's
    /// job — it stops walking batches at it — so arriving here full is a bug in
    /// the planner, and a writer dropped quietly instead would clip the frame
    /// wrong with nothing left to point at.
    pub fn push(&mut self, writer: MaskWriter) {
        self.writers[self.len] = writer;
        self.len += 1;
    }
}

impl std::ops::Deref for MaskWriters {
    type Target = [MaskWriter];
    fn deref(&self) -> &[MaskWriter] {
        &self.writers[..self.len]
    }
}

/// So the draw walk can retarget each writer's cutout at the renderer key its
/// node's pack resolved to — see `AtlasKeys`. Planning cannot do it: a writer
/// takes its `texture_hash` from the batch, which speaks in cellmap hashes.
impl std::ops::DerefMut for MaskWriters {
    fn deref_mut(&mut self) -> &mut [MaskWriter] {
        &mut self.writers[..self.len]
    }
}

impl<'a> IntoIterator for &'a MaskWriters {
    type Item = &'a MaskWriter;
    type IntoIter = std::slice::Iter<'a, MaskWriter>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// The player-local rectangle a frame's writers cover, as `[min_x, min_y,
/// max_x, max_y]`, or `None` when they cover nothing.
///
/// This is the rectangle the coverage bitmap is stretched over:
/// normalising it to `[0,1]` is what makes the bitmap's resolution independent
/// of how big the mask is on screen, and what lets one affine serve both the
/// bake and the replay.
///
/// **Player-local, not world**: the points are taken after each part's own world
/// matrix but before the player's placement, so moving or scaling the player
/// does not move the bitmap under its own coverage.
pub fn writer_bounds(frame: &FrameView, writers: &[MaskWriter]) -> Option<[f32; 4]> {
    let mut bounds = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    let mut any = false;

    let mut include = |x: f32, y: f32| {
        // A NaN from a degenerate matrix must not poison the whole rectangle.
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        bounds[0] = bounds[0].min(x);
        bounds[1] = bounds[1].min(y);
        bounds[2] = bounds[2].max(x);
        bounds[3] = bounds[3].max(y);
        any = true;
    };

    for writer in writers {
        let part = writer.part_idx;
        match writer.geom {
            GeomKind::Quad => {
                let (m, v) = (part * 16, part * 10);
                if m + 16 > frame.world_matrices.len() || v + 10 > frame.local_vertices.len() {
                    continue;
                }
                let mat = &frame.world_matrices[m..m + 16];
                // The four corners; the fifth pair is the centre, which is inside
                // them and cannot widen the rectangle.
                for k in 0..4 {
                    let (lx, ly) = (frame.local_vertices[v + k * 2], frame.local_vertices[v + k * 2 + 1]);
                    include(mat[0] * lx + mat[4] * ly + mat[12], mat[1] * lx + mat[5] * ly + mat[13]);
                }
            }
            GeomKind::Mesh => {
                let offsets = &frame.mesh_vertex_offsets;
                if offsets.is_empty() || part >= offsets.len() - 1 {
                    continue;
                }
                let (start, end) = (offsets[part] as usize, offsets[part + 1] as usize);
                if end > frame.mesh_vertices_x.len() || end > frame.mesh_vertices_y.len() {
                    continue;
                }
                // Mesh vertices arrive already skinned into player-local space.
                for i in start..end {
                    include(frame.mesh_vertices_x[i], frame.mesh_vertices_y[i]);
                }
            }
            GeomKind::Shape => {
                let count = frame.shape_vertex_counts.get(part).copied().unwrap_or(0).max(0) as usize;
                let (m, base) = (part * 16, part * SHAPE_VERTICES_STRIDE);
                if count == 0 || m + 16 > frame.world_matrices.len() || base + count * 2 > frame.shape_vertices.len() {
                    continue;
                }
                let mat = &frame.world_matrices[m..m + 16];
                for k in 0..count {
                    let (lx, ly) = (frame.shape_vertices[base + k * 2], frame.shape_vertices[base + k * 2 + 1]);
                    include(mat[0] * lx + mat[4] * ly + mat[12], mat[1] * lx + mat[5] * ly + mat[13]);
                }
            }
            GeomKind::Unsupported => {}
        }
    }

    any.then_some(bounds)
}

/// Plan the frame's stencil writers by walking `draw_batches`, mirroring the
/// WebGL `MaskPlanner.plan`. `mask_flags` is indexed by part index (static
/// ssab data). Returns writers in ascending rank order, capped at
/// `MAX_MASK_WRITERS` (this module's cap on writers per frame).
///
/// A writer's `texture_hash` comes from the *batch* it belongs to (like WebGL):
/// pure `Mask` batches carry `texture_hash == 0` (→ no cutout, whole geometry),
/// while clipping writers inherit their content batch's real texture.
pub fn plan_writers(frame: &FrameView, mask_flags: &[PartMaskFlags]) -> MaskWriters {
    let mut writers = MaskWriters::default();
    if frame.frame_data.is_empty() {
        return writers;
    }
    // SAFETY: `frame_data` is a valid FlatBuffers `FrameData` produced by the
    // runtime for this frame (same contract as the render loop).
    let fd = unsafe { root_as_frame_data_unchecked(frame.frame_data) };
    let draw_order = fd.draw_order();
    let parts = fd.parts();

    'batches: for batch in fd.draw_batches() {
        let kind = batch.kind();
        let start_rank = batch.start_rank() as usize;
        let count = batch.count() as usize;
        let texture_hash = batch.texture_hash();

        let is_mask_batch = kind == DrawBatchKind::Mask;

        for rank in start_rank..start_rank + count {
            let part_idx = draw_order.get(rank) as usize;
            let flags = mask_flags.get(part_idx).copied().unwrap_or_default();

            // Only pure-mask batches and clipping content parts write coverage.
            let is_pure = is_mask_batch; // Mask batches are always pure writers.
            let is_clipping = !is_mask_batch && flags.is_clipping();
            if !is_pure && !is_clipping {
                continue;
            }

            // Per-frame cutout value; `mask == 0` opts the writer out.
            let mask_val = parts.get(part_idx).mask();
            if mask_val <= 0.0 {
                continue;
            }

            // texture 0 (pure masks / shapes) → no cutout.
            let threshold = if texture_hash != 0 { (255.0 - mask_val) / 255.0 } else { -1.0 };

            writers.push(MaskWriter { part_idx, rank, op_invert: flags.mask_influence, is_clipping, threshold, texture_hash, geom: flags.geom });

            if writers.len() >= MAX_MASK_WRITERS {
                break 'batches;
            }
        }
    }

    writers
}
