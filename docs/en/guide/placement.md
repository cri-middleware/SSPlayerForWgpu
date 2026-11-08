# Fitting into a box

The player draws at **authored scale** into whatever target it is given. That is what a game wants —
a character is the size the artist made it, and the game places and scales it from there. It is not what a UI
wants: a panel, a card or a preview pane is a box of a size the layout decided, and the animation
has to end up inside it.

`fitInto` is that step. Give it the box and it computes the placement, then writes it through the
same seam as `setTransform` (pixels from the view's centre, `y` downward) — so there is nothing new
to draw and nothing extra per frame.

```
   fit: contain              fit: cover               fit: none
 ┌──────────────┐         ┌──────────────┐         ┌──────────────┐
 │░░┌────────┐░░│         │██┌────────┐██│         │              │
 │░░│        │░░│         │██│        │██│         │    ┌────┐    │
 │░░│  art   │░░│         │██│  art   │██│         │    │art │    │
 │░░│        │░░│         │██│        │██│         │    └────┘    │
 │░░└────────┘░░│         │██└────────┘██│         │              │
 └──────────────┘         └──────────────┘         └──────────────┘
  all of it is in          no gaps; the art         authored scale,
  the box; ░ is the        spills past the box      centred; you place
  leftover space           (█) and is not clipped   it yourself
```

## The vocabulary

It is `<Image resizeMode>`'s, because that is the vocabulary a host already has, and it is the same
across the SpriteStudio Player family.

| Field | Meaning |
|---|---|
| `fit` | `contain` (default) fits the whole box in; `cover` fills the box and spills; `none` skips the fit |
| `align` | Which point of the content lands on the matching point of the view: `center` (default), `top`, `bottom`, `left`, `right`, `top-left`, `top-right`, `bottom-left`, `bottom-right` |
| `frame` | The box to fit, in player-local units. **Supply it and nothing is read from the pack** |
| `offset` | Pixels from the aligned position, applied **after** the fit, so it is not scaled by it. A pair in Rust; `offset_x` / `offset_y` in C, `offsetX` / `offsetY` in JavaScript |
| `scale` | A multiplier **on top of** the fit's scale, not a replacement for it |
| `flipX`, `flipY` | Mirrors. The alignment follows them — a left-aligned mirrored character still sits against the left edge. `flip_x` / `flip_y` in Rust and C |
| `rotation` | Passed straight through. Rotation has no part in the fit itself |

=== "Rust"

    ```rust
    use ssplayer_wgpu::{Align, Fit, Placement};

    // Contain, centred — the default.
    player.fit_into(width, height, &Placement::default());

    // Fill the panel and let it spill, anchored at the bottom.
    let placed = player.fit_into(width, height, &Placement {
        fit: Fit::Cover,
        align: Align::Bottom,
        ..Default::default()
    });
    println!("drawn at {}x", placed.scale_x);
    ```

=== "C"

    ```c
    ss_placement_t placement;
    ss_placement_default(&placement);   // never zero-initialise it: scale would be 0
    placement.fit = SS_FIT_COVER;
    placement.align = SS_ALIGN_BOTTOM;

    ss_placed_t placed;
    ss_player_fit_into(player, width, height, &placement, &placed);
    ```

=== "JavaScript"

    ```js
    const placed = player.fitInto(canvas.width, canvas.height, {
      fit: 'cover',
      align: 'bottom',
    });
    console.log(placed.scaleX);
    ```

=== "Bevy"

    ```rust
    // Not wired into the adapter. `SsTransform` places a player in world
    // units, and a fit needs the view's pixel size, which only the render world
    // knows.
    ```

## The box it fits

Unless you supply a `frame`, the box is the animation's **authored canvas**: the frame the artist
composed in, placed against the origin by the animation's pivot.

That is a deliberate choice over measuring the art. A measured box would mean baking every frame of
the animation at load, and it changes size as the animation plays. The authored canvas costs nothing
to read, holds still while the animation plays, and is where the author decided the empty space
belongs — a character authored standing on the ground keeps the headroom above it.

Each animation carries its own canvas, and one pack's can differ — Ringo's `dead` is 1300 wide where
its other animations are 800 — so fitting again after a switch can rescale the character. To keep
one scale across a pack, read the canvas once after the first `setAnimation` and pass it as the
`frame` from then on.

=== "Rust"

    ```rust
    // What will be fitted, in player-local units (y downward).
    let rect = player.canvas_rect();

    // Or fit something tighter that you measured or authored yourself.
    player.fit_into(width, height, &Placement {
        frame: Some(ssplayer_wgpu::Rect { x: -100.0, y: -200.0, width: 200.0, height: 200.0 }),
        ..Default::default()
    });
    ```

=== "C"

    ```c
    ss_rect_t rect;
    if (ss_player_canvas_rect(player, &rect) == SS_OK) { /* … */ }

    placement.has_frame = true;
    placement.frame = (ss_rect_t){ -100.0f, -200.0f, 200.0f, 200.0f };
    ```

=== "JavaScript"

    ```js
    const rect = player.canvasRect();   // { x, y, width, height } or undefined

    player.fitInto(w, h, { frame: { x: -100, y: -200, width: 200, height: 200 } });
    ```

=== "Bevy"

    ```rust
    // Not exposed on Bevy: the authored canvas is read inside the render world,
    // which gameplay code cannot reach. Work the scale out from your own box and
    // write it to `SsTransform`.
    ```

!!! note "A pivot is not a bounding box"

    Player-local `y` runs **downward**, and the pivot says where the origin sits inside the canvas.
    A character authored with its origin on the ground (`pivot 0 -0.5` in the editor) therefore
    reports a box that is entirely *negative* on `y` — the canvas is above the origin. That is what
    makes such a character stand on the floor of its box rather than float in the middle of it.

## Calling it again

The size is in **pixels of the render target**, so the placement is only right for the box you
passed. Call it again when that box changes:

* on resize, before or after `resize` / `set_viewport` — the two are independent
* after `setAnimation`, if the new animation's canvas differs from the old one

It is pure arithmetic over a rectangle. There is no measurement, no GPU work and no allocation in
it, so calling it every frame is also fine if that is simpler than tracking changes.

## When it does not fit

A **collapsed box** (a `width` or `height` of 0, or a `frame` with no area), an **animation that is not set up**, and `fit: none` all
take the same path: no fit, origin at the view's centre, scale 1 before your own `scale`. None of
them is an error — a host that lays out a box before it has measured it gets its animation back the
moment the box has a size.

What *is* an error is an unrecognised `fit` or `align`: the C face returns
`SS_ERROR_INVALID_ARGUMENT` and the JavaScript face throws, naming what it accepts. A typo that
silently centred would be a bug you could not see.

`cover` spills past the box on purpose, and the player has **no scissor of its own** — clipping the
overflow is the host's, with its own scissor rect or render target.
