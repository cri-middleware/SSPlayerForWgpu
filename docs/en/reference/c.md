---
title: C API reference
---

# C API reference

Every entry point in `ssplayer_wgpu.h`, grouped by what it is for. The header itself carries the
per-function detail and the safety notes; this page is the map.

Get the header from a [release zip or a source build](../start/cpp.md#install). It is C99 and has
no dependencies.

## Conventions

| | |
|---|---|
| Return values | Functions that can fail return `int32_t`: `SS_OK` (0), `SS_ERROR_INVALID_ARGUMENT` (-1), `SS_ERROR_GPU` (-2) |
| Error text | `ss_last_error()` — the last failure on the calling thread. For logging, not for branching |
| Null handles | Every function tolerates a null handle and answers `SS_ERROR_INVALID_ARGUMENT` (or a zero value) rather than crashing |
| Strings | `const char *` in, UTF-8. Strings *out* are owned by the scene and valid until the next call of the same kind — copy what you keep |
| Sizes | Always **drawable pixels**, never points. Scale by the display factor yourself |

## Lifetime

| | |
|---|---|
| `ss_scene_create(desc)` | Build a scene on a window. Returns null on failure |
| `ss_scene_destroy(scene)` | Tear it down — **and every player in it** |
| `ss_scene_create_player(scene)` | Add a character |
| `ss_player_destroy(player)` | Remove one. The handle box stays allocated until the scene goes, so a double destroy is a safe no-op |
| `ss_scene_player_count(scene)` | Diagnostic |

Handles are generation-tagged: a stale `ss_player_t *` resolves to "destroyed" rather than to
whichever character took its slot.

## The frame

| | |
|---|---|
| `ss_scene_update(scene, delta_seconds)` | Advance every player, build this frame's geometry |
| `ss_scene_render(scene)` | One render pass for the whole cast, then present |
| `ss_scene_resize(scene, width, height)` | Drawable pixels |
| `ss_scene_stats(scene, out)` | See [Performance](../guide/performance.md) |
| `ss_scene_is_device_lost(scene)` / `ss_scene_restore_device(scene)` | See [Device loss](../guide/device-loss.md) |

## Loading

| | |
|---|---|
| `ss_player_load(player, path)` | The one call: pack, atlases, sibling packs |
| `ss_player_load_ssab(player, name, data, len)` | Register bytes and select that pack |
| `ss_player_add_ssab(player, name, data, len)` | Register bytes only, for instance parts to find |
| `ss_player_missing_count(player)` / `ss_player_missing_at(player, i, out)` | What did not arrive. **Not an error** |
| `ss_player_texture_count(player, pack)` / `ss_player_texture_at(player, pack, i, out)` | What atlases a pack asks for, and where the exporter put them |
| `ss_scene_add_texture(scene, key, rgba, w, h)` | Supply an atlas as straight-alpha RGBA8, under `SsTextureRef::key` |
| `ss_scene_add_texture_png(scene, key, data, len)` | ... or as encoded PNG bytes, decoded by the library |

Textures register on the **scene**, because the texture map is device-scoped: every player in the
window sees it, and a pack shown five times is uploaded once.

## Selecting and describing an animation

| | |
|---|---|
| `ss_player_set_animation(player, name)` | Build the playback tree. The allocating call |
| `ss_player_animation_count(player)` / `ss_player_animation_name(player, i)` | What the pack offers |
| `ss_player_canvas_size(player, &w, &h)` | The authored canvas, for laying out around it |
| `ss_player_total_frames(player)` / `ss_player_start_frame` / `ss_player_end_frame` | The current range |

## Transport

| | |
|---|---|
| `ss_player_play(player)` | **Head-out** — rewinds and re-arms the loop count |
| `ss_player_play_from(player, frame)` | The same, from a frame |
| `ss_player_pause(player)` / `ss_player_resume(player)` | Hold, and carry on in place |
| `ss_player_stop(player)` | End; the playhead stays where it is |
| `ss_player_seek(player, frame)` | Jump. Fractional values are honoured |
| `ss_player_set_speed_scale(player, speed)` | A multiple of the authored rate. `0` and negatives stop |
| `ss_player_set_frame_rate(player, fps)` / `ss_player_frame_rate` | Replace the authored rate |
| `ss_player_set_loop_count(player, n)` / `ss_player_loop_count` / `ss_player_remaining_loops` | `n` cycles; `-1` or `0` forever |
| `ss_player_set_animation_section(player, start, end)` | Restrict the playhead. Call it **after** `set_animation` |
| `ss_player_set_playback_direction(player, dir, style)` | `0`/`1` = forward/backward, normal/ping-pong |
| `ss_player_playback_direction` / `ss_player_playback_style` | Read them back |
| `ss_player_set_sub_frame_enabled(player, on)` / `ss_player_sub_frame_enabled` | Interpolate between authored frames |
| `ss_player_set_frame_skip_enabled(player, on)` / `ss_player_frame_skip_enabled` | Cross several frames in one long tick |

See [Playback and transport](../guide/playback.md) — in particular why `play` is not how you lift a
`pause`.

## Asking what playback did

| | |
|---|---|
| `ss_player_is_playing` / `ss_player_is_pausing` / `ss_player_is_playing_forward` | State |
| `ss_player_is_finished` | **Sticky**: every configured loop has played. Never true under an infinite loop |
| `ss_player_just_started` | **Edge**: the first update after `ss_player_play` / `ss_player_play_from`. Not raised by `ss_player_resume` |
| `ss_player_just_finished` | **Edge**: true for exactly the update that completed. Not raised by `stop` |
| `ss_player_frame_no` | Where the playhead is |
| `ss_player_just_looped` | **Edge**: the last update crossed a loop boundary. One report however many cycles it crossed |

## Events

| | |
|---|---|
| `ss_player_event_count(player)` | How many the last update crossed — **including frames it skipped over** |
| `ss_player_event_at(player, i, out)` | One `SsEventData`: `kind` says which fields carry anything |
| `ss_player_event_signal_param_count(player, i)` / `ss_player_event_signal_param_at(player, i, p, out)` | A signal's parameters |

`kind` is `SS_EVENT_USER_DATA` (0), `SS_EVENT_SIGNAL` (1) or `SS_EVENT_AUDIO` (2); a signal
parameter's `value_type` is one of `SS_SIGNAL_VALUE_NONE` / `_BOOL` / `_INDEX` / `_INTEGER` /
`_FLOATING` / `_TEXT` (0–5). See [Events and user data](../guide/events.md).

## Placing a character

| | |
|---|---|
| `ss_player_set_transform(player, x, y, sx, sy, rotation)` | Position, scale, rotation |
| `ss_player_set_order(player, order)` | Draw order within the scene, ascending |
| `ss_player_set_visible(player, visible)` | Hidden characters **do not advance** and report no events |
| `ss_player_set_alpha(player, a)` / `ss_player_set_tint(player, rgb)` | Whole-character alpha and tint |

## Parts and overrides

Every override has a `_by_index` twin that skips the name lookup; resolve the index once with
`ss_player_find_part_index` and keep it.

| | |
|---|---|
| `ss_player_part_count` / `ss_player_find_part_index` | Enumerate and resolve |
| `ss_player_part_transform(player, part, out)` (+ `_by_index`) | Six floats: `[m00, m01, m10, m11, tx, ty]`, animation-local Y-down |
| `ss_player_is_part_skinned_mesh(player, part)` (+ `_by_index`) | `1` skinned, `0` not, `-1` for an unknown part or no pack bound |
| `ss_player_set_part_visibility_override(player, part, hide, cascade)` (+ `_by_index`, + `clear_`) | A keyframe hide never cascades, so this is the only way to hide a sub-tree |
| `ss_player_set_part_color_override(player, part, rgba, blend_op, priority)` (+ `_corners`, `_by_index`, `clear_`) | `blend_op` 0–3, `priority` 0–2 |
| `ss_player_set_part_cell_override(player, part, cellmap, cell, priority)` (+ `_by_index`, `clear_`) | Draw a different image |
| `ss_player_clear_all_part_overrides(player)` | Reset the lot |

See [Part overrides](../guide/part-overrides.md) for what `blend_op` and `priority` mean.

## Runtime texture replacement

Swap the whole texture a cell map binds — a costume or palette change — from your program. `pack`
null selects the main pack.

| | |
|---|---|
| `ss_player_cellmap_texture_count(player, pack)` / `ss_player_cellmap_texture_name(player, pack, i)` | The replaceable names: cell maps and TextureChange targets both |
| `ss_player_set_cellmap_texture(player, pack, name, rgba, w, h)` | Replace with straight-alpha RGBA8; **`rgba` NULL restores** the original |
| `ss_player_is_cellmap_overridden(player, pack, name)` | Whether one is currently overridden |

The replacement should match the original atlas's size and cell layout, and reaches the GPU on the
next scene update. See [Texture replacement](../guide/texture-replacement.md).

## Not on this face

**Motion blending** (the track stack) is Rust-only, and so is drawing into a render pass you own —
this face builds and owns its device. See [Scopes and names](../guide/concepts.md#what-each-face-can-do)
for the full matrix, and
[Limitations](../limitations.md#no-device-sharing-across-the-c-abi) for why device sharing is not
offered here.
