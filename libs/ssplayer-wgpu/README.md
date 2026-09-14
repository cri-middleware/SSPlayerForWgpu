# ssplayer-wgpu

A [wgpu](https://wgpu.rs/) renderer for [SpriteStudio](https://www.webtech.co.jp/spritestudio/)
animations (`.ssab`).

This is first and foremost a **Rust** player: a clean Rust API over the
SpriteStudio runtime, with WebGPU (wasm-bindgen) and C/C++ (cbindgen) as thin
wrappers on top. Because it renders through wgpu, one code path covers WebGPU,
Vulkan, DX12 and Metal.

For the repository as a whole — examples, build scripts, asset generation — see
the [top-level README](../../README.md).

## The borrow model

The player does not own the wgpu `Device` / `Queue` / `Surface`, nor the
`SSRenderer`. **The host owns them and lends them per frame**, so the player
rides along inside any wgpu application — your own app, egui, macroquad, Bevy —
without locking you into an engine.

Three types, split by how long their resources stay useful:

| Type | Scope | Holds |
|---|---|---|
| `SSRenderer` | one **device** | pipelines, bind group layouts, sampler, atlas textures |
| `SSScene` | one **render target** | projection, the frame's geometry arena, the draw list |
| `SSPlayer` | one **character** | the playback tree, transform, order, visibility, alpha |

A player holds no GPU resource at all, so a scene carries any number of them and
they share one upload of a pack's atlases. The host keeps a single `SSRenderer`
per device — never one per canvas or window.

## Usage

```rust
use ssplayer_wgpu::{SSPlayer, SSRenderer, SSScene, SceneConfig};

// Once, against the device the host already owns.
let mut renderer = SSRenderer::new(&device);
let mut scene = SSScene::new(&device, &SceneConfig {
    color_format: surface_format,
    stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8), // None disables masking
});
scene.set_viewport(&queue, width as f32, height as f32, /* y_down */ false);

// One call loads the pack, its atlases in either export mode, and the sibling
// packs its instance parts play from.
let mut player = SSPlayer::new();
player.load("assets/overall/Basic.ssab")?;
player.set_animation("Basic")?;
player.play();

// Per frame: advance every player and build this target's geometry …
scene.update(&mut renderer, &device, &queue, delta_secs, &mut [&mut player]);

// … then record draws into a render pass the host opened.
scene.draw(&renderer, &mut rpass);
```

`set_animation` builds the instance tree and is the allocating step; `play` /
`pause` / `resume` / `stop` / `set_speed_scale` are allocation-free transport. Players are drawn
in ascending `set_order`, and `set_transform` / `set_visible` / `set_alpha` place
each one in the scene.

Hosts that supply their own textures — an engine with an asset system — lend a
`TextureView` through `SSRenderer::set_texture` instead of letting the player
upload; the player never takes ownership of a GPU texture.

## Faces and features

| Feature | Default | What it adds |
|---|---|---|
| `embedded` | ✅ | Decodes embedded-mode `.ssab` atlases in-crate (the `image` crate). Turn off where the host decodes instead. |
| `web` | | The WebGPU / JS face: `SsContextWeb` (page) / `SsSurfaceWeb` (canvas) / `SsPlayerWeb` (character), via wasm-bindgen. Deliberately does **not** pull `embedded` — the browser already ships a decoder, so atlases go out encoded and come back through `createImageBitmap`. |
| `ffi` | | The C / C++ face: `ss_scene_*` + `ss_player_*` entry points and `NativeHost`, which builds wgpu from a raw window handle. The companion `ssplayer-wgpu-ffi` crate compiles these into the binary library; its `build.rs` runs cbindgen to emit `ssplayer-wgpu-ffi/include/ssplayer_wgpu.h`. |

This crate is a plain `rlib`; the companion `ssplayer-wgpu-ffi` crate is built as
`cdylib` + `staticlib` and as the wasm module, so the same source still serves
a Rust dependency, a browser and a native shared library.

## Modules

| Module | Role |
|---|---|
| `renderer` | device-scoped: pipelines (built on first use), shaders, the texture map |
| `scene` | target-scoped: projection, the frame arena, the draw list, GPU timing |
| `player` | the playback node tree, loading, transport, per-part overrides |
| `pack` | `SSPack` — a `.ssab` shared between players, CPU data only |
| `assets` / `decode` | the loader seam, and bulk atlas decoding |
| `mask` | the stencil planner |
| `events` / `metrics` / `roster` | animation events, frame counters, generation-tagged handles |
| `web` / `host` + `ffi` | the two wrapper faces |

## Requirements

- Rust 1.98+
- A GPU backend supported by wgpu **29**
- For the web face: `wasm-pack` and `rustup target add wasm32-unknown-unknown`

## wgpu version

wgpu is pinned to the **29** line, because the Vtuber application composes a
Slint UI and this player on one device and Slint's `unstable-wgpu-NN` feature
requires an exact wgpu major. A workspace resolves exactly one wgpu, so this is a
repository-wide constraint: check Slint's supported major before bumping it.

## Using it in your own project

**A git dependency works today**, and cargo checks out the `SpriteStudio-SDK`
submodule for you:

```toml
[dependencies]
ssplayer-wgpu = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
```

The one requirement is that the SDK repository is reachable for whoever builds
it, and that the commit this repository's submodule points at has been pushed.
While the SDK is private, only people with access to it can build against this
crate.

**Publishing to crates.io** removes that requirement and is not possible yet:
`cargo publish` refuses a crate whose dependencies have no version requirement,
so it needs, in order:

1. `ssab`, `ssinterpolate` and `ssruntime` published from the SDK repository
   (all three names are still free on crates.io).
2. `version = "..."` added alongside `path` in this workspace's `ssruntime` and
   `ssab` dependencies. Cargo strips the `path` and keeps the version when
   packaging, so local development is unaffected.
3. `cargo publish -p ssplayer-wgpu`, then any adapter crates.

Until then `cargo publish --dry-run` fails with *"all dependencies must have a
version requirement specified when publishing"* — expected, not a
misconfiguration.

## Why the SDK is a submodule

Unlike the other players (Godot, Unity, Ren'Py, Web, Flutter), which talk to the
SDK through the stable C-API in a released zip, this player links `ssruntime` and
`ssab` as Rust crates and reads their FlatBuffers types directly, so it has to
track unreleased `.ssab` schema changes. Bump it with:

```bash
git -C SpriteStudio-SDK fetch origin && git -C SpriteStudio-SDK checkout origin/develop
cargo test --workspace
git add SpriteStudio-SDK && git commit -m "chore: bump SDK submodule"
```

## License

BSD-3-Clause. See [LICENSE.md](../../LICENSE.md).
