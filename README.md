# ssplayer-wgpu

A [wgpu](https://wgpu.rs/) renderer for [OPTPiX SpriteStudio 7](https://www.webtech.co.jp/spritestudio/) animations.

This is first and foremost a **Rust** player: a clean Rust API on top of the
SpriteStudio runtime, with WebGPU (wasm-bindgen) and C/C++ (cbindgen) as thin
wrappers over it. Because it renders through wgpu, one code path covers WebGPU,
Vulkan, DX12, Metal and GL.

Its defining design decision is the **borrow model**: the player does not own the
wgpu `Device`/`Queue`/`Surface`. The host owns them and lends them per frame, so
the player can ride along inside any wgpu application — your own app, egui,
macroquad, Bevy — without locking you into an engine.

The canonical design document is [`SSPLAYER_WGPU_SPEC.md`](SSPLAYER_WGPU_SPEC.md).

## Status

Implemented: Normal / Mesh / Shape / Instance / Effect / Mask draw batches, all 12
blend modes, part colours (varg) with premultiplied alpha, the 13 `ss_shader`
integration shaders, stencil-based masking, and both texture modes (embedded and
reference). WebGPU has been verified in a browser.

Not yet implemented: the native C-API (`ffi` feature) and its C++ verification
viewers, Text and 9-slice geometry, and the Bevy adapter.

## Requirements

- Rust 1.97+
- A GPU backend supported by wgpu **29** (see [wgpu version](#wgpu-version))
- For the web example: `wasm-pack` and `rustup target add wasm32-unknown-unknown`

## Getting started

```bash
git clone --recursive https://github.com/cri-middleware/ssplayer-wgpu.git
cd ssplayer-wgpu
# (if you cloned without --recursive)
git submodule update --init

./scripts/deploy-examples.sh          # build ssconverter-cli and convert the demo assets
cargo run -p ssplayer-wgpu-viewer     # native winit viewer
```

The viewer plays `assets/overall`. Right/Left switches `.ssab` packs, Up/Down
switches animations, Space pauses, `F` flips the Y axis.

```bash
cargo test --workspace                # includes headless GPU readback tests
cd examples/web && ./build.sh && python3 -m http.server   # WebGPU demo
```

## Layout

| Path | What |
|---|---|
| `core/` | crate `ssplayer-wgpu` — the player (`rlib` + `cdylib` for wasm) |
| `viewer/` | crate `ssplayer-wgpu-viewer` — native winit viewer used for development |
| `examples/web/` | `<ss-player-webgpu>` custom element + WebGPU demo page |
| `scripts/deploy-examples.sh` | converts the SDK's test projects into `assets/` |
| `SpriteStudio-SDK/` | the SDK, as a git submodule |

The two crates are versioned independently — the library and the development
viewer are different products.

## Using it in your own project

Two routes, in order of preference.

**A git dependency works today** and needs no knowledge of this repository's
layout — cargo checks out the `SpriteStudio-SDK` submodule for you:

```toml
[dependencies]
ssplayer-wgpu = { git = "https://github.com/cri-middleware/ssplayer-wgpu" }
```

The one requirement is that the SDK repository is reachable for whoever builds
it, and that the commit this repository's submodule points at has been pushed.
While the SDK is private, that means only people with access to it can build
against this crate.

**Publishing to crates.io** is the route that removes that requirement, and it
is not possible yet. `cargo publish` refuses a crate whose dependencies have no
version requirement, so it needs, in order:

1. `ssab`, `ssinterpolate` and `ssruntime` published from the SDK repository
   (all three names are still free on crates.io).
2. `version = "..."` added alongside `path` in this workspace's `ssruntime` and
   `ssab` dependencies. Cargo strips the `path` and keeps the version when
   packaging, so local development is unaffected.
3. `cargo publish -p ssplayer-wgpu`, then any adapter crates.

Until then, `cargo publish --dry-run` fails with *"all dependencies must have a
version requirement specified when publishing"* — that is expected, not a
misconfiguration.

## SDK submodule

The SDK is consumed as a git submodule and linked by path, not as a released
crate. Unlike the other players (Godot, Unity, Ren'Py, Web, Flutter), which talk
to the SDK through the stable C-API in a released zip, this player links
`ssruntime` and `ssab` as Rust crates and reads their FlatBuffers types directly,
so it has to track unreleased `.ssab` schema changes.

Bump the submodule to pick up SDK changes:

```bash
git -C SpriteStudio-SDK fetch origin && git -C SpriteStudio-SDK checkout origin/develop
cargo test --workspace
git add SpriteStudio-SDK && git commit -m "chore: bump SDK submodule"
```

## wgpu version

wgpu is pinned to the **29** line, because the Vtuber application composes a
Slint UI and this player on one device and Slint's `unstable-wgpu-NN` feature
requires an exact wgpu major. A workspace resolves exactly one wgpu, so this is a
repository-wide constraint: check Slint's supported major before bumping it.

## License

BSD-3-Clause. See [LICENSE.md](LICENSE.md).
