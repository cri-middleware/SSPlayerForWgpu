# Self-contained `.ssab` preview — working notes

One preview of a chosen animation, reachable from every host, shipped as a **single
self-contained HTML file opened in the browser**. Not an in-editor renderer.

This file is the cold start for that work. Planned and rejected work at large lives in
[`ROADMAP.md`](./ROADMAP.md); this is the one feature that needed more room than a
roadmap entry.

## Shape

- A host adds a **per-animation** "Preview" button. Pressing it writes a self-contained
  HTML file to a temp directory and opens it in the browser.
- It is a **one-shot dump, not a live view**: no file watching, no following a
  reconvert, no IPC back to the editor. To see new bytes, press the button again.
  Converting is a build step, not a preview's job.
- The generator also runs as a CLI, so hosts with no editor (Flutter, React Native,
  Ren'Py, Web) and tools outside any engine (VS Code, SSConverterGUI, the docs site)
  produce the same artifact from the same code.
- Because the artifact is one ordinary file, it works offline and **attaches to a bug
  report**: "renders here, not in my engine" becomes a repro the reporter can send.

## Why the preview is not drawn in the inspector

"The inspector shows what the canvas shows" is a promise only the engine's own renderer
can keep. Driving a Godot or Unity inspector from wgpu would *guarantee* a mismatch with
that engine's canvas, and the mismatch would be in rasterisation — blending, masking,
colour — which is the hardest kind to explain. A separate window is a reference view and
makes no such promise.

If an in-engine preview is ever wanted, write it as a thin host over the engine's
**existing** renderer — Godot's `ss_internal_player.h` already names editor previews as
a host alongside `Node2D`, and Unity reaches the same place with `PreviewRenderUtility`
over the shipped graphic. That is a host, not a second renderer. It is not this work.

## Verified

Chrome stable, macOS 26.6.2, fresh profile, no flags, page at `file://`.

- `isSecureContext` is true, `navigator.gpu` is present, and `requestAdapter`,
  `requestDevice`, `canvas.getContext('webgpu')` + `configure`, and a real submitted
  draw all succeed.
- **`var<storage, read>` visible to the vertex stage works.** This is the construction
  `renderer.rs` depends on — the vertex stages read the arena, the primitive table and
  the mesh stream instead of a vertex buffer — and the reason there is no WebGL2
  fallback to retreat to.
- Blocked at `file://`: `fetch()` and an external `<script type="module" src=…>`.
  An **inline** `<script type="module">` runs.

So: inline everything and `file://` is enough. **No local HTTP server is needed** — and
one would have to be written twice, in C++ for Godot and C# for Unity, on top of port
allocation, lifecycle, firewall prompts and a token to keep the local filesystem from
being readable by anything that can reach the port.

Unverified: Chrome and Edge on Windows and Linux, Safari 26, Firefox 141.

## Build

```sh
wasm-pack build libs/ssplayer-wgpu-ffi \
  --target no-modules --release \
  --out-name ssplayer_wgpu --out-dir <out-dir> \
  -- --no-default-features --features web
```

`scripts/build.sh platform=web` builds the same crate with `--target web`, which emits
an ES module and therefore cannot load from `file://`. **The preview needs its own
`no-modules` build**; it does not reuse the npm package's wasm.

Release sizes, after `wasm-opt`: wasm 839,836 B, glue 159,529 B. Those are the fixed
cost of every generated page. Whole-file sizes: 1,332,615 B for
`examples/web/assets_emb/Basic.ssab`, 1,648,832 B for
`examples/web/assets_emb/Ringo/Ringo.ssab`.

## Page composition

Inline, in this order, with no external `src` and no `import` anywhere:

1. the `no-modules` glue verbatim,
2. the wasm as base64 → `wasm_bindgen({ module_or_path: bytes })`,
3. the `.ssab` as base64 → `loadSsab(bytes, undefined, undefined)`.

Feed it an **embedded-mode** `.ssab`. Then `missingAssets` comes back empty and the page
supplies no textures at all: the `web` face decodes the atlases itself through
`createImageBitmap`, which is why the feature deliberately does not pull `embedded`.
One input file, no atlas plumbing.

## Call sequence

```js
await wasm_bindgen({ module_or_path: wasmBytes });
const ctx     = await wasm_bindgen.SsContextWeb.create();   // once per page
const surface = ctx.createSurface(canvas);
const player  = surface.createPlayer();
await player.loadSsab(ssabBytes, undefined, undefined);
player.setAnimation(name);   // selection and transport are separate calls (SPEC §9.3)
player.play();
// then per rAF: surface.frame(dt)
```

## What the page can show with no new Rust

`animationNames`, `canvasSize`, `totalFrames`, `frameRate`, `frameNo`, `partCount`,
`partNames`, `cellmapNames`, `missingAssets`, and the surface's `stats`. UserData and
Signal arrive as events shaped `{ kind: 'userData' | 'signal', partIndex, … }`.

`missingAssets` is worth putting on screen rather than keeping for diagnostics: a pack
whose atlas did not arrive should say which one, not draw blank.

## Open — fit/align

The pack's canvas and the `<canvas>` element are different sizes and nothing maps
between them yet, so the drawing is cropped: Ringo is 800×600 rendered into a 512×512
element. **The preview cannot ship without fit/align**, which makes it the first thing to
settle rather than a detail to discover later.

## Out of scope

- Live reload, file watching, or reconverting from the preview.
- In-inspector thumbnails and project-window previews. A different need — identifying a
  file at a glance rather than inspecting one — and a different mechanism.
- An HTTP server inside the editor plugins.

## Fallback

If a target cannot use `file://` (site policy, or a default browser with no WebGPU), the
fallback is the **native** `examples/viewer`, not a server. It is winit + wgpu, so Metal,
DX12 and Vulkan are chosen automatically and it is one binary rather than one per API,
and it already plays an arbitrary `.ssab` dropped on the window. What it lacks is command
line arguments — `find_asset_dir()` hard-codes `assets/overall`. The cost it carries that
the HTML does not is shipping and signing an executable.

## Re-running the `file://` check

A page that does no `fetch` and imports nothing, opened as a file, logging to the console:

```sh
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --user-data-dir="$(mktemp -d)" --no-first-run --no-default-browser-check \
  --enable-logging=stderr --v=1 "file://$PWD/probe.html" 2>&1 | grep CONSOLE
```

Check, in order: `isSecureContext`, `navigator.gpu`, `requestAdapter`, `requestDevice`,
`configure`, a submitted draw, a render pipeline whose vertex stage binds
`var<storage, read>`, and `WebAssembly.instantiate` over inline bytes. Confirm the two
negatives as well — `fetch` of a sibling file and an external module script — so a later
"it works" is not resting on a page that quietly kept fetching.
