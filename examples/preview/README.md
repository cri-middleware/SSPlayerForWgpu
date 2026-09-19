# The self-contained preview shell

`shell.html` is the **source** of the one-file `.ssab` preview. Build the artifact with:

```sh
./scripts/build-preview.sh            # -> bin/ssplayer-preview.html (~1.3 MB)
```

Then open that file. Not serve it — **open it**, from the Finder, from a terminal, from a
host's "Preview" button. There is no server, no install and no network. Drop a `.ssab` on
it and it plays.

The design, what it is for and what was measured is [`PREVIEW.md`](../../PREVIEW.md) in
the repository root. This file is about working on it.

## What the build does, and what it must not

`build-preview.sh` substitutes four markers into `shell.html` — the wasm-bindgen
`no-modules` glue, the wasm as base64, and two stamps — and writes one file. Everything
else ships exactly as written here.

The one rule the page lives by: **nothing may reach outside the file.** At `file://` an
external script and `fetch` are both blocked, while an inline `<script type="module">`
runs. So there is no `src`, no `import`, no `fetch` and no `XMLHttpRequest` in this
source, and the build fails if one appears. A slip would work perfectly over `http://`
and fail only on the machine that opened the artifact.

The wasm is its own build (`wasm-pack --target no-modules`), not the npm package's
(`--target web`), because an ES module cannot load from `file://`.

## The two holes

A generator fills these to ship a pack inside the page; the shell as built has both empty
and opens to "drop a `.ssab` here".

```html
<script id="ssab" type="application/base64" data-name="Ringo.ssab">…</script>
<script id="atlases" type="application/json">{"Ringo.png": "…base64…"}</script>
```

`textureRefs()` names the atlas files a reference pack wants, so a generator inlines
exactly those and never computes a hash. Dropping by hand works the same way: drop the
`.ssab` **together with** its `.png` siblings, in one gesture, and they are matched by
file name.

## The event strip fills itself

There is no authored-event read-out on the player, on purpose: it would be an API with
one caller — this page — and no sibling Player has one. So the strip and the list are
built from `events()`, the ordinary playback pulse, as the animation plays. The runtime
reports **every** frame a tick stepped across rather than only the one it landed on, so
nothing is missed at any frame rate, and one pass over the section is a complete table.

What that costs, stated plainly: before the first pass over a frame, its marker is not
there yet. The preview plays on open, so the strip completes in one loop.

## Poking at it

The page puts `window.ssPreview = { ctx, surface, player }` on the console. This artifact
exists so someone can say "renders here, not in my engine", and the next question is
always a number — `ssPreview.player.canvasRect()`, `ssPreview.surface.stats()`.
