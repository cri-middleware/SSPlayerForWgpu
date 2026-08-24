# ssplayer-wgpu-bevy

SpriteStudio 7 characters inside a [Bevy](https://bevy.org) app, drawn on the
device Bevy already owns.

An adapter, not a second player: the drawing is `ssplayer-wgpu`, and this crate
is the seam between it and Bevy's two worlds. It lives in its own crate so the
player never depends on an engine, and so this one can follow Bevy's release
line without dragging the player along.

## Supported Bevy

**0.19.x, and no other line.**

Bevy and the player must resolve to one `wgpu`: `bevy_render` 0.19 requires
`wgpu ^29.0.3` and the player is pinned to the 29 line. Two majors of `wgpu` are
two unrelated sets of types, and no glue bridges them — a mismatch is a compile
error, not a runtime surprise. npm would call this a peer dependency; cargo has
no such thing, so the range is stated here and the workspace resolving a single
`wgpu` is what enforces it.

## Using it

```rust
use bevy::prelude::*;
use bevy::render::view::Msaa;
use ssplayer_wgpu_bevy::{SsPlayback, SsPlayer, SsPlayerPlugin, SsTransform};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(SsPlayerPlugin)
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    // MSAA has to be off: the player's pipelines are single-sampled.
    commands.spawn((Camera2d, Msaa::Off));

    commands.spawn((
        SsPlayer { src: "assets/Ringo/Ringo.ssab".into(), animation: "wait1".into() },
        SsTransform { scale_x: 0.6, scale_y: 0.6, ..default() },
        SsPlayback { order: 1, ..default() },
    ));
}
```

`SsTransform` and `SsPlayback` are required components, so spawning `SsPlayer`
alone gives you both at their defaults.

| Component | What it carries | Cost of changing it |
|---|---|---|
| `SsPlayer` | `src`, `animation` | Rebuilds the playback tree — this is the allocating one |
| `SsTransform` | `x`, `y`, `scale_x`, `scale_y`, `rotation` | Free |
| `SsPlayback` | `playing`, `speed`, `order`, `alpha`, `visible` | Free |

Deliberately not Bevy's `Transform`: that is a 3D affine whose decomposition into
the player's five scalars is lossy. An app that wants the two linked can write
the one system that copies across, and keeps the choice of which is authoritative.

## Example

```sh
# from the repository root, where assets/ is
cargo run -p ssplayer-wgpu-bevy --example bevy_viewer
```

`scripts/deploy-examples.sh` (or `.ps1`) generates `assets/` first.

## How it is wired

Bevy renders in two worlds, and the crate is split along that seam.

| Where | What |
|---|---|
| Main world | The three components above. The whole authoring surface |
| `ExtractSchedule` | Copies them, plus the frame delta, into the render world |
| `Render` / `RenderSystems::Prepare` | Applies them to the playback trees; loads a pack whose `src` changed |
| `Core2d` / `Core2dSystems::MainPass` | Advances, builds and draws, after `main_transparent_pass_2d` |

The playback trees, the device-scoped `SSRenderer` and the target-scoped
`SSScene` all live in the render world. The player never owns Bevy's `Device` /
`Queue` / render target — the draw system lends them for the frame, which is the
same borrow model the winit viewer and the C++ hosts honour.

The draw system is ordered *after* `main_transparent_pass_2d`, not merely placed
in the same set. Two systems that each take a `RenderContext` get their own
command encoder, and only an explicit edge decides which buffer reaches the queue
first — without it, whether the characters land over or under Bevy's sprites is a
race.

## Masks, without a stencil

Bevy's 2d depth texture is `Depth32Float`, which carries no stencil — so this
adapter selects the player's **coverage-bitmap** mask backend instead of its
stencil one. The player rasterises each masking character's mask parts into a
bitmap of its own and the content shader replays them per fragment; nothing is
asked of Bevy's attachments, and the draw stays inside Bevy's own pass.

The bake is recorded onto the same `CommandEncoder` the draw pass is opened on,
in the same system. That ordering is the whole guarantee: passes in one encoder
run in order, so a character is clipped by *this* frame's mask. Splitting the
bake into its own system would give it a separate encoder and put the order back
in the hands of whatever ran first.

The cost is one extra render pass per masking character per frame.

## What this version does not do yet

* **One view.** `SSScene::update` fuses the timeline advance with the geometry
  build, so drawing a second view would advance every animation twice per frame.
  Later views are skipped, with a warning. Lifting this needs the advance split
  off from the build in `ssplayer-wgpu` itself.
* **MSAA must be off.**
* **Loading is synchronous** and reads the filesystem directly rather than going
  through Bevy's `AssetServer`, so it does not get hot reload, packed assets or
  shared atlases.
* **Nothing crosses back.** Timeline events and the completion edge stay in the
  render world, so gameplay cannot yet react to them.
