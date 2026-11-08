//! `fit` / `align` — placing an animation inside a box the host laid out.
//!
//! The player draws at authored scale into whatever target it is given, which is
//! what a game wants and what a UI host cannot use: a UI host has a box and needs
//! the art fitted into it. The vocabulary here is the family's — `contain` /
//! `cover` / `none` and nine alignments, the words `<Image resizeMode>` already
//! taught every host — so this is a port of a settled contract rather than a new
//! design.
//!
//! Everything in this module is arithmetic over a box and a target size. There is
//! no GPU work in it and no state, which is why it lives in the player where the
//! C ABI and every adapter inherit it, rather than in one host's adapter crate.

/// How the content box is scaled into the view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// Scale until the box fits entirely inside the view: `min(sx, sy)`.
    #[default]
    Contain,
    /// Scale until the box covers the view: `max(sx, sy)`. The overflow is not
    /// clipped — the player has no scissor of its own, so a host that needs the
    /// spill hidden supplies one.
    Cover,
    /// No fit at all: scale 1, origin at the view's centre. The placement is then
    /// the caller's own [`Placement::scale`] and [`Placement::offset`].
    None,
}

/// Which point of the content box lands on the matching point of the view.
///
/// It decomposes into two independent axes ([`Align::axes`]), which is the form
/// the arithmetic uses; the nine names exist because they are what a host says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Center,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Align {
    /// The horizontal and vertical axes, each `-1 | 0 | 1`.
    ///
    /// On the vertical axis **`-1` is the top of the view** — screen terms, not
    /// maths terms, matching the Y-down space the player places itself in.
    pub fn axes(self) -> (i8, i8) {
        match self {
            Align::Center => (0, 0),
            Align::Top => (0, -1),
            Align::Bottom => (0, 1),
            Align::Left => (-1, 0),
            Align::Right => (1, 0),
            Align::TopLeft => (-1, -1),
            Align::TopRight => (1, -1),
            Align::BottomLeft => (-1, 1),
            Align::BottomRight => (1, 1),
        }
    }
}

/// A box in **player-local** units — the space
/// [`SSPlayer::part_transform`](crate::SSPlayer::part_transform) reports in, with
/// `y` downward and the origin where the animation was authored around.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Low edge on x.
    pub x: f32,
    /// Low edge on y, which is the **top** edge on screen.
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Everything that places an animation inside a view.
///
/// The defaults are `contain`, centred, unscaled, unflipped, unrotated — so
/// `Placement::default()` is "make it fit and put it in the middle", which is
/// what a host reaching for this wants nine times out of ten.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub fit: Fit,
    pub align: Align,
    /// The box to fit, in player-local units. **Supply it and nothing is
    /// measured** — [`SSPlayer::fit_into`](crate::SSPlayer::fit_into) otherwise
    /// falls back to the authored canvas
    /// ([`SSPlayer::canvas_rect`](crate::SSPlayer::canvas_rect)).
    pub frame: Option<Rect>,
    /// Pixels from the aligned position, applied **after** the fit, so it is not
    /// scaled by it.
    pub offset: (f32, f32),
    /// A multiplier **on top of** the fit's scale, not a replacement for it.
    pub scale: f32,
    /// Mirror horizontally. A flip is a negative scale on that axis, and the
    /// alignment follows it — a left-aligned mirrored character still sits
    /// against the left edge.
    pub flip_x: bool,
    /// Mirror vertically.
    pub flip_y: bool,
    /// Passed through to [`SSPlayer::set_transform`](crate::SSPlayer::set_transform)
    /// untouched. Rotation has no part in the fit contract; it is here so that
    /// fitting does not silently discard the rotation a caller already had.
    pub rotation: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self { fit: Fit::default(), align: Align::default(), frame: None, offset: (0.0, 0.0), scale: 1.0, flip_x: false, flip_y: false, rotation: 0.0 }
    }
}

