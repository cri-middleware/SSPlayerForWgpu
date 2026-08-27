# Migrating from SS6Player for CPP

Moving from [SS6Player for CPP](https://github.com/SpriteStudio/SS6PlayerForCpp) means three changes
at once: the implementation language (C++ → a Rust core), the graphics backend (fixed OpenGL → wgpu:
WebGPU / Vulkan / DX12 / Metal), and the asset format (`.ssfb2` → `.ssab`).

It is not a drop-in library swap. Budget for **reconverting your assets** and **rewriting the host
code** — but the host code gets considerably smaller, and this page is the map from old to new.

!!! tip "Staying in C++ is fine"
    The [C API](../reference/c.md) is a first-class face, not an afterthought, and existing DX12 /
    Metal / Vulkan applications drive it directly. Both the Rust and the C form are shown throughout.

## 1. Assets

| | SS6Player for CPP | This player |
|---|---|---|
| Format | `.ssfb2` (FlatBuffers) | **`.ssab`** (SpriteStudio Anime Binary) |
| Granularity | one project, one (large) file | **one file per animation pack** |
| Textures | always external PNG | **reference** (external) or **embedded** |
| Converter | SS6Converter | **`ssconverter-cli`** (in the SDK) or [SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) |
| Name lookup | string comparison | **`uint32` hashes** |

!!! danger "`.ssfb2` cannot be read"
    Reconvert from the original `.sspj`. See [Preparing assets](../start/assets.md).

## 2. Lifecycle: from a global system to the borrow model

**SS6Player for CPP** had a global system layer: `SystemBootUp()`, then `SystemProcessMainPre()` /
`MainPost()` / `RenderPost()` every frame, with OpenGL state, buffers and textures owned inside the
player.

**This player has no global initialisation and no shutdown.** The wgpu `Device` / `Queue` /
`Surface` belong to your application, which lends them per frame — so the player rides along inside
any renderer or engine without locking you in. See [The borrow model](../wgpu/borrow-model.md).

| Role | SS6Player for CPP | Rust | C API |
|---|---|---|---|
| System init / shutdown | `SystemBootUp` / `SystemShutDown` | *(none)* | *(none)* |
| Device-scoped resources | *(internal, implicit)* | `SSRenderer` | *(inside `ss_scene_t`)* |
| Render target | *(no distinction)* | `SSScene` | `ss_scene_t` |
| One character | `Entity` | `SSPlayer` | `ss_player_t` |
| Asset ownership | `Project` | `SSPlayer` (holding `SSPack`s) | `ss_player_t` |

### Several characters, one pass

`Entity::Draw()` was per character. Here, **every character of a target goes through one update and
one render pass**:

```text
[SS6Player for CPP]
Entity A -> Draw()   (its own buffer upload and draw)
Entity B -> Draw()   (its own buffer upload and draw)

[this player]
SSScene -> update(&renderer, &device, &queue, dt, &mut [a, b])
        -> draw(&renderer, &mut rpass)      // one pass, one shared geometry arena
```

## 3. Loading

**Before**: read the `.ssfb2`, `Project.BootUp()`, then loop `Project.CountGetTexture()` times
loading each image yourself and registering it with `Project.TextureSet()`.

**Now**: one call, which resolves the pack, its atlases (external *or* embedded, including
TextureChange targets) and the sibling `.ssab` files its instance parts play from.

=== "Rust"

    ```rust
    player.load("assets/character/character.ssab")?;
    ```

=== "C"

    ```c
    ss_player_load(player, "assets/character/character.ssab");
    ```

Two things to know: `load` takes **no `Device` or `Queue`** — what it read reaches the GPU on the
first `update` — and a missing atlas is **not** an error, it is an entry in
`missing_assets()` / `ss_player_missing_count`. See [Loading and assets](../guide/loading.md).

## 4. Playback

### Tracks are gone; animations are named

`IndexGetAnimation("walk")` + `AnimationPlay(track, index, …)` becomes `set_animation("walk")` +
`play()`.

=== "Rust"

    ```rust
    player.set_animation("walk")?;
    player.set_loop_count(-1);       // -1 = forever
    player.play();
    ```

=== "C"

    ```c
    ss_player_set_animation(player, "walk");
    ss_player_set_loop_count(player, -1);
    ss_player_play(player);
    ```

=== "SS6Player for CPP"

    ```cpp
    int idx = entity.IndexGetAnimation("walk");
    entity.AnimationPlay(0, idx, 0);   // track 0, loop forever
    ```

!!! note "'Track' means something else here now"
    SS6's tracks were parallel animation slots on one entity. This player has a
    [track stack](../guide/motion-blending.md) too — but it is a *blending* stack (layer, weight,
    crossfade), reached through `set_track_animation` and friends on the Rust API, not through the
    transport calls.

### The mapping

| SS6Player for CPP | Rust | C API | Note |
|---|---|---|---|
| `IndexGetAnimation` + `AnimationPlay` | `set_animation(name)` + `play()` | `ss_player_set_animation` / `_play` | **Selecting and starting are separate** |
| `AnimationStop(track)` | `stop()` | `ss_player_stop` | The playhead stays where it is |
| `AnimationPause(track, true/false)` | `pause()` / `resume()` | `ss_player_pause` / `_resume` | **`play()` rewinds — it cannot un-pause** |
| `AnimationPlay(…, timesPlay)` | `set_loop_count(n)` | `ss_player_set_loop_count` | `-1` or `0` = forever |
| `RateTimeSet(track, rate)` | `set_speed_scale(f32)` | `ss_player_set_speed_scale` | A positive multiple; `0` and negatives stop |
| a negative `rateTime` | `set_playback_direction(Backward, …)` | `ss_player_set_playback_direction` | **Speed and direction are separate** |
| `KindStylePlay::PINGPONG` | `set_playback_direction(dir, PingPong)` | the same call | |
| `AnimationPlay(…, frameOffset)` / `CursorSet` | `seek(f32)` / `play_from(f32)` | `ss_player_seek` / `_play_from` | Fractional frames are honoured |
| `CursorGet(track)` | `frame_no()` | `ss_player_frame_no` | |
| `labelRangeStart` / `labelRangeEnd` | `set_animation_section(start, end)` | `ss_player_set_animation_section` | **By frame number** — label strings are not supported |
| `AnimationPlay(…, framePerSecond)` | `set_frame_rate(fps)` | `ss_player_set_frame_rate` | |
| `StatusGetPlay(track)` | `is_playing()` | `ss_player_is_playing` | True while paused, too |
| `StatusGetPause(track)` | `is_pausing()` | `ss_player_is_pausing` | |
| *(none)* | `is_playing_forward()` | `ss_player_is_playing_forward` | Which way the playhead is **actually** moving — false on ping-pong's return leg even when the direction reads Forward. **Gate audio on this** |
| `CountGetAnimation` / `NameGetAnimation` | `animation_names()` | `ss_player_animation_count` / `_name` | |
| `Entity::RateSetOpacity` | `set_alpha(f32)` | `ss_player_set_alpha` | |
| `RateSetScaleLocal` / `Draw`'s `matrixWorld` | `set_transform(x, y, sx, sy, rot)` | `ss_player_set_transform` | Rotation in radians |
| *(draw order was call order)* | `set_order(i32)` | `ss_player_set_order` | Ascending, within the scene |
| *(none)* | `set_visible(bool)` | `ss_player_set_visible` | Hiding **skips the timeline advance** |
| `CanvasGet()` | `canvas_size()` | `ss_player_canvas_size` | |

### Loop counts

| Meaning | SS6 `timesPlay` | `set_loop_count` |
|---|---|---|
| Forever | `0` | **`-1`** (preferred) or `0` |
| Once | `1` | `1` |
| n times | `n` | `n` |

!!! warning "What `set_animation` keeps, and what it drops"
    Speed, frame-rate override, playback direction and frame skip **survive** a `set_animation`, and
    this player deliberately restores the **loop count** as the sibling players do.

    What does **not** survive is the **playback section** — frame numbers mean nothing against a
    different clip. So re-apply `set_animation_section` after every `set_animation`, and do not
    assume the rest was reset.

## 5. Events: callbacks became polling

**Before**: register `CallBackUserData` / `CallBackSignal` / `CallBackPlayEnd` function pointers,
called *during* `Entity::Update`.

**Now**: read them after the update. Under the borrow model, handing control back to the application
mid-update would risk a conflict over the very resources it lent.

| SS6Player for CPP | Rust | C API |
|---|---|---|
| `CallBackUserData` | `events()` → `SsEvent::UserData` | `ss_player_event_at`, `kind == SS_EVENT_USER_DATA` |
| `CallBackSignal` | `events()` → `SsEvent::Signal` | the same, plus `ss_player_event_signal_param_at` |
| `CallBackPlayEnd` | `just_finished()` | `ss_player_finished_last_update` |
| *(new)* | `just_looped()` | `ss_player_looped_last_update` |
| *(new)* | `events()` → `SsEvent::Audio` | `ss_player_event_at`, `kind == SS_EVENT_AUDIO` |

=== "Rust"

    ```rust
    scene.update(&mut renderer, &device, &queue, dt, &mut [&mut player]);

    for event in player.events() {
        match event {
            SsEvent::UserData(e) => println!("user data: part={} frame={}", e.part_name, e.frame_no),
            SsEvent::Signal(e)   => println!("signal: {}", e.command_id),
            SsEvent::Audio(e)    => println!("audio: {}", e.sound_name),
        }
    }

    if player.just_looped()   { /* one cycle done */ }
    if player.just_finished() { /* every configured loop done */ }
    ```

=== "C"

    ```c
    ss_scene_update(scene, dt);

    uint32_t count = ss_player_event_count(player);
    for (uint32_t i = 0; i < count; ++i) {
        SsEventData e;
        if (ss_player_event_at(player, i, &e) != SS_OK) continue;
        if (e.kind == SS_EVENT_USER_DATA) {
            if (e.has_integer) { /* e.integer */ }
            if (e.has_text)    { /* e.text */ }
        }
    }

    if (ss_player_finished_last_update(player)) { /* ... */ }
    ```

Events belong to the update that produced them and are cleared by the next one. **Frames skipped
over under a long tick are all reported**, so nothing is lost when the frame rate drops. See
[Events and user data](../guide/events.md).

## 6. Parts

Part identification moves from `IDGetParts(name)` to `find_part_index(name)`, and per-part changes
are *overrides* with an explicit lifetime.

| SS6Player for CPP | Rust | C API |
|---|---|---|
| `IDGetParts(name)` | `find_part_index(name)` | `ss_player_find_part_index` |
| `CountGetParts()` | `part_count()` | `ss_player_part_count` |
| `HideSet(id, flag, cascade)` | `set_part_visibility_override(part, hide, cascade)` | `ss_player_set_part_visibility_override` |
| `CellChangeParts(id, map, cell, …)` | `set_part_cell_override(part, cellmap, cell, priority)` | `ss_player_set_part_cell_override` |
| *(none)* | `set_part_color_override(part, rgba, blend_op, priority)` | `ss_player_set_part_color_override` |
| *(none)* | `clear_all_part_overrides()` | `ss_player_clear_all_part_overrides` |
| `PartGetTransform(id, …)` | `part_transform(part)` | `ss_player_part_transform` |

Cell maps and cells are named by **string** here, not by index — and every override carries a
[priority](../guide/part-overrides.md#blend-op-and-priority) that says how long it lives.

## 7. A whole program, before and after

=== "SS6Player for CPP"

    ```cpp
    #include <ss6player_platform.h>
    #include <Entity/ss6player_entity.h>
    #include <Project/ss6player_project.h>

    SpriteStudio6::SystemBootUp();

    SpriteStudio6::Project project;
    project.BootUp(dataSsfb2, sizeSsfb2);
    for (int i = 0; i < project.CountGetTexture(); i++) {
        /* load the image yourself, then: */
        project.TextureSet(i, myTexture);
    }

    SpriteStudio6::Entity entity;
    entity.BootUp(&project, 0);
    entity.AnimationPlay(0, entity.IndexGetAnimation("walk"), 0);

    /* every frame */
    SpriteStudio6::SystemProcessMainPre();
    entity.Update(deltaTime);
    entity.Draw(matrixWorld);
    SpriteStudio6::SystemProcessMainPost();
    SpriteStudio6::SystemProcessRenderPost();
    ```

=== "C"

    ```c
    #include <ssplayer_wgpu.h>

    ss_scene_desc_t desc = {
        .window      = { .kind = SS_WINDOW_WIN32, .window = hwnd },
        .backend     = SS_BACKEND_AUTO,
        .width       = 960, .height = 540,
        .clear_color = { 0.1f, 0.1f, 0.12f, 1.0f },
        .enable_mask = true,
    };
    ss_scene_t *scene = ss_scene_create(&desc);

    ss_player_t *player = ss_scene_create_player(scene);
    ss_player_load(player, "assets/character/character.ssab");   /* textures included */
    ss_player_set_animation(player, "walk");
    ss_player_set_loop_count(player, -1);
    ss_player_play(player);

    /* every frame */
    ss_scene_update(scene, delta_seconds);
    ss_scene_render(scene);

    ss_scene_destroy(scene);   /* players go with it */
    ```

=== "Rust"

    ```rust
    use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig {
        color_format: surface_format,
        stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8),
        mask: MaskBackend::Stencil,
    });
    scene.set_viewport(&queue, width as f32, height as f32, false);

    let mut player = SSPlayer::new();
    player.load("assets/character/character.ssab")?;
    player.set_animation("walk")?;
    player.set_loop_count(-1);
    player.play();

    // every frame
    scene.update(&mut renderer, &device, &queue, dt, &mut [&mut player]);
    scene.draw(&renderer, &mut rpass);
    ```

## 8. Things SS6 had that work differently

| | |
|---|---|
| **Multiple tracks per entity** | Replaced by the [track stack](../guide/motion-blending.md), which blends rather than merely plays in parallel — Rust API only. From C or JavaScript, stack several characters and order them with `set_order` |
| **Sequencer (`.ssqb`)** | No equivalent. Detect `just_finished()` and `set_animation` the next one |
| **Per-part flip (`FlipSetParts`)** | Not available. Flipping follows the authored keyframes |
| **Effect seed (`SeedOffsetSet`)** | Not available. Seeds are decided per playback slot |
| **Label-named sections** | Not available. Use frame numbers with `set_animation_section` |

## What you gain

* one code path across **WebGPU, Vulkan, DX12 and Metal**;
* a much smaller host: no global system, no texture loop, one call to load;
* several characters per render pass, sharing one atlas upload;
* [motion blending](../guide/motion-blending.md), if you move to the Rust API.

## Next

* [Get started with C / C++](../start/cpp.md) — if you are staying in C++
* [Get started with Rust](../start/rust.md) — if you are moving over
* [Limitations & Scope](../limitations.md)
