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

A wgpu renderer for SpriteStudio 7 animations, split out of the SpriteStudio-SDK monorepo. It is first and foremost a **Rust** player: a clean Rust API over `ssruntime`, with WebGPU (`wasm-bindgen`) and C/C++ (`cbindgen`) as thin wrappers on top. Three layers — `ssruntime` (computation, from the SDK) / drawing / `SSPlayer` (control) — live in one crate. The load-bearing decision is the **borrow model**: the player never owns the wgpu `Device` / `Queue` / `Surface`; the host owns them and lends them per frame, so the player rides along inside any wgpu application without engine lock-in. The drawing layer splits by resource scope: `SSRenderer` (device-scoped — pipelines, sampler, texture map) and `SSScene` (render-target-scoped — projection, frame vertex arena, the players drawn into it), so **one target carries any number of players**. That split is P7 in the SPEC and is now complete on all three faces — read SPEC §1.2 and §12.1 before touching layer 2. What is left is expressiveness, not speed: SPEC §14.2 measured the throughput items and retired them. Unlike the other Players, this one links `ssruntime` and `ssab` as Rust crates rather than through the C-API, which is why the SDK is a submodule here.

## Key components

| Path | Role | Docs |
|---|---|---|
| `libs/ssplayer-wgpu/` | Crate `ssplayer-wgpu`. `lib` / `frame` / `renderer` (device-scoped: pipelines, shaders, texture map) / `scene` (target-scoped: projection, frame arena, draw calls) / `player` (playback node tree) / `pack` (the shared `.ssab`) / `mask` (stencil planner) / `metrics` (frame counters + GPU timing) / `roster` (generation-tagged handles, shared by the web and C faces) / `web` (`web` feature) / `host` + `ffi` (`ffi` feature). | [SPEC](./SSPLAYER_WGPU_SPEC.md) |
| `examples/viewer/` | Crate `ssplayer-wgpu-viewer`. Native winit host used for development and visual checks. | — |
| `examples/web/` | `ss-context.js` (the page's single `SsContextWeb`), the `<ss-player-webgpu>` convenience element, demo page and `build.sh` (wasm-pack). | [README](./examples/web/README.md) |
| `examples/viewer-metal-cpp/` | Cocoa/Objective-C++ host that drives the C API on Metal. CMake builds the library through cargo. | [README](./examples/viewer-metal-cpp/README.md) |
| `scripts/deploy-examples.sh` | Builds `ssconverter-cli` from the submodule and converts the SDK's test projects into `assets/`. | — |
| `SpriteStudio-SDK/` | The SDK, as a git submodule. `ssruntime` / `ssab` are path dependencies into it. | [SDK AGENTS](./SpriteStudio-SDK/AGENTS.md) |
| `assets/` | Generated playback data. Gitignored — never commit it, never hand-edit it. | — |

## Hard constraints

*   **Borrow model:** the player must not own or store `Device` / `Queue` / `Surface` — nor, after P7, the `SSRenderer`. Any API that takes ownership of wgpu state is wrong (SPEC §1). Data ownership is equally fixed: `.ssab` is owned by the player and borrowed by `ssruntime`; FrameData / SoA buffers are owned by `ssruntime` and borrowed by the player, never held across frames.
*   **GPU resources live at their widest scope (SPEC §1.2).** Pipelines, samplers and the texture map are device-scoped (`SSRenderer`, host-owned); the projection uniform and the frame geometry arena are render-target-scoped (`SSScene`); only the playback tree, transform and order are per player. **`SSPlayer` must not gain a wgpu-typed field** — that rule is what keeps "1 canvas = 1 player" from coming back. One device per host/page: never `request_device` per canvas or per window.
*   **There is no vertex buffer (SPEC §5.5).** The runtime's SoA arrays are memcpy'd into the scene's arena and expanded by `vs_quad` / `vs_mesh` / `vs_shape` / `vs_effect` from `@builtin(vertex_index)`; the CPU writes one 80-byte `Prim` per part and nothing per vertex. Do not reintroduce per-vertex CPU work to add a feature — put it on the `Prim` (that is where player alpha lives, and where tint goes). The vertex stage's position maths is deliberately written as scalar accumulation rather than `mat4 * vec4`, so it rounds identically to the CPU path it replaced; rewriting it "cleanly" invites FMA contraction and costs the bit-exact golden comparison.
*   **Textures are borrowed, not owned (SPEC §8.2).** `SSRenderer::set_texture` takes a `TextureView` the supplier owns and only builds a bind group from it; `upload_texture` is a convenience path that happens to create the texture itself. `SSPack` holds CPU data only. Owning GPU textures would put a second lifecycle in competition with an engine's asset system (Bevy's `Assets<Image>` / `RenderAssets<GpuImage>`). Per-texture facts that vary (`AlphaMode`, texel size) go in the group(1) `TexInfo` uniform, never into the pipeline key — mixing straight and premultiplied atlases must not double the pipeline count.
*   **wgpu is pinned to the 29 line.** Slint's `unstable-wgpu-NN` requires an exact wgpu major and the Vtuber application composes a Slint UI with this player on one device. A Cargo workspace resolves exactly one wgpu, so this is a repository-wide constraint — check Slint's supported major before bumping, and see SPEC §2.2 for the 30↔29 API differences.
*   **`[workspace] exclude = ["SpriteStudio-SDK"]` is load-bearing.** Without it cargo claims the SDK crates as members of *this* workspace and fails to resolve their inherited dependencies (`flatbuffers was not found in workspace.dependencies`).
*   **No engine dependencies in `ssplayer-wgpu`.** Slint and Bevy must not appear in the player crate; adapters are separate crates under `libs/`. This keeps the OSS adoption path open and lets an adapter move to its own release line if its engine's wgpu major ever diverges from ours.
*   **Crate versions are independent.** There is deliberately no `workspace.package.version`: the library and the development viewer are different products.
*   **`ffi.rs` holds no logic.** The C face is argument validation, type conversion and error-code mapping — nothing else. Anything it would need lives in `host.rs` (wgpu) or `player.rs` (playback), so the Rust API gets it too (SPEC §11).
*   **Selecting an animation and starting it are separate calls.** `SSPlayer::set_animation` builds the instance tree (the allocating step); `play` / `pause` / `stop` / `set_speed` are allocation-free transport, and `play()` also resumes after `pause()`. This now holds for the js face too — the old `<ss-player>` single `play(name)` is not carried over, because the reference implementation (`ssplayer-webgl`) itself split into `setAnimation` + `play` (SPEC §9.3, §0.2).
*   **Slots step from the part list, never from the draw order (SPEC §7.1).** `step_slots` advances every instance and effect slot the pack declares; `render_node` only draws. They are separate walks because the runtime drops `Hide`-ed parts from `draw_order` / `draw_batches` entirely, so stepping from the draw walk stops a slot the moment its part is hidden — and an independently-played child then resumes late by exactly the time it spent hidden. `tests/slots.rs` pins it. The loop pulse those slots re-arm on is `Context::is_looped()`, read immediately after `update`; "the frame went backwards" is not the same thing.
*   **Each player leaves the stencil buffer as it found it (SPEC §6).** `render_node` unwinds every mask writer it applied (`unwind_masks`). Without that, stacking two players in one render pass lets the first one's mask clip the second; `LoadOp::Clear(0)` with one player per pass hid the bug for a long time, so the regression test compares "N players, one pass" against "one pass each" pixel for pixel.

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
| C / ffi handle lifetimes | `cargo test -p ssplayer-wgpu --features ffi --lib` |
| Native viewer | `cargo run -p ssplayer-wgpu-viewer` |
| Metal C++ viewer (macOS) | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| Web demo | `cd examples/web && ./build.sh && python3 -m http.server` |

