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
Hit testing, UI states and reduced motion follow the SDK porting guide's
[UI integration page](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/50_features/80_ui_integration.md).

- [ ] **Part outline read-out and hit testing** — `part_transform` reports where a part is
      but nothing reports how big it is, so a host cannot turn a part into a clickable
      region. `part_quad`, built as the SDK guide recommends (a cell part's drawn corners, a
      Mesh part's vertex box, a Shape's size around its pivot), then
      `hit_test_part(x, y, parts)` over that in player-local coordinates, answering from the
      parts the host names (every part with an outline when it names none), is C-ABI shaped
      and needs no GPU work, so every adapter crate inherits it.
- [ ] **Bevy UI (`Node`)** — `ssplayer-bevy` gives Bevy a player in the world; a player inside
      Bevy's UI tree is a different thing — it has to size from `ComputedNode`, respect the UI
      camera's stacking and clipping, and answer picking through `Hovered` / `Pressed`
      (`Interaction` is deprecated in Bevy 0.20). The part outline above
      and `SSPlayer::fit_into` — which `ssplayer-bevy` does not wire — are what it would be
      built on, and it is the only Rust UI target that costs nothing new to reach. The UI
      states, the selected state (from `Checked`) and reduced motion belong in the same adapter
      crate, over a label-section call (`set_animation_section_by_label`) the player does not
      have yet.

## Expressiveness

- [ ] **One mask backend, once Bevy's 2d pass carries a stencil** — both backends exist
      because each is the cheap answer somewhere: a host that lends a stencil pays
      nothing extra for it, while coverage
      ([mask doc §5](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/40_parts/40_mask.md#5-implementing-on-engines-without-a-stencil-coverage-bitmap))
      is the only answer for a pass with none to lend, and Bevy's 2d pass is one
      (`CORE_2D_DEPTH_FORMAT` is `Depth32Float`). When that constant gains a stencil,
      coverage goes and the stencil backend stays, and a unit test in `ssplayer-bevy`
      fails when it changes; until then every mask feature lands in both. Blocked on:
      Bevy's 2d pass carrying a stencil.
- [ ] **Exact masks, past what the stencil can express** — mixed INCR / INVERT overlaps
      are approximated and nesting is limited by 8 bits; the coverage backend lifts the
      nesting limit, but both backends stop at 96 writers across a player's instance
      tree. The Godot Player shares that limit, so going past it alone would fork it.
- [ ] **Backdrop-reading blends (`Div2` / `Overlay2`)** — they read what is behind the
      part, which `SSScene::encode_offscreen` could copy for them, so this is blocked on
      semantics rather than on wgpu: no reference defines the maths, the sibling players
      fall back to `Mix` (which the published docs call intentional), and no fixture
      authors the modes. Blocked on: a formula with a source, a fixture, and agreement
      across the other Players.

## Hosts & platforms

- [ ] **Vulkan C++ viewer (Linux)** — `examples/viewer-vulkan-cpp` is built by CI on
      Linux for every pull request but never run, because the runner has no display, so
      the Xlib and Wayland arms of `surface_target` are unverified. Closing it takes one
      run per window path on a Linux desktop, since under Xwayland an X11 window never
      reaches the `SS_WINDOW_WAYLAND` arm.
- [ ] **Bevy: more than one camera** — the adapter binds its one `SSScene` to the first
      view that renders and skips the rest with a warning, so a UI camera, a
      render-to-texture portrait, a minimap or a split screen leaves every character in
      whichever view came first. A scene per view, keyed by the view `Entity`, closes it
      with no change to the player; the adapter has to settle which view a player draws
      into (`RenderLayers`), what a player no view draws does (it never advances, so it
      freezes), and a feedback store that today hears only the last view to run.
- [ ] **An offscreen target on the C face** — a scene renders only to the window or
      layer it was created on, so a C/C++ engine cannot composite a character with its
      own drawing, and the route the docs give (render into a texture, bring it in
      through the engine's interop) has no call behind it. It needs a scene created on a
      texture and a way for that texture to leave — **undecided** whether that is a
      native shared handle per backend (a DXGI shared handle, an `IOSurface`, Vulkan
      external memory), a CPU readback as the portable floor, or both.
- [ ] **Self-contained HTML preview** — a Preview button per pack in each host opens
      the shell its plugin ships in Chrome on the `.ssab`'s path, so every host shows the
      same preview without an in-editor renderer; every Release carries the shell for a
      plugin to fetch by its tag. Missing: the buttons themselves, which live in the
      Player repositories (design and measurements in [`PREVIEW.md`](./PREVIEW.md)).

## Resource management

- [ ] **Pack and texture eviction** — a player only accumulates packs, the renderer
      cannot list the textures it holds, and only the Rust face can forget one
      (`SSRenderer::forget_texture`), so a host that swaps characters grows `.ssab`
      bytes and VRAM with no way to reclaim them. Missing: a per-player pack release on
      every face, which must retire the pack rather than drop it, since a `Context`
      points at its resource by raw pointer.

## Distribution & verification

- [ ] **Confirm the three borrowed blend states** — `Mulalpha` and `Mul2` draw with `Mul`'s
      state and `Screen2` with `Screen`'s (`blend_state` in `renderer.rs`), as SpriteStudio Player
      for Web and for Flutter do; none of the three has been checked against the editor.
      Blocked on: a reference render of each.

- [ ] **crates.io** — `ssruntime` and `ssab` are path dependencies into the
      submodule; cargo will not publish a crate that depends on paths only.
      Blocked on: the SDK crates `ssab` and `ssruntime` being published first.

## Needs SDK

- [ ] **Crowd members at their own phase** — `SSPlayer::set_crowd` draws one
      playhead at N placements, and that face stays here: it is GPU instancing of
      one evaluation's geometry and needs nothing of the runtime. N members at N
      points of one animation is the SDK's
      [Animation Instancing](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/ROADMAP.md#phase-3-feature-expansion-post-release),
      one evaluation context driving many instances. Waits for: that item.

## Deliberate non-goals

- **Throughput optimisation** — draw-call merging, bindless textures, texture
  bind-group dedup, render bundles, threading the player walk, indirect draws for
  Shape. Measured against a 256-player frame (`tests/scale.rs`), natively and in the
  browser, and not worth their complexity: merging every mergeable draw would remove
  28% of the calls, a fraction of a millisecond. Bindless is additionally unavailable
  on WebGPU.
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
