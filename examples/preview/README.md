# The self-contained preview shell

`shell.html` is the **source** of the one-file `.ssab` preview. Build the artifact with:

```sh
./scripts/build-preview.sh            # -> bin/ssplayer-preview.html (~1.7 MB)

# ... and to ship a pack inside one:
cargo run -p ssplayer-preview-gen -- pack=assets/Ringo/Ringo.ssab open=yes
```

Then open that file. Not serve it — **open it**, from the Finder, from a terminal, from a
host's "Preview" button. There is no server, no install and no network. Drop a `.ssab` on
it, or the folder that holds one, and it plays.

The design, what it is for and what was measured is [`PREVIEW.md`](../../PREVIEW.md) in
the repository root. This file is about working on it.

## What the build does, and what it must not

`build-preview.sh` substitutes five markers into `shell.html` — the wasm-bindgen
`no-modules` glue, the wasm as base64, two stamps, and `LICENSE.md` with
`THIRD_PARTY_NOTICES.md` as one JSON string — and writes one file. Everything else ships
exactly as written here.

The one rule the page lives by: **nothing may reach outside the file unasked.** At
`file://` an external script is blocked, and so is `fetch` unless Chrome was started with
`--allow-file-access-from-files`, while an inline `<script type="module">` runs. So there
is no `src`, no `import` and no `XMLHttpRequest` in this source, and one `fetch`: the URL
entrance's, which a host opens in exactly that Chrome. The build fails on a second one. A
slip would work perfectly over `http://` and fail only on the machine that opened the
artifact.

The build also fails when the page calls a method on `ctx`, `surface` or `player` that the
glue it inlines does not define. Those are the js face's names, and a rename in the Rust
otherwise surfaces only as a `TypeError` in a browser. CI's wasm job builds the shell on
every pull request, so that is where it fails.

The wasm is its own build (`wasm-pack --target no-modules`), not the npm package's
(`--target web`), because an ES module cannot load from `file://`.

## Opening it by path

A host's Preview button opens the shell with the pack in its query:

| Key | Value |
|---|---|
| `ssab` | the `.ssab`'s `file://` URL |
| `animation` | the animation to open on, when the editor has one selected |
| `converter` | `ss_converter_version()` of the converter that wrote the `.ssab` |

A host encodes each value as a query component (`Uri.EscapeDataString`,
`String.uri_encode`), since a path may hold a space, `#`, `&` or `+`. Only `ssab` is
required. In a Chrome
started with `--allow-file-access-from-files` (*The URL entrance* in `PREVIEW.md` has the
whole command line and why), the page reads the pack, its atlases and sibling packs, and
lists the other packs in its directory; without it, it says it cannot read the pack. To
try it as a host would:

```sh
./scripts/build-preview.sh
open -na "Google Chrome" --args --user-data-dir="$PWD/target/preview-profile" \
  --allow-file-access-from-files --no-first-run --no-default-browser-check \
  --app="file://$PWD/bin/ssplayer-preview.html?ssab=file://$PWD/assets/Ringo/Ringo.ssab"
```

## The two holes

The shell as built has both empty. A generator fills them to ship a pack inside the page:

```html
<script id="ssab" type="application/base64" data-name="Ringo.ssab" data-animation="wait1">…</script>
<script id="files" type="application/json">{"Ringo.png": "…", "Ringo_sub.ssab": "…"}</script>
```

The second hole is **everything the pack depends on**, keyed by the path the pack names
it by: its atlases and the sibling `.ssab` packs its instance parts play from.
`textureRefs()` and `instancePacks()` are what name them, so nobody computes a hash or
guesses a path; `examples/preview-gen` is the reference walk.

Dropping by hand works the same way: drop the `.ssab` **together with** its `.png` files
and any sibling packs, in one gesture, or the folder that holds them. Every `.ssab` in the
drop is offered in the pack picker, the shallowest first.

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