/// A solved placement, in [`SSPlayer::set_transform`](crate::SSPlayer::set_transform)'s
/// terms: pixels from the **view's centre**, `y` downward.
///
/// The conversion from the contract's view coordinates (origin at the view's
/// top-left) happens once, in [`solve`]. Doing it anywhere else is the mistake
/// that looks like a correct fit displaced by half a screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
}

/// Solve one axis: where the content's origin lands, in view coordinates.
///
/// `scale` is **signed** — a flip is a negative scale, and which end of the box
/// is the low one on screen depends on that sign, so the two cannot be solved
/// separately and mirrored afterwards.
fn axis(view: f32, scale: f32, lo: f32, size: f32, side: i8) -> f32 {
    if side == 0 {
        // Sign-independent, which is why centring is the safe default.
        return view / 2.0 - scale * (lo + size / 2.0);
    }
    let (low_edge, high_edge) = if scale >= 0.0 { (lo, lo + size) } else { (lo + size, lo) };
    if side < 0 { -scale * low_edge } else { view - scale * high_edge }
}

/// Fit `content` into a `view_width` x `view_height` box.
///
/// A zero-sized view, an absent or zero-sized box, and [`Fit::None`] all take the
/// same path: **no fit**, origin at the view's centre, scale 1 before the
/// caller's own. That is a documented outcome rather than an error — there is
/// nothing to divide by and nothing to measure, and a host that laid out a
/// collapsed box gets its animation back the moment the box has a size.
pub fn solve(view_width: f32, view_height: f32, content: Option<Rect>, placement: &Placement) -> Placed {
    let measured = content.filter(|rect| rect.width > 0.0 && rect.height > 0.0);
    let fitted = match measured {
        Some(rect) if placement.fit != Fit::None && view_width > 0.0 && view_height > 0.0 => {
            let (sx, sy) = (view_width / rect.width, view_height / rect.height);
            let scale = if placement.fit == Fit::Cover { sx.max(sy) } else { sx.min(sy) };
            Some((rect, scale))
        }
        _ => None,
    };

    let scale = fitted.map_or(1.0, |(_, scale)| scale) * placement.scale;
    let scale_x = if placement.flip_x { -scale } else { scale };
    let scale_y = if placement.flip_y { -scale } else { scale };

    let (origin_x, origin_y) = match fitted {
        Some((rect, _)) => {
            let (ax, ay) = placement.align.axes();
            (axis(view_width, scale_x, rect.x, rect.width, ax), axis(view_height, scale_y, rect.y, rect.height, ay))
        }
        None => (view_width / 2.0, view_height / 2.0),
    };

    // `offset` is added last, to the solved origin; then the seam: `set_transform`
    // measures from the view's centre, the contract from its top-left corner.
    Placed { x: origin_x + placement.offset.0 - view_width / 2.0, y: origin_y + placement.offset.1 - view_height / 2.0, scale_x, scale_y }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: (f32, f32) = (400.0, 200.0);
    /// Taller than it is wide, and not centred on the origin — so a mistake in the
    /// box's low edge cannot hide behind a symmetric fixture.
    const BOX: Rect = Rect { x: -100.0, y: -300.0, width: 200.0, height: 400.0 };

    fn solved(placement: &Placement) -> Placed {
        solve(VIEW.0, VIEW.1, Some(BOX), placement)
    }

    /// The two ratios are 400/200 = 2 on x and 200/400 = 0.5 on y, so `contain`
    /// takes 0.5 and `cover` takes 2.
    #[test]
    fn contain_takes_the_smaller_ratio_and_cover_the_larger() {
        assert_eq!(solved(&Placement::default()).scale_x, 0.5);
        assert_eq!(solved(&Placement { fit: Fit::Cover, ..Default::default() }).scale_x, 2.0);
    }

    /// The box's centre lands on the view's centre, and the answer is in
    /// `set_transform` terms — pixels from that centre — so it is zero here.
    #[test]
    fn centring_puts_the_boxs_middle_on_the_views_middle() {
        let placed = solved(&Placement::default());
        // Box centre is (0, -100) player-local, scaled by 0.5 → (0, -50) from the
        // player's origin, so the origin sits 50 px below the view centre.
        assert_eq!((placed.x, placed.y), (0.0, 50.0));
    }

    /// Each named alignment puts the corresponding edge of the scaled box on the
    /// corresponding edge of the view.
    #[test]
    fn each_edge_alignment_lands_flush() {
        let scale = 0.5;
        for (align, (want_lo_x, want_lo_y)) in [
            (Align::TopLeft, (0.0, 0.0)),
            (Align::BottomRight, (VIEW.0 - BOX.width * scale, VIEW.1 - BOX.height * scale)),
            (Align::Top, ((VIEW.0 - BOX.width * scale) / 2.0, 0.0)),
            (Align::Right, (VIEW.0 - BOX.width * scale, (VIEW.1 - BOX.height * scale) / 2.0)),
        ] {
            let placed = solved(&Placement { align, ..Default::default() });
            // Where the box's low corner landed, back in view coordinates.
            let lo_x = placed.x + VIEW.0 / 2.0 + BOX.x * placed.scale_x;
            let lo_y = placed.y + VIEW.1 / 2.0 + BOX.y * placed.scale_y;
            assert!((lo_x - want_lo_x).abs() < 1e-3, "{align:?}: x {lo_x} != {want_lo_x}");
            assert!((lo_y - want_lo_y).abs() < 1e-3, "{align:?}: y {lo_y} != {want_lo_y}");
        }
    }

    /// A flip is a negative scale, and the alignment follows it: the mirrored box
    /// still ends up against the left edge, not off it.
    #[test]
    fn a_flip_keeps_the_alignment_it_was_given() {
        let placed = solved(&Placement { align: Align::Left, flip_x: true, ..Default::default() });
        assert_eq!(placed.scale_x, -0.5);
        let (a, b) = (placed.x + VIEW.0 / 2.0 + BOX.x * placed.scale_x, placed.x + VIEW.0 / 2.0 + (BOX.x + BOX.width) * placed.scale_x);
        assert!((a.min(b) - 0.0).abs() < 1e-3, "left edge at {}", a.min(b));
    }

    /// Multiplies the fit rather than replacing it, and rides on top of a flip's
    /// sign rather than cancelling it.
    #[test]
    fn scale_multiplies_the_fit() {
        let placed = solved(&Placement { scale: 3.0, flip_y: true, ..Default::default() });
        assert_eq!((placed.scale_x, placed.scale_y), (1.5, -1.5));
    }

    /// Pixels, applied after the fit, so it is not scaled by it.
    #[test]
    fn offset_is_added_after_the_fit() {
        let base = solved(&Placement::default());
        let moved = solved(&Placement { offset: (10.0, -20.0), ..Default::default() });
        assert_eq!((moved.x - base.x, moved.y - base.y), (10.0, -20.0));
    }

    /// No box, no size, or `Fit::None` — all three answer "centred, unscaled"
    /// rather than dividing by zero. The caller's own scale and offset still
    /// apply, because with `none` they are the whole placement.
    #[test]
    fn the_degenerate_cases_centre_rather_than_fail() {
        let centred = Placed { x: 0.0, y: 0.0, scale_x: 1.0, scale_y: 1.0 };
        assert_eq!(solve(VIEW.0, VIEW.1, None, &Placement::default()), centred);
        assert_eq!(solve(VIEW.0, VIEW.1, Some(Rect::default()), &Placement::default()), centred);
        assert_eq!(solve(0.0, 0.0, Some(BOX), &Placement::default()), centred);
        assert_eq!(solve(VIEW.0, VIEW.1, Some(BOX), &Placement { fit: Fit::None, ..Default::default() }), centred);

        let placed = solve(VIEW.0, VIEW.1, Some(BOX), &Placement { fit: Fit::None, scale: 2.0, offset: (5.0, 5.0), ..Default::default() });
        assert_eq!(placed, Placed { x: 5.0, y: 5.0, scale_x: 2.0, scale_y: 2.0 });
    }
}
