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
    playback.alpha = 0.5;   // tint is not exposed as a component
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
  returns, so code ported from the WebGL player reads across unchanged;
* they are in **animation-local, Y-down space** — neither the character's own `set_transform` nor
  the target's projection is applied. Compose those yourself, which is what lets you place the
  result in whatever space your UI lives in;
* read them **after** the update, because they describe the frame it produced. Hidden parts still
  report where they would have been.

## Next

* [Events and user data](./events.md) — the other half of "react to what the animation is doing"
* [Motion blending](./motion-blending.md) — blending whole animations rather than parts
