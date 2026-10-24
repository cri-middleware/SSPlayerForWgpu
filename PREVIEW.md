# Self-contained `.ssab` preview — working notes

One preview of a chosen animation, reachable from every host, shipped as a **single
self-contained HTML file opened in the browser**. Not an in-editor renderer.

It is a catalogue of what a `.ssab` holds — its animations, their playback, their events,
the assets it could not find — played by this Player as a reference. Matching each
engine's Player is not its aim, and the page says so on screen ("Reference playback
(wgpu)").

This file is the cold start for that work. Planned and rejected work at large lives in
[`ROADMAP.md`](./ROADMAP.md); this is the one feature that needed more room than a
roadmap entry.

## Shape

What ships is a **shell**: one HTML file with the wasm and the glue already inlined and
holes where a pack and its files go. The shell as built has the holes empty, so it is a
valid page as it stands and opens to "drop a `.ssab` here" rather than to an error.

It has three entrances and one interior:

- **From a host** — a "Preview" button opens the shell its plugin ships in Chrome, with
  the `.ssab`'s path in the URL, and the page reads the pack and everything it depends on
  itself. No temp file, no base64, no server (see *The URL entrance*).
- **By hand** — the shell is opened as it ships and a `.ssab` is dropped on it with its
  atlases and sibling packs, or the folder that holds them. Nothing else has to be fetched
  or installed, which is what serves the hosts with no editor to put a button in.
  `examples/viewer` already plays a dropped pack, so the gesture is not a new one to
  teach.
- **Filled** — `examples/preview-gen` puts a pack and everything it depends on into the
  holes, and the result opens anywhere with nothing beside it. It is the form that
  attaches to a bug report: a page opened by URL reads the reporter's files, so its HTML
  alone reproduces nothing.

What follows from that shape:

- **One button per pack, not per animation.** The page holds the whole pack and
  `animationNames` returns all of it, so the animation picker belongs in the page. A
  host that has one selected passes it as the initial value; otherwise it passes
  nothing. No host ever needs to know the animation list, and none lists the other packs
  either: the page offers the ones in the same directory.
- It is **not a live view**: no file watching, no following the editor's selection, no
  IPC back to the editor. To see new bytes after a reconvert, press reload in the page,
  or the button again. Converting is a build step, not a preview's job.

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

## The URL entrance

A host's button does one thing: start Chrome on the shell's `file://` URL with three
query values — the `.ssab`'s `file://` URL, the animation the editor has selected if
any, and the converter's version (`ss_converter_version()`). The names are in
[the shell's README](./examples/preview/README.md). It passes no fit or alignment (the
page fits the authored canvas, *Fitting the canvas*) and no dependencies (the page reads
them, *Page composition*), so a host never reads the `.ssab` and never computes a hash.

```text
--user-data-dir=<a profile of its own> --allow-file-access-from-files
--no-first-run --no-default-browser-check --app=<the shell's URL with the query>
```

- **Chrome by name, not the default browser.** A flag reaches only a Chrome it starts,
  never one already running, so the host uses a profile of its own; a second press opens
  a window in that same instance. macOS: `open -na "Google Chrome" --args …`.
- **Only the shell, in an app window.** In that profile any `file://` page can read
  local files, so the host opens nothing else there, and `--app=` leaves no URL bar to
  open anything else with. The shell loads nothing from outside itself.
- **Reload is a fresh page.** The renderer keeps an atlas it already holds under its
  URL, so loading a reconverted pack again in place would draw it with the old atlas.
  The page's reload button and its pack picker open the page again on new query values
  instead, which also keeps the URL the whole of the page's state.

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
- A dropped `File` reads (`File.arrayBuffer`). A dropped folder reads through
  `getAsFileSystemHandle()`, subfolders included, while `webkitGetAsEntry()` hands out
  entries whose `file()` and `readEntries()` throw `EncodingError`; over http both work.
  Drops injected through the DevTools Protocol (`Input.dispatchDragEvent`), not dragged
  from the OS.

So: inline everything and `file://` is enough. **No local HTTP server is needed** — and
one would have to be written twice, in C++ for Godot and C# for Unity, on top of port
allocation, lifecycle, firewall prompts and a token to keep the local filesystem from
being readable by anything that can reach the port.

**With `--allow-file-access-from-files`** (Chrome 156, a profile of its own, headless and
windowed alike): a `file://` page reads local files with `fetch`, subfolders included,
with status 200 and every byte; a file that is not there rejects with `Failed to fetch`;
a directory answers Chrome's listing page — one `addRow("name","url",isDir,…)` per
entry — with status 0. Windowed through `open -na "Google Chrome" --args …` shows no
warning bar.

Measured on the assembled shell:

- `Ringo.ssab` by URL plays with its atlas, opens on the animation the query names, and
  lists `Ringo_tyu.ssab` beside it. Without the flag, the same URL says it cannot read
  the pack and offers the drop.
- A copy of `Ringo.ssab` whose atlas path was rewritten to `x/Rin.png`, with `Rin.png`
  beside it and no `x/`, draws 26 prims with nothing missing; a fresh page reading by
  path alone reports `x/Rin.png` missing.
