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
- [ ] **Coverage bitmaps past the device's array-layer limit** — the coverage backend
      allocates a layer per masking player, and one more per further 24 writers in its
      instance tree, without checking `max_texture_array_layers`; past that many masking
      players on screen (256 under the default limits) creating the array is a
      validation error, which panics under wgpu's default handler. Closing it means
      drawing the players past the limit unmasked, as group alpha already does past its
      own, or splitting the array.
- [ ] **Atlas colour on the web face** — `createImageBitmap` applies an image's gAMA
      chunk or ICC profile, which the native decoders ignore, so such an atlas draws
      slightly differently in the browser (the SDK's `Ringo.png` carries gAMA, `Doll.png`
      an ICC profile). Passing `colorSpaceConversion: 'none'` matches the native faces;
      it wants a side-by-side check on those two.

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
- [ ] **Web: a device-lost frame repeats the last tick** — `SsSurfaceWeb.frame` returns
      before the update while the device is lost, so `events()` and the edges still
      describe the last tick that ran, and `<ss-player-webgpu>` dispatches them once more.
      It should report an empty tick, as a culled player does.
- [ ] **Web: one blank frame per resize** — resizing a canvas clears it and the redraw
      waits for the next animation frame, so a continuous resize flickers. Drawing the
      current frame again straight after the resize, without advancing, closes it.
- [ ] **Bevy: feedback a late main world misses** — the render world replaces the batch
      it hands back each frame, so when the main world falls a frame behind, the events
      and the started / finished / looped edges of the unread batch are lost. Appending
      events and edges to an unread batch, and replacing only the state, closes it.
- [ ] **JS typings for `fitInto()` and `stats()`** — both return `object` in the
      generated `.d.ts`, and `events()`, `missingAssets()` and `textureRefs()` return
      `any[]`, so `placed.scaleX` and `stats.drawCalls` do not type-check. Typed returns
      on the wasm-bindgen side, with the interfaces in the package, close it.

## Resource management

- [ ] **Pack and texture eviction** — a player only accumulates packs, the renderer
      cannot list the textures it holds, and only the Rust face can forget one
      (`SSRenderer::forget_texture`), so a host that swaps characters grows `.ssab`
      bytes and VRAM with no way to reclaim them. Missing: a per-player pack release on
      every face, which must retire the pack rather than drop it, since a `Context`
      points at its resource by raw pointer.
- [ ] **Atlases past the device's texture limit** — an atlas a load reads, or a
      replacement handed to the Rust face's `set_cellmap_texture`, that is larger than
      `max_texture_dimension_2d` is refused with a line on stderr and draws white, while
      `missing_assets()` stays empty and the call returns `Ok`; only the C face's
      `ss_scene_add_texture*` refuses it with an error. Reporting it means carrying the
      refusal from the flush, the first point a device is lent, back to the player.

## Distribution & verification

- [ ] **Confirm the three borrowed blend states** — `Mulalpha` and `Mul2` draw with `Mul`'s
      state and `Screen2` with `Screen`'s (`blend_state` in `renderer.rs`), as SpriteStudio Player
      for Web and for Flutter do; none of the three has been checked against the editor.
      Blocked on: a reference render of each.

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
- **UI integration on the C, JavaScript and element faces.** The SDK porting guide's
  [UI integration page](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/50_features/80_ui_integration.md)
  hangs hit testing, UI states and reduced motion on a UI toolkit's button, and none of
  these faces has a toolkit to integrate with: wgpu is a GPU API, a C/C++ engine brings
  its own UI, and on a page the DOM is the toolkit. `<ss-player-webgpu>` follows the Web
  Player's `<ss-player>`, which leaves UI to the page for the same reason
  ([its non-goal](https://github.com/cri-middleware/SSPlayerForWeb/blob/HEAD/ROADMAP.md#deliberate-non-goals)).
  A host that uses a player as a button answers clicks with its own box and plays a
  state's frames with `set_animation_section`; what that leaves out is a click limited
  to the artwork, since `part_transform` reports where a part is but nothing reports its
  outline. Revisit on a concrete case that a box cannot serve.
- **A Bevy UI adapter.** `ssplayer-bevy` puts a player in the world; one inside Bevy's
  UI tree — sized from `ComputedNode`, answering picking through `Hovered` / `Pressed`,
  and taking the UI states, the selected state (from `Checked`) and reduced motion —
  would be a crate of its own. The player plays in the render world, and only playback
  state and events come back to the main world, a frame late (`feedback.rs`), so part
  outlines would need a read-back that only marked players pay for. The node would need
  drawing into, through a texture per node, since the adapter draws into the first view
  alone. And the player would need two Rust-only read-outs it lacks — a part outline and
  the labels of an `SSPack` — while the C and JavaScript faces stay without them. Bevy's
  UI API also moves with each release (`Interaction` is deprecated in 0.20). It waits on
  a concrete request.
- **The GL backend.** The renderer pulls its vertices from storage buffers bound
  to the vertex stage, which GL exposes only from 4.3 / GLES 3.1 and WebGL not at
  all. No GL adapter is selected or verified anywhere: the instance takes
  `Backends::PRIMARY` (Vulkan / Metal / DX12 / WebGPU), and the C API's backend
  selector offers DX12, Metal and Vulkan.
- **An egui or iced adapter.** Both are immediate-mode-shaped hosts whose
  integration point is a texture, so the route needs no crate: draw through the Rust
  API into a texture you own — the host opens the render pass, so its target can be
  any texture — and hand that texture to the toolkit. A first-class adapter crate
  waits on a concrete request.
- **A WebGL fallback.** `ssplayer-webgl` is maintained and is the answer for
  environments without WebGPU.
