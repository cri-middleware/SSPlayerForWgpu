# ssplayer-wgpu — AI Agent Guide

## Operating rules

*   Keep changes scoped to the requested task.
*   Do not commit unless the user explicitly asks.
*   Before editing, read enough surrounding context to understand callers and invariants; if the read tool truncated the file, read the remaining ranges before making non-trivial changes.
*   Follow existing code style in touched files (naming, type usage, control flow, error handling).
*   Run formatting and linting after editing Rust code (see [Verification](#verification)). If you use `cargo clippy --fix`, first `git add` your own edits, then run with `--allow-staged` only (not `--allow-dirty`) so clippy's auto-fixes appear as unstaged changes and can be reviewed separately from your work.
*   **Design canon:** [`SSPLAYER_WGPU_SPEC.md`](./SSPLAYER_WGPU_SPEC.md) is the single source of truth for this player's architecture, rendering decisions, implementation phases and verification plan. Read the relevant section before changing rendering behaviour, and update it when a decision changes.
*   **SDK Alignment:** playback semantics (the "Brain") are canonically documented in the SDK, not here. See [`SpriteStudio-SDK/AGENTS.md`](./SpriteStudio-SDK/AGENTS.md) for runtime constraints and [`SpriteStudio-SDK/libs/ssruntime/docs/`](./SpriteStudio-SDK/libs/ssruntime/docs/) for the Player porting guide. Do not duplicate that material into this repository.
*   **Never commit inside the submodule.** A change that needs `ssruntime` / `ssab` is a pull request in the SDK repository, followed by a submodule bump here.

## Architecture in one paragraph

A wgpu renderer for SpriteStudio 7 animations, split out of the SpriteStudio-SDK monorepo. It is first and foremost a **Rust** player: a clean Rust API over `ssruntime`, with WebGPU (`wasm-bindgen`) and C/C++ (`cbindgen`) as thin wrappers on top. Three layers — `ssruntime` (computation, from the SDK) / `SSInternalPlayer` (drawing) / `SSPlayer` (control) — live in one crate as the `internal` and `player` modules. The load-bearing decision is the **borrow model**: the player never owns the wgpu `Device` / `Queue` / `Surface`; the host owns them and lends them per frame, so the player rides along inside any wgpu application without engine lock-in. Unlike the other Players, this one links `ssruntime` and `ssab` as Rust crates rather than through the C-API, which is why the SDK is a submodule here.

## Key components

| Path | Role | Docs |
|---|---|---|
| `libs/ssplayer-wgpu/` | Crate `ssplayer-wgpu`. `lib` / `frame` / `internal` (drawing, pipelines, shaders) / `player` (playback node tree) / `mask` (stencil planner) / `web` (`web` feature) / `host` + `ffi` (`ffi` feature). | [SPEC](./SSPLAYER_WGPU_SPEC.md) |
| `examples/viewer/` | Crate `ssplayer-wgpu-viewer`. Native winit host used for development and visual checks. | — |
| `examples/web/` | `<ss-player-webgpu>` custom element, demo page and `build.sh` (wasm-pack). | [README](./examples/web/README.md) |
| `examples/viewer-metal-cpp/` | Cocoa/Objective-C++ host that drives the C API on Metal. CMake builds the library through cargo. | [README](./examples/viewer-metal-cpp/README.md) |
| `scripts/deploy-examples.sh` | Builds `ssconverter-cli` from the submodule and converts the SDK's test projects into `assets/`. | — |
| `SpriteStudio-SDK/` | The SDK, as a git submodule. `ssruntime` / `ssab` are path dependencies into it. | [SDK AGENTS](./SpriteStudio-SDK/AGENTS.md) |
| `assets/` | Generated playback data. Gitignored — never commit it, never hand-edit it. | — |

## Hard constraints

*   **Borrow model:** the player must not own or store `Device` / `Queue` / `Surface`. Any API that takes ownership of wgpu state is wrong (SPEC §1). Data ownership is equally fixed: `.ssab` is owned by the player and borrowed by `ssruntime`; FrameData / SoA buffers are owned by `ssruntime` and borrowed by the player, never held across frames.
*   **wgpu is pinned to the 29 line.** Slint's `unstable-wgpu-NN` requires an exact wgpu major and the Vtuber application composes a Slint UI with this player on one device. A Cargo workspace resolves exactly one wgpu, so this is a repository-wide constraint — check Slint's supported major before bumping, and see SPEC §2.2 for the 30↔29 API differences.
*   **`[workspace] exclude = ["SpriteStudio-SDK"]` is load-bearing.** Without it cargo claims the SDK crates as members of *this* workspace and fails to resolve their inherited dependencies (`flatbuffers was not found in workspace.dependencies`).
*   **No engine dependencies in `ssplayer-wgpu`.** Slint and Bevy must not appear in the player crate; adapters are separate crates under `libs/`. This keeps the OSS adoption path open and lets an adapter move to its own release line if its engine's wgpu major ever diverges from ours.
*   **Crate versions are independent.** There is deliberately no `workspace.package.version`: the library and the development viewer are different products.
*   **`ffi.rs` holds no logic.** The C face is argument validation, type conversion and error-code mapping — nothing else. Anything it would need lives in `host.rs` (wgpu) or `player.rs` (playback), so the Rust API gets it too (SPEC §11).
*   **Selecting an animation and starting it are separate calls.** `SSPlayer::set_animation` builds the instance tree (the allocating step); `play` / `pause` / `stop` / `set_speed` are allocation-free transport, and `play()` also resumes after `pause()`. Only the `<ss-player>`-compatible js face keeps a single `play(name)` (SPEC §9.2).

## Verification

| Check | Command |
|---|---|
| Fetch the SDK | `git submodule update --init` |
| Generate demo assets (required once) | `./scripts/deploy-examples.sh` |
| Lint (check) | `cargo clippy --workspace --all-targets -- -D warnings` |
| Lint (auto-fix) | **After `git add` your edits**: `cargo clippy --workspace --all-targets --fix --allow-staged -- -D warnings` |
| Format check / apply | `cargo fmt --all -- --check` / `cargo fmt --all` |
| Tests | `cargo test --workspace` |
| WebGPU / js face (wasm32) | `cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web` |
| C / ffi face (+ header regen) | `cargo build -p ssplayer-wgpu --features ffi` |
| Native viewer | `cargo run -p ssplayer-wgpu-viewer` |
| Metal C++ viewer (macOS) | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| Web demo | `cd examples/web && ./build.sh && python3 -m http.server` |

The tests read `assets/`, so `deploy-examples.sh` is a prerequisite, not an optional step.

**The wasm build is not optional.** `libs/ssplayer-wgpu/src/web.rs` sits behind the `web` feature and is compiled by *no* native command — not `build`, not `clippy --all-targets`, not `test`. Breakage there is invisible until the wasm target is built, which is how three of the four wgpu-30→29 API changes escaped the native checks.

**`host.rs` / `ffi.rs` have the same blind spot**, behind the `ffi` feature: build with `--features ffi` after touching the player's public API, or the C face silently rots. That build also regenerates `libs/ssplayer-wgpu/include/ssplayer_wgpu.h` through cbindgen — commit the regenerated header with the change that moved it.

**Beware green rendering tests.** `libs/ssplayer-wgpu/tests/render.rs` renders offscreen and asserts on drawn-pixel counts, but skips itself when no GPU adapter is present — a pass on a headless machine may have verified nothing. When changing rasterisation, dump the readback to a PNG in the scratch directory and look at it; pixel counts alone have hidden real regressions (and a dump threshold below the background colour once counted the background as drawn).

## Workspace conventions

*   Single Cargo workspace, Rust edition 2024, MSRV 1.97.
*   `[workspace.dependencies]` is the single source of truth for `wgpu`, the SDK crates and the wasm-bindgen family, so the player crate, the viewer and the wasm build never drift.
*   Rendering parity is judged against the WebGL player (`../SSPlayerForWeb/packages/ssplayer-webgl/`), which the drawing code was ported from; consult it before inventing new behaviour.
*   CI enforces `cargo fmt --check`, `cargo clippy -D warnings`, the test suite and the wasm32 build.
*   Documentation is English; the SPEC is Japanese, matching how it is used.
