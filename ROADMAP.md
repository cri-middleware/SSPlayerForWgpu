# SpriteStudio Player for wgpu — Roadmap

This file lists unfinished work only. Technical constraints live in the [limitations page](./docs/en/limitations.md);
SDK-side work lives in the [SpriteStudio-SDK ROADMAP](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/ROADMAP.md).
This player is a **Rust** API first, with WebGPU (`wasm-bindgen`) and C/C++ (`cbindgen`) as thin wrappers over it;
the constraints a change most often breaks are in [CONTRIBUTING.md](./CONTRIBUTING.md#technical-constraints--guiding-principles).

## Playback & rendering

- [ ] **9-slice parts** — the runtime already emits the batches and the geometry
      (`get_nines_*`), so this is one WGSL vertex entry, one `GeomClass`, one
      `GeomKind` — which also gives Nines mask coverage — and the `FrameView`
      fields to carry it. `assets/overall/NineSlice.ssab` exists and draws nothing
      today.
- [ ] **Text parts** — much larger than 9-slice despite sharing its label: the
      runtime hands over a batch with no geometry and a static string, so font
      loading, layout and a glyph atlas all land on this side.
- [ ] **Integration shader gaps** — `map0` / `map1` secondary textures, shader
      parts combined with a mask, and parts other than Normal.

## UI integration

wgpu is a GPU API, not a UI toolkit, so this splits in two: the portable half belongs in the
player, where every host and the C ABI reach it, and the host half belongs in an adapter crate.

- [ ] **Part rect read-out and hit testing** — `part_transform` reports where a part is;
      nothing reports how big it is, so a host cannot turn a part into a clickable region.
      The `PartState` the runtime already delivers every frame carries `size_x` / `size_y`,
      `anchor` and `pivot` — `part_rect` over those and the part transform, then
      `hit_test_part(x, y)` over that, in player-local coordinates. Portable: it is C-ABI
      shaped, needs no GPU work, and every adapter crate inherits it.
- [ ] **Bevy UI (`Node`)** — `ssplayer-bevy` gives Bevy a player in the world; a player inside
      Bevy's UI tree is a different thing — it has to size from `ComputedNode`, respect the UI
      camera's stacking and clipping, and answer `Interaction` picking. The part rect above
      and `SSPlayer::fit_into` — which `ssplayer-bevy` does not wire — are what it would be
      built on, and it is the only Rust UI target that costs nothing new to reach.

## Expressiveness

- [ ] **One mask backend, once Bevy's 2d pass carries a stencil** — both
      exist because each is the cheap answer somewhere: a host that lends a
      stencil pays nothing extra for it, while coverage (mask doc §5) is the only
      answer for a pass with none to lend — and Bevy's is one, `bevy_core_pipeline`
      0.20 fixing `CORE_2D_DEPTH_FORMAT` at `Depth32Float`. Folding onto coverage
      would charge every stencil host an extra pass and a coverage array per
      masking player, so the fold waits for that constant to gain a stencil, and
      goes the other way: dropping coverage takes the extra pass with it, while
      `SceneConfig::stencil_format` and `TreeMask::finish` stay. A unit test in
      `ssplayer-bevy/src/render.rs` fails when the constant changes, so a Bevy bump
      cannot move it unnoticed. Until then every mask feature lands in both. Blocked
      on: Bevy's 2d pass carrying a stencil.
- [ ] **Exact masks, past what the stencil can express** — mixed INCR / INVERT
      overlaps are approximated and nesting is limited by 8 bits. The coverage
      backend already lifts the nesting limit — its accumulators are shader
      variables, not stencil bits — but both backends still stop at 96 writers
      across a player's instance tree, the limit the Godot Player shares; the
      coverage backend spends one more RGBA8 layer per 24 of them. Going past 96
      alone would fork that limit.
- [ ] **An offscreen path, for group alpha first** — `set_alpha` composites each
      part at that alpha in turn, so coverage accumulates where parts overlap —
      two halves of a character at 0.5 leave the overlap at 0.75. Drawing the
      player whole and compositing once fixes it, as `SSGroupAlpha.ts` does in the
      WebGL player. **Settle the API first**: the host opens the render pass, so
      inserting an offscreen changes what a host lends.
- [ ] **Backdrop-reading blends (`Div2` / `Overlay2`)** — same offscreen path,
      blocked on semantics rather than on wgpu. No reference defines the maths —
      the sibling players fall back to `Mix` and the published docs call that
      fallback intentional — and no fixture authors the modes. Blocked on: a formula
      with a source, a fixture, and agreement across the other Players.
- [ ] **Crowd rendering (one data set : N instances)** — undesigned. The vertex
      path suits it: stack N placement matrices, select with
      `@builtin(instance_index)`, and CPU cost stops scaling with body count.
      Members cannot then differ in playback position or interleave front-to-back,
      and masks and effects are shared. **Undecided**: the API shape
      (`SSPlayer::set_instances` or a separate `SSCrowd`), which attributes vary
      per member, and how the js and C faces expose it. Members at *different*
      playback positions are not this design; they belong to the SDK's
      [Animation Instancing](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/ROADMAP.md#phase-3-feature-expansion-post-release),
      one evaluation driving many instances. That item has two possible faces —
      one playhead drawn at N placements (this) and N members at N phases (not
      this) — and which of them it covers decides how much of this is local.

## Hosts & platforms

- [ ] **Vulkan C++ viewer (Linux)** — `examples/viewer-vulkan-cpp`. Written on
      macOS, where neither the X11 nor the Wayland code compiles, and built by CI on
      Linux for every pull request — but never run, because the runner has no
      display. It carries both window paths in one binary because
      Xwayland means an X11 window never reaches the `SS_WINDOW_WAYLAND` arm, so
      closing this needs one run per path on a Linux desktop. Until then the Xlib
      and Wayland arms of `surface_target` stay unverified.
- [ ] **Bevy: more than one camera** — the adapter holds one `SSScene`, binds it to
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
- [ ] **An offscreen target on the C face** — a scene renders only to the window or
      layer it was created on, so a C/C++ engine cannot composite a character with its
      own drawing; the route the docs give for it — render into a texture and bring it
      in through the engine's interop — has no call behind it. Needs: a scene created
      on a texture rather than a window, and a way for that texture to leave.
      **Undecided**: whether that is a native shared handle per backend (a DXGI shared
      handle, an `IOSurface`, Vulkan external memory — none of which wgpu creates by
      default), a CPU readback as the portable floor, or both.
- [ ] **Self-contained HTML preview** — a Preview button per pack in each host writes one
      fully inlined HTML file and opens it in the browser, so every host shows the same
      preview without an in-editor renderer. Every Release carries the shell, for a
      plugin to fetch by its tag rather than build. Missing: the buttons themselves,
      which live in the Player repositories. Design and measurements:
      [`PREVIEW.md`](./PREVIEW.md).

## Resource management

- [ ] **Pack and texture eviction** — `SSRenderer::forget_texture` drops one texture by
      hash on the Rust face, but a player only accumulates packs, the renderer cannot
      list the textures it holds, and the C and JavaScript faces have no forget call —
      so a host that swaps characters grows `.ssab` bytes and VRAM with no way to
      reclaim them. Missing: a per-player pack release on every face. It must retire
      rather than drop: a `Context` points at its pack's resource by raw pointer.
- [ ] **Generation-checked scene handles on the C face** — `ss_player_t` is safe
      against a stale handle; the `*mut SsScene` it holds is not, so destroying
      the scene first cannot be detected. Closing it is an ABI change.

## Distribution & verification

- [ ] **crates.io** — `ssruntime` and `ssab` are path dependencies into the
      submodule; cargo will not publish a crate that depends on paths only.
      Blocked on: the SDK crates `ssab` and `ssruntime` being published first.

## Needs SDK

- [ ] **GPU expansion of effect particles** — passing particle state and expanding
      in the vertex stage cuts the transfer to a quarter and removes the CPU
      trigonometry, but it changes the shape of `EffectDrawPlan`: an SDK pull
      request plus a submodule bump. Waits for: an `EffectDrawPlan` that carries
      particle state (the SDK ROADMAP has no entry for it).

## Deliberate non-goals

- **Throughput optimisation** — draw-call merging, bindless textures, texture
  bind-group dedup, render bundles, threading the player walk, indirect draws for
  Shape. Measured against a 256-player frame and not worth their complexity;
  bindless is additionally unavailable on WebGPU. The browser does not change that.
  `tests/scale.rs`'s cast in Chrome 155 (release wasm, Apple M4 Max, macOS 26.7) draws
  the same list as native — at 256 players 2496 draws, 704 mergeable, 1983 rebinds —
  and pays more per call for it: 1.86 ms per `surface.frame()` against 0.87 ms native,
  and 0.52 ms of GPU pass against 0.10 ms. The browser's figure also covers acquiring
  the canvas texture and the submit, about 0.16 ms of it fixed. Merging every
  mergeable draw would remove 28% of the calls, a fraction of a millisecond.
- **One player drawn into several views** — a minimap, a split screen, a portal, a
  tool's preview beside its output. `SSScene::update` fuses the timeline
  advance with the geometry build, so handing one player to two scenes plays it at
  double speed, and splitting the two here would build half of the SDK's
  [Animation Instancing](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/ROADMAP.md#phase-3-feature-expansion-post-release)
  — one evaluation driving many instances — a second time. Two players over one
  `.ssab` is what serves it meanwhile: `SSPack` is three `Arc`s, the renderer
  uploads an atlas once, and two trees fed the same `dt` stay identical down to
  the effect particles, whose seed is the part index. A tool whose preview must
  *be* its output wants neither — render once into a texture it owns and present
  that texture in both places.
- **A counter for dropped mask writers.** Player safety stays minimal here: a pack
  past the 96 writers both backends share (*Exact masks* above) is the data's to
  fix, not a tally for the player to keep.
- **Discarding transparent texels instead of writing zero.** Parity with the
  WebGL player, which does the same.
- **Sharing an engine's device through the C ABI.** wgpu types do not cross a C
  boundary; engine embedding is a Rust-only capability, delivered as adapter
  crates. From C the route would be offscreen rendering plus a shared texture, which
  waits on *An offscreen target on the C face* above.
- **The GL backend.** The renderer pulls its vertices from storage buffers bound
  to the vertex stage, which GL exposes only from 4.3 / GLES 3.1 and WebGL not at
  all. No GL adapter is selected or verified anywhere: the instance takes
  `Backends::PRIMARY` (Vulkan / Metal / DX12 / WebGPU), and the C API's backend
  selector offers DX12, Metal and Vulkan.
- **An egui or iced adapter.** Both are immediate-mode-shaped hosts whose
  integration point is a texture, so the route needs no crate: draw through the Rust
  API into a texture you own — the host opens the render pass, so its target can be
  any texture — and hand that texture to the toolkit. A first-class adapter crate
  waits on a concrete request; Bevy UI above is the Rust UI target that is actually
  asked for.
- **A WebGL fallback.** `ssplayer-webgl` is maintained and is the answer for
  environments without WebGPU.
