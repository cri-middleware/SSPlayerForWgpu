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
- [ ] **Player-level tint.** Applies after the PartsColor mix, so it cannot fold
      into vertex colour; it belongs on `Prim`, beside the player alpha.
- [ ] **Integration shader gaps** — `map0` / `map1` secondary textures, shader
      parts combined with a mask, and parts other than Normal.

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
- [ ] **Vulkan C++ viewer** (Linux). Its absence is also why the Xlib and Wayland
      window-handle paths are unverified.
- [ ] **Bevy adapter** (`libs/ssplayer-wgpu-bevy`). Unblocked by the `set_texture`
      borrow channel. A separate crate, so the core never depends on Bevy and the
      adapter can follow its own wgpu major.

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

- [ ] **Release automation.** No `release.yml`, and CI has no macOS job — so the
      Metal viewer is never built there. Two faces need one: web (npm, from
      `pkg/`) and desktop FFI. Cross-compilation is unsupported, so each platform
      needs its own runner.
- [ ] **The npm package is not yet the shape it will ship in.** `scripts/build.sh
      platform=web` produces a valid package directory — ES module, `.d.ts`,
      `main` / `types` / `files` all set — but three things are still wrong for a
      published one:
      - The name is bare `ssplayer-wgpu`; the sibling Web packages are all under
        the `@spritestudio` scope.
      - `<ss-player-webgpu>` and the shared-context helper are plain JavaScript in
        `examples/web` and are not part of the package, so installing it gets the
        `SsContextWeb` / `SsSurfaceWeb` / `SsPlayerWeb` API and nothing else.
        `ssplayer-webgl` ships its custom element as an entry point of the package;
        this should too, and stay optional so a page that does not want it pays
        nothing.
      - wasm-pack copies the crate's README into the package, and that README is
        written for Rust callers. The npm page needs its own.
- [ ] **macOS signing and notarization** for the cdylib. Portable from the sibling
      players, using the existing organisation secrets.
- [ ] **Version stamping.** Versions are hand-written, and the SDK is a submodule
      rather than a release zip, so artifacts need to record the SDK commit they
      were built from.
- [ ] **Missing repository assets** — `CHANGELOG.md`, `THIRD_PARTY_NOTICES.md`,
      `SHA256SUMS` for artifacts, and the `CONTRIBUTING.md` / `SUPPORT.md` /
      `CODE_OF_CONDUCT.md` every sibling player carries.
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
- **A WebGL fallback.** `ssplayer-webgl` is maintained and is the answer for
  environments without WebGPU.
