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

How this player relates to the SDK and the other official players is documented in
[SpriteStudio Docs](https://cri-middleware.github.io/SpriteStudio-Docs/), the portal for
the whole ecosystem (it goes live with the first release).

## Status

Implemented: Normal / Mesh / Shape / Instance / Effect / Mask draw batches, all 12
blend modes, part colours (varg) with premultiplied alpha, the 13 `ss_shader`
integration shaders, stencil-based masking, and both texture modes (embedded and
reference). WebGPU has been verified in a browser, and the native C-API (`ffi`
feature) on Metal via the C++ viewer.

The DX12 C++ viewer (`examples/viewer-dx12-cpp`) is written but has not been run
on Windows hardware yet, so the Win32 → surface path and the DX12 backend are
still unverified.

Not yet implemented: the Vulkan C++ verification viewer, Text and 9-slice
geometry, and the Bevy adapter.

## Requirements

- Rust 1.97+
- A GPU backend supported by wgpu **29** (see [wgpu version](#wgpu-version))
- For the web example: `wasm-pack` and `rustup target add wasm32-unknown-unknown`

## Getting started

```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu
# (if you cloned without --recursive)
git submodule update --init

./scripts/deploy-examples.sh          # convert the SDK's test projects into assets/
./scripts/build.sh                    # build the library for this host
cargo run -p ssplayer-wgpu-viewer     # native winit viewer
```

The viewer plays `assets/overall`. Right/Left switches `.ssab` packs, Up/Down
switches animations, Space pauses, `F` flips the Y axis.

## Build scripts

Two scripts, split by what they produce — the same shape as the sibling players'
`build-runtime.*` / `deploy-examples.*`, with their `key=value` argument style.

**`scripts/build.sh`** (`build.ps1` on Windows) builds the **library** and
collects its artifacts:

```bash
./scripts/build.sh                            # = platform=host build=release
./scripts/build.sh platform=web build=debug   # wasm/js face, deployed into examples/web
./scripts/build.sh --help                     # every option
```

| Option | Values | What it does |
|---|---|---|
| `platform` | `host` (default), `web` | `host` builds the Rust library and the C face; `web` builds the wasm/js face with wasm-pack. It does not cross-compile, so `macos`/`windows`/`linux` are accepted only when they name the host |
| `build` | `release` (default), `debug` | Cargo profile; `debug` also means `wasm-pack --dev`, which skips `wasm-opt` — never quote its size |
| `ffi` | `yes` (default), `no` | `host` only: also build `--features ffi`, which regenerates `libs/ssplayer-wgpu/include/ssplayer_wgpu.h` with cbindgen |
| `deploy` | `yes` (default), `no` | `web` only: copy the wasm package into `examples/web/wasm` |

What it builds is gathered in one place, so a packaging script reads a fixed
layout instead of knowing where each toolchain drops its output:

```
bin/include/ssplayer_wgpu.h                 # ffi=yes (cbindgen)
bin/LICENSE.md
bin/libs/<platform>/<arch>/libssplayer_wgpu.{a,dylib}    # .lib/.dll on Windows
pkg/                                        # platform=web: the wasm-pack package
```

**`scripts/deploy-examples.sh`** (`.ps1`) owns the playback data end to end — it
builds `ssconverter-cli` from the SDK submodule, converts the test projects into
`assets/` in **both** export modes, and deploys them to `examples/web`:
`assets/` (reference, plus its sibling `.png`) and `assets_emb/` (embedded, atlas
baked in), because the demo page has a selector for the two. `build.sh` never
touches `.ssab`.

```bash
cargo test --workspace                                     # includes headless GPU readback tests
./scripts/deploy-examples.sh && ./scripts/build.sh platform=web   # then serve the demo:
cd examples/web && python3 -m http.server                  # → http://localhost:8000
```

The C/C++ viewers drive the same `ffi` library through CMake, which runs cargo
itself — one per backend, each self-contained:

```bash
cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal
```

```powershell
cd examples\viewer-dx12-cpp; cmake -B build; cmake --build build --config Release; .\build\Release\ssviewer-dx12.exe
```

## Layout

| Path | What |
|---|---|
| `libs/ssplayer-wgpu/` | crate `ssplayer-wgpu` — the player (`rlib` + `cdylib` for wasm) |
| `examples/viewer/` | crate `ssplayer-wgpu-viewer` — native winit viewer used for development |
| `examples/web/` | `<ss-player-webgpu>` custom element + WebGPU demo page |
| `examples/viewer-metal-cpp/` | Cocoa/Objective-C++ viewer driving the C API on Metal |
| `examples/viewer-dx12-cpp/` | Win32/C++ viewer driving the C API on DX12 (untested on hardware) |
| `scripts/build.sh` / `.ps1` | builds the library for one platform (`platform=host\|web`), collecting it into `bin/` |
| `scripts/deploy-examples.sh` / `.ps1` | converts the SDK's test projects into `assets/` and deploys both export modes to `examples/web` |
| `bin/` | collected library artifacts. Gitignored |
| `SpriteStudio-SDK/` | the SDK, as a git submodule |

The two crates are versioned independently — the library and the development
viewer are different products.

## Using it in your own project

Two routes, in order of preference.

**A git dependency works today** and needs no knowledge of this repository's
layout — cargo checks out the `SpriteStudio-SDK` submodule for you:

```toml
[dependencies]
ssplayer-wgpu = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
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
