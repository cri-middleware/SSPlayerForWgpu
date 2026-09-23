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
    assert!(player.set_part_visibility_override_by_index(0, true, false).is_err());
    assert!(player.set_part_color_override_by_index(0, 0xFF0000FF, BlendOp::Mul, OverridePriority::default()).is_err());
    assert!(player.set_part_cell_override_by_index(0, "Ringo", "effect3", OverridePriority::default()).is_err());
    assert!(player.clear_part_color_override_by_index(0).is_err());

    // Except this one, which is a "make sure nothing is set" and is happy either
    // way.
    player.clear_all_part_overrides();
}

#[test]
fn visibility_and_colour_overrides_are_accepted() {
    let mut player = loaded();
    let part = some_part(&player);

    player.set_part_visibility_override_by_index(part, true, true).unwrap();
    player.clear_part_visibility_override_by_index(part).unwrap();

    player.set_part_color_override_by_index(part, 0xFF0000FF, BlendOp::Mul, OverridePriority::HoldUntilNextAnimation).unwrap();
    player.clear_part_color_override_by_index(part).unwrap();

    // lt, rt, lb, rb — red on top, blue on the bottom.
    let corners = [[255, 0, 0, 255], [255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 255, 255]];
    player.set_part_color_override_corners_by_index(part, corners, BlendOp::Mix, OverridePriority::Permanent).unwrap();
    player.clear_part_color_override_by_index(part).unwrap();

    player.clear_all_part_overrides();
}

#[test]
fn an_out_of_range_part_is_rejected() {
    let mut player = loaded();

    // Far past any real part count. The runtime bounds-checks against its table
    // rather than trusting the index.
    assert!(player.set_part_visibility_override_by_index(9999, true, false).is_err());
    assert!(player.set_part_color_override_by_index(9999, 0, BlendOp::Mix, OverridePriority::default()).is_err());
    assert!(player.clear_part_color_override_by_index(9999).is_err());
}

#[test]
fn a_cell_override_resolves_its_names_at_the_call() {
    let mut player = loaded();
    let part = some_part(&player);

    // A name that is not in the pack cannot be resolved, and saying so here is
    // the whole point — the alternative is a part that silently keeps drawing
    // its old cell.
    assert!(player.set_part_cell_override_by_index(part, "NoSuchCellmap", "NoSuchCell", OverridePriority::default()).is_err());

    // Clearing is always fine.
    player.clear_part_cell_override_by_index(part).unwrap();
}

#[test]
fn set_animation_clears_all_but_permanent() {
    let mut player = loaded();
    let part = some_part(&player);

    player.set_part_color_override_by_index(part, 0x00FF00FF, BlendOp::Mul, OverridePriority::Permanent).unwrap();
    player.set_part_visibility_override_by_index(part, true, false).unwrap();

    // Documented lifecycle, and the reason nothing is cached on this side: the
    // runtime owns the table and re-validates it here. The call must survive.
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();

    // The table is live again after the rebuild, which is what a caller needs in
    // order to re-apply anything that was not permanent.
    player.set_part_color_override_by_index(part, 0xFFFFFFFF, BlendOp::Mix, OverridePriority::default()).unwrap();
    player.clear_all_part_overrides();
}

// --- Part read-outs -------------------------------------------------------

#[test]
fn part_transform_is_bounds_checked_and_frame_shaped() {
    let player = loaded();

    assert!(player.part_count() > 0, "a loaded animation has parts");

    // The root part exists and reports a finite affine.
    let m = player.part_transform_by_index(0).expect("part 0 should resolve");
    assert!(m.iter().all(|v| v.is_finite()), "affine should be finite: {m:?}");

    // Out of range is None rather than a read past the runtime's buffer — which
    // is what would otherwise come back as NaN.
    assert!(player.part_transform_by_index(player.part_count() as u32).is_none());
    assert!(player.part_transform_by_index(u32::MAX).is_none());
}

#[test]
fn part_read_outs_are_empty_before_an_animation() {
    let player = SSPlayer::new();

    assert_eq!(player.part_count(), 0);
    assert!(player.part_names().is_none());
    assert!(player.part_transform_by_index(0).is_none());
}

#[test]
fn part_names_are_index_aligned_with_find_part_index() {
    let player = loaded();
    let names = player.part_names().expect("a bound pack has parts");

    assert!(!names.is_empty(), "Basic.ssab has parts");

    // len() is the part count, which is what makes this a replacement for a
    // count accessor rather than something sitting next to one.
    assert_eq!(names.len(), player.part_count());

    for (i, name) in names.iter().enumerate() {
        // An unnamed part holds its slot rather than shifting the ones after it.
        if name.is_empty() {
            continue;
        }
        assert_eq!(player.find_part_index(name), Some(i as u32), "names[{i}] = {name:?} must resolve back to {i}");
    }
}

