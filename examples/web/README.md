# ssplayer-wgpu — WebGPU demo (js face)

Renders a SpriteStudio `.ssab` to an HTML `<canvas>` via **WebGPU**, using the
wasm-bindgen bindings from the player's `web` feature. This is
not a separate crate — the WebGPU player lives in `libs/ssplayer-wgpu` behind
`--features web`; this directory is just the static page.

It drives the **published package as it ships**: `build.sh platform=web` stages
[`packages/ssplayer-wgpu`](../../packages/ssplayer-wgpu) (`@spritestudio/ssplayer-wgpu`)
into `./ssplayer-wgpu`, so the element and the shared-context helper the demo uses
are the ones an `npm i` gets, not a second copy that can drift.

## Prerequisites

- `rustup target add wasm32-unknown-unknown`
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/)
- `pnpm` (`corepack enable`) — the package's js layer is TypeScript, built by tsdown
- A WebGPU-capable browser (Chrome / Edge 113+, or Safari Technology Preview)

## Build & run

This directory has no build script of its own. Two repository scripts fill it,
split by what they produce — the code and the data:

```sh
../../scripts/deploy-examples.sh       # .ssab + atlases, both export modes
../../scripts/build.sh platform=web    # release wasm (build=debug for a fast, larger one)
python3 -m http.server                 # serve THIS directory
# open http://localhost:8000
```

1. `build.sh platform=web` runs `wasm-pack build` on `libs/ssplayer-wgpu-ffi` with
   `--features web` → `packages/ssplayer-wgpu/wasm` (wasm + JS bindings), builds the
   TypeScript layer with tsdown → `packages/ssplayer-wgpu/dist`, then copies the
   package to `./ssplayer-wgpu`. `deploy=no` stops at the package, for a release
   script that wants the artifact and not the demo. It needs `pnpm` as well as
   `wasm-pack`.
2. `deploy-examples.sh` deploys the demo data in **both export modes**:
   reference-mode `.ssab` plus its sibling PNGs → `./assets`, and embedded-mode
   `.ssab` (`ssconverter-cli --embedded`, atlas PNG baked in) → `./assets_emb`.
   The page has a selector for the two, so the fetch path is exercised as well as
   the embedded one. Run it on its own to refresh the data without rebuilding the
   wasm; the `_emb` suffix is the same one the repository's `assets/overall` vs
   `assets/overall_emb` uses.

`ssplayer-wgpu/`, `assets/` and `assets_emb/` are all generated (git-ignored).

## The three pages

Each is self-contained static HTML — no build step, no bundler, one inline module
script — and all three share the same staged package and the same data.

| Page | What it is for |
|---|---|
| [`index.html`](./index.html) | The two shapes of the API side by side: one `<ss-player-webgpu>` element, and one canvas driven directly with several players on it, printing `SceneStats`. Both export modes are selectable, which is what exercises the fetch path as well as the embedded one. |
| [`element.html`](./element.html) | The Web Component on its own: the declarative markup, a panel over `element.player` for transport / speed / loop count, a copy-pasteable snippet that tracks the panel, and `loadSrc` switching packs at runtime. |
| [`multi.html`](./multi.html) | Many elements on one page, in article flow — block, inline, overlapped with HTML above and below, and styled with `border-radius` / `filter`. The HUD counts elements, canvases and **distinct `SsContextWeb` objects**, so "one device however many elements" is measured rather than claimed; per-animation frame counters make `pause-when-hidden` visible. |

Both new pages fit the animation to the element's box themselves
(`canvasSize()` + `setTransform`), because the player draws one authored pixel to
one CSS pixel and puts the animation's **origin** — wherever the artist put it — at
the centre of the box. Ringo stands on his origin, so those elements ask for it at
the foot of the box instead; that decision is the host's and the demo shows it
being made rather than hiding it.

`build=debug` passes `--dev` to wasm-pack, which skips `wasm-opt` — the result is
several times larger, so never quote its size.

## Usage

### Single character — the `<ss-player-webgpu>` element

```html
<script type="module">
  import { defineSsPlayerWebGPUElement } from './ssplayer-wgpu/dist/element.js';
  defineSsPlayerWebGPUElement();   // opt-in: registering is not an import side effect
</script>
<ss-player-webgpu src="assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

The element makes its own canvas, takes a surface from the page's shared context
and adds one player to it, then drives `requestAnimationFrame`. Call
`element.loadSrc(url)` to switch packs at runtime. Put ten of these on a page and
they still share **one `GPUDevice`** — see below.

| Attribute | Default | Meaning |
|---|---|---|
| `src` | — | `.ssab` URL, either export mode; its atlases and instance packs come with it |
| `pack` | from the `.ssab` | Pack name override. Leave it alone — external instance references are resolved by the exporter's name |
| `anim` | first | Animation to play |
| `pause-when-hidden` | on | Stop the rAF loop while the element is scrolled off-screen or the tab is backgrounded. Opt out with `pause-when-hidden="false"` |

### Several characters — drive the context directly

The element is a convenience wrapper, not the shape of the API. Underneath are
three objects, one per resource scope:

| Object | How many | Owns |
|---|---|---|
| `SsContextWeb` | **one per page** | adapter, device, queue, `SSRenderer` (pipelines + atlas cache) |
| `SsSurfaceWeb` | one per canvas | `wgpu::Surface`, `SSScene` (projection, frame arena) |
| `SsPlayerWeb` | any number | one playback tree — **no GPU resource at all** |

```js
import { sharedContext } from './ssplayer-wgpu/dist/index.js';

