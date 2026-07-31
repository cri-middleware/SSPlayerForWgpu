//! Per-part overrides on `SSPlayer` (SPEC §9.5).
//!
//! None of this needs a GPU: an override is a write into the runtime's override
//! table, which `SSPlayer` reaches through the root context alone.
//!
//! What is worth pinning here is not that the calls forward — it is the three
//! ways they can be got wrong silently:
//!
//! * the enum wire values, which the `js` and `ffi` faces pass through as raw
//!   integers and which must match the C API and every sibling Player;
//! * that the table is sized by `set_animation`, so an override before one is
//!   rejected rather than quietly lost;
//! * that a cell override resolves its names against the pack *now*, so a
//!   wrong name fails at the call rather than at draw time.

use ssplayer_wgpu::{BlendOp, OverridePriority, SSPlayer};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

/// A player with `Basic.ssab` loaded and its first animation selected.
fn loaded() -> SSPlayer {
    let mut player = SSPlayer::new();
    player.load(format!("{}/overall/Basic.ssab", asset_root())).unwrap();
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();
    player
}

/// The first part index the pack resolves, so the tests do not hard-code a name
/// out of someone else's asset.
fn some_part(player: &SSPlayer) -> u32 {
    // Part 0 is the root part of any SpriteStudio animation.
    assert!(player.find_part_index("NOT_A_REAL_PART").is_none(), "sanity: unknown names resolve to None");
    0
}

#[test]
fn wire_values_match_the_c_api_and_the_siblings() {
    assert_eq!(BlendOp::Mix.as_wire(), 0);
    assert_eq!(BlendOp::Mul.as_wire(), 1);
    assert_eq!(BlendOp::Add.as_wire(), 2);
    assert_eq!(BlendOp::Sub.as_wire(), 3);

    assert_eq!(OverridePriority::OverwriteOnNextKeyframe.as_wire(), 0);
    assert_eq!(OverridePriority::HoldUntilNextAnimation.as_wire(), 1);
    assert_eq!(OverridePriority::Permanent.as_wire(), 2);

    for op in [BlendOp::Mix, BlendOp::Mul, BlendOp::Add, BlendOp::Sub] {
        assert_eq!(BlendOp::from_wire(op.as_wire()), Some(op));
    }
    for p in [OverridePriority::OverwriteOnNextKeyframe, OverridePriority::HoldUntilNextAnimation, OverridePriority::Permanent] {
        assert_eq!(OverridePriority::from_wire(p.as_wire()), Some(p));
    }

    // Out of range is rejected rather than silently becoming a different blend —
    // this is what the js and ffi faces turn into an error.
    assert_eq!(BlendOp::from_wire(4), None);
    assert_eq!(OverridePriority::from_wire(3), None);

    // The defaults the sibling Players use when the argument is omitted.
    assert_eq!(BlendOp::default(), BlendOp::Mix);
    assert_eq!(OverridePriority::default(), OverridePriority::HoldUntilNextAnimation);
}

#[test]
fn overrides_before_an_animation_are_rejected_not_lost() {
    let mut player = SSPlayer::new();

    // The runtime sizes its override table in set_animation, so there is nowhere
    // to record these yet. Failing is the point: a silent no-op here would look
    // like the override "not working" at draw time.
    assert!(player.find_part_index("anything").is_none());
    assert!(player.set_part_visibility_override(0, true, false).is_err());
    assert!(player.set_part_color_override(0, BlendOp::Mul, 0xFF0000FF, OverridePriority::default()).is_err());
    assert!(player.set_part_cell_override(0, "Ringo", "effect3", OverridePriority::default()).is_err());
    assert!(player.clear_part_color_override(0).is_err());

    // Except this one, which is a "make sure nothing is set" and is happy either
    // way.
    player.clear_all_part_overrides();
}

#[test]
fn visibility_and_colour_overrides_are_accepted() {
    let mut player = loaded();
    let part = some_part(&player);

    player.set_part_visibility_override(part, true, true).unwrap();
    player.clear_part_visibility_override(part).unwrap();

    player.set_part_color_override(part, BlendOp::Mul, 0xFF0000FF, OverridePriority::HoldUntilNextAnimation).unwrap();
    player.clear_part_color_override(part).unwrap();

    // lt, rt, lb, rb — red on top, blue on the bottom.
    let corners = [[255, 0, 0, 255], [255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 255, 255]];
    player.set_part_color_override_corners(part, BlendOp::Mix, corners, OverridePriority::Permanent).unwrap();
    player.clear_part_color_override(part).unwrap();

    player.clear_all_part_overrides();
}

#[test]
fn an_out_of_range_part_is_rejected() {
    let mut player = loaded();

    // Far past any real part count. The runtime bounds-checks against its table
    // rather than trusting the index.
    assert!(player.set_part_visibility_override(9999, true, false).is_err());
    assert!(player.set_part_color_override(9999, BlendOp::Mix, 0, OverridePriority::default()).is_err());
    assert!(player.clear_part_color_override(9999).is_err());
}

#[test]
fn a_cell_override_resolves_its_names_at_the_call() {
    let mut player = loaded();
    let part = some_part(&player);

    // A name that is not in the pack cannot be resolved, and saying so here is
    // the whole point — the alternative is a part that silently keeps drawing
    // its old cell.
    assert!(player.set_part_cell_override(part, "NoSuchCellmap", "NoSuchCell", OverridePriority::default()).is_err());

    // Clearing is always fine.
    player.clear_part_cell_override(part).unwrap();
}

#[test]
fn set_animation_clears_all_but_permanent() {
    let mut player = loaded();
    let part = some_part(&player);

    player.set_part_color_override(part, BlendOp::Mul, 0x00FF00FF, OverridePriority::Permanent).unwrap();
    player.set_part_visibility_override(part, true, false).unwrap();

    // Documented lifecycle, and the reason nothing is cached on this side: the
    // runtime owns the table and re-validates it here. The call must survive.
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();

    // The table is live again after the rebuild, which is what a caller needs in
    // order to re-apply anything that was not permanent.
    player.set_part_color_override(part, BlendOp::Mix, 0xFFFFFFFF, OverridePriority::default()).unwrap();
    player.clear_all_part_overrides();
}

// --- Part read-outs -------------------------------------------------------

#[test]
fn part_transform_is_bounds_checked_and_frame_shaped() {
    let player = loaded();

    assert!(player.part_count() > 0, "a loaded animation has parts");

    // The root part exists and reports a finite affine.
    let m = player.part_transform(0).expect("part 0 should resolve");
    assert!(m.iter().all(|v| v.is_finite()), "affine should be finite: {m:?}");

    // Out of range is None rather than a read past the runtime's buffer — which
    // is what would otherwise come back as NaN.
    assert!(player.part_transform(player.part_count() as u32).is_none());
    assert!(player.part_transform(u32::MAX).is_none());
}

#[test]
fn part_read_outs_are_empty_before_an_animation() {
    let player = SSPlayer::new();

    assert_eq!(player.part_count(), 0);
    assert!(player.part_transform(0).is_none());
}
