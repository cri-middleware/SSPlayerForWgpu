//! Mask planning (stencil) — a Rust port of the WebGL `MaskPlanner`.
//!
//! SpriteStudio's "mask" and "clipping mask" both resolve to the same stencil
//! mechanism (see `libs/ssruntime/docs/40_components/40_mask.ja.md`). The
//! runtime only emits `DrawBatchKind::Mask` batches, `mask_indices` and the
//! per-batch `FLAG_MASK_INFLUENCE` bit; rasterising coverage into a stencil
//! buffer is the Player's job. This module classifies, per frame, which parts
//! write coverage (writers) and how, so [`crate::player`] can drive the
//! pre-write + scoped-replay interleave that a single hardware stencil needs.
//!
//! Data sources (SPEC §6, mask doc §1):
//! * **Static per-part flags** ([`PartMaskFlags`]) come from ssab `PartData` and
//!   are read once at node-build time.
//! * **Per-frame values** (`PartState.mask`, draw order, batches) come from
//!   `FrameData` and are re-read every frame here.

use crate::frame::FrameView;
use ssruntime::runtime::framedata::ss::runtime::{DrawBatchKind, root_as_frame_data_unchecked};

/// Upper bound on writers per frame (matches the WebGL planner). A single 8-bit
/// stencil / 24-bit coverage encoding can only distinguish so many writers.
pub const MAX_MASK_WRITERS: usize = 24;

/// Stencil test mode baked into a content pipeline. Selected per masked part
/// from the target's `visible_inside_mask` polarity (mask doc §2-4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StencilTest {
    /// Not masked — compare `Always` (draw everywhere, ignore stencil).
    None,
    /// `visible_inside_mask == true`: draw where coverage is set (`NotEqual`, 0).
    Inside,
    /// `visible_inside_mask == false`: draw where coverage is clear (`Equal`, 0).
    Outside,
}

/// Stencil write operation, resolved by the caller from a writer's op and
/// whether it is being added or removed from coverage (mask doc §2-3). Increment
/// pairs with decrement (its inverse); invert is its own inverse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskOp {
    /// `stencil + 1` (increment, clamped).
    Incr,
    /// `stencil - 1` (decrement, clamped) — undoes a prior increment.
    Decr,
    /// `~stencil` (bitwise invert) — its own inverse.
    Invert,
}

/// Geometry a part contributes as a mask writer. Text/Nines are not yet
/// rendered (P6), so they cannot write coverage yet.
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
/// `mask_influence` is deliberately dual-purpose (mask doc §1-2): for a writer
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
#[derive(Clone, Copy, Debug)]
pub struct MaskWriter {
    /// Part index (into parts / draw_order / SoA).
    pub part_idx: usize,
    /// Rank in `draw_order`; drives the scope replay (pure = backward scope,
    /// clipping = forward scope, mask doc §2-1).
    pub rank: usize,
    /// Stencil OP: `false` = increment, `true` = invert (mask doc §2-3).
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

/// Plan the frame's stencil writers by walking `draw_batches`, mirroring the
/// WebGL `MaskPlanner.plan`. `mask_flags` is indexed by part index (static
/// ssab data). Returns writers in ascending rank order, capped at
/// [`MAX_MASK_WRITERS`].
///
/// A writer's `texture_hash` comes from the *batch* it belongs to (like WebGL):
/// pure `Mask` batches carry `texture_hash == 0` (→ no cutout, whole geometry),
/// while clipping writers inherit their content batch's real texture.
pub fn plan_writers(frame: &FrameView, mask_flags: &[PartMaskFlags]) -> Vec<MaskWriter> {
    let mut writers = Vec::new();
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

            // Per-frame cutout value; `mask == 0` opts the writer out (§2-2).
            let mask_val = parts.get(part_idx).mask();
            if mask_val <= 0.0 {
                continue;
            }

            // texture 0 (pure masks / shapes) → no cutout (§2-5).
            let threshold = if texture_hash != 0 { (255.0 - mask_val) / 255.0 } else { -1.0 };

            writers.push(MaskWriter { part_idx, rank, op_invert: flags.mask_influence, is_clipping, threshold, texture_hash, geom: flags.geom });

            if writers.len() >= MAX_MASK_WRITERS {
                break 'batches;
            }
        }
    }

    writers
}