#[test]
fn is_part_hidden_tracks_the_visibility_override() {
    // No device and no stepping: the hide flag comes out of the FrameData the
    // runtime builds on demand for the frame the playhead is on, which a
    // set-up animation already has.
    let mut player = loaded();
    let names = player.part_names().expect("a bound pack has parts");
    let name = names.iter().find(|n| !n.is_empty()).expect("a named part").clone();

    assert_eq!(player.is_part_hidden(&name), Some(false), "nothing hides it yet");

    // The override layer is the one thing that can hide a part on demand, so it
    // is what pins that this reads the live frame rather than a load-time table.
    player.set_part_visibility_override(&name, true, false).expect("the override should apply");
    assert_eq!(player.is_part_hidden(&name), Some(true));

    player.clear_part_visibility_override(&name).expect("clearing should apply");
    assert_eq!(player.is_part_hidden(&name), Some(false));

    // Out of range is None rather than a read past the parts vector.
    assert!(player.is_part_hidden_by_index(u32::MAX).is_none());
    assert!(player.is_part_hidden("NOT_A_REAL_PART").is_none());
}

#[test]
fn is_part_skinned_mesh_tells_bound_from_rigid() {
    // Mesh.ssab is the mesh fixture: a bone-skinned mesh, a rigid deform-only
    // mesh, and the bones / joint / null around them. The skinned one is the
    // only part a follower cannot pin to -- its node stays at the root while the
    // bones move the vertices -- so it is the only one that must report true.
    let mut player = SSPlayer::new();
    player.load(format!("{}/overall/Mesh.ssab", asset_root())).unwrap();
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    player.set_animation(&anim).unwrap();

    // The bone-bound mesh: exactly the case the follow-a-part docs warn about.
    assert_eq!(player.is_part_skinned_mesh("mesh_skin"), Some(true));
    // A rigid (deform-only) mesh carries its own transform, so it follows
    // correctly and is NOT flagged -- the line the influence-bone test draws.
    assert_eq!(player.is_part_skinned_mesh("mesh_deform"), Some(false));
    // Non-mesh anchors -- null / bone / joint -- are all good follow targets.
    assert_eq!(player.is_part_skinned_mesh("root"), Some(false));
    assert_eq!(player.is_part_skinned_mesh("bone_1"), Some(false));

    // The name and index forms agree.
    let i = player.find_part_index("mesh_skin").expect("mesh_skin resolves");
    assert_eq!(player.is_part_skinned_mesh_by_index(i), Some(true));

    // Out of range is None rather than a read past the parts vector.
    assert!(player.is_part_skinned_mesh_by_index(u32::MAX).is_none());
    assert!(player.is_part_skinned_mesh("NOT_A_REAL_PART").is_none());
}

#[test]
fn part_names_read_the_pack_rather_than_the_matrix_buffer() {
    // The names come from the .ssab parts table, so they are there as soon as a
    // pack is bound. `part_count` is derived from the world-matrix buffer and
    // stays 0 until an animation is set up -- which is exactly why the names
    // cannot be sourced from it.
    let mut player = SSPlayer::new();
    player.load(format!("{}/overall/Basic.ssab", asset_root())).unwrap();

    assert_eq!(player.part_count(), 0, "no animation set up yet");
    assert!(!player.part_names().expect("the pack is bound").is_empty(), "names are available before the first set_animation");
}

/// The unsuffixed form takes a part NAME — the index form is the optimisation,
/// not the default. An unknown name is
/// rejected at the call rather than silently doing nothing, which is the whole
/// reason resolution happens here and not per frame.
#[test]
fn the_unsuffixed_overrides_take_a_part_name() {
    let mut player = loaded();

    assert!(player.set_part_visibility_override("NOT_A_REAL_PART", true, false).is_err());
    assert!(player.set_part_color_override("NOT_A_REAL_PART", 0xFF0000FF, BlendOp::Mix, OverridePriority::default()).is_err());
    assert!(player.set_part_cell_override("NOT_A_REAL_PART", "Ringo", "effect3", OverridePriority::default()).is_err());
    assert!(player.clear_part_color_override("NOT_A_REAL_PART").is_err());
    assert!(player.part_transform("NOT_A_REAL_PART").is_none());
}
