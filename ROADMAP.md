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
- [x] Runtime texture replacement — swap the texture a cellmap binds from the
      program (SDK `70_external_resources.md`), `set_cellmap_texture` /
      `ss_player_set_cellmap_texture` / `setCellmapTexture`, plus the Bevy
      `SsCellmapTextures` component. A Player-side override, distinct from the
      per-part Override Layer. Where the sibling Players overwrite an entry in a
      per-player texture map, this one keeps that map in the device-scoped
      renderer, so an override is a *redirect* of the player's own atlas-key table
      to a private key — per player, an instance child (its own pack) does not
      inherit it, and a restore points back at the untouched atlas rather than
      reloading it.
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
      `just_finished()` (`ss_player_just_looped` /
      `ss_player_just_finished`, `justLooped()` / `justFinished()`,
      `SsLooped` / `SsFinished`). There is no `just_started()`. Every one of the
      other six Players publishes the event.
      **Low priority because the caller usually already knows**: `set_animation`
      + `play` is the host's own call, so on a polling API the edge is readable
      at the call site, and §4 lets a host that cannot subscribe publish pulses
      instead of events. What it does not cover is a start the host did not
      initiate — an instance slot heading out, or a declarative binding that
      swapped the animation — which is the case that would justify doing it.
      The shape if it lands: `just_started()` beside the other two, cleared by
      the same `build_frame` path that clears them, `ss_player_just_started`
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
      per member, and how the js and C faces expose it. Members at *different*
      playback positions are not this design; they belong to the SDK's
      [Animation Instancing](https://github.com/cri-middleware/SpriteStudio-SDK/blob/main/ROADMAP.md#phase-3-feature-expansion-post-release),
      one evaluation driving many instances. That item has two possible faces —
      one playhead drawn at N placements (this) and N members at N phases (not
      this) — and which of them it covers decides how much of this is local.

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
- [ ] **Bevy: more than one camera.** The adapter holds one `SSScene`, binds it to
      the first view that renders and skips the rest with a warning
      (`ssplayer-bevy/src/render.rs`, `bound_view`), so a UI camera, a
      render-to-texture portrait, a minimap or a split screen leaves every
      character in whichever view happened to come first. A scene per view, keyed
      by the view `Entity`, closes it **with no change to the player**: each player
      still belongs to one view, so nothing is advanced twice, and
      `tests/pack.rs::scenes_share_one_renderer` already pins N scenes on one
      renderer. What the adapter has to settle: which view a player draws into
      (`RenderLayers`), what a player no view draws does (it never advances, so it
      freezes), and moving the feedback store off the per-view path —
      `FeedbackChannel::store` replaces the batch wholesale, so today the last view
      to run would be the only one the main world hears about.
- [ ] **Self-contained HTML preview** — a per-animation button in a host writes one
      fully inlined HTML file and opens it in the browser, so every host shows the same
      preview without an in-editor renderer. `file://` is enough — WebGPU and a
      vertex-stage `var<storage, read>` both work there — so no local HTTP server. Needs
      its own `--target no-modules` wasm. Design and the verified findings:
      [`PREVIEW.md`](./PREVIEW.md); the fit/align it waited on is in.

## UI integration

wgpu is a GPU API, not a UI toolkit, so this splits in two: the portable half belongs in the
player, where every host and the C ABI reach it, and the host half belongs in an adapter crate.

- [ ] **Part rect read-out and hit testing.** `part_transform` reports where a part is;
      nothing reports how big it is, so a host cannot turn a part into a clickable region.
      The `PartState` the runtime already delivers every frame carries `size_x` / `size_y`,
      `anchor` and `pivot` — `part_rect` over those and the part transform, then
      `hit_test_part(x, y)` over that, in player-local coordinates. Portable: it is C-ABI
      shaped, needs no GPU work, and every adapter crate inherits it.
- [x] **`fit` / `align`** — `SSPlayer::fit_into` / `ss_player_fit_into` / `fitInto`, over the
      family's vocabulary (`contain` / `cover` / `none` and nine alignments). Pure arithmetic in
      `fit.rs`, so the three faces are translation only and an adapter inherits it; the contract
      it ports is [`FIT_ALIGN.md`](./FIT_ALIGN.md). The box is the animation's authored canvas
      (`canvas_rect`, the canvas placed against the origin by its pivot) unless the caller supplies
      one — **measured boxes are not built**: `frameScope: animation | pack` would mean baking every
      frame at load, and nothing needs it yet. Not wired into `ssplayer-bevy`, which is the first
      half of *Bevy UI* below.
- [ ] **Bevy UI (`Node`).** `ssplayer-bevy` gives Bevy a player in the world; a player inside
      Bevy's UI tree is a different thing — it has to size from `ComputedNode`, respect the UI
      camera's stacking and clipping, and answer `Interaction` picking. The two primitives
      above are what it would be built on, and it is the only Rust UI target that costs
      nothing new to reach.

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
- **One player drawn into several views** — a minimap, a split screen, a portal, a
  Vtuber tool's preview beside its output. `SSScene::update` fuses the timeline
  advance with the geometry build, so handing one player to two scenes plays it at
  double speed, and splitting the two here would build half of the SDK's
  [Animation Instancing](https://github.com/cri-middleware/SpriteStudio-SDK/blob/main/ROADMAP.md#phase-3-feature-expansion-post-release)
  — one evaluation driving many instances — a second time. Two players over one
  `.ssab` is what serves it meanwhile: `SSPack` is three `Arc`s, the renderer
  uploads an atlas once, and two trees fed the same `dt` stay identical down to
  the effect particles, whose seed is the part index. A tool whose preview must
  *be* its output wants neither — render once into a texture it owns and present
  that texture in both places.
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
- **An egui or iced adapter.** Both are immediate-mode-shaped hosts whose
  integration point is a texture, so the route is the one already documented for C:
  render offscreen and hand over the texture. A first-class adapter crate waits on a
  concrete request; Bevy UI above is the Rust UI target that is actually asked for.
- **A WebGL fallback.** `ssplayer-webgl` is maintained and is the answer for
  environments without WebGPU.
