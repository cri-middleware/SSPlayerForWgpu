# Get started on the web

The player compiled to WebAssembly, drawing through **WebGPU**. One `GPUDevice` per page, one
canvas per `SsSurfaceWeb`, and any number of characters drawn into one canvas in a single render
pass — ten characters cost one device, one set of pipelines and one atlas upload.

!!! tip "You do not need to know what wgpu is"
    wgpu is what the library is written against; in the browser it resolves to the WebGPU your
    browser already ships. **The device is created for you** by `sharedContext()`, and no wgpu type
    appears in the API. What you write is `load`, `setAnimation`, `play`.

## Requirements

A browser with WebGPU: **Chrome / Edge 113+, Safari 26+, or Firefox 141+** on a supported platform.
`sharedContext()` rejects with a readable error where it is missing, so a fallback — a still image,
or the [WebGL player](https://www.npmjs.com/package/@spritestudio/ssplayer-webgl) — is a `catch`
away.

## Install

=== "Released"

    🚧 **Not on npm yet.** When it publishes:

    ```bash
    npm i @spritestudio/ssplayer-wgpu@beta
    ```

    ESM-only, no runtime dependencies. It ships the wasm binary, and a bundler (Vite, webpack 5,
    Rollup) picks it up as an asset with no configuration.

=== "From source"

    Build the package and install it from the checkout. Needs `rustup target add
    wasm32-unknown-unknown`, [`wasm-pack`](https://rustwasm.github.io/wasm-pack/) and `pnpm`
    (`corepack enable`) — plus access to the SDK submodule.

    ```bash
    git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
    cd SSPlayerForWgpu
    ./scripts/build.sh platform=web          # wasm-pack + tsdown -> packages/ssplayer-wgpu/{wasm,dist}
    cd packages/ssplayer-wgpu && pnpm pack    # -> spritestudio-ssplayer-wgpu-<version>.tgz
    ```

    Then in your app: `npm i /path/to/spritestudio-ssplayer-wgpu-<version>.tgz`. `prepack` refuses
    to build a tarball whose advertised `dist/` or `wasm/` is missing, so a packed tarball is
    always complete.

    To just *see* it running, the repository's own demo page is one command more:

    ```bash
    ./scripts/deploy-examples.sh              # animation data, once
    ./scripts/build.sh platform=web
    (cd examples/web && python3 -m http.server)
    ```

## One character: the custom element

The shortest path — an ad creative, a mascot in a corner. One element is one canvas is one
character.

```js
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';

defineSsPlayerWebGPUElement(); // registers <ss-player-webgpu>
```

```html
<ss-player-webgpu src="/assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

| Attribute | Meaning |
|---|---|
| `src` | URL of the `.ssab` — reference or embedded export, either works |
| `pack` | Pack-name override; defaults to the name inside the `.ssab` |
| `anim` | Animation to play; defaults to the first one |
| `autoplay` | Start once loaded. A boolean attribute, so **absent means no** — without it the element holds its first frame, poster-like |
| `pause-when-hidden` | Stop the rAF loop while scrolled off-screen or backgrounded. On by default; opt out with `"false"` |

`element.loadSrc(src, pack?, anim?)` switches packs at runtime and resolves to the animation names.
Registration is **opt-in** rather than a side effect of the import, so a bundle that only wants the
API below never carries the element.

The element is documented in full — layout, visibility gating, device loss, and how to reach the
player API from it — in [`<ss-player-webgpu>`](../reference/web-component.md).

## Many characters: the API the element wraps

```js
import { sharedContext } from '@spritestudio/ssplayer-wgpu';

const ctx = await sharedContext();            // one per page — device + pipelines + atlas cache
const surface = ctx.createSurface(canvas);    // one per canvas — projection + frame arena

const a = surface.createPlayer();
await a.load('/assets/Basic.ssab');           // pack + atlases + sibling packs, one call
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

**The `load` family is the only asynchronous part of the player API.** It fetches the pack, its
atlases from both texture tables, and transitively the sibling `.ssab` files its instance parts play
from. Anything that did not arrive comes back from `missingAssets()` rather than throwing.
`setAnimation()` and every transport call are synchronous.

One load at a time per player: a second one started before the first settles is rejected rather
than silently losing one of the two, so `await` it — or chain the calls, the way the element does.
Two *different* players load concurrently as usual.

Handles are explicit: call `player.destroy()` / `surface.destroy()` when you are done with one —
dropping the JS reference would leave a swapchain behind. Both are idempotent.

## Loading the wasm

The bindings self-locate the binary with `new URL('ssplayer_wgpu_bg.wasm', import.meta.url)`, which
every ESM bundler resolves. Where `import.meta.url` does not survive the build (a classic
`<script>` or IIFE bundle), or the binary is served from elsewhere, point at it before the first
`sharedContext()`:

```js
import { setWasmUrl, sharedContext } from '@spritestudio/ssplayer-wgpu';

setWasmUrl('https://cdn.example.com/ssplayer-wgpu/<version>/wasm/ssplayer_wgpu_bg.wasm');
await sharedContext();
```

Pin the **same version** as the package: the JS glue and the binary share a wasm-bindgen ABI.

## Device loss

The browser can take the `GPUDevice` at any time — a backgrounded tab, a GPU-process crash. Check
`ctx.isDeviceLost()` in your frame loop and call `restoreSharedDevice(ctx)`; every canvas and every
player on the page is rebuilt from its retained source and playback resumes where it stopped. The
custom element does this for you. See [Device loss](../guide/device-loss.md).

## Next

* [`<ss-player-webgpu>`](../reference/web-component.md) — the element in full
* [JavaScript API reference](../reference/js.md) — every class and method
* [Preparing assets](./assets.md) — where the `.ssab` and its atlases have to sit on your server
* [Playback and transport](../guide/playback.md) — `play` vs `resume`, loops, sections
