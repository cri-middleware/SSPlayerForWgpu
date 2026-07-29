# ssplayer-wgpu — WebGPU demo (js face)

Renders a SpriteStudio 7 `.ssab` to an HTML `<canvas>` via **WebGPU**, using the
wasm-bindgen bindings from the player's `web` feature (SPEC §12 P1). This is
not a separate crate — the WebGPU player lives in `libs/ssplayer-wgpu` behind
`--features web`; this directory is just the static page + build glue.

## Prerequisites

- `rustup target add wasm32-unknown-unknown`
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/)
- A WebGPU-capable browser (Chrome / Edge 113+, or Safari Technology Preview)

## Build & run

```sh
./build.sh          # release wasm (or ./build.sh --dev for a fast, larger build)
python3 -m http.server   # serve THIS directory
# open http://localhost:8000
```

`build.sh` does two things:
1. `wasm-pack build` of `libs/ssplayer-wgpu` with `--features web` → `./pkg` (wasm + JS bindings).
2. Generates embedded-mode `.ssab` demo assets → `./assets`
   (`ssconverter-cli --embedded`). Embedded ssab bakes the atlas PNG in, so the
   page fetches a single self-contained file and needs no host texture supply.

Both `pkg/` and `assets/` are generated (git-ignored).

## Usage

```html
<script type="module" src="ss-player-webgpu.js"></script>
<ss-player-webgpu src="assets/Basic.ssab" style="width:360px;height:360px"></ss-player-webgpu>
```

The `<ss-player-webgpu>` element owns the wgpu stack (created from its canvas) and
drives `requestAnimationFrame`. Call `element.loadSrc(url)` to switch animations at
runtime.

| Attribute | Default | Meaning |
|---|---|---|
| `src` | — | Embedded `.ssab` URL |
| `pack` | file stem | Pack name (for external instance references) |
| `anim` | first | Animation to play |
| `pause-when-hidden` | on | Stop the rAF loop while the element is scrolled off-screen or the tab is backgrounded. Opt out with `pause-when-hidden="false"` |

## Notes

- **Borrow model, browser edition**: the element is the host — it owns
  instance/surface/device/queue and lends device+queue to `SSPlayer` each frame,
  exactly like the native `viewer`. The event loop is the browser's rAF.
- Masks/shaders work: the demo configures a depth/stencil buffer.
- **Device loss is recovered, not fatal** (SPEC §14): the browser can take the
  WebGPU device at any time (backgrounded tab, GPU process crash). `SsPlayerWeb`
  watches `GPUDevice.lost`, the element stops drawing and calls `restoreDevice()`,
  and `SSPlayer::restore_device` rebuilds every pipeline/buffer/texture against the
  replacement — playback resumes on the frame it was interrupted on, with no
  re-fetch of the `.ssab`.
- The dev wasm is ~3 MB; a `--release` build (wasm-opt) is much smaller.
