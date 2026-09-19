# `fit` / `align` — the placement contract

The player draws at authored scale into whatever target it is given. A host that lays out
a box — any UI host, and the preview — needs the animation fitted into it.

The vocabulary is the family's and is already settled, so **this is a port, not a
design**. The reference implementation is `SSPlayerForReactNative`'s
`packages/react-native-spritestudio/src/core/fit.ts`: 156 lines, deliberately free of
React and of any renderer so the arithmetic can be checked without one. That repository
is not reachable from here, which is why the contract is written out in full below rather
than linked.

## Vocabulary

**`fit`** — `contain` | `cover` | `none`. `<Image resizeMode>`'s words, because that is
the vocabulary a host already has. `none` is the behaviour from before there was a fit at
all: scale 1, origin at the view's centre, and the caller supplies `offset` and `scale`
itself.

**`align`** — which point of the content box lands on the matching point of the view.
Nine values: `center`, `top`, `bottom`, `left`, `right`, `top-left`, `top-right`,
`bottom-left`, `bottom-right`. It decomposes into two independent axes, each `-1 | 0 | 1`.
On the vertical axis, **`-1` is the top of the view** — screen terms, not maths terms.

**`frameScope`** — `animation` | `pack`. Which animations the content box is measured
over. `animation` is the default and the tighter picture: the box of the one animation
playing. `pack` unions every animation in the file, which costs more to measure and draws
smaller, and buys the one thing the default cannot: a **stable** scale, so a character
does not resize when its animation changes. The difference is not marginal — on `Ringo`,
`attack1` spans 383×527 player-local units while the pack's union spans about 1030×878,
because `dead` slides the character most of a screen to the right.

**The rest of the placement**, all optional:

| Field | Meaning |
|---|---|
| `frame` | The box to fit, in player-local units. **Supply it and nothing is measured.** |
| `offset` | Pixels from the aligned position. Applied **after** the fit. |
| `scale` | A multiplier **on top of** the fit's scale, not a replacement for it. |
| `flipX`, `flipY` | Mirrors. They change which end of the box the alignment anchors to. |

Defaults: `fit` is `contain`, `align` is `center`, `scale` is 1, both flips are false.

## The arithmetic

**Scale.** `sx = width / box.width`, `sy = height / box.height`. `contain` takes
`min(sx, sy)`, `cover` takes `max(sx, sy)`. The result multiplies the caller's `scale`;
it does not replace it.

**Flips become signed scale**, and the alignment has to follow. The draw maps
`p → origin + s·p`, so a negative `s` swaps which end of the box is the low one. Solve
each axis with the *signed* scale rather than mirroring afterwards.

**One axis**, given the view's size along it, the signed scale `s`, the box's low edge and
extent, and the side (`-1` | `0` | `1`):

- centre (`0`): `viewSize / 2 - s · (boxLo + boxSize / 2)`. This comes out
  sign-independent, which is why it is the safe default.
- otherwise, the player-local edge that maps to the low screen edge is `boxLo` when
  `s > 0` and `boxLo + boxSize` when `s < 0`; the high edge is the other one. Then
  low side (`-1`) is `-s · lowEdge`, and high side (`1`) is `viewSize - s · highEdge`.

**The degenerate case is not an error.** A zero-sized or absent box, or a zero-sized view,
means "nothing measured": skip the fit and return the pre-fit behaviour — scale 1, origin
at the view's centre — rather than dividing by zero. `fit: 'none'` takes the same path.

**`offset` is added last**, to the solved origin, after everything else.

## Where this lands in this repository

`ROADMAP.md`'s *UI integration* section sets the split: the portable half belongs in the
player, where every host and the C ABI reach it, and only the host-shaped half belongs in
an adapter. `fit`/`align` is entirely the portable half — it is arithmetic over a box and
a target size, with no GPU work in it — so it lives in `libs/ssplayer-wgpu` and every face
inherits it.

**The seam already exists, and its coordinates differ from the reference's.**
`SSPlayer::set_transform(x, y, scale_x, scale_y, rotation)` (`player.rs:1929`) places a
player **in pixels from the canvas centre**, with `y` downward. The reference returns an
origin in *view* coordinates — its default is `(width / 2, height / 2)`, the view's
centre, not `(0, 0)`. Converting is a subtraction of half the target size on each axis,
and getting it wrong looks like a correct fit displaced by half a screen.

Two further notes on the seam:

- `set_transform` takes **separate `scale_x` and `scale_y`**, so a flip is a negative
  scale on that axis and there is no separate flag to carry. The reference returns
  `flipX`/`flipY` alongside the scale only because its draw applies them.
- `rotation` has no part in this contract. Pass whatever the caller already had.

## Where the box comes from

Nothing here measures one, and nothing needs to. There are three possible sources and only
the first costs nothing:

1. **The authored canvas** — `canvas_size`, with the pivot placing the origin inside it.
   Free, stable across animations, and the frame the artist composed in.
2. **The playing animation's box** — `frameScope: 'animation'`. Needs measurement.
3. **The pack's union** — `frameScope: 'pack'`. Needs more measurement, and draws small.

**The first is what `SSPlayer::canvas_rect` reports and what `fit_into` uses** when the caller
supplies no `frame`. The measured scopes are **not built**: measuring means baking every frame
of the animation at load, `pack` additionally draws the character small — on `Ringo`, `attack1`
spans 383×527 player-local units while the pack's union spans about 1030×878, because `dead`
slides the character most of a screen to the right — and supplying `frame` buys the same scale
stability for free. `frameScope` is therefore absent from the vocabulary here rather than
present and half-honoured; adding it later is additive.

The pivot is normalised from the canvas centre and the runtime does not apply it (it carries
no pivot in its metadata), so `canvas_rect` reads it out of the `.ssab` itself and flips it into
the player's Y-down space. `pivot (0, -0.5)` — the editor's bottom edge, which is how a
character standing on the ground is authored — reports a box entirely above the origin.

## What landed

`libs/ssplayer-wgpu/src/fit.rs` holds the whole of the arithmetic above: `Fit`, `Align`,
`Rect`, `Placement`, `Placed` and `solve`. `SSPlayer::fit_into` is the one line of policy on
top of it — the caller's `frame` or `canvas_rect` — and the faces are translation only:
`ss_player_fit_into` / `ss_placement_default` / `ss_player_canvas_rect` on C, `fitInto` /
`canvasRect` on JS. `ssplayer-bevy` has no component for it; that is the first half of
*Bevy UI* in the roadmap.

The guide is [`docs/en/guide/placement.md`](./docs/en/guide/placement.md).
