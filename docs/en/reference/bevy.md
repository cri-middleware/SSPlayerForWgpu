---
title: Bevy API reference
---

# Bevy API reference

The whole authoring surface is one plugin, one asset, four components (one of them optional), two
state components, four feedback messages and four command messages. Generated docs:
<a href="../api/ssplayer_bevy/index.html" target="_blank" rel="noreferrer">**rustdoc for `ssplayer_bevy`**</a>.

**Bevy 0.20.x, and no other line** — see [Get started with Bevy](../start/bevy.md#supported-bevy).

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
| `animation: String` | animation to select. Empty means "not yet" | The same, unless `transition_ms` is set. Only a change of value acts, so a system may write it every frame |
| `transition_ms: f32` | how a change of `animation` is made: `0` (the default) switches at once; above zero crossfades over that many milliseconds | The first animation, and the first after a change of `pack`, is always switched to at once |

`animation` is the only way to change track 0's animation: a `TrackCommand` that would lay one on
track 0 is refused, so what it names and what track 0 plays never part.

`SsTransform`, `SsPlayback`, `SsPlaybackState` and `SsTrackState` are **required components**, so
spawning `SsPlayer` alone gives you all five at their defaults.

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
* `SsPackLoader` is the loader the plugin registers for `.ssab`. Only the `.ssab` itself can fail
  to load — `SsPackLoadError::Io` when it cannot be read, `::Parse` when it is not a valid pack —
  and Bevy reports that as an `AssetLoadFailedEvent<SsPack>`. A sibling pack that does not arrive
  is not an error: the instance parts playing it do not appear.

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
| `speed_scale: f32` | `1.0` | A multiple of the authored rate. `0` and negatives stop |
| `order: i32` | `0` | Draw order; lower draws first. Ties keep extraction order |
| `alpha: f32` | `1.0` | Whole-character fade |
| `tint: u32` | `0xFFFFFF` | Whole-character tint, `0xRRGGBB`, multiplied in after PartsColor |
| `visible: bool` | `true` | False skips the draw; the timeline keeps running |
| `culled: bool` | `false` | True skips **the timeline advance** as well as the draw — for an off-screen character |
| `restart: u32` | `0` | **Bump it** (any change of value) to replay from the head, re-arming the loop count |
| `direction: PlaybackDirection` | `Forward` | Which way the playhead walks |
| `style: PlaybackStyle` | `Normal` | `Normal` wraps at the far end, `PingPong` turns back |
| `loop_count: i32` | `-1` | Cycles before stopping; `-1` or `0` loops for ever. A change re-arms the count |
| `frame_rate: i32` | `0` | Replaces the authored rate; `0` keeps it. Prefer `speed_scale` |
| `frame_skip_enabled: bool` | `true` | Off advances one frame per tick instead of keeping to the wall clock |
| `sub_frame_enabled: bool` | `true` | Off floors the frame that is drawn |
| `animation_section: Option<(i32, i32)>` | `None` | Restrict the playhead to `start..=end` of the current animation. Setting it puts the playhead at its start |

`restart` is a generation counter rather than a flag so a host can re-trigger a one-shot at will
with nothing to clear afterwards, and so two restarts in a row are two restarts.

### `SsPlaybackState` — what playback did (polled)

| Field | |
|---|---|
| `frame_no: f32` | the frame the timeline last landed on |
| `finished: bool` | whether the animation has reached its end |
| `playing_forward: bool` | which way the playhead is actually travelling — false on a ping-pong's return leg too. Gate audio on it |

### `SsTrackState` — the track stack (polled) { #sstrackstate }

What [`SsTrackCommand`](#motion-blending) built, refreshed every frame and **one frame behind**.

| Field | |
|---|---|
| `tracks: Vec<SsTrackInfo>` | one per track, track 0 first: `main_source`, `alpha`, `blend`, `sync`, `frame`, `fade_progress` (`None` when not fading), the playback settings `speed`, `frame_rate`, `frame_skip_enabled`, `direction` and `style`, `loop_count` (`None` on an empty track), `finished`, `just_looped` and `outgoing_event_threshold` |
| `sources: Vec<SsSourceInfo>` | the sources laid with `TrackCommand::AddSource` that are still playing: `source` (the handle), `track`, `animation`, `weight`, `frame` |

### `SsCellmapTextures` — runtime texture replacement (optional)

Swap the whole texture a cell map binds — a costume or palette change — for a Bevy image.

| Field | |
|---|---|
| `overrides: HashMap<String, Handle<Image>>` | cell map (or TextureChange-target) name → the image to bind for it |

Insert an entry to override, remove it (or the whole component) to restore. An image still loading
is applied the frame it becomes ready, so a freshly-requested handle is fine. The swap is
per-character, and assumes a straight-alpha Bevy image **loaded with `is_srgb: false`** — an sRGB one
draws too dark, and the adapter warns once. Only characters that swap textures need spawn it. See
[Texture replacement](../guide/texture-replacement.md).

## Messages (reactive)

| Message | Carries |
|---|---|
| `SsEventMessage` | `entity`, `event: SsEvent` — user data, signal or audio |
| `SsStarted` | `entity` — the start **edge**: a new animation or pack while `playing`, `playing` turned on for the first time, or a bumped `restart`. Not a resume |
| `SsFinished` | `entity` — the completion **edge** |
| `SsLooped` | `entity` — the loop boundary edge |

Three commands go the other way, from gameplay to the character. They are messages rather than
fields, because each happens once rather than being a state the character is in. One sent before the
pack has loaded is applied once the animation is set up, and of several of a kind in one frame, the
last wins:

| Message | Carries |
|---|---|
| `SsPlay` | `entity`, `start_frame: Option<f32>` — **play**: rewind to the head of the section, or to `start_frame`, re-arm the loop count and start, even a finished or stopped animation. `None` is what bumping `restart` does. `playing` still governs afterwards |
| `SsStop` | `entity` — **stop**: end playback where it stands. The frame stays drawn, and turning `playing` off and on does not start it again; `SsPlay` or `restart` does |
| `SsSeek` | `entity`, `frame: f32` — move the playhead there. Fractional frames are honoured, and it stays inside the section |

Sent together in one frame they apply as play, stop, seek.

### Motion blending { #motion-blending }

`SsTrackCommand { entity, command }` performs one operation on the character's track stack;
`command` is a `TrackCommand`, each variant the `SSPlayer` call of the same name — `SetAnimation`,
`Crossfade`, `FadeTrack`, `CancelFade`, `Clear`, `SetAlpha`, `SetBlend`, `SetSync`, `SetFrame`,
`SetSpeed`, `SetFrameRate`, `SetFrameSkipEnabled`, `SetPlaybackDirection`, `SetLoopCount`,
`SetOutgoingEventThreshold`, `AddSource`, `SetSourceWeight`, `SetSourceFrame`,
`SetSourceEventThreshold`. `SetAnimation` and `Crossfade` are refused on track 0, which
`SsPlayer::animation` sets, and so are `SetSpeed`, `SetFrameRate`, `SetFrameSkipEnabled`,
`SetPlaybackDirection` and `SetLoopCount`: track 0's playback settings and loop count are
`SsPlayback`'s. Unlike the
three above, **every one applies, in the order written**, and ones sent before the pack has loaded
wait for the animation. A refused one is logged. The result comes back in
[`SsTrackState`](#sstrackstate), where an `AddSource` finds its handle.

```rust
fn run(mut tracks: MessageWriter<SsTrackCommand>, character: Single<(Entity, &mut SsPlayer)>) {
    let (entity, mut player) = character.into_inner();
    tracks.write(SsTrackCommand { entity, command: TrackCommand::SetSync { track: 0, sync: TrackSync::Start } });
    // Track 0 crossfades through the component.
    player.transition_ms = 300.0;
    player.animation = "run".into();
}

fn react(mut tracks: MessageWriter<SsTrackCommand>, character: Single<Entity, With<SsPlayer>>) {
    let entity = *character;
    for command in [
        TrackCommand::SetAnimation { track: 1, animation: "hit_react".into() },
        TrackCommand::SetLoopCount { track: 1, count: 1 },   // once, then hold the last frame
    ] {
        tracks.write(SsTrackCommand { entity, command });
    }
}
```

**Changing `SsPlayer::animation` with `transition_ms` at 0 resets the stack** to track 0 alone, as
`set_animation` does; send the overlays and track settings again after it. With `transition_ms`
set the stack stays, as `crossfade` leaves it. See
[Motion blending](../guide/motion-blending.md).

```rust
fn jump(mut seeks: MessageWriter<SsSeek>, character: Single<Entity, With<SsPlayer>>) {
    seeks.write(SsSeek { entity: *character, frame: 18.0 });
}
```

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
| `Render` / `RenderSystems::Prepare` | applies them to the playback trees; rebuilds a character whose `pack` or `animation` changed |
| `Core2d` / `Core2dSystems::MainPass` | advances, builds and draws — ordered **after** `main_transparent_pass_2d` |

That last ordering is an explicit edge, not a coincidence of set membership: two systems that each
take a `RenderContext` get their own command encoder, and only an explicit edge decides which
buffer reaches the queue first. Without it, whether characters land over or under Bevy's sprites
would be a race.

## What this adapter does not do yet

| | |
|---|---|
| **One view** | The adapter holds one `SSScene` with every character in it, and `SSScene::update` fuses the timeline advance with the geometry build, so a second view would advance every animation twice per frame. Later views are skipped, with a warning |
| **MSAA** | must be off. The adapter warns once and skips a multisampled target rather than crashing |
| **Part overrides** | not exposed as components. See [Scopes and names](../guide/concepts.md#what-each-face-can-do) |

## Masks and device loss

Masks use the **coverage** backend automatically, because Bevy's 2D depth texture carries no
stencil; the cost is one extra render pass per masking character per frame
([Masks](../guide/masks.md)).

Device loss recovers by riding Bevy's own recovery, but **you must opt in** with a
`RenderErrorHandler` returning `RenderErrorPolicy::Recover`
([Device loss](../guide/device-loss.md)).
