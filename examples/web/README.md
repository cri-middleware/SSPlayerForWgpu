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

### Single character — the `<ss-player-webgpu>` element

```html
<script type="module" src="ss-player-webgpu.js"></script>
<ss-player-webgpu src="assets/Basic.ssab" style="width:360px;height:360px"></ss-player-webgpu>
```

The element makes its own canvas, takes a surface from the page's shared context
and adds one player to it, then drives `requestAnimationFrame`. Call
`element.loadSrc(url)` to switch packs at runtime. Put ten of these on a page and
they still share **one `GPUDevice`** — see below.

| Attribute | Default | Meaning |
|---|---|---|
| `src` | — | Embedded `.ssab` URL |
| `pack` | file stem | Pack name (for external instance references) |
| `anim` | first | Animation to play |
| `pause-when-hidden` | on | Stop the rAF loop while the element is scrolled off-screen or the tab is backgrounded. Opt out with `pause-when-hidden="false"` |

### Several characters — drive the context directly

The element is a convenience wrapper, not the shape of the API. Underneath are
three objects, one per resource scope (SPEC §1.2, §10):

| Object | How many | Owns |
|---|---|---|
| `SsContextWeb` | **one per page** | adapter, device, queue, `SSRenderer` (pipelines + atlas cache) |
| `SsSurfaceWeb` | one per canvas | `wgpu::Surface`, `SSScene` (projection, frame arena) |
| `SsPlayerWeb` | any number | one playback tree — **no GPU resource at all** |

```js
import { sharedContext } from './ss-context.js';

const ctx     = await sharedContext();        // once for the page
const surface = ctx.createSurface(canvas);    // once per canvas
const player  = surface.createPlayer();       // as many as you like

player.loadSsab('Basic', bytes);
player.setAnimation('idle');                  // selecting allocates …
player.play();                                // … starting does not (SPEC §9.3)
player.setTransform(x, y, scaleX, scaleY, rotation);
player.setOrder(1);                           // lower draws first

surface.frame(dt);                            // one call draws every player
player.destroy(); surface.destroy();          // explicit — they live in the context
```

**Ask for the context once.** A second `SsContextWeb.create()` builds a second
`GPUDevice`, and nothing is shared across a device: the same character on two
canvases would compile its pipelines twice and upload its atlas twice. That was
the previous shape of this API and P7 removed it.

Selecting an animation and starting it are separate calls, matching the
redesigned `ssplayer-webgl`; the old single `play(name)` is not carried over.

## Notes

- **Borrow model, browser edition**: `SsContextWeb` is the host — it owns
  instance/adapter/device/queue and lends device+queue to the scene and its
  players each frame, exactly like the native `viewer`. The event loop is the
  browser's rAF.
- Masks/shaders work: the demo configures a depth/stencil buffer.
- **Device loss is recovered, not fatal** (SPEC §14): the browser can take the
  WebGPU device at any time (backgrounded tab, GPU process crash). `SsContextWeb`
  watches `GPUDevice.lost`; drawing stops and `ctx.restoreDevice()` rebuilds every
  pipeline/buffer/atlas on the page against the replacement — playback resumes on
  the frame it was interrupted on, with no re-fetch of any `.ssab`. The loss is
  the device's, so recovery is **one call for the whole page**; `ss-context.js`
  collapses the concurrent calls from N canvases into one.
- **`surface.stats()`** returns the frame counters (players / prims / drawCalls /
  mergeableDraws / redundantTextureBinds / uploadedBytes, plus `gpuPassTimeNs`
  when the browser grants `timestamp-query`). SPEC §14.2 was measured on native
  only; this is how to take the same numbers in a browser.
- The dev wasm is ~3 MB; a `--release` build (wasm-opt) is much smaller.
