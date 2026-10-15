# SSPlayerForWgpu — AI Agent Guide

## Operating rules

*   Keep changes scoped to the requested task. Unrequested hardening is a suggestion, not a commit; a defect
    you actually hit in the code you touched is not — fix it, or say plainly why you did not.
*   Before editing, read enough surrounding context to understand callers and invariants, the ones below
    included.
*   **Anything that exists twice changes twice** — a page and its translation, a `.sh` and its `.ps1`.
    Missing one half is this family's most common defect.
*   **Never break a line between two Japanese characters**; Chrome renders the break as a space.
*   **Finish with this repository's checks**: the matrix in [CONTRIBUTING.md](./CONTRIBUTING.md). For
    `clippy --fix`, `git add` your edits first and pass `--allow-staged`, never `--allow-dirty`. Read what a
    run says it verified, not its exit code — several checks pass, or skip themselves, without a
    prerequisite.
*   **Do not restate what another file owns**; the table below says which file. A fact no file owns goes
    where its staleness will be noticed.
*   **Never commit inside the `SpriteStudio-SDK` submodule.** A core change is an SDK pull request, then a
    submodule bump here.
*   **Playback and rendering semantics are the SDK's**: its [AGENTS.md](./SpriteStudio-SDK/AGENTS.md) and
    the porting guide in `SpriteStudio-SDK/libs/ssruntime/docs/`. This player is the guide's reference:
    what it settles where the guide is silent goes into the guide in the same change.
*   **Code comments never cite the porting guide** (sections, paths, quotes): it moves, and nothing checks
    the citation. Say what breaks instead.

## What this repo is

A wgpu renderer for SpriteStudio and, first of all, a **Rust** player: a Rust API over `ssruntime`,
linked as a crate rather than through the C-API, with WebGPU (`wasm-bindgen`) and C/C++ (`cbindgen`)
faces as thin wrappers.

## Where each answer is written

| Question | File |
|---|---|
| Use, per face | [README.md](./README.md), the [docs site](./docs/en/) |
| Constraints, build, verify, docs, scripts | [CONTRIBUTING.md](./CONTRIBUTING.md), each script's `--help` |
| Releases, maintainers only | [RELEASING.md](./RELEASING.md) |
| Plans, and what was measured and declined | [ROADMAP.md](./ROADMAP.md), and [PREVIEW.md](./PREVIEW.md) |

## Invariants

*   **Resources live at their widest scope.** Pipelines, samplers and textures are device-scoped
    (`SSRenderer`), the projection uniform and frame arena target-scoped (`SSScene`), and only the playback
    tree, transform and order belong to a player. One device per host or page; FrameData and SoA are never
    held across frames.
*   **No per-frame allocations in the playback hot path.** `FrameChunk::reset` empties every buffer and
    keeps its capacity, and GPU buffers only grow; a new per-frame buffer goes on the chunk, or on the
    player's `Walk` when only that player's walk uses it. A host that keeps its players hands them to
    `SSScene::update_with` rather than collecting a `Vec` of them. `tests/allocations.rs` pins it, in
    the player crate and in the Bevy adapter.
*   **Textures are borrowed**: `set_texture` takes a supplier-owned `TextureView`, and `SSPack` holds CPU
    data only. Per-texture facts that vary (`AlphaMode`, texel size) go in the group(1) `TexInfo` uniform,
    never the pipeline key. Every texture list reads `external_textures()` as well as `cellmaps()` — a
    missed TextureChange hash draws a white silhouette — and dedupes by path, since one file may be named
    twice.
*   **The four transport verbs stay distinct on every face.** Selecting an animation and starting it are
    separate calls; `play()` rewinds to the section start and re-arms the loop count, and `resume()`
    continues in place.
*   **Slots step from the part list, never the draw order**: the runtime drops hidden parts from
    `draw_order`. They re-arm on `Context::just_looped()`.
*   **A pack outlives every tree built from it**: `build_node` binds each `Context` to its pack by raw
    pointer, so re-registering a name never changes what is already playing.
*   **A tick that did not run reports nothing.** `set_culled(true)` skips the advance, and `build_frame`
    clears the frame's events and edges.
*   **The two mask backends must agree**: which one a scene uses is the host's choice, not the animation's.
    Keep the coverage path inside the mask doc's numbers, which the Godot Player implements too. It bakes
    and samples in one encoder, never across frames.
*   **Loading is one call on every face**; never make a host collect textures. Only `AssetLoader` is per
    platform, and `tests/loader_contract.rs` pins its contract. `load` takes no `Device` / `Queue` /
    `SSRenderer`, so `texture_size(hash)` is `None` right after it. `std::fs` cannot stand in for the
    loader: it compiles on wasm32 and fails at runtime. Only an unreadable `.ssab` is an `Err`; the rest
    comes back from `missing_assets()`.
*   **Three faces compile under no default command** — `web.rs` behind `web`, the C face behind `ffi`, the
    npm layer against wasm-pack's `.d.ts`. Build all three after touching public API.
*   **When changing rasterisation, dump the readback to a PNG and look at it**; pixel counts alone have
    hidden real regressions.
