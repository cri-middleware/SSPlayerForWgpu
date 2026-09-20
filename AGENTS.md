# ssplayer-wgpu — AI Agent Guide

## Operating rules

*   Keep changes scoped to the requested task.
*   Do not commit, push, or open PRs unless the user explicitly asks.
*   Before editing, read enough surrounding context to understand callers and invariants; if the
    read tool truncated the file, read the remaining ranges first. Follow the existing code style
    of the file you touch (naming, type usage, control flow, error handling).
*   Validate with the matrix in [CONTRIBUTING.md](./CONTRIBUTING.md). For `clippy --fix`, `git add`
    your own edits first and pass `--allow-staged`, never `--allow-dirty`.
*   **Playback semantics are the SDK's** — see [its AGENTS.md](./SpriteStudio-SDK/AGENTS.md) and
    [the Player porting guide](./SpriteStudio-SDK/libs/ssruntime/docs/); do not duplicate them
    here. **Never commit inside the submodule**: a change that needs `ssruntime` or `ssab` is a
    pull request there, followed by a submodule bump.

## What this repo is

A wgpu renderer for SpriteStudio, and first of all a **Rust** player: a clean Rust API over
`ssruntime`, with WebGPU (`wasm-bindgen`) and C/C++ (`cbindgen`) as thin wrappers on top. Unlike
the other Players it links `ssruntime` and `ssab` as Rust crates rather than through the C-API,
which is why the SDK is a submodule. The repository layout is in the README; below is only what
an agent would otherwise break silently.

## Where each answer is written — do not restate it here

| Question | File |
|---|---|
| What the player does, and how to use each face | [README.md](./README.md), the [docs site](./docs/en/) |
| How to build, run and verify, and what each script takes | [CONTRIBUTING.md](./CONTRIBUTING.md), each script's `--help` |
| How the site and the shipped artifacts are built | [RELEASING.md](./RELEASING.md), maintainers only |
| What to build next, and what was measured and declined | [ROADMAP.md](./ROADMAP.md), plus the two entries that outgrew it: [PREVIEW.md](./PREVIEW.md), [FIT_ALIGN.md](./FIT_ALIGN.md) |

## Invariants

*   **Borrow model, and resources at their widest scope.** The player never owns or stores `Device` /
    `Queue` / `Surface` / `SSRenderer`. Pipelines, samplers and the texture map are device-scoped
    (`SSRenderer`), the projection uniform and frame arena target-scoped (`SSScene`), and only the
    playback tree, transform and order are per player — so **`SSPlayer` must not gain a wgpu-typed
    field**, which keeps "1 canvas = 1 player" from coming back. One device per host or page. `.ssab`
    is the player's and borrowed by `ssruntime`; FrameData and SoA are `ssruntime`'s, never held
    across frames.
*   **There is no vertex buffer.** The SoA arrays are memcpy'd into the scene's arena and expanded in
    the vertex stage from `@builtin(vertex_index)`; the CPU writes one 80-byte `Prim` per part and
    nothing per vertex. Put a new feature on the `Prim`. The scalar position accumulation is
    deliberate — `mat4 * vec4` invites FMA contraction and costs the bit-exact golden comparison.
*   **Textures are borrowed, not owned.** `set_texture` takes a `TextureView` the supplier owns and
    `SSPack` holds CPU data only; per-texture facts that vary (`AlphaMode`, texel size) go in the
    group(1) `TexInfo` uniform, never the pipeline key.
*   **Every texture list must read both `cellmaps()` and `external_textures()`.**
    `DrawBatch.texture_hash` is a cellmap's `name_hash` for an ordinary part but a texture *file
    name*'s for one carrying a TextureChange. Missing the second table fails quietly: the hash falls
    back to the 1x1 white texture and the part draws as a white silhouette. One file may be named
    twice, so dedupe by path, never by hash.
*   **The workspace has two fixed points.** wgpu is pinned to the 29 line, because Slint's
    `unstable-wgpu-NN` wants an exact major and one workspace resolves exactly one wgpu; and
    `[workspace] exclude = ["SpriteStudio-SDK"]` stops cargo claiming the SDK crates as members here.
    **No engine may appear in `ssplayer-wgpu`** — adapters are separate crates under `libs/` — and
    **`ffi.rs` holds no logic**: validation, conversion and error-code mapping only, so anything more
    lives in `host.rs` or `player.rs`, where the Rust API gets it too.
*   **The C and JS faces are the same shape, and neither may fuse its handles.** Fusing them makes two
    characters in one window structurally impossible, and a page of N elements build N devices. Both
    name their children through `roster.rs`'s generation-tagged slots, so a stale handle resolves to
    "destroyed" rather than to its successor.
*   **The playback contract, in four parts.** Selecting an animation and starting it are separate
    calls, and **`play()` is a head-out, not a resume** — it rewinds to the section start and re-arms
    the loop count, while `resume()` carries on in place; the four transport verbs stay distinct on
    every face. **Slots step from the part list, never from the draw order**, because the runtime
    drops hidden parts from `draw_order` and a slot would stall the moment its part is hidden; the
    pulse they re-arm on is `Context::is_looped()`. **A pack outlives every tree built from it** —
    `build_node` binds each `Context` to its pack's resource by raw pointer, so re-registering a name
    never changes what is already playing. And **a tick that did not run reports nothing**:
    `set_visible(false)` skips the advance, so `build_frame` clears `events` / `looped` /
    `finished_edge`, while `prev_finished` stays, being the latch that stops a consumed completion
    edge firing twice.
*   **Each player leaves the stencil buffer as it found it** (`unwind_masks`), or the first player's
    mask clips the second in a shared pass. **The two mask backends must agree**: which one a scene
    uses is the host's property, never the animation's, so keep the coverage path inside the mask
    doc's numbers — deviating forks a spec the Godot Player implements too. It **bakes and samples in
    one encoder, never across frames**, hence no mask lag here.
*   **Device loss recovers in one order:** `SSRenderer::restore_device`, `SSScene::restore_device`,
    `SSPlayer::restore_textures`. A format change does not — the pipelines are baked against it.
*   **Loading is one call on every face**, written once — never add an API that makes a host collect
    textures by hand. The only per-platform part is `AssetLoader`, whose contract
    `tests/loader_contract.rs` pins: `reset_base()`, then `read` gets `src` verbatim, *then*
    `set_base(src)`, and every path after that is a bare name. **The walk builds no paths and never
    looks inside `src`.** `load` takes no `Device` / `Queue` / `SSRenderer`, so right after it
    `texture_size(hash)` is still `None`; and `std::fs` cannot replace the loader seam, since it
    compiles on wasm32 and fails at runtime. A load reports `Err` only for an unreadable `.ssab`;
    everything else comes back from `missing_assets()`.
*   **Three faces compile under no default command**: `web.rs` behind `web`, the C face behind `ffi`,
    and the npm layer typed against wasm-pack's `.d.ts`. Build all three after touching public API.
    Building `ssplayer-wgpu-ffi` regenerates the committed C header; read that diff, because cbindgen
    sweeps up every module-level `pub const`.
*   **Atlas decoding at load time is threaded; the per-frame walk is not.** It is a
    `std::thread::scope` gone before the call returns, never a pool beside the host's, and
    `flush_textures` settles which hash wins before dispatching.
*   **Beware green rendering tests.** `tests/render.rs` skips itself when no GPU adapter is present, so a pass there verified nothing. When changing rasterisation, dump the readback to a PNG and look at it — pixel counts alone have hidden real regressions.
