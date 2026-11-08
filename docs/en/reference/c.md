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
| Buffers you pass in | Read during the call and not retained: `data`, `rgba`, `members`, `corners` and strings may be freed or reused when it returns |
| Threads | A scene and its players take one call at a time: serialise calls from different threads, a resize from the UI thread and a render from a render thread included. On macOS, `ss_scene_create` and the `NSView` / `CAMetalLayer` it is given belong to the main thread |
| Return values | Functions that can fail return `int32_t`: `SS_OK` (0), `SS_ERROR_INVALID_ARGUMENT` (-1), `SS_ERROR_GPU` (-2), or, when the runtime refuses a request (an unreadable `.ssab`, an unknown animation, a part index out of range, a bad section), its own positive code (1–7 or 99). A part name the pack does not have is `SS_ERROR_INVALID_ARGUMENT`. Test against `SS_OK`, not a specific code |
| Error text | `ss_last_error()` — the last failure on the calling thread. For logging, not for branching |
| Versions | `ss_version()` — this library's version. `ss_sdk_version()` — the SpriteStudio-SDK version it links, written as its release tag (`v7.0.0`), or `local` for a build without `scripts/build.sh`. Both static; never free them |
| Null handles | Every function tolerates a null handle and answers `SS_ERROR_INVALID_ARGUMENT`, or for a getter what a player with no animation reads back, rather than crashing. That is zero for most getters, but `1.0` for the speed scale and alpha, true for `ss_player_is_playing_forward`, `ss_player_sub_frame_enabled`, `ss_player_frame_skip_enabled` and `ss_player_is_visible`, `0xFFFFFF` for `ss_player_tint`, and `-1` for the part lookups (`ss_player_find_part_index`, `ss_player_is_part_hidden`, `ss_player_is_part_skinned_mesh` and their `_by_index` forms) |
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
| `ss_scene_set_group_alpha(scene, on)` / `ss_scene_group_alpha(scene)` | Fade each faded player as one layer. See [Whole-character tint and alpha](../guide/part-overrides.md#whole-character-tint-and-alpha) |
| `ss_scene_stats(scene, out)` | See [Performance](../guide/performance.md) |
| `ss_scene_is_device_lost(scene)` / `ss_scene_restore_device(scene)` | See [Device loss](../guide/device-loss.md) |

## Loading

| | |
|---|---|
| `ss_player_load(player, path)` | The one call: pack, atlases, sibling packs |
| `ss_player_load_ssab(player, name, data, len)` | Register bytes and select that pack |
| `ss_player_add_ssab(player, name, data, len)` | Register bytes only, for instance parts to find |
| `ss_player_missing_count(player)` / `ss_player_missing_at(player, i, out)` | What did not arrive. **Not an error**. `kind` is `SS_MISSING_TEXTURE` (0, an atlas: its parts draw as silhouettes) or `SS_MISSING_PACK` (1, a sibling pack: its instance parts do not appear); `name` is the path or pack name |
| `ss_player_texture_count(player, pack)` / `ss_player_texture_at(player, pack, i, out)` | What atlases a pack asks for, and where the exporter put them |
| `ss_scene_add_texture(scene, key, rgba, w, h)` | Supply an atlas as straight-alpha RGBA8, under `ss_texture_ref_t.key` |
| `ss_scene_add_texture_png(scene, key, data, len)` | ... or as encoded PNG bytes, decoded by the library |

Textures register on the **scene**, because the texture map is device-scoped: every player in the
window sees it, and a pack shown five times is uploaded once.

A texture the device cannot hold — wider or taller than its `max_texture_dimension_2d`, which is 8192
on the device this library creates — is refused with `SS_ERROR_INVALID_ARGUMENT` and uploads
nothing. `ss_player_set_cellmap_texture` refuses one the same way.

## Selecting and describing an animation

| | |
|---|---|
| `ss_player_set_animation(player, name)` | Build the playback tree. The allocating call |
| `ss_player_animation_count(player)` / `ss_player_animation_name(player, i)` | What the pack offers |
| `ss_player_current_animation(player)` | The one set up, or null |
| `ss_player_part_name(player, i)` | Part names in part-index order; null past the last |
| `ss_player_cellmap_count` / `ss_player_cellmap_name(player, i)` / `ss_player_cell_count(player, cellmap)` / `ss_player_cell_name(player, cellmap, i)` | The names a cell override takes |
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
| `ss_player_set_speed_scale(player, speed)` / `ss_player_speed_scale` | A multiple of the authored rate. `0` and negatives stop |
| `ss_player_set_frame_rate(player, fps)` / `ss_player_frame_rate` | Replace the authored rate |
| `ss_player_set_loop_count(player, n)` / `ss_player_loop_count` / `ss_player_remaining_loops` | Set `n` cycles, or `-1` or `0` for forever. Reading back, forever is `-1`, and `0` means no animation is set |
| `ss_player_set_animation_section(player, start, end)` | Restrict the playhead. Call it **after** `set_animation`. An end before the start, both in range, is refused, changing nothing |
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
| `ss_player_event_at(player, i, out)` | One `ss_event_data_t`: `kind` says which fields carry anything |
| `ss_player_event_signal_param_count(player, i)` / `ss_player_event_signal_param_at(player, i, p, out)` | A signal's parameters |

`kind` is `SS_EVENT_USER_DATA` (0), `SS_EVENT_SIGNAL` (1) or `SS_EVENT_AUDIO` (2); a signal
parameter's `value_type` is one of `SS_SIGNAL_VALUE_NONE` / `_BOOL` / `_INDEX` / `_INTEGER` /
`_FLOATING` / `_TEXT` (0–5). See [Events and user data](../guide/events.md).

## Placing a character

| | |
|---|---|
| `ss_player_set_transform(player, x, y, sx, sy, rotation)` | Position in pixels from the view centre (`y` measured **downward**), scale, rotation in radians |
| `ss_player_reset_transform(player)` | Back to the identity: centred, unscaled, unrotated |
| `ss_player_fit_into(player, width, height, &placement, &placed)` | Fit the animation into a box and write the transform; `placed` may be null |
| `ss_placement_default(&placement)` | Fill an `ss_placement_t` before changing it — **a zeroed one has scale 0** |
| `ss_player_canvas_rect(player, &rect)` | The box `ss_player_fit_into` fits when `has_frame` is false |
| `ss_player_set_order(player, order)` / `ss_player_order(player)` | Draw order within the scene, ascending |
| `ss_player_set_visible(player, visible)` / `ss_player_is_visible(player)` | Hidden characters draw nothing but keep playing and reporting events |
| `ss_player_set_culled(player, culled)` / `ss_player_is_culled(player)` | Culled characters **do not advance** and report no events |
| `ss_player_set_alpha(player, a)` / `ss_player_set_tint(player, rgb)` | Whole-character alpha and tint. Read back with `ss_player_alpha` / `ss_player_tint` |
| `ss_player_set_crowd(player, members, count)` / `ss_player_crowd_count(player)` | Draw the character at each `ss_crowd_member_t`'s placement; `count` 0 draws it once. See [Crowds](../guide/crowds.md) |

`fit` is one of `SS_FIT_CONTAIN` / `_COVER` / `_NONE` (0–2) and `align` one of `SS_ALIGN_CENTER`,
`_TOP`, `_BOTTOM`, `_LEFT`, `_RIGHT`, `_TOP_LEFT`, `_TOP_RIGHT`, `_BOTTOM_LEFT`, `_BOTTOM_RIGHT`
(0–8). See [Fitting into a box](../guide/placement.md).

## Parts and overrides

Every override has a `_by_index` twin that skips the name lookup; resolve the index once with
`ss_player_find_part_index` and keep it.

| | |
|---|---|
| `ss_player_part_count` / `ss_player_find_part_index` | Enumerate and resolve |
| `ss_player_part_transform(player, part, out)` (+ `_by_index`) | Six floats: `[m00, m01, m10, m11, tx, ty]`, animation-local Y-down |
| `ss_player_is_part_hidden(player, part)` (+ `_by_index`) | `1` hidden on the displayed frame, `0` shown, `-1` for an unknown part or no animation set up |
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

## Motion blending { #motion-blending }

The track stack: tracks fold onto the setup pose in ascending order, track 0 being what
`ss_player_set_animation` establishes. See [Motion blending](../guide/motion-blending.md).

| | |
|---|---|
| `ss_player_set_track_animation(player, track, name)` | Lay an animation on a track, replacing what it played |
| `ss_player_crossfade(player, track, name, ms)` | Fade a track over to another animation. On track 0 it is the current animation from the call on |
| `ss_player_fade_track(player, track, target, ms)` / `ss_player_cancel_track_fade(player, track)` | Drive a track's alpha somewhere / stop the fade where it stands |
| `ss_player_track_fade_progress(player, track)` | 0 to 1; **-1** when the track is not fading |
| `ss_player_set_track_alpha` / `ss_player_track_alpha` | A track's strength |
| `ss_player_set_track_blend` / `ss_player_track_blend` | `SS_TRACK_BLEND_MIX` (0) or `SS_TRACK_BLEND_ADDITIVE` (1) |
| `ss_player_set_track_sync` / `ss_player_track_sync` | `SS_TRACK_SYNC_FREE` (0) or `SS_TRACK_SYNC_START` (1) — set it before laying |
| `ss_player_set_track_frame` / `ss_player_track_frame` / `ss_player_set_track_speed` | Scrub a track; speed 0 parks it |
| `ss_player_track_speed` / `ss_player_set_track_frame_rate` / `ss_player_track_frame_rate` / `ss_player_set_track_frame_skip_enabled` / `ss_player_track_frame_skip_enabled` | A track's own playback settings; track 0's are the transport calls |
| `ss_player_set_track_playback_direction(player, track, direction, style)` / `ss_player_track_playback_direction` / `ss_player_track_playback_style` | `0` / `1`, as `ss_player_set_playback_direction` |
| `ss_player_set_track_loop_count` / `ss_player_track_loop_count` | The loop count of a track's animation — set it after laying one; `-1` loops forever, **0** means no animation |
| `ss_player_is_track_finished` / `ss_player_track_just_looped` | A track's end (a state) and its loop (one update) |
| `ss_player_set_track_outgoing_event_threshold` / `ss_player_track_outgoing_event_threshold` | How far into a crossfade the outgoing side keeps firing events; infinity (the default) silences it |
| `ss_player_track_count(player)` / `ss_player_clear_track(player, track)` | How many tracks; silence one (not track 0) |
| `ss_player_track_main_source(player, track)` | The source carrying a track's animation; **`SS_INVALID_SOURCE`** when it has none |
| `ss_player_add_track_source(player, track, name, weight, &source)` | Lay another animation beside it, mixed by weight |
| `ss_player_set_source_weight` / `_frame` / `_event_threshold`, and the reads without `set_` | One source's weight, playhead and event threshold |

Commands return a status. A read of a track or source that does not exist answers **0**, so check
`ss_player_track_count` first; the two exceptions are marked above, where 0 is a real answer.
`ss_event_data_t.source` says which source fired an event.

## Not on this face

Drawing into a render pass you own is not offered on this face: it builds and owns its device. Only the Rust face, and the Bevy adapter built on it, draw into a pass they are lent. See [Scopes and names](../guide/concepts.md#what-each-face-can-do)
for the full matrix, and
[Limitations](../limitations.md#no-device-sharing-across-the-c-abi) for why device sharing is not
offered here.
