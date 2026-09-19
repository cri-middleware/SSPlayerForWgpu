//! `fit` / `align` against real authored data.
//!
//! The arithmetic is unit-tested in `src/fit.rs`; what needs a fixture is the
//! **box**, because it is read straight out of the `.ssab` and placed against the
//! origin by the animation's pivot. Get that flip wrong and every fit is half a
//! canvas out, with the arithmetic still perfectly correct.
//!
//! No GPU: `SSPlayer` holds no wgpu types and `load` borrows no `Device`, so
//! fitting is reachable without an adapter.

use ssplayer_wgpu::{Fit, Placed, Placement, Rect, SSPlayer, fit::solve};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

fn played(pack: &str, animation: &str) -> SSPlayer {
    let mut player = SSPlayer::new();
    player.load(format!("{}/{pack}", asset_root())).unwrap();
    player.set_animation(animation).unwrap();
    player
}

/// The two shapes a pivot takes, both read from the fixtures the repository
/// already ships: `Basic/anime_1` is authored around the canvas centre, `Ringo`
/// around its feet (`pivot 0 -0.5`, the editor's bottom edge).
///
/// Player-local `y` runs **downward**, so the ground-pivoted box is entirely
/// *negative* on y — above the origin on screen. That sign is the whole test.
#[test]
fn the_box_is_the_authored_canvas_placed_by_its_pivot() {
    let centred = played("overall/Basic.ssab", "anime_1");
    assert_eq!(centred.canvas_rect(), Some(Rect { x: -240.0, y: -240.0, width: 480.0, height: 480.0 }));

    let grounded = played("Ringo/Ringo.ssab", "attack1");
    assert_eq!(grounded.canvas_rect(), Some(Rect { x: -400.0, y: -600.0, width: 800.0, height: 600.0 }));
}

/// A player with no animation set up has no canvas to report, and fitting one is
/// the documented no-fit answer rather than an error.
#[test]
fn a_player_with_no_animation_has_no_box_and_fits_to_the_centre() {
    let mut player = SSPlayer::new();
    assert_eq!(player.canvas_rect(), None);
    assert_eq!(player.fit_into(400.0, 400.0, &Placement::default()), Placed { x: 0.0, y: 0.0, scale_x: 1.0, scale_y: 1.0 });
}

/// What the player adds to [`solve`] is one decision: where the box comes from.
/// A supplied frame is used verbatim, and nothing is read from the pack for it.
#[test]
fn a_supplied_frame_wins_over_the_authored_canvas() {
    let mut player = played("Ringo/Ringo.ssab", "attack1");
    let frame = Rect { x: -100.0, y: -200.0, width: 200.0, height: 200.0 };

    let with_canvas = player.fit_into(400.0, 400.0, &Placement::default());
    assert_eq!(with_canvas, solve(400.0, 400.0, player.canvas_rect(), &Placement::default()));

    let supplied = Placement { frame: Some(frame), ..Default::default() };
    assert_eq!(player.fit_into(400.0, 400.0, &supplied), solve(400.0, 400.0, Some(frame), &supplied));
    // 400/200 on both axes, against the canvas's 400/800.
    assert_eq!(player.fit_into(400.0, 400.0, &supplied).scale_x, 2.0);
}

/// The end of the contract, in one number: a character authored standing on the
/// ground, contained in a square view, keeps its feet on the floor of its canvas
/// rather than in the middle of the view.
///
/// 800x600 into 400x400 is a scale of 0.5, so the canvas draws 400x300 and sits
/// centred — from y = 50 to y = 350 in view coordinates. The origin is the
/// canvas's bottom edge, so it lands at 350, which is 150 below the view's
/// centre — and `set_transform` measures from that centre.
#[test]
fn a_ground_pivoted_character_stands_on_the_bottom_of_its_canvas() {
    let mut player = played("Ringo/Ringo.ssab", "attack1");
    assert_eq!(player.fit_into(400.0, 400.0, &Placement::default()), Placed { x: 0.0, y: 150.0, scale_x: 0.5, scale_y: 0.5 });
}

/// `cover` fills the view and spills; the player has no scissor, so the spill is
/// the host's to clip. Worth pinning that the two modes really differ on a real
/// canvas rather than only on the unit test's synthetic box.
#[test]
fn cover_fills_where_contain_fits() {
    let mut player = played("Ringo/Ringo.ssab", "attack1");
    let contain = player.fit_into(400.0, 400.0, &Placement::default());
    let cover = player.fit_into(400.0, 400.0, &Placement { fit: Fit::Cover, ..Default::default() });
    assert_eq!(contain.scale_x, 0.5, "400/800 is the smaller ratio");
    assert!((cover.scale_x - 400.0 / 600.0).abs() < 1e-6, "400/600 is the larger");
}
