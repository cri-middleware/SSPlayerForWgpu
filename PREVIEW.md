# Self-contained `.ssab` preview — working notes

One preview of a chosen animation, reachable from every host, shipped as a **single
self-contained HTML file opened in the browser**. Not an in-editor renderer.

This file is the cold start for that work. Planned and rejected work at large lives in
[`ROADMAP.md`](./ROADMAP.md); this is the one feature that needed more room than a
roadmap entry.

## Shape

What ships is a **shell**: one HTML file with the wasm and the glue already inlined and
a hole where a pack goes. The hole is the content of a
`<script id="ssab" type="application/base64">` element, so the shell is a valid page as
it stands and opens to "no pack loaded" rather than to an error.

It has two entrances and one interior:

- **From a host** — a "Preview" button fills the hole, writes the file to a temp
  directory, and opens it in the browser.
- **By hand** — the shell is opened as it ships and a `.ssab` is dropped on it. Nothing
  else has to be fetched or installed, which is what serves the hosts with no editor to
  put a button in. `examples/viewer` already plays a dropped pack, so the gesture is not
  a new one to teach.

What follows from that shape:

- **One button per pack, not per animation.** The page holds the whole pack and
  `animationNames` returns all of it, so the animation picker belongs in the page. A
  host that has one selected passes it as the initial value; otherwise it passes
  nothing. No host ever needs to know the animation list.
- It is a **one-shot dump, not a live view**: no file watching, no following a
  reconvert, no IPC back to the editor. To see new bytes, press the button again.
  Converting is a build step, not a preview's job.
- Because the artifact is one ordinary file, it works offline and **attaches to a bug
  report**: "renders here, not in my engine" becomes a repro the reporter can send.

A CLI that fills the hole from a command line is a convenience on top, worth adding once
the npm package ships. Nothing else waits on it.

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

## Distribution

The shell is a **release artifact of this repository**, produced by `scripts/build.sh`
alongside the npm package, and it carries the SDK version it was built from.

It is not committed into the Players. It is generated content of about 1.1 MB, and a
copy in each Player is a copy that drifts; the native binaries are already built rather
than committed, and the shell travels the same way — each plugin's packaging script
fetches a pinned release. `MAINTAINING_PLAYERS.md` §4.1 is the constraint that decides
this: nothing behind a Player's first door may need a clone, so the shell has to arrive
with the plugin rather than from a sibling checkout.

**The version stamp is load-bearing.** The `.ssab` schema is still moving at version 1,
so a stale shell does not fail — it quietly draws something else. The page shows the
version it was built from, and a host compares it against the version it converted with
and says so when they differ. Without that, every "the preview disagrees with my engine"
report has to be re-diagnosed from scratch.

## Page composition

Inline, in this order, with no external `src` and no `import` anywhere:

1. the `no-modules` glue verbatim,
2. the wasm as base64 → `wasm_bindgen({ module_or_path: bytes })`,
3. the `.ssab` as base64 → `loadSsab(bytes, undefined, undefined)`.

(3) arrives either already in the `<script id="ssab">` hole or from a dropped file. A
dropped `File` is read through `FileReader` / `File.arrayBuffer`, which is user-initiated
and so is not the `fetch` path that `file://` blocks — confirm that on the first drop
rather than taking it on trust, the way the rest of this list was confirmed.

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

## Blocked on fit/align

The pack's canvas and the `<canvas>` element are different sizes and nothing maps between
them yet, so the drawing is cropped: Ringo is 800×600 rendered into a 512×512 element.
**The preview cannot ship without fit/align.**

It is a port rather than a design — the vocabulary is settled and the reference
implementation lives in a repository this one cannot see, so the contract is written out
in [`FIT_ALIGN.md`](./FIT_ALIGN.md).

What the preview asks of it: **supply the authored canvas as the box** (`canvas_size`
plus the pivot) rather than a measured one. It costs no measurement, it does not resize
when the picker changes animation, and it shows the character standing in the frame it
was composed in — which is what makes the preview comparable to an engine's canvas.
`contain` and `center` are the defaults and are what the preview wants.

## Work order

1. **`fit`/`align` in `libs/ssplayer-wgpu`** — [`FIT_ALIGN.md`](./FIT_ALIGN.md). Everything
   else here waits on it, and it is worth more than the preview: it is the portable half
   of *UI integration* in the roadmap, so every face and the C ABI inherit it.
2. **The shell, built by `scripts/build.sh`** — a `--target no-modules` wasm inlined into
   one page, the `<script id="ssab">` hole, the drop receiver, the animation picker, the
   version stamp. Finishing this is what unblocks every host with no editor, because they
   need nothing else.
3. **The generator** — fills the hole from a pack and an optional animation name. The
   thing a host calls; also the CLI, once there is somewhere to publish it from.

Confirm on the first drop that a dropped `File` reads through `FileReader` at `file://`,
which is the one item in this document taken on reasoning rather than measurement.

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