The tests read `assets/`, so `deploy-examples.sh` is a prerequisite, not an optional step.

**The wasm build is not optional.** `libs/ssplayer-wgpu/src/web.rs` sits behind the `web` feature and is compiled by *no* native command — not `build`, not `clippy --all-targets`, not `test`. Breakage there is invisible until the wasm target is built, which is how three of the four wgpu-30→29 API changes escaped the native checks. Nothing in `web.rs` is covered by a test either, so the invariant it rests on — one device-scoped `SSRenderer` serving several render targets — is pinned natively instead, by `tests/pack.rs::scenes_share_one_renderer`.

**`host.rs` / `ffi.rs` have the same blind spot**, behind the `ffi` feature: build with `--features ffi` after touching the player's public API, or the C face silently rots. That build also regenerates `libs/ssplayer-wgpu/include/ssplayer_wgpu.h` through cbindgen — commit the regenerated header with the change that moved it, and read the diff: cbindgen sweeps up every module-level `pub const` in the crate, so a new one lands in the public C header (`#define MIX 0` nearly did). Keep such constants `pub(crate)`/`pub(super)`, or add them to `cbindgen.toml`'s exclude list.

**The C and JS faces are the same shape, and neither may fuse its handles again.** C is `ss_scene_t` (the window) + `ss_player_t` (one character); JS is `SsContextWeb` (the page's device + renderer) + `SsSurfaceWeb` (one canvas) + `SsPlayerWeb` (one character). Fusing them is what made two characters in one window structurally impossible, and what made a page of N elements build N `GPUDevice`s. Both faces name their children through `roster.rs`'s generation-tagged slots for the same reason: a handle is not a pointer into the owner's list, so a stale one resolves to "destroyed" instead of to its successor. On the C side `ss_player_destroy` additionally leaves the handle box allocated until the scene goes, so a double destroy is a safe no-op; on the JS side `destroy()` is explicit and idempotent, because the surface and player live in the shared context and dropping the JS wrapper would otherwise leak a swapchain (SPEC §11, §10).

**Beware green rendering tests.** `libs/ssplayer-wgpu/tests/render.rs` renders offscreen and asserts on drawn-pixel counts, but skips itself when no GPU adapter is present — a pass on a headless machine may have verified nothing. When changing rasterisation, dump the readback to a PNG in the scratch directory and look at it; pixel counts alone have hidden real regressions (and a dump threshold below the background colour once counted the background as drawn).

**A measured negative result is still a result.** SPEC §14.2 tried two "obvious" CPU optimisations and measured both: parallelising the per-player walk (item 6) is *slower* below ~64 players, and render bundles (item 5) are slower at every size and were reverted. Both for the same reason — P7-6 removed the CPU work they were meant to save. Check the numbers before assuming a change helped, and write them down when it did not.

**When measuring anything that caches across frames, drive a real frame loop.** Timing `update` and the render pass in separate loops made the render bundle look 300x better than it was, because a loop that never updates never invalidates the cache. `tests/scale.rs` interleaves them for exactly this reason.

**Measure before and after any performance change.** `SSScene::stats()` reports primitives, draw calls, bytes uploaded, how much of the draw list is mergeable or re-binds the same texture, and — when the device was created with `Features::TIMESTAMP_QUERY` — GPU pass time; both viewers print it. As of the P8 measurements a 256-player frame costs 1.3 ms CPU + 0.19 ms GPU, so assume there is *no* performance problem until a number says otherwise. SPEC §14.2 lists the remaining "written like WebGL" items, and every one of them is a claim about issuing less work, so a change made without comparing those numbers has not been shown to help. GPU time is read back a frame or two late by design; do not add a stall to make it current.

**For anything that touches how geometry reaches the GPU, take a golden dump first.** `tests/golden.rs::golden_corpus` (`#[ignore]`d) renders all 134 fixture/animation/frame/stencil combinations and byte-compares them against a previous run — `SS_GOLDEN_DIR=/tmp/before cargo test --test golden -- --ignored`, then re-run with `SS_GOLDEN_COMPARE=/tmp/before`. It is what proved the P7-6 vertex-path replacement changed nothing. The reference must come from the same machine (GPU-specific), so it is a before/after tool, not a checked-in golden set.

## Workspace conventions

*   Single Cargo workspace, Rust edition 2024, MSRV 1.97.
*   `[workspace.dependencies]` is the single source of truth for `wgpu`, the SDK crates and the wasm-bindgen family, so the player crate, the viewer and the wasm build never drift.
*   Rendering parity is judged against the WebGL player (`../SSPlayerForWeb/packages/ssplayer-webgl/`), which the drawing code was ported from; consult it before inventing new behaviour.
*   CI enforces `cargo fmt --check`, `cargo clippy -D warnings`, the test suite and the wasm32 build.
*   Documentation is English; the SPEC is Japanese, matching how it is used.