- The reference `Basic.ssab` and the embedded one, both by URL, draw byte-identical
  screenshots on the same paused frame (19 prims, 1 draw call).
- A filled `Instance.ssab` plays with its sibling pack and both atlases, nothing missing,
  and so does `Instance.ssab` picked out of a dropped `assets/overall` folder.

Unverified: Chrome and Edge on Windows and Linux, Safari 26, Firefox 141.

## Build

```sh
./scripts/build-preview.sh            # -> bin/ssplayer-preview.html
```

It runs wasm-pack over `libs/ssplayer-wgpu-ffi` with `--target no-modules --release
--no-default-features --features web`, naming the SDK release in `SSPLAYER_SDK_VERSION`
as `scripts/build.sh` does, and substitutes the glue, the wasm, two stamps and the
license notices into `examples/preview/shell.html`.

`scripts/build.sh platform=web` builds the same crate with `--target web`, which emits
an ES module and therefore cannot load from `file://`. **The preview needs its own
`no-modules` build**; it does not reuse the npm package's wasm.

Release sizes, after `wasm-opt`: wasm 929,925 B, glue 227,682 B and notices 228,237 B,
which make a built shell of 1,723,638 B — the fixed cost of every page. Filled pages:
1,773,325 B for `examples/web/assets_emb/Basic.ssab`, 2,060,069 B for
`examples/web/assets_emb/Ringo/Ringo.ssab`.

## Distribution

The shell is a **release artifact of this repository**: every `ssplayer-wgpu` Release
carries it as `ssplayer-preview.html`, listed in `SHA256SUMS`, built by
`scripts/build-preview.sh` in the release workflow's web job.

It is not committed into the Players. It is generated content of about 1.7 MB, and a
copy in each Player is a copy that drifts; the native binaries are already built rather
than committed, and the shell travels the same way — each plugin's packaging script
fetches a pinned release:

```text
https://github.com/cri-middleware/SSPlayerForWgpu/releases/download/ssplayer-wgpu-v<version>/ssplayer-preview.html
```

