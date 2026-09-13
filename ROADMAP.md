# SpriteStudioPlayer for wgpu — Roadmap

The wgpu Player's roadmap. Runtime and converter work lives in the
[SpriteStudio-SDK roadmap](https://github.com/cri-middleware/SpriteStudio-SDK/blob/main/ROADMAP.md);
items marked **needs SDK** wait on a capability from there.

This player is first and foremost a **Rust** API, with WebGPU (`wasm-bindgen`) and
C/C++ (`cbindgen`) as thin wrappers over it. The invariants every change has to
respect are in [`AGENTS.md`](./AGENTS.md) under *Hard constraints*.

`[x]` = done · `[ ]` = planned · ⛔ = blocked

## Playback & rendering

- [x] Draw kinds — Normal, Mesh, Shape, Instance, Effect, Mask.
- [x] All 12 blend modes and part colours, premultiplied throughout.
- [x] Player-level tint — `set_tint` / `ss_player_set_tint` / `setTint`, taking
      `0xRRGGBB` like `ssplayer-webgl`'s `tint`. It applies after the PartsColor
      mix, so it could not fold into vertex colour: it rides on `Prim` beside the
      player alpha and multiplies the finished colour in every fragment path.
- [x] Stencil masking, including clipping writers and per-player unwind.
- [x] Coverage-bitmap masking (mask doc §3-2), for hosts whose pass lends no
      stencil. Same semantics, no one-frame lag, one extra pass per masking
      player. `MaskBackend` picks between the two.
- [x] The 13 `ss_shader` integration shaders (the `SHDR` part attribute).
- [x] Playback control — play / pause / stop, seek, section, reverse, ping-pong,
      speed, frame rate, loop counts.
- [x] Per-part runtime overrides — colour, cell, visibility.
- [x] Events — user data, signal, audio, animation finished / looped.
- [x] Many players per render target, drawn in one pass in ascending `order`.
- [ ] **9-slice parts.** The runtime already emits the batches and the geometry
      (`get_nines_*`), so this is one WGSL vertex entry, one `GeomClass`, one
      `GeomKind` — which also gives Nines mask coverage — and the `FrameView`
      fields to carry it. `assets/overall/NineSlice.ssab` exists and draws nothing
      today.
- [ ] **Text parts.** Much larger than 9-slice despite sharing its label: the
      runtime hands over a batch with no geometry and a static string, so font
      loading, layout and a glyph atlas all land on this side.
- [ ] **Integration shader gaps** — `map0` / `map1` secondary textures, shader
      parts combined with a mask, and parts other than Normal.
- [ ] **The started edge — low priority.** The [Player API
      conventions](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/20_design/40_api_conventions.md#4-events)
      make `animationStarted` one of six required events, and it is the only one
      with no counterpart here on any face: `SsEvent` carries `UserData` /
      `Signal` / `Audio`, and the two lifecycle edges are `just_looped()` /
      `just_finished()` (`ss_player_looped_last_update` /
      `ss_player_finished_last_update`, `justLooped()` / `justFinished()`,
      `SsLooped` / `SsFinished`). There is no `just_started()`. Every one of the
      other six Players publishes the event.
      **Low priority because the caller usually already knows**: `set_animation`
      + `play` is the host's own call, so on a polling API the edge is readable
      at the call site, and §4 lets a host that cannot subscribe publish pulses
      instead of events. What it does not cover is a start the host did not
      initiate — an instance slot heading out, or a declarative binding that
      swapped the animation — which is the case that would justify doing it.
      The shape if it lands: `just_started()` beside the other two, cleared by
      the same `build_frame` path that clears them, `ss_player_started_last_update`
      on the C face, `justStarted()` on the js face, `SsStarted` on Bevy, and a
      row in the *Completion and loop edges* table in
      [`docs/{en,ja}/guide/events.md`](./docs/en/guide/events.md). Until then the
      absence is worth stating there rather than leaving a reader to infer it.

## Expressiveness

- [ ] **Retire one of the two mask backends.** Coverage (mask doc §3-2) now
      exists beside the stencil, which is a migration state rather than a
      destination: the same `.ssab` rendering differently depending on what the
      host's pass could lend is a poor thing to keep forever. The decision wants
      numbers, not opinion — run the golden corpus through both and count where
      they part, then measure what the extra pass costs. Retiring the stencil
      would also drop `SceneConfig::stencil_format` from the host contract and a
      whole invariant (`unwind_masks`); keeping both means every future mask
      feature lands twice.
- [ ] **Exact masks, past what the stencil can express.** Mixed INCR / INVERT
      overlaps are approximated and nesting is limited by 8 bits. The coverage
      backend already lifts the nesting limit — its accumulators are shader
      variables, not stencil bits — but both backends still stop at the planner's
      24 writers, which is the coverage encoding's own budget (RGBA8, one bit
      each). Going past 24 means a wider encoding, and doing it here alone would
      fork a limit the other Players share.
- [ ] **An offscreen path, for group alpha first.** `set_alpha` composites each
      part at that alpha in turn, so coverage accumulates where parts overlap —
      two halves of a character at 0.5 leave the overlap at 0.75. Drawing the
      player whole and compositing once fixes it, as `SSGroupAlpha.ts` does in the
      WebGL player. **Settle the API first**: the host opens the render pass, so
      inserting an offscreen changes what a host lends.
- [ ] ⛔ **Backdrop-reading blends (`Div2` / `Overlay2`).** Same offscreen path,
      blocked on semantics rather than on wgpu. No reference defines the maths —
      the sibling players fall back to `Mix` and the published docs call that
      fallback intentional — and no fixture authors the modes. Needs a formula
      with a source, a fixture, and agreement across the other Players first.
- [ ] **Crowd rendering (one data set : N instances).** Undesigned. The vertex
      path suits it: stack N placement matrices, select with
      `@builtin(instance_index)`, and CPU cost stops scaling with body count.
      Members cannot then differ in playback position or interleave front-to-back,
      and masks and effects are shared. **Undecided**: the API shape
      (`SSPlayer::set_instances` or a separate `SSCrowd`), which attributes vary
      per member, and how the js and C faces expose it.

## Hosts & platforms

- [x] Rust — `examples/viewer` (winit).
- [x] WebGPU / js — one device per page, any number of canvases and players.
- [x] C / C++ — verified on macOS (Metal) and Windows (DX12).
- [x] Device-loss recovery on every face.
- [ ] **Vulkan C++ viewer** (Linux) — `examples/viewer-vulkan-cpp`. Written, and
      never built or run: it was written on macOS, where neither the X11 nor the
      Wayland code compiles. It carries both window paths in one binary because
      Xwayland means an X11 window never reaches the `SS_WINDOW_WAYLAND` arm, so
      closing this needs one run per path on a Linux desktop. Until then the Xlib
      and Wayland arms of `surface_target` stay unverified.
- [x] Bevy — `libs/ssplayer-bevy`, with `examples/viewer-bevy` and
      `examples/overall-bevy` as its hosts. A separate crate, so the core never
      depends on Bevy and the adapter can follow its own wgpu major; it tracks one
      Bevy line at a time, because Bevy and the player must resolve to a single
      `wgpu`.

## Resource management

- [ ] **Pack and texture eviction.** A player only accumulates packs, and the
      renderer cannot enumerate the textures it holds, so a host that swaps
      characters grows `.ssab` bytes and VRAM with no way to reclaim them. Any
      such API must retire rather than drop: a `Context` points at its pack's
      resource by raw pointer.
- [ ] **Generation-checked scene handles on the C face.** `ss_player_t` is safe
      against a stale handle; the `*mut SsScene` it holds is not, so destroying
      the scene first cannot be detected. Closing it is an ABI change.

## Distribution & verification

- [ ] **Release automation.** `release.yml` builds every platform and
      `build-release.sh` assembles the zips, but CI has no macOS job — so the
      Metal viewer is never built there. Cross-compilation is unsupported, so each
      platform needs its own runner.
- [ ] **Nothing publishes the npm package yet.** `packages/ssplayer-wgpu` is
      `@spritestudio/ssplayer-wgpu` in its shipping shape — scoped name, the
      custom element as its own `./element` entry point, its own JavaScript-facing
      README, `prepack` refusing a package whose `dist/` or `wasm/` is missing —
      and `scripts/build.sh platform=web` fills it. What is missing is the channel:
      - A `publish_npm` input on `release.yml` and an `npm publish` step, with an
        npm automation token as a repository secret. The version is
        `7.0.0-beta.1` and `publishConfig.tag` is `beta`, so a publish cannot take
        over `latest` by accident.
      - **npm provenance stays off** until this repository is public — npm rejects
        the publish outright rather than degrading, which is the same reason
        `SSPlayerForWeb` has it off.
      - Version bumps are hand-written here (no changesets, one package), so the
        publish step has to be the thing that refuses to re-publish a version that
        is already on the registry.
- [ ] **macOS signing and notarization** for the cdylib. Portable from the sibling
      players, using the existing organisation secrets.
- [ ] **Version stamping.** Versions are hand-written, and the SDK is a submodule
      rather than a release zip, so artifacts need to record the SDK commit they
      were built from.
- [ ] **`CHANGELOG.md`** — with the first release, not before it. Entries written
      now would describe a version nobody can install, and the shape of the
      first one is decided by what actually ships.
- [x] **`THIRD_PARTY_NOTICES.md`.** `scripts/generate-licenses.sh` runs `cargo about`
      over `libs/ssplayer-wgpu` with `--features embedded,ffi,web` — the union of what
      the Rust library, the C face and the npm package link, which is exactly what the
      archives carry. `libs/ssplayer-bevy` sits outside that set, so the `accepted`
      list stays the SDK's own: Bevy's tree pulls `MPL-2.0` (the symphonia family),
      `BSD-2-Clause`, `MIT-0` and one `(MIT OR Apache-2.0) AND NCSA`. The adapter is
      consumed as source, so that tree is the host's to account for; putting it in an
      archive would need the review first.
- [ ] ⛔ **crates.io.** `ssruntime` and `ssab` are path dependencies into the
      submodule; cargo will not publish a crate that depends on paths only.
- [ ] **Browser-side measurement.** `surface.stats()` is wired through to
      `examples/web` but has never been read in a browser, where one command is
      one JS call and the ratios differ from native.

## Needs SDK

- [ ] **GPU expansion of effect particles.** Passing particle state and expanding
      in the vertex stage cuts the transfer to a quarter and removes the CPU
      trigonometry, but it changes the shape of `EffectDrawPlan` — an SDK pull
      request plus a submodule bump.

## Deliberate non-goals

- **Throughput optimisation** — draw-call merging, bindless textures, texture
  bind-group dedup, render bundles, threading the player walk, indirect draws for
  Shape. Measured against a 256-player frame and not worth their complexity;
  bindless is additionally unavailable on WebGPU.
- **A counter for dropped mask writers.** Player safety stays minimal here, and
  the cap it would report on goes away with the coverage bitmap above.
- **Discarding transparent texels instead of writing zero.** Parity with the
  WebGL player, which does the same.
- **Sharing an engine's device through the C ABI.** wgpu types do not cross a C
  boundary; engine embedding is a Rust-only capability, delivered as adapter
  crates. From C the route is offscreen rendering plus a shared texture.
- **The GL backend.** The renderer pulls its vertices from storage buffers bound
  to the vertex stage, which GL exposes only from 4.3 / GLES 3.1 and WebGL not at
  all. No GL adapter is selected or verified anywhere: the instance takes
  `Backends::PRIMARY` (Vulkan / Metal / DX12 / WebGPU), and the C API's backend
  selector offers DX12, Metal and Vulkan.
- **A WebGL fallback.** `ssplayer-webgl` is maintained and is the answer for
  environments without WebGPU.
