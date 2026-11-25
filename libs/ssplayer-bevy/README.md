# ssplayer-bevy

[SpriteStudio](https://www.webtech.co.jp/eng/spritestudio/) characters inside a [Bevy](https://bevy.org) app, drawn on the
device Bevy already owns. SpriteStudio is a 2D animation tool: an artist adds bones, mesh
deformation and effects to their own illustrations, and this crate draws the result.

An adapter, not a second player: the drawing is `ssplayer-wgpu`, and this crate
is the seam between it and Bevy's two worlds. It lives in its own crate so the
player never depends on an engine, and so this one can follow Bevy's release
line without dragging the player along.

▶️ **[Live demo](https://cri-middleware.github.io/SSPlayerForWgpu/demo/)** — what the player draws,
running in your browser on WebGPU. It is `ssplayer-wgpu` on its own rather than inside Bevy; drop a
folder holding your own `.ssab` onto the page to play it.

The documentation is at <https://cri-middleware.github.io/SSPlayerForWgpu/>. The same pages for this
version are in this crate's `docs/` as Markdown, readable offline: start at `docs/index.md`; the Bevy
pages are `docs/start/bevy.md` and `docs/reference/bevy.md`.

## Install

**Bevy 0.20.x, and no other line.** Bevy and the player must resolve to one `wgpu`, so another
line is a compile error rather than a runtime surprise
([why](https://cri-middleware.github.io/SSPlayerForWgpu/start/bevy/#supported-bevy)).

```sh
cargo add bevy@0.20 ssplayer-bevy
```

A new project adds Bevy too, since the app uses it directly; one that already has Bevy 0.20 adds
`ssplayer-bevy` alone.

To use changes that are not released yet, depend on the repository over git instead:

```toml
[dependencies]
ssplayer-bevy = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
```

## First frame

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
    // MSAA has to be off: the player's pipelines are single-sampled.
    // CompositingSpace::Srgb is needed for correct colour; without it every character is too bright.
    commands.spawn((Camera2d, Msaa::Off, CompositingSpace::Srgb));

    commands.spawn((
        SsPlayer { pack: asset_server.load("Ringo/Ringo.ssab"), animation: "wait1".into(), ..default() },
        SsTransform { scale_x: 0.6, scale_y: 0.6, ..default() },
        SsPlayback { order: 1, ..default() },
    ));
}
```

The pack is an asset path: put the converted `.ssab` and the images beside it under your app's
`assets/` folder. No `.sspj` of your own yet? SpriteStudio's [official sample data](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/) works
as-is — the page is in Japanese, the downloads are not. [Ringo](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/#ringo) is a good character to
start with. To make your own, you use SpriteStudio itself — see [its site](https://www.webtech.co.jp/eng/spritestudio/).

**Read next:** [Get started with Bevy](https://cri-middleware.github.io/SSPlayerForWgpu/start/bevy/) ·
[Bevy API reference](https://cri-middleware.github.io/SSPlayerForWgpu/reference/bevy/) ·
[Engine integration](https://cri-middleware.github.io/SSPlayerForWgpu/wgpu/engines/#case-study-bevy)

## Examples

Two runnable hosts live in their own crates under the repository's `examples/`,
alongside the native and C++ viewers — kept out of this crate so its dependencies
stay honest about what the adapter itself needs (audio, UI and text are a host's,
not the adapter's).

```sh
# from the repository root, where assets/ is (scripts/deploy-examples.sh first)
cargo run -p ssplayer-bevy-overall    # the guided feature tour — read first
cargo run -p ssplayer-bevy-viewer     # the interactive development viewer
```

[`examples/overall-bevy`](../../examples/overall-bevy) steps one character through the whole sample
set; [`examples/viewer-bevy`](../../examples/viewer-bevy) is a free-form viewer of a cast in one pass.
Each README lists its keys.
