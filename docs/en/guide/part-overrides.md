# Part overrides

An override changes what one part draws, on top of what the animator authored, without touching the
`.ssab`. Three kinds — **visibility**, **cell** and **colour** — plus a read-only lookup of where a
part currently is.

Typical uses: swapping a weapon, tinting a character red on damage, hiding an accessory, hanging a
health bar off a bone.

## Naming a part

Every override comes in two forms: **by name** and **by index**. They do the same thing; the index
form skips a lookup.

=== "Rust"

    ```rust
    let index = player.find_part_index("body").expect("no such part");
    player.set_part_visibility_override_by_index(index, true, false);
    ```

=== "C"

    ```c
    int32_t index = ss_player_find_part_index(player, "body");
    ss_player_set_part_visibility_override_by_index(player, (uint32_t)index, true, false);
    ```

=== "JavaScript"

    ```js
    const index = player.findPartIndex('body');
    player.setPartVisibilityOverrideByIndex(index, true, false);
    ```

Resolve the index **once**, when you set the animation, and keep it: names are matched by hash but
the lookup still walks the part list. `part_names()` / `ss_player_part_count` + the name accessors /
`partNames()` enumerate what a pack has.

## Visibility

```
set_part_visibility_override(part, hide, cascade)
```

`cascade` is what makes this useful on a rig: with it, force-hiding a part hides everything parented
under it — a whole arm, not just the upper arm. **A keyframe hide never cascades, so this is the only
way to hide a sub-tree.** Clearing puts the part back under the animator's control.

Like every override, it shows from the next update: `is_part_hidden` read before then reports the
frame the last update built.

## Cell

Swap the image a part draws for another cell of any cell map the pack has:

=== "Rust"

    ```rust
    use ssplayer_wgpu::OverridePriority;

    player.set_part_cell_override("weapon", "items", "sword_02", OverridePriority::Permanent)?;
    player.clear_part_cell_override("weapon")?;
    ```

=== "C"

    ```c
    /* priority: 0 = until the next keyframe, 1 = until the next animation, 2 = permanent */
    ss_player_set_part_cell_override(player, "weapon", "items", "sword_02", 2);
    ss_player_clear_part_cell_override(player, "weapon");
    ```

=== "JavaScript"

    ```js
    player.setPartCellOverride('weapon', 'items', 'sword_02', 2 /* permanent */);
    player.clearPartCellOverride('weapon');
    ```

`cellmap_names()` and `cell_names(cellmap)` list what is available, so a UI can offer the choices
rather than hard-coding them.

To swap the whole *texture* a cell map binds — a costume or palette change across every part that
uses it, rather than one part's cell — see [Texture replacement](./texture-replacement.md).

## Colour

A colour override is an RGBA value, a **blend operation** and a **priority**. There are two shapes:
one colour for the whole part, or one per corner for a gradient.

=== "Rust"

    ```rust
    use ssplayer_wgpu::{BlendOp, OverridePriority};

    // 0xRRGGBBAA
    player.set_part_color_override("body", 0xff0000_80, BlendOp::Mix, OverridePriority::Permanent)?;

    // Four corners, each [r, g, b, a]
    player.set_part_color_override_corners("cape", corners, BlendOp::Mix, OverridePriority::Permanent)?;

    player.clear_part_color_override("body")?;
    player.clear_all_part_overrides();
    ```

=== "C"

    ```c
    /* rgba, blend_op (0 = mix), priority (2 = permanent) */
    ss_player_set_part_color_override(player, "body", 0xff000080u, 0, 2);
    ss_player_clear_part_color_override(player, "body");
    ss_player_clear_all_part_overrides(player);
    ```

=== "JavaScript"

    ```js
    player.setPartColorOverride('body', 0xff000080, 0 /* mix */, 2 /* permanent */);
    player.clearPartColorOverride('body');
    player.clearAllPartOverrides();
    ```

### Blend op and priority

**Blend op** is how the colour combines with the texel. **Priority is not about who wins — it is
about how long the override lives**, which is the part most people get wrong on the first read.

| Blend op | Rust | Wire (C / JS) |
|---|---|---|
| Mix | `BlendOp::Mix` (default) | `0` |
| Multiply | `BlendOp::Mul` | `1` |
| Add | `BlendOp::Add` | `2` |
| Subtract | `BlendOp::Sub` | `3` |

| Priority | Rust | Wire | Lives until |
|---|---|---|---|
| Until the next keyframe | `OverridePriority::OverwriteOnNextKeyframe` | `0` | the animation next drives that attribute — so it shows only on frames the animation leaves alone |
| Until the next animation | `OverridePriority::HoldUntilNextAnimation` (default) | `1` | another animation is set up |
| Permanent | `OverridePriority::Permanent` | `2` | the pack changes — it survives animation switches |

Pick `Permanent` for a costume choice, `HoldUntilNextAnimation` for a state that ends with the
motion, `OverwriteOnNextKeyframe` for a nudge that should yield the moment the animator says
something.

## Whole-character tint and alpha

An override is per part. For the whole character there are two cheaper controls that cost no
lookup at all, because they ride on the per-part primitive the player already writes:

=== "Rust"

    ```rust
    player.set_alpha(0.5);       // 0..1, multiplied through the character
    player.set_tint(0x80c0ff);   // 0xRRGGBB
    ```

=== "C"

    ```c
    ss_player_set_alpha(player, 0.5f);
    ss_player_set_tint(player, 0x80c0ffu);
    ```

=== "JavaScript"

    ```js
    player.setAlpha(0.5);
    player.setTint(0x80c0ff);
    ```

=== "Bevy"

    ```rust
    playback.alpha = 0.5;
    playback.tint = 0x80c0ff;
    ```

## Reading a part's transform

