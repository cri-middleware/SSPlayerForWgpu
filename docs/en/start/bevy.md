---
title: Get started with Bevy
---

# Get started with Bevy

Spawn a character as an entity, and Bevy draws it in its own 2D pass on the device it already owns.
`ssplayer-bevy` is an adapter, not a second player: the drawing is `ssplayer-wgpu`, and this
crate is the seam.

**You never touch wgpu here.** If you find yourself wanting a `SSRenderer`, you are on the
[Rust path](./rust.md), not this one.

## Supported Bevy

**0.20.x, and no other line.**

Bevy and the player must resolve to one `wgpu`: `bevy_render` 0.20 requires `wgpu ^30`, and the
player is pinned to the 30 line. Two majors of `wgpu` are two unrelated sets of types and no glue
bridges them — a mismatch is a compile error, not a runtime surprise. npm would call this a peer
dependency; cargo has no such thing, so the range is stated here and enforced by the workspace
resolving a single `wgpu`.

## Install

```bash
cargo add ssplayer-bevy
```

It pulls `ssplayer-wgpu` in; you do not depend on both. To use changes that are not released yet,
make it a git dependency on this repository (`ssplayer-bevy = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }`). To vendor the source instead, point at
`libs/ssplayer-bevy` inside a checkout of this repository and run
`git submodule update --init --recursive` there. If your own project is a Cargo workspace and the
checkout sits inside it, list it under `exclude`, as on the [Rust path](./rust.md#install).

## A whole app

```rust
use bevy::prelude::*;
use bevy::camera::CompositingSpace;
use bevy::render::view::Msaa;
use ssplayer_bevy::{SsPlayback, SsPlayer, SsPlayerPlugin, SsTransform};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(SsPlayerPlugin)
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // MSAA off: the player's pipelines are single-sampled.
    // CompositingSpace::Srgb: see "Colour" below — without it everything is too bright.
    commands.spawn((Camera2d, Msaa::Off, CompositingSpace::Srgb));

    commands.spawn((
        SsPlayer { pack: asset_server.load("Ringo/Ringo.ssab"), animation: "wait1".into(), ..default() },
        SsTransform { scale_x: 0.6, scale_y: 0.6, ..default() },
        SsPlayback { order: 1, ..default() },
    ));
}
```

`SsPlayer` requires `SsTransform` and `SsPlayback`, which you write, and `SsPlaybackState` and
`SsTrackState`, which the adapter writes back; spawning `SsPlayer` alone gives you all four at their
defaults.

| Component | Carries | Cost of changing it |
|---|---|---|
| `SsPlayer` | `pack`, `animation`, `transition_ms` | A new `pack` or `animation` rebuilds the playback tree — the allocating one. With `transition_ms` above 0, a new `animation` crossfades on the tree instead |
| `SsTransform` | `x`, `y`, `scale_x`, `scale_y`, `rotation` | Free |
| `SsPlayback` | `playing`, `speed_scale`, `order`, `alpha`, `tint`, `visible`, `culled`, `restart`, `direction`, `style`, `loop_count`, `frame_rate`, `frame_skip_enabled`, `sub_frame_enabled`, `animation_section` | Free |

It is **not** Bevy's `Transform`: that is a 3D affine, and decomposing it into the player's five
scalars is lossy. If you want the two linked, write the one system that copies across — you keep the
choice of which is authoritative.

## Where the files go

The pack loads through Bevy's `AssetServer`, so `"Ringo/Ringo.ssab"` is an asset path: put the
converted `.ssab` and the images beside it under your app's `assets/` folder, as the converter wrote
them ([Preparing assets](./assets.md); `Ringo` is in the official sample data). The character appears once the pack and its atlases have loaded — never a frame drawn without
them — and characters sharing a pack share one copy of it and one texture per atlas. With Bevy's
`file_watcher` feature, reconverting in SpriteStudio rebuilds the character from the new file.

## Colour: the camera needs `CompositingSpace::Srgb`

SpriteStudio atlases are authored in sRGB and the player writes those bytes verbatim into whatever
target it is given — it does no colour management, exactly like the native and C++ hosts. Bevy's
**default** 2D camera renders into an **sRGB** target, which encodes those bytes a second time on
write, so every character comes out visibly too bright.

`CompositingSpace::Srgb` makes Bevy pick a non-sRGB (`Rgba8Unorm`) target and composite in sRGB
space — which is the player's model. The adapter warns once if it finds
itself drawing into an sRGB target without it.

!!! warning "HDR is a separate case"
    An `Hdr` camera renders at `Rgba16Float` (linear light), where the player's sRGB-space output is
    not tone-mapping-correct. For faithful colour, use an SDR camera with `CompositingSpace::Srgb`.

## Restarting, pausing, reacting

`SsPlayback::playing` pauses and resumes **without rewinding**, so it cannot replay a finished
animation. To restart one from its head, bump `SsPlayback::restart` — any change of value. It is a
generation counter, so a host re-triggers a one-shot by incrementing it, with nothing to clear
afterwards.

Playback runs in the render world, so what it produces comes back two ways, polled and as messages:

```rust
use ssplayer_bevy::{SsEvent, SsEventMessage};

// added with .add_systems(Update, play_sounds)
fn play_sounds(mut events: MessageReader<SsEventMessage>) {
    for message in events.read() {
        if let SsEvent::Audio(audio) = &message.event {
            // The player produces no sound itself; feed your own audio stack.
            info!("play {:?} x{}", audio.sound_name, audio.loop_num);
        }
    }
}
```

| Surface | Kind | Use it for |
|---|---|---|
| `SsPlaybackState` component | Polled | the current `frame_no`, whether the animation has `finished`, and whether it is `playing_forward` |
| `SsEventMessage` | `MessageReader` | timeline events — user data, signals, audio cues |
| `SsStarted` / `SsFinished` / `SsLooped` | `MessageReader` | the one-shot start, completion and loop edges |

Both are **one frame behind**: what you read this frame is what playback produced last
frame, because it ran in the render world after last frame's `Update`.

## Masks, and device loss

Bevy's 2D depth texture is `Depth32Float`, which carries no stencil, so the adapter selects the
player's **coverage-bitmap** mask backend automatically. Nothing is asked of Bevy's attachments and
the draw stays inside Bevy's own pass; the cost is one extra render pass per masking character per
frame. See [Masks](../guide/masks.md).

Device loss recovers by riding Bevy's own recovery, but **you must opt in**: Bevy's default error
handler *quits* on device loss. Install a `RenderErrorHandler` that answers a `DeviceLost` error with
`RenderErrorPolicy::Recover(RenderCreation::default())` — [Device loss](../guide/device-loss.md) has
the handler.

## Run the samples

```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu
./scripts/deploy-examples.sh
cargo run -p ssplayer-bevy-overall    # the guided feature tour — read first
cargo run -p ssplayer-bevy-viewer     # a free-form viewer of a whole cast
```

## Next

* [Bevy API reference](../reference/bevy.md) — every component, message and limitation
* [Playback and transport](../guide/playback.md) — the vocabulary the components map onto
* [Engine integration](../wgpu/engines.md) — how the adapter is wired, if you are writing one for
  another engine
