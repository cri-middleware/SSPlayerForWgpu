//! Headless probe for mask planning (no wgpu). Loads `Mask.ssab`, builds the
//! static per-part mask flags exactly like `build_node`, then runs
//! `plan_writers` over the animation to confirm the runtime emits mask writers
//! and that pure-mask parts produce `Mask` batches (no colour draw).

use ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked};
use ssplayer_wgpu::{FrameView, GeomKind, PartMaskFlags, plan_writers};
use ssruntime::runtime::framedata::ss::runtime::{DrawBatchKind, root_as_frame_data_unchecked};
use ssruntime::{Context, SsabData, SsabResource};
use std::sync::Arc;

fn load(path: &str) -> Arc<SsabResource> {
    let bytes = std::fs::read(path).unwrap_or_else(|_| panic!("read {path}"));
    // SAFETY: the converter wrote this pack from the suite's own fixtures, so it is a valid `.ssab`.
    unsafe { SsabResource::new(SsabData::load_copy(&bytes).unwrap()) }
}

/// Build static per-part mask flags from ssab `PartData` (mirrors `build_node`).
fn mask_flags(ctx: &Context<'static>) -> Vec<PartMaskFlags> {
    let Some(bytes) = ctx.get_ssab_data() else { return Vec::new() };
    let ab = unsafe { root_as_ss_anime_binary_unchecked(bytes) };
    let parts = ab.parts();
    let mut flags = vec![PartMaskFlags::default(); parts.len()];
    for (i, part) in parts.iter().enumerate() {
        let part_type = part.part_type_type();
        let geom = match part_type {
            PartType::PartTypeNormal | PartType::PartTypeMask => GeomKind::Quad,
            PartType::PartTypeMesh => GeomKind::Mesh,
            PartType::PartTypeShape => GeomKind::Shape,
            _ => GeomKind::Unsupported,
        };
        flags[i] = PartMaskFlags { is_mask_part: part_type == PartType::PartTypeMask, mask_write: part.mask_write(), mask_influence: part.mask_influence(), visible_inside_mask: part.visible_inside_mask(), draw_as_mask: part.draw_as_mask(), geom };
    }
    flags
}

#[test]
fn probe_mask() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/overall");
    let res = load(&format!("{dir}/Mask.ssab"));
    let names = res.resource().map(|r| r.get_animation_names()).unwrap_or_default();
    eprintln!("=== Mask.ssab animations: {names:?} ===");
    let Some(anim) = names.first().cloned() else {
        eprintln!("no animation, skipping");
        return;
    };

    let mut ctx = Context::new();
    ctx.bind_resource(unsafe { &*res.resource_static_ptr() });
    if ctx.setup_animation(&anim).is_err() {
        eprintln!("setup failed, skipping");
        return;
    }
    ctx.play();

    let flags = mask_flags(&ctx);
    let writer_capable = flags.iter().filter(|f| f.is_writer()).count();
    // Mask.ssab's "Shape-Mask-True" is a Shape part used as a mask (draw_as_mask):
    // it must be classified as Shape geometry, not silently unsupported.
    let shape_mask_capable = flags.iter().filter(|f| f.draw_as_mask && f.geom == GeomKind::Shape).count();
    let target_capable = flags.iter().filter(|f| f.mask_influence && !f.is_writer()).count();
    eprintln!("parts={} writer_capable={writer_capable} target_capable={target_capable}", flags.len());
    for (i, f) in flags.iter().enumerate() {
        if f.is_writer() || f.mask_influence {
            eprintln!("  part {i}: {f:?}");
        }
    }

    let dt = 1.0 / 60.0;
    let mut max_writers = 0usize;
    let mut saw_mask_batch = false;
    let mut saw_masked_content = false;
    let mut saw_shape_writer = false;
    for f in 0..120 {
        ctx.update(dt * 1000.0);
        let pf = ctx.get_frame_no();
        let Ok(bytes) = ctx.get_frame_data(pf) else { continue };
        // plan_writers only reads draw_order / parts / draw_batches.
        let frame = FrameView { frame_data: unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) }, ..Default::default() };
        let writers = plan_writers(&frame, &flags);
        max_writers = max_writers.max(writers.len());
        if writers.iter().any(|w| w.geom == GeomKind::Shape) {
            saw_shape_writer = true;
        }

        let fd = unsafe { root_as_frame_data_unchecked(frame.frame_data) };
        let mut mask_batches = 0;
        let mut masked_content = 0;
        for b in fd.draw_batches() {
            if b.kind() == DrawBatchKind::Mask {
                mask_batches += 1;
                saw_mask_batch = true;
            }
            if (b.flags() & 0x01) != 0 && matches!(b.kind(), DrawBatchKind::Normal | DrawBatchKind::Mesh) {
                masked_content += 1;
                saw_masked_content = true;
            }
        }
        if f < 8 || !writers.is_empty() {
            eprintln!("f{f:03} pf={pf:6.1} writers={} (pure={}, clip={}) mask_batches={mask_batches} masked_content={masked_content}", writers.len(), writers.iter().filter(|w| !w.is_clipping).count(), writers.iter().filter(|w| w.is_clipping).count());
        }
    }

    eprintln!("max_writers={max_writers} saw_mask_batch={saw_mask_batch} saw_masked_content={saw_masked_content} saw_shape_writer={saw_shape_writer}");

    // Mask.ssab is a real mask sample: if the pack declares mask writers, the
    // runtime must actually emit coverage on some frame.
    if writer_capable > 0 {
        assert!(saw_mask_batch, "Mask.ssab declares mask writers but emitted no Mask draw batches");
        assert!(max_writers > 0, "Mask.ssab declares mask writers but plan_writers found none across the animation");
    }
    // The "Shape-Mask-True" part must rasterise as Shape coverage, not be skipped.
    if shape_mask_capable > 0 {
        assert!(saw_shape_writer, "a Shape mask part was declared but produced no Shape-geometry writer");
    }
}

/// `plan_writers` takes any bytes: what is not a FrameData plans no writers,
/// rather than being read as one.
#[test]
fn bytes_that_are_not_a_frame_plan_no_writers() {
    for bytes in [&[][..], &[0xff; 64][..], &[4, 0, 0, 0, 0xff, 0xff, 0xff, 0x7f][..]] {
        let frame = FrameView { frame_data: bytes, ..Default::default() };
        assert!(plan_writers(&frame, &[]).is_empty(), "{bytes:?} planned writers");
    }
}