The tag is the pin, so the file name carries no version, like the zips'.
[`MAINTAINING_PLAYERS.md` §4.1](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/MAINTAINING_PLAYERS.md#41-nothing-behind-the-first-door-may-need-a-clone) is the constraint that decides this: nothing behind a
Player's first door may need a clone, so the shell has to arrive with the plugin rather
than from a sibling checkout.

**It carries its own license notices** — `LICENSE.md` and `THIRD_PARTY_NOTICES.md`,
behind the page's Licenses link — because it travels alone: as a Release asset and inside
each Player's package, with the wasm holding wgpu and the rest of the tree.

**The version check is load-bearing.** A shell from another SDK release reads the same
`.ssab` and draws it as that release does, so a stale shell does not fail — it quietly
draws something else. The stamp names the SDK as the converter spells it,
`<release>+<commit>`. A host puts the converter's version in the URL; the page compares
the release before the `+` with the one it links (`ctx.sdkVersion`), says so above the
picture when they differ, and shows the commit without comparing it. Players pin an SDK
release, so a plugin whose pins agree stays quiet. Without the check, every "the preview
disagrees with my engine" report has to be re-diagnosed from scratch.

## Page composition

Inline, with no external `src` and no `import` anywhere: the `no-modules` glue
verbatim, then the wasm as base64 → `wasm_bindgen({ module_or_path: bytes })`.

Every entrance then ends in one call, `player.load(url, read)`: the player's own walk
reads the `.ssab`, the sibling packs its instance parts play from and every atlas
through `read`, and `missingAssets()` names what `read` did not answer. What differs per
entrance is only `read`:

- **By URL** — `fetch`, the page's one call to it, which reads at `file://` only in a
  Chrome started with the flag. Anywhere else the page says it cannot read the pack.
- **A drop** — the dropped files, filed under a base URL of the drop's own, by their
  paths from the drop. The renderer keys atlases by URL, so two drops that both carry a
  `common.png` must not share a base.
- **Filled holes** — the same, keyed by the paths the pack names.

**Path, then file name.** A file that is not at the path the pack names is looked for by
its file name beside the main `.ssab`. That is the order Unity's importer resolves in
(`ResolveTexturePath`), and wider than Godot's, so what draws in the engine is not
missing here.

### Sibling packs

An instance part plays a sub-animation out of **another** `.ssab`, at
`<the pack's directory>/<name>.ssab`. `SSPack::instance_packs()` is what names them,
transitively; it is `textureRefs`'s counterpart for playback data, and the walk asks
`read` for each. Missing one is the quietest failure in this whole document: the
instance part simply draws nothing, with no warning and no gap in the picture to point
at — so `missingAssets()` names it on screen.

### Both pack forms, because the editors produce the reference one

An **embedded-mode** pack is one input file: its atlases arrive inside it, and the
`web` face decodes them itself through `createImageBitmap`.

That is not what the hosts we are integrating produce. Godot converts with
`embedded = false` (`ss_importer.cpp`) and so does Unity
(`SpriteStudioConversionRunner.cs`), so what an editor has on disk is a **reference**
pack plus sibling `.png` files, and the `.ssab` alone draws nothing. That is why every
entrance hands the page files and not only a pack.

## Call sequence

```js
await wasm_bindgen({ module_or_path: wasmBytes });
const ctx     = await wasm_bindgen.SsContextWeb.create();   // once per page
const surface = ctx.createSurface(canvas);
const player  = surface.createPlayer();
await player.load(ssabUrl, read);
player.setAnimation(name);   // selection and transport are separate calls
player.play();
// then per rAF: surface.frame(dt)
```

## What the page can show with no new Rust

`animationNames`, `canvasSize`, `totalFrames`, `frameRate`, `frameNo`, `partCount`,
`partNames`, `cellmapNames`, `missingAssets`, the surface's `stats`, and the context's
`sdkVersion`. UserData and Signal arrive as events shaped
`{ kind: 'userData' | 'signal', partIndex, … }`.

## The timeline, and why the player grew no event table

The page has a seek bar and, under it, a strip of markers — one per event, coloured by
kind, aligned to the same coordinate space as the thumb — plus a list of frame, kind,
part and payload. Clicking either seeks. The marker for a frame the playhead just
crossed flashes, so the static picture and the live pulse are visibly the same thing.

The seek bar needed no new Rust: `seek`, `frameNo`, `startFrame`, `endFrame`,
`setSpeedScale`, `setSubFrameEnabled` and the four transport verbs are all on the js face
already, and a paused player still draws, so scrubbing while stopped updates the picture.

**The strip fills itself from playback**, and the player deliberately did not gain a
"list every event this animation authors" read-out to serve it. Such an API is ten lines
— `decode_events` already takes the frame indices to decode and "all of them" is the
whole table — and it would cost nothing at runtime (measured: ~150–300 ns per authored
event, once, against the 240 µs `set_animation` that precedes it). It was still the wrong
trade: no sibling Player has one (ForWeb exposes events only as `onUserData` /
`onSignal` / `onAudio` subscriptions), so the only caller would be this page, and a
viewer-shaped API in the player is a cost every face pays forever.

What the pulse gives instead is complete for this purpose: the runtime reports **every**
frame a tick stepped across, not merely the one it landed on, so one pass over the
section is the whole table at any frame rate. The honest cost is that a marker does not
exist until the playhead has been past it once — and the preview plays on open, so that
is one loop.

The one thing the faces did have to gain is **`frameNo` and `partName` on the event
objects**. The Rust structs have carried both from the start, with a doc comment
explaining that the authored frame is not the frame the tick landed on, and both wrapper
faces were dropping them on the floor — so a js or C host could not tell which frame a
key belonged to when one tick crossed several. That is a plain gap, not a viewer feature.

## Fitting the canvas into the element

The pack's canvas and the `<canvas>` element are different sizes — Ringo is 800×600 in a
512×512 element — so the preview fits rather than crops:

```js
player.fitInto(canvas.width, canvas.height);
```

The defaults are what it wants: `contain`, `center`, and the **authored canvas** as the
box, which is what the player fits when no `frame` is supplied. That is the choice worth
keeping — it costs no measurement, it holds still while the animation plays, and it shows
the character standing in the frame it was composed in, which is what makes the preview
comparable to an engine's canvas. Each animation brings its own canvas, so one whose
canvas is wider (Ringo's `dead`, 1300 against 800) draws smaller in the same element.

Call it again when the element resizes and after the picker changes animation. See
[the guide](./docs/en/guide/placement.md); the contract it ports is the porting guide's
[Placement: Fit and Align](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/50_features/70_placement.md).

## Work order

The shell (`examples/preview/shell.html` + `scripts/build-preview.sh`) and the generator
(`examples/preview-gen`) are built, and every Release carries the shell. What is left
lives in the Player repositories: each plugin's packaging script fetching the shell from
a pinned Release, and **the buttons** — find Chrome, build the URL, start it as *The URL
entrance* says.

## Out of scope

- Live reload, file watching, following the editor's selection, or reconverting from
  the preview.
- In-inspector thumbnails and project-window previews. A different need — identifying a
  file at a glance rather than inspecting one — and a different mechanism.
- An HTTP server inside the editor plugins.
- **A route for machines without WebGPU** — WebGL, pre-rendered frames, the native
  viewer. A machine that runs the Unity or Godot editor has the D3D12, Metal or Vulkan
  that WebGPU sits on; what is left is virtual machines and remote desktops with no GPU
  passed through, and GPUs or drivers on Chrome's blocklist, too few to pay for keeping
  a second route. The page says WebGPU is missing and how to check (`chrome://gpu`).

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

For the URL entrance, add `--allow-file-access-from-files` and check the positives the
page relies on: `fetch` of a sibling file and of one in a subfolder (status 200, every
byte), and of the directory (status 0, Chrome's listing with its `addRow` lines).
