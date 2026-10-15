# Crowds

A crowd is one character drawn at many placements: one playhead, one evaluation of the animation,
one upload of its geometry and one character's draw calls, however many members it has. Its CPU
cost stays where one character's is as the crowd grows, where the same number of characters costs
that many updates. The GPU still draws every member's pixels.

=== "Rust"

    ```rust
    use ssplayer_wgpu::CrowdMember;

    player.set_crowd(&[
        CrowdMember { x: -200.0, ..CrowdMember::default() },
        CrowdMember { x: 0.0, alpha: 0.5, ..CrowdMember::default() },
        CrowdMember { x: 200.0, scale_x: -1.0, tint: 0x80c0ff, ..CrowdMember::default() },
    ]);
    player.set_crowd(&[]);   // back to one
    ```

=== "C"

    ```c
    /* x, y, scale_x, scale_y, rotation, alpha, tint */
    ss_crowd_member_t members[2] = {
        { -200.0f, 0.0f, 1.0f, 1.0f, 0.0f, 1.0f, 0xFFFFFFu },
        {  200.0f, 0.0f, 1.0f, 1.0f, 0.0f, 0.5f, 0x80C0FFu },
    };
    ss_player_set_crowd(player, members, 2);
    ss_player_set_crowd(player, NULL, 0);   /* back to one */
    ```

=== "JavaScript"

    ```js
    // Six numbers a member: x, y, scaleX, scaleY, rotation, alpha.
    player.setCrowd(new Float32Array([
      -200, 0, 1, 1, 0, 1,
       200, 0, 1, 1, 0, 0.5,
    ]), new Uint32Array([0xffffff, 0x80c0ff]));   // the tints, optional
    player.setCrowd(new Float32Array());          // back to one
    ```

=== "Bevy"

    ```rust
    use ssplayer_bevy::{CrowdMember, SsCrowd};

    commands.entity(character).insert(SsCrowd {
        members: vec![
            CrowdMember { x: -200.0, ..default() },
            CrowdMember { x: 200.0, alpha: 0.5, ..default() },
        ],
    });
    ```

## What a member is

| Field | Meaning |
|---|---|
| `x`, `y`, `scale_x`, `scale_y`, `rotation` | Where it stands, in `set_transform`'s terms — `y` downward, rotation in radians — composed **under** the character's own placement, so moving the character moves the whole crowd |
| `alpha` | 0..1, multiplied into the character's alpha |
| `tint` | `0xRRGGBB`, multiplied into the character's tint |

The default member stands where the character does, unfaded and untinted, so a crowd of one default
member draws exactly what no crowd does. A crowd holds up to 4096 members (`MAX_CROWD_MEMBERS`,
`SS_MAX_CROWD_MEMBERS`): the Rust face and Bevy leave the rest out, the C face refuses a longer list
with `SS_ERROR_INVALID_ARGUMENT`, and JavaScript throws.

The members are copied in. Set them again every frame to move them; on the Rust face and Bevy,
doing so allocates nothing unless the crowd grows.

## What members share

Everything but where they stand and how they are faded and tinted:

* **The frame.** Every member plays the same frame of the same animation, with the same tracks and
  overrides. Members at different points of an animation are separate characters.
* **Events**, reported once rather than once per member.
* **Effects.** Every member's particles are the same particles, placed at each member.
* **Masks.** Under the coverage backend each member is clipped by its own placement of the mask.
  Under the stencil backend — the C and JavaScript faces' — the members' masks are written together,
  so they clip correctly only where members do not overlap.
* **The character's own settings.** `set_order`, `set_visible`, `set_culled`, and the character's
  alpha and tint apply to the crowd as a whole. To hide one member, leave it out.

`part_transform` stays in the character's own space, without its placement or any member's: compose
a member's placement onto it to follow that member.

## Where members overlap

Each of the character's draws is issued once for every member before the next draw is. Where two
members overlap, a later part of the first can therefore cover an earlier part of the second. A
crowd laid out apart never shows it; members that must overlap and keep their own depth order are
separate characters.

With [group alpha](./part-overrides.md#whole-character-tint-and-alpha) on, a faded crowd is one
character, so it fades as one layer, and the members' own alphas still fade part by part.

## Next

* [Fitting into a box](./placement.md) — placing the character the crowd stands under
* [Performance](./performance.md) — what the frame statistics say a crowd costs