Where is the hand *right now* — to hang a weapon, a UI marker or a particle emitter off it?

=== "Rust"

    ```rust
    if let Some(m) = player.part_transform("hand_r") {
        // [m00, m01, m10, m11, tx, ty] — a 2D affine in animation-local space
    }
    ```

=== "C"

    ```c
    float m[6];
    if (ss_player_part_transform(player, "hand_r", m) == SS_OK) { /* ... */ }
    ```

=== "JavaScript"

    ```js
    const m = player.partTransform('hand_r');   // [m00, m01, m10, m11, tx, ty]
    ```

Three things to know about the numbers:

* they are the **six numbers of a 2D affine**, the same shape `ssplayer-webgl`'s `getPartTransform`
  returns, so code ported from SpriteStudio Player for Web's WebGL package reads across unchanged;
* they are in **animation-local, Y-down space** — neither the character's own `set_transform` nor
  the target's projection is applied. Compose those yourself, which is what lets you place the
  result in whatever space your UI lives in;
* read them **after** the update, because they describe the frame it produced. Hidden parts still
  report where they would have been.

!!! warning "Mesh (skinned) parts cannot be followed directly"

    The `hand_r` above is the trap. A **bone-bound mesh part** — a deforming part such as a hand or
    a face — has its vertices drawn by bone skinning, while the part's *own* transform stays at its
    setup node position, usually near the character root. So `part_transform` on such a part reports
    the root, not the visible art, and a follower pinned to it snaps to the root.

    To follow the position of a mesh part, add a **null part** (a bone-point / empty part) at that
    spot in SpriteStudio and follow that part instead. A bone (armature) or joint part works the same
    way — each carries a transform the drawn vertices actually track.

    Ask which parts are affected so a "follow this part" picker can warn or grey them out. A **rigid**
    (deform-only) mesh follows correctly and reports `false`, as does every non-mesh part.

    === "Rust"

        ```rust
        if player.is_part_skinned_mesh("hand_r") == Some(true) {
            // follow a null / bone part near the hand instead
        }
        ```

    === "C"

        ```c
        /* 1 = skinned, 0 = follows correctly, -1 = no such part */
        if (ss_player_is_part_skinned_mesh(player, "hand_r") == 1) { /* warn */ }
        ```

    === "JavaScript"

        ```js
        if (player.isPartSkinnedMesh('hand_r')) { /* warn */ }
        ```

## Following a part

There is no follower object to attach — the player hands you a part's pose and *you* place your own
thing with it, every frame. This is deliberate: the same reading drives a DOM overlay, a second
canvas, or a quad in your own render pass, and the player stays a renderer rather than a scene graph.
Two rules: read the pose **after the update**, and **compose it into your space** yourself.

`part_transform` is a six-term affine in **animation-local, Y-down** space. To reach the screen you
pass it through the same two transforms the renderer does — the player's own placement, then the
projection:

```
screen  ←  projection  ←  the player's set_transform  ←  part_transform
```

The part's pivot is the translation pair `(tx, ty)` — the last two of the six numbers.

!!! note "The pose carries no skew"
    A part's **skew** is a per-part vertex deform (SpriteStudio's shear), applied to the part's own
    drawn geometry — not to its node transform — and it does not inherit to child parts. So
    `part_transform` reports the part's position, rotation and scale but never its shear, and a
    follower pinned to a part under a sheared parent is not itself sheared. This matches the
    SpriteStudio editor, where skew does not propagate down the tree.

### On the WebGPU canvas — a DOM marker

The built-in surface centres the animation and uses **1 world unit = 1 backing pixel, Y pointing
down** (no flip). So a world point lands on the canvas at *half the box, plus the point*, and a DOM
element over the canvas needs only the device-pixel-ratio divide:

```js
// A null / bone / joint part tracks its art; a skinned mesh does not (see the warning above),
// which is why this follows a bone in the hand chain rather than `hand_r` itself.
const PART = 'bone_14';
const dpr = window.devicePixelRatio || 1;
const s = 0.6, ox = 0, oy = 120 * dpr;   // the player's own placement: setTransform(ox, oy, s, s, 0)
player.setTransform(ox, oy, s, s, 0);

function frame(dt) {
  surface.frame(dt);                     // update + draw first
  const m = player.partTransform(PART);  // [m00, m01, m10, m11, tx, ty] — animation-local, Y-down
  if (m) {
    const wx = s * m[4] + ox;            // compose the player's placement (no rotation here)
    const wy = s * m[5] + oy;
    // world -> canvas CSS pixels: centre of the box, plus the point, over dpr. Y-down, no flip.
    marker.style.left = canvas.clientWidth  / 2 + wx / dpr + 'px';
    marker.style.top  = canvas.clientHeight / 2 + wy / dpr + 'px';
  }
  requestAnimationFrame(frame);
}
```

If you never call `setTransform`, the player is centred and unscaled, so it collapses to
`wx = m[4]`, `wy = m[5]`.

### In your own render pass — native / C

You already own a projection and a viewport, so there is nothing surface-specific: multiply your
projection by the player's root transform by the part affine — exactly what `SSScene` does — and use
the result to place your marker geometry. The six numbers are the same `[m00, m01, m10, m11, tx, ty]`
on every face; `ss_player_part_transform` writes them into a `float[6]`.

### Bevy

Not yet exposed. The Bevy adapter surfaces placement, playback and events, but not per-part poses, so
there is no built-in part-follow path there today.

The **part-follow demo** (`examples/web/part-follow.html`) drives exactly this on Ringo: pick a part
and watch a marker track it, with the skinned parts flagged.

## Next

* [Events and user data](./events.md) — the other half of "react to what the animation is doing"
* [Motion blending](./motion-blending.md) — blending whole animations rather than parts
