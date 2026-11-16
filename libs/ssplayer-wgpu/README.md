# ssplayer-wgpu

A [wgpu](https://wgpu.rs/) renderer for [SpriteStudio](https://www.webtech.co.jp/spritestudio/)
animations (`.ssab`).

This is first and foremost a **Rust** player: a clean Rust API over the
SpriteStudio runtime, with WebGPU (wasm-bindgen) and C/C++ (cbindgen) as thin
wrappers on top. Because it renders through wgpu, one code path covers WebGPU,
Vulkan, DX12 and Metal. It never owns the device: the host lends its `Device`,
`Queue` and render pass per frame, so the player rides along inside any wgpu
application — your own app, egui, Bevy.

The documentation is at <https://cri-middleware.github.io/SSPlayerForWgpu/>. The same pages for this
version are in this crate's `docs/` as Markdown, readable offline: start at `docs/index.md`; the Rust
pages are `docs/start/rust.md` and `docs/reference/rust.md`.

## Install

Rust 1.100 or newer, and an app on the **30** line of wgpu. Add the crate as a git dependency; cargo
checks out the `SpriteStudio-SDK` submodule for you:

```toml
[dependencies]
ssplayer-wgpu = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
```

## First frame

```rust
use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

// Once, against the device the host already owns.
let mut renderer = SSRenderer::new(&device);
let mut scene = SSScene::new(&device, &SceneConfig {
    color_format: surface_format,
    stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8),
    mask: MaskBackend::Stencil, // Coverage when the pass lends no stencil
});
scene.set_viewport(&queue, width as f32, height as f32, /* y_down */ false);

// One call loads the pack, its atlases in either export mode, and the sibling
// packs its instance parts play from.
let mut player = SSPlayer::new();
player.load("assets/overall/Basic.ssab")?;
player.set_animation("anime_1")?;
player.play();

// Per frame: advance every player and build this target's geometry …
scene.update(&mut renderer, &device, &queue, delta_secs, &mut [&mut player]);

// … then record draws into a render pass the host opened.
scene.draw(&renderer, &mut rpass);
```

**Read next:** [Get started with Rust](https://cri-middleware.github.io/SSPlayerForWgpu/start/rust/) ·
[Rust API reference](https://cri-middleware.github.io/SSPlayerForWgpu/reference/rust/) ·
[The borrow model](https://cri-middleware.github.io/SSPlayerForWgpu/wgpu/borrow-model/)

## Working on it

For the repository as a whole — examples, build scripts, asset generation — see
the [top-level README](../../README.md) and [CONTRIBUTING.md](../../CONTRIBUTING.md). Unlike the
other players, this one links the SDK's `ssruntime` and `ssab` as Rust crates through the
`SpriteStudio-SDK` submodule rather than its released C-API; how to bump it is in the
[Build Guide](https://cri-middleware.github.io/SSPlayerForWgpu/development/build/).

## License

BSD-3-Clause. See [LICENSE.md](../../LICENSE.md).
