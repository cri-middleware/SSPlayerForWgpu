# Self-contained `.ssab` preview — working notes

One preview of a chosen animation, reachable from every host, shipped as a **single
self-contained HTML file opened in the browser**. Not an in-editor renderer.

This file is the cold start for that work. Planned and rejected work at large lives in
[`ROADMAP.md`](./ROADMAP.md); this is the one feature that needed more room than a
roadmap entry.

## Shape

What ships is a **shell**: one HTML file with the wasm and the glue already inlined and
holes where a pack and its atlases go. The pack's hole is the content of a
`<script id="ssab" type="application/base64">` element, so the shell is a valid page as
it stands and opens to "no pack loaded" rather than to an error; a reference pack fills a
second hole with its atlases (see *Page composition*).

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

Measured again on the assembled shell, opened as a file with a fresh profile:
`isSecureContext` and `navigator.gpu` both true, the inlined wasm instantiates, the pack
in the hole plays (`frameNo` advancing, 1 prim / 1 draw), and the event strip fills.

**A dropped `File` reads at `file://`** — `File.arrayBuffer()` returns the whole file and
the drop handler loads it (`Sound.ssab`, 3 events found). That was the one item here
taken on reasoning rather than measurement; it no longer is.

Unverified: Chrome and Edge on Windows and Linux, Safari 26, Firefox 141.

## Build

```sh
./scripts/build-preview.sh            # -> bin/ssplayer-preview.html
```

It runs wasm-pack over `libs/ssplayer-wgpu-ffi` with `--target no-modules --release
--no-default-features --features web`, and substitutes the glue, the wasm and two stamps
into `examples/preview/shell.html`.

`scripts/build.sh platform=web` builds the same crate with `--target web`, which emits
an ES module and therefore cannot load from `file://`. **The preview needs its own
`no-modules` build**; it does not reuse the npm package's wasm.

Release sizes, after `wasm-opt`: wasm 839,836 B, glue 159,529 B. Those are the fixed
cost of every generated page. Whole-file sizes: 1,332,615 B for
`examples/web/assets_emb/Basic.ssab`, 1,648,832 B for
`examples/web/assets_emb/Ringo/Ringo.ssab`.

## Distribution

The shell is a **release artifact of this repository**, produced by
`scripts/build-preview.sh`, and it carries the SDK commit it was built from.

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
dropped `File` is read through `File.arrayBuffer`, which is user-initiated and so is not
the `fetch` path that `file://` blocks — measured, see *Verified*.

### Sibling packs, for the same reason

An instance part plays a sub-animation out of **another** `.ssab`, and the page cannot
fetch it: at `file://` there is nothing to fetch it from, and `loadSsab` given no `src`
has no base to resolve against. So sibling packs go in the second hole beside the
atlases, keyed by file name, and the page registers each with `addSsab` **before**
loading the main pack — the main pack's own walk then skips what is already there.

`SSPack::instance_packs()` is what names them, transitively; it is `textureRefs`'s
counterpart for playback data. Missing one is the quietest failure in this whole
document: the instance part simply draws nothing, with no warning and no gap in the
picture to point at.

Registering a sibling makes the player try to load *its* atlases through the web loader,
which fails at `file://`. Those console warnings are expected; the supply step below is
what feeds them.

### Both pack forms, because the editors produce the reference one

An **embedded-mode** pack is one input file: `missingAssets` is empty and the page
supplies no textures at all, the `web` face having decoded the atlases itself through
`createImageBitmap`.

That is not what the hosts we are integrating produce. Godot converts with
`embedded = false` (`ss_importer.cpp`) and so does Unity
(`SpriteStudioConversionRunner.cs`), so what an editor has on disk is a **reference**
pack plus sibling `.png` files, and the `.ssab` alone draws nothing. **The shell needs a
second hole**, holding the atlases as base64 keyed by file name.

The supply path is the documented one, and it is the page's work, not the host's:

```js
for (const [name, bytes] of siblingPacks) await player.addSsab(bytes, name, undefined);
await player.loadSsab(ssabBytes, undefined, undefined);
for (const pack of [undefined, ...siblingNames]) {
  for (const r of player.textureRefs(pack)) {       // {key, path, embedded}
    if (r.embedded) continue;
    await ctx.addTextureFromBytes(r.key, atlasBytesFor(r.path));   // on the CONTEXT
  }
}
```

`textureRefs` names the files, so a generator knows exactly which siblings to inline and
**a host never computes a hash**. Measured: the reference `Basic.ssab` supplied this way
draws identically to the embedded one — 58,784 non-transparent pixels, 19 prims, 1 draw
call, 9,204 bytes uploaded, the same figures on both.

`setCellmapTextureFromBytes` also works and takes a name instead of a key, but it is the
runtime *override* mechanism: it leaves `isCellmapOverridden` true, claiming the viewer
replaced an atlas when it only supplied one. `addTextureFromBytes` leaves it false, keeps
the encoded bytes so `restoreDevice` can rebuild on its own, and is what the doc comment
on that method describes as the reference pack's route. Prefer it.

**`missingAssets` does not clear when the textures arrive.** It is a record of what failed
to resolve at load, not a live list, and after a successful supply it still names both
atlases. Do not put it on screen unfiltered — on a reference pack it would report missing
textures over a picture that is drawing correctly.

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

`missingAssets` needs the qualification in *Page composition* before it reaches the
screen: it does not clear once the atlases are supplied, so a reference pack reports both
of them as missing while drawing perfectly. Filter it against what was actually supplied,
and then it is worth showing — a pack whose atlas really did not arrive should name it
rather than draw blank.

## The timeline, and why the player grew no event table

The page has a seek bar and, under it, a strip of markers — one per event, coloured by
kind, aligned to the same coordinate space as the thumb — plus a list of frame, kind,
part and payload. Clicking either seeks. The marker for a frame the playhead just
crossed flashes, so the static picture and the live pulse are visibly the same thing.

The seek bar needed no new Rust: `seek`, `frameNo`, `startFrame`, `endFrame`,
`setSpeedScale`, `setSubFrame` and the four transport verbs are all on the js face
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

Both halves are built — the shell (`examples/preview/shell.html` +
`scripts/build-preview.sh`) and the generator (`examples/preview-gen`, which a host in
another language ports rather than links). What is left is packaging:

1. **A release job**, so a plugin fetches a pinned shell rather than builds one. Until
   then a host has to build it, which means a Rust toolchain and wasm-pack.
2. **The buttons**, which live in the Player repositories rather than here: convert (or
   find) the `.ssab`, run the gather, write to a temp directory, open it.

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
