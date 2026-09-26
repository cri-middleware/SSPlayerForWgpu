---
title: Bevy API reference
---

# Bevy API reference

The whole authoring surface is one plugin, three components, one state component and three
messages. Generated docs:
<a href="../api/ssplayer_bevy/index.html" target="_blank" rel="noreferrer">**rustdoc for `ssplayer_bevy`**</a>.

**Bevy 0.19.x, and no other line** — see [Get started with Bevy](../start/bevy.md#supported-bevy).

## Plugin

```rust
app.add_plugins(SsPlayerPlugin);
```

It adds the extract, prepare and draw systems, and owns the device-scoped renderer and the
target-scoped scene inside the render world. Nothing else is required.

The camera, however, needs two things: `Msaa::Off` (the player's pipelines are single-sampled) and
`CompositingSpace::Srgb` (or every character is visibly too bright).

## Components

### `SsPlayer` — what to play

| Field | | |
|---|---|---|
| `pack: Handle<SsPack>` | the `.ssab`, from `asset_server.load(...)` | **Changing it rebuilds the playback tree**, once the new pack has loaded |
| `animation: String` | animation to select. Empty means "not yet" | The same |

`SsTransform`, `SsPlayback` and `SsPlaybackState` are **required components**, so spawning
`SsPlayer` alone gives you all four at their defaults.

### `SsPack` — the `.ssab` as an asset

A `.ssab` loads through Bevy's `AssetServer`, from whatever source the app configures. The loader
runs the player's own walk: it reads the packs the pack's instance parts play from, and requests each
atlas as a Bevy `Image` — non-sRGB, because the player takes atlas bytes as authored.

* A character is built only once its pack and every atlas have settled, so it never shows a frame
  drawn without them. An atlas that failed to load draws white.
* Characters sharing a handle share one parse of each pack and one GPU texture per atlas, lent to the
  player rather than copied.
* With Bevy's `file_watcher` feature, changing the `.ssab` or a sibling pack rebuilds the characters
  playing it; changing an atlas re-lends it in place.
* `SsPack::pack()` is the parsed pack, for its name and `animation_names()`.

### `SsTransform` — where it sits

| Field | Default |
|---|---|
| `x`, `y` | `0.0` |
| `scale_x`, `scale_y` | `1.0` |
| `rotation` (radians) | `0.0` |

Free to change, every frame if you like.

### `SsPlayback` — transport and compositing

| Field | Default | |
|---|---|---|
| `playing: bool` | `true` | Pauses and resumes **in place**. Never rewinds, so it cannot replay a finished animation |
| `speed: f32` | `1.0` | A multiple of the authored rate |
| `order: i32` | `0` | Draw order; lower draws first. Ties keep extraction order |
| `alpha: f32` | `1.0` | Whole-character fade |
| `visible: bool` | `true` | False skips **the timeline advance** as well as the draw |
| `restart: u32` | `0` | **Bump it** (any change of value) to replay from the head, re-arming the loop count |

`restart` is a generation counter rather than a flag so a host can re-trigger a one-shot at will
with nothing to clear afterwards, and so two restarts in a row are two restarts.

### `SsPlaybackState` — what playback did (polled)

| Field | |
|---|---|
| `frame_no: f32` | the frame the timeline last landed on |
| `finished: bool` | whether the animation has reached its end |

### `SsCellmapTextures` — runtime texture replacement (optional)

Swap the whole texture a cell map binds — a costume or palette change — for a Bevy image.

| Field | |
|---|---|
| `overrides: HashMap<String, Handle<Image>>` | cell map (or TextureChange-target) name → the image to bind for it |

Insert an entry to override, remove it (or the whole component) to restore. An image still loading
is applied the frame it becomes ready, so a freshly-requested handle is fine. The swap is
per-character; a straight-alpha Bevy image is assumed. Only characters that swap textures need spawn
it. See [Texture replacement](../guide/texture-replacement.md).

## Messages (reactive)

| Message | Carries |
|---|---|
| `SsEventMessage` | `entity`, `event: SsEvent` — user data, signal or audio |
| `SsStarted` | `entity` — the start **edge**: a new animation or pack while `playing`, `playing` turned on for the first time, or a bumped `restart`. Not a resume |
| `SsFinished` | `entity` — the completion **edge** |
| `SsLooped` | `entity` — the loop boundary edge |

```rust
fn on_finish(mut finished: MessageReader<SsFinished>, mut commands: Commands) {
    for m in finished.read() {
        commands.entity(m.entity).despawn();
    }
}
```

!!! warning "Both feedback surfaces are one frame behind"
    Playback runs in the render world, after that frame's `Update`. What you read this frame is
    what playback produced last frame. For a sound cue or a state change that is imperceptible; a
    host that cannot tolerate it would need playback in the main world, which this adapter does not
    do.

## Where the work happens

| Schedule | What |
|---|---|
| Main world | the components above — the whole authoring surface |
| `ExtractSchedule` | copies them, plus the frame delta, into the render world |
| `Render` / `RenderSystems::Prepare` | applies them to the playback trees; loads a pack whose `src` changed |
| `Core2d` / `Core2dSystems::MainPass` | advances, builds and draws — ordered **after** `main_transparent_pass_2d` |

That last ordering is an explicit edge, not a coincidence of set membership: two systems that each
take a `RenderContext` get their own command encoder, and only an explicit edge decides which
buffer reaches the queue first. Without it, whether characters land over or under Bevy's sprites
would be a race.

## What this adapter does not do yet

| | |
|---|---|
| **One view** | `SSScene::update` fuses the timeline advance with the geometry build, so a second view would advance every animation twice per frame. Later views are skipped, with a warning |
| **MSAA** | must be off. The adapter warns once and skips a multisampled target rather than crashing |
| **Part overrides, motion blending** | not exposed as components. See [Scopes and names](../guide/concepts.md#what-each-face-can-do) |

## Masks and device loss

Masks use the **coverage** backend automatically, because Bevy's 2D depth texture carries no
stencil; the cost is one extra render pass per masking character per frame
([Masks](../guide/masks.md)).

Device loss recovers by riding Bevy's own recovery, but **you must opt in** with a
`RenderErrorHandler` returning `RenderErrorPolicy::Recover`
([Device loss](../guide/device-loss.md)).
