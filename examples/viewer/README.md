# viewer — native Rust host (winit + wgpu)

The development host for the **Rust face**. It owns a winit window and a full wgpu
stack, then drives borrow-model players with it — which is exactly what your own
application does. For the shortest answer to "how do I use this crate?", read
[`../hello`](../hello) first: one character, one file.

This is the development viewer: several characters in one pass, each on its
own pack and animation, with per-frame cost printed live.

## Build and run

```bash
git submodule update --init          # once
./scripts/deploy-examples.sh         # once — generates assets/
cargo run -p ssplayer-wgpu-viewer
```

It starts on `assets/overall` with two characters. Dropping a `.ssab` file — or a
folder of them — onto the window plays your own assets. The sibling packs a pack's instance
parts play from are found beside it, as `load` finds them.

## What the host owns, and what it lends

The shape below is the whole integration. The host creates one `SSRenderer` per
**device** and one `SSScene` per **render target**, then lends both to the
players every frame; the players themselves hold no GPU resource.

```rust
// Once.
let mut renderer = SSRenderer::new(&device);
let mut scene = SSScene::new(&device, &SceneConfig {
    color_format: surface_format,
    stencil_format: Some(STENCIL_FORMAT),   // None if you never use mask parts
    mask: MaskBackend::Stencil,             // or Coverage, which needs no stencil
});
scene.set_viewport(&queue, width, height, y_down);

// Per frame, before recording.
let mut players: Vec<&mut SSPlayer> = cast.iter_mut().map(|m| &mut m.player).collect();
scene.update(&mut renderer, &device, &queue, delta_secs, &mut players);
// On the encoder you open your pass with: coverage masks and group-alpha layers.
scene.encode_offscreen(&mut renderer, &device, &mut encoder);

// Inside a render pass you opened — the scene never opens one itself.
scene.draw(&renderer, &mut rpass);
```

Three details worth copying:

- **The stencil attachment is yours.** `SceneConfig::stencil_format` only tells
  the scene which pipelines to bake; the host still attaches a matching
  depth/stencil buffer to every pass, and recreates it on resize.
- **Resize touches two things**: reconfigure the surface, then
  `scene.set_viewport(...)` so the projection follows.
- **GPU timing is opt-in at device creation.** Request
  `Features::TIMESTAMP_QUERY` when the adapter has it, pass
  `scene.timestamp_writes()` into the pass descriptor, and call
  `scene.resolve_timestamps(&mut encoder)` afterwards. Skip it and everything
  still works — `stats()` just reports no GPU time.

## Controls

| Key | Action |
|---|---|
| Space | pause / resume |
| Left / Right | previous / next `.ssab` pack |
| Up / Down | previous / next animation |
| R | restart the current animation |
| Tab | move focus to the next character — pack, animation and the keys below apply to it |
| `+` / `-` | add / remove a character (1–8), all in one scene and one pass |
| L | layout: Row (side by side) or Stack (overlapping in the centre) |
| O | reverse draw order |
| V | toggle `set_visible` on the focused character — hidden, it keeps playing |
| X | toggle `set_culled` on the focused character — culled, its clock stops |
| `[` / `]` | lower / raise `set_alpha` on the focused character |
| F | flip the Y axis (Y-up ↔ Y-down projection) |
| K | toggle the mask backend: stencil / coverage |
| Escape | quit |
| drop | play a dropped `.ssab`, or add every `.ssab` in a dropped folder |

The track keys — `C`, `S`, `A`, `Y`, `0`–`4`, `,` / `.`, `T`, `E` — are listed in
[Motion blending](../../docs/en/guide/motion-blending.md#see-it).

**Stack mode is the one that shows the interesting behaviour.** Characters only
interact where they overlap, so a row of them looks the same as a single one. Put
a `Mask` pack and a `Basic` pack on top of each other to see that the first one's
mask does not clip the second, and use `O` to check that draw order decides who
is in front.

## Reading the status line

The status bar refreshes twice a second from `SSScene::stats()`: how many players
are visible, how many primitives and draw calls the frame issued, bytes uploaded,
and GPU pass time when the adapter supports timestamps. Change the character
count with `+` / `-` and watch what does — and does not — scale: a second copy of
the same pack adds primitives and draws but uploads nothing, because the atlas
and the pipelines belong to the device-scoped renderer.

## Related

- [The Rust API reference](../../docs/en/reference/rust.md) — the crate's API
- [`../web`](../web) — the same player in a browser
- [`../viewer-metal-cpp`](../viewer-metal-cpp) / [`../viewer-dx12-cpp`](../viewer-dx12-cpp) — the C API in a C++ host