const ctx     = await sharedContext();        // once for the page
const surface = ctx.createSurface(canvas);    // once per canvas
const player  = surface.createPlayer();       // as many as you like

await player.load('assets/Basic.ssab');       // pack + assets (see below)
player.setAnimation('idle');                  // selecting allocates …
player.play();                                // … starting does not
player.setTransform(x, y, scaleX, scaleY, rotation);
player.setOrder(1);                           // lower draws first

surface.frame(dt);                            // one call draws every player
player.destroy(); surface.destroy();          // explicit — they live in the context
```

### Loading — one call brings everything the pack needs

`load(src)` fetches a `.ssab` **and everything it depends on**: its atlases in
either export mode, and any sibling `.ssab` its instance parts play from,
transitively. `src` is a URL or a path, absolute or relative to the document.

Same shape as `SS6PlayerForGodot`'s `setSSABResource` (`_loadTextures` +
`_load_external_ssabs`) and `ssplayer-core`'s `load(ssabUrl)`. Assets belong to
initialisation, so `setAnimation` stays a pure, **synchronous** selection —
switching motions must not carry a promise.

| What | Where it is | What `load` does |
|---|---|---|
| atlas, embedded | inside the `.ssab` | decodes what the pack carries; fetches nothing |
| atlas, reference | sibling files | fetches each one relative to the `.ssab` |
| instance pack | `<dir>/<packName>.ssab` | fetches it by the name the exporter wrote, then its atlases, then whatever *it* references |

A URL is what makes this one call: it answers both "what are the bytes" and
"what do the siblings resolve against". `loadSsab(bytes, pack?, baseUrl?)` is the
same thing for bytes that are not URL-addressable — a drop, a bundler, an archive
— where `baseUrl` supplies the second answer and `pack` can override the pack's
name. Omit `pack`: the name the exporter wrote is the one external instance parts
are resolved against (`ref_anime_pack`), and a name of your own **silently breaks
those references**.

These are `async` only because a browser leaves no choice — `fetch` and
`createImageBitmap` are asynchronous where Godot's `ResourceLoader::load` is not.
That is the one place the asynchrony appears.

Anything the shared renderer or the player already holds is skipped, so N players
of one pack load its atlas once between them, and a cycle of packs referencing
each other terminates. Anything that cannot be fetched or decoded is reported on
the console and skipped: a missing file costs you that file, not the character.

**The wasm carries no image decoder** — one is already installed on every page,
and bundling a second cost ~29% of the transferred bytes to do the same job worse. Decoding is `createImageBitmap` (off the main thread,
premultiplied) and `copyExternalImageToTexture` moves the pixels straight into
the texture without passing through wasm memory. The *encoded* bytes are kept, so
`ctx.restoreDevice()` rebuilds every texture after a device loss with no re-fetch.

Three context-level paths remain for hosts that supply their own images:
`ctx.addTextureFromBytes(key, bytes)` (encoded — same route `loadSsab` takes),
`ctx.addTextureFromBitmap(key, bitmap)` for a host with its own asset pipeline
(device loss is then yours to repair), and `ctx.addTexture(key, rgba, w, h)` for
raw straight-alpha RGBA. `player.textureRefs()` / `player.embeddedTextures()`
answer what a pack needs, if you would rather load it yourself — each entry's
`key` is what to register under. It is an identity for the *image*, which a cell
map's name is not: every Adobe Animate export calls its atlas `spritemap1.png`,
so two projects on one page would otherwise collide on one key and the second
would draw the first one's atlas.

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
- **Device loss is recovered, not fatal**: the browser can take the
  WebGPU device at any time (backgrounded tab, GPU process crash). `SsContextWeb`
  watches `GPUDevice.lost`; drawing stops and `ctx.restoreDevice()` rebuilds every
  pipeline/buffer/atlas on the page against the replacement — playback resumes on
  the frame it was interrupted on, with no re-fetch of any `.ssab` and none of any
  atlas. The loss is the device's, so recovery is **one call for the whole page**;
  the package's `restoreSharedDevice()` collapses the concurrent calls from N
  canvases into one.
- **`surface.stats()`** returns the frame counters (players / prims / drawCalls /
  mergeableDraws / redundantTextureBinds / uploadedBytes, plus `gpuPassTimeNs`
  when the browser grants `timestamp-query`). The throughput trade-offs in
  [`ROADMAP.md`](../../ROADMAP.md) were measured on native only — one WebGPU
  command is one JS call, so this is how to take the same numbers in a browser.
- Measure size from a `build=release` build only, and with `wasm-opt` applied.
  The web face deliberately leaves out the bundled `image` crate and decodes
  through `createImageBitmap` instead, which is a large share of the payload.
