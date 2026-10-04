---
title: Get started with Rust
---

# Get started with Rust

**This page is for a Rust application that owns a `wgpu::Device`** — your own renderer, a `winit`
window, `egui`, `Slint`, or anything else built on wgpu. The player draws into a render pass *you*
open; it never creates a device, a queue or a surface of its own.

If your app is a **Bevy** app, take [the Bevy path](./bevy.md) instead: the adapter does all of
this for you.

## Requirements

| | |
|---|---|
| Rust | 1.99 or newer, edition 2024 |
| wgpu | the **30** line — your app and the player must resolve to one `wgpu`, so a different major is a compile error |
| SDK access | while [SpriteStudio-SDK](https://github.com/cri-middleware/SpriteStudio-SDK) is private, whoever builds needs access to it |

## Install

=== "Released"

    🚧 **Not on crates.io yet.** `cargo publish` refuses a crate whose dependencies carry no
    version requirement, so this waits on `ssab` and `ssruntime` publishing from the SDK
    repository first. When they do, it becomes:

    ```toml
    [dependencies]
    ssplayer-wgpu = "7.0.0-beta.1"
    ```

    Nothing else about your code changes — the git dependency below builds the same crate.

=== "From source"

    A git dependency works today, and cargo checks out the `SpriteStudio-SDK` submodule for you:

    ```toml
    [dependencies]
    ssplayer-wgpu = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
    ```

    To **vendor the source into your own tree** instead — a submodule of your repository, or a
    checkout you patch — point at the crate directory rather than the repository root:

    ```toml
    [dependencies]
    ssplayer-wgpu = { path = "third_party/SSPlayerForWgpu/libs/ssplayer-wgpu" }
    ```

    That path is a crate, not a workspace, so it slots into yours. Run
    `git submodule update --init --recursive` in the checkout: the player's own `SpriteStudio-SDK`
    submodule is a path dependency, and without it cargo cannot resolve `ssruntime` / `ssab`.

## The three objects

| | Scope | You keep one per |
|---|---|---|
| `SSRenderer` | device | `wgpu::Device` — **never one per window or canvas** |
| `SSScene` | render target | surface or texture you draw into |
| `SSPlayer` | character | character on screen. Holds no GPU resource, so one scene carries any number |

Full story in [Scopes and names](../guide/concepts.md); the reasoning is in
[The borrow model](../wgpu/borrow-model.md).

## Set up, once

```rust
use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

// Device-scoped. Pipelines, sampler, atlas textures.
let mut renderer = SSRenderer::new(&device);

// Target-scoped. `color_format` must match the target you will draw into.
let mut scene = SSScene::new(&device, &SceneConfig {
    color_format: surface_format,
    stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8),
    mask: MaskBackend::Stencil,
});
scene.set_viewport(&queue, width as f32, height as f32, /* y_down */ false);

// One character.
let mut player = SSPlayer::new();
player.load("assets/overall/Basic.ssab")?;   // pack + atlases + sibling packs, in one call
player.set_animation("anime_1")?;            // selecting is separate from starting
player.play();
```

`load` touches no GPU state — the borrow model lends none until the frame starts, so what it read
waits and reaches the GPU on the first `update`. See [Loading and assets](../guide/loading.md).

## Per frame

```rust
// 1. Advance every player and build this target's geometry.
scene.update(&mut renderer, &device, &queue, delta_secs, &mut [&mut player]);

// 2. Bake mask coverage, if this scene uses that backend. A no-op under `Stencil`,
//    and it must go on the SAME encoder as the pass below.
scene.encode_mask_coverage(&mut renderer, &device, &mut encoder);

// 3. Draw into a pass you opened.
let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
    label: Some("ss pass"),
    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
        view: &frame_view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
    })],
    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
        view: &stencil_view,
        depth_ops: None,
        stencil_ops: Some(wgpu::Operations {
            load: wgpu::LoadOp::Clear(0),
            store: wgpu::StoreOp::Store,
        }),
    }),
    ..Default::default()
});
let draw_calls = scene.draw(&renderer, &mut rpass);
```

Pass **every** player of that target to one `update` and draw them in one pass — that is what makes
ten characters cost one atlas upload and one set of pipelines. Their order on screen is
`SSPlayer::set_order`, ascending.

Four rules the pass has to honour:

* **The colour format must match `SceneConfig::color_format`.** Pipelines are baked against it; a
  format change means a new `SSScene`.
* **A stencil attachment is required for `MaskBackend::Stencil`**, in the format you declared. No
  stencil to lend? Use `MaskBackend::Coverage` — see [Masks](../guide/masks.md).
* **MSAA must be off.** The player's pipelines are single-sampled.
* **Give it a non-sRGB target.** The player writes atlas bytes verbatim and does no colour
  management, so an sRGB target encodes them a second time and everything comes out too bright.
  `examples/hello` picks its surface format with
  `capabilities.formats.iter().find(|f| !f.is_srgb())`; compositing in sRGB space works too.

On resize, recreate your own attachments and call `set_viewport` again.

## Run the sample

```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu
./scripts/deploy-examples.sh          # converts the SDK's test projects into assets/
cargo run -p ssplayer-wgpu-hello
```

[`examples/hello`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/examples/hello/src/main.rs)
is this page as one program: a winit window and one character, fitted into the window. When you
want the rest of the surface — pack switching, draw order, mask backends, per-part overrides,
motion blending, live stats — `cargo run -p ssplayer-wgpu-viewer` exercises all of it.

## Next

* [Playback and transport](../guide/playback.md) — what `play` really does, and why `resume` exists
* [Loading and assets](../guide/loading.md) — where files are resolved from, and what to do in an engine
* [The borrow model](../wgpu/borrow-model.md) — the reasoning, and how to slot the player into an
  existing render graph
