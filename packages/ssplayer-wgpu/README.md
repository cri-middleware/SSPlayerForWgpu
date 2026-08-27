# @spritestudio/ssplayer-wgpu

A **WebGPU** renderer for [SpriteStudio](https://www.webtech.co.jp/spritestudio/)
animations (`.ssab`), compiled to WebAssembly from the Rust/[wgpu](https://wgpu.rs/) player.

One `GPUDevice` per page, one `SSScene` per canvas, and **any number of characters drawn into one
canvas in a single render pass** — putting ten characters on screen costs one device, one set of
pipelines and one atlas upload.

日本語版は [README.ja.md](./README.ja.md)。

## Requirements

A browser with WebGPU: Chrome / Edge 113+, Safari 26+, or Firefox 141+ on a supported platform.
`sharedContext()` rejects with a readable error where it is missing, so a fallback (a static image,
the [WebGL player](https://www.npmjs.com/package/@spritestudio/ssplayer-webgl)) is a `catch` away.

## Install

```bash
npm i @spritestudio/ssplayer-wgpu@beta
```

The package is ESM-only and has no runtime dependencies. It ships the wasm binary; a bundler
(Vite / webpack 5 / Rollup) picks it up as an asset with no configuration — see
[Loading the wasm](#loading-the-wasm) for the cases that need a URL.

## Quick start — the custom element

One element = one canvas = one character. The shortest path for a single-character use such as an ad
creative:

```js
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';

defineSsPlayerWebGPUElement(); // registers <ss-player-webgpu>
```

```html
<ss-player-webgpu src="/assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

| Attribute | Meaning |
|---|---|
| `src` | URL of the `.ssab` (embedded or reference export). |
| `pack` | Pack-name override; defaults to the name inside the `.ssab`. |
| `anim` | Animation to play; defaults to the first one. |
| `autoplay` | Start playing once the pack is loaded. A boolean attribute, so **absent means no** — without it the element holds its first frame, poster-like. |
| `pause-when-hidden` | Stop the rAF loop while scrolled off-screen or backgrounded. On by default — opt out with `"false"`. |

`element.loadSrc(src, pack?, anim?)` switches packs at runtime and resolves to the animation names.

Registration is **opt-in** rather than a side effect of the import, so a bundle that only wants the
API below never carries the element.

## Quick start — many characters in one canvas

The element is a wrapper over this. `SsContextWeb` is the page's device; a surface is a canvas; a
player is a character:

```js
import { sharedContext } from '@spritestudio/ssplayer-wgpu';

const ctx = await sharedContext();            // one per page — the device + pipelines
const surface = ctx.createSurface(canvas);    // one per canvas — projection + frame arena

const a = surface.createPlayer();
await a.load('/assets/Basic.ssab');           // pack + its atlases + sibling .ssab, one call
a.setAnimation(a.animationNames()[0]);        // selecting and starting are separate calls
a.play();

const b = surface.createPlayer();
await b.load('/assets/Effect.ssab');          // an atlas already uploaded is not uploaded twice
b.setAnimation(b.animationNames()[0]);
b.setTransform(120, 0, 0.5, 0.5, 0);
b.setOrder(1);                                // ascending draw order within the canvas
b.play();

let last = performance.now();
const loop = (now) => {
  const dt = Math.min((now - last) / 1000, 0.05); // seconds, clamped over long gaps
  last = now;
  surface.frame(dt);                          // updates every player and draws them in one pass
  requestAnimationFrame(loop);
};
requestAnimationFrame(loop);
```

`load()` is the only asynchronous call: it fetches the pack, its atlases from **both** texture tables
(cell maps and TextureChange targets) in either export mode, and transitively the sibling `.ssab`
files that its instance parts play from. Anything that did not arrive comes back from
`missingAssets()` rather than throwing. `setAnimation()` and the transport calls
(`play` / `pause` / `resume` / `stop` / `seek` / `setSpeedScale`) are synchronous.

There is no image decoder in the wasm, on purpose: the browser already ships one, so embedded
atlases go out as encoded bytes and come back through `createImageBitmap`.

## Loading the wasm

The bindings self-locate the binary with `new URL('ssplayer_wgpu_bg.wasm', import.meta.url)`, which
every ESM bundler resolves. Where `import.meta.url` does not survive the build (a classic `<script>`
or IIFE bundle), or the binary is served from elsewhere, point at it before the first
`sharedContext()`:

```js
import { setWasmUrl, sharedContext } from '@spritestudio/ssplayer-wgpu';

setWasmUrl('https://cdn.jsdelivr.net/npm/@spritestudio/ssplayer-wgpu@<version>/wasm/ssplayer_wgpu_bg.wasm');
await sharedContext();
```

Pin the **same version** as the package: the JS glue and the binary share a wasm-bindgen ABI.

## Device loss

The browser can take the `GPUDevice` at any time (a backgrounded tab, a GPU-process crash). Check
`ctx.isDeviceLost()` in your frame loop and call `restoreSharedDevice(ctx)`; every canvas and every
player on the page is rebuilt from its retained source and playback resumes where it stopped. The
custom element does this for you.

## API

| Object | Scope | What it owns |
|---|---|---|
| `SsContextWeb` | the page | adapter, device, pipelines, the atlas cache |
| `SsSurfaceWeb` | one canvas | swapchain, projection, the frame vertex arena |
| `SsPlayerWeb` | one character | playback tree, transform, draw order |

Full type definitions ship with the package (`dist/index.d.ts`). `surface.stats()` reports
primitives, draw calls, uploaded bytes and mergeable draws for measurement.

Handles are explicit: call `player.destroy()` / `surface.destroy()` when you are done with one —
dropping the JS reference would leave a swapchain behind. Both are idempotent.

## Producing `.ssab`

`.ssab` is the SpriteStudio binary format, produced from a `.sspj` project by the converter that
ships with the SDK. Reference export keeps the atlases as external PNGs beside the `.ssab`; embedded
export carries them inside it. Both work here through the same `load()` call.

## Links

- [Repository](https://github.com/cri-middleware/SSPlayerForWgpu) — the Rust library, plus native
  C/C++ viewers for Metal, DX12 and Vulkan built on the same core.
- [Issues](https://github.com/cri-middleware/SSPlayerForWgpu/issues)
- [SpriteStudio](https://www.webtech.co.jp/spritestudio/)

## License

BSD-3-Clause. See [LICENSE.md](./LICENSE.md).
