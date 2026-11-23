# Preparing assets

Every face plays the same file: a **`.ssab`**, converted ahead of time from the `.sspj` project your
artist works in. **This player converts nothing at runtime and bundles no converter.** The `.sspj` has to be
authored in SpriteStudio **7.6 or later**.

## Converting your own

| Tool | Use it for |
|---|---|
| [SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) | a desktop drag-and-drop conversion, no toolchain |
| `ssconverter-cli` — the zip for your OS from a [SpriteStudio-SDK Release](https://github.com/cri-middleware/SpriteStudio-SDK/releases)'s **Assets** | a build pipeline, CI, a watch loop |

With `ssconverter-cli` on your `PATH`, one command converts a project into a folder of `.ssab` files
and the images they name:

```bash
ssconverter-cli overall.sspj -o assets/overall
```

!!! tip "No `.sspj` of your own yet?"
    The [official SpriteStudio sample data](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/) works as-is
    (that page is only available in Japanese; the downloads themselves are language-neutral).
    [Ringo](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/#ringo) is a good character sample, and
    [Particle](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/#Perticle_sample) is a good one if you want
    effects. The downloaded project feeds either tool above as-is.

The portal covers picking one and putting it in a pipeline:
[Converting Assets](https://cri-middleware.github.io/SpriteStudio-Docs/overview/conversion/) and
[Standalone conversion tools](https://cri-middleware.github.io/SpriteStudio-Docs/overview/convert-with-a-standalone-tool/).
That material is the SDK's and is not repeated here.

In a checkout of this repository, the samples run on the SDK's own test animations, which
`./scripts/deploy-examples.sh` converts into `assets/`.

## The two export modes

A `.ssab` names its atlases one of two ways, and **both work through the same one-call load** on
every face — you do not choose differently in code.

| Mode | On disk | Good for |
|---|---|---|
| **Reference** | `Character.ssab` plus its `.png` / `.webp` siblings | shared atlases, a CDN, hot-swapping art without reconverting |
| **Embedded** | one `Character.ssab`, atlas bytes inside it | a single file to ship, no second fetch, no missing-texture class of bug |

Under reference mode the image paths are written **relative to the `.ssab`'s own directory** — so
keep the exported files together, and the loader finds them.

!!! note "Embedded costs nothing extra in the browser"
    The web face carries no image decoder: the browser already ships one, so embedded atlases go out
    as encoded bytes and come back through `createImageBitmap`. On native, the
    `embedded` feature (on by default) carries the decoder.

## Where the files have to be

| Face | `load` takes | Resolved against |
|---|---|---|
| Rust | a filesystem path | the process's working directory; everything the pack names, against the `.ssab`'s own directory |
| Bevy | an asset path, through `asset_server.load` into `SsPlayer::pack` | the app's asset root, like any Bevy asset; everything the pack names, against the `.ssab`'s own directory |
| C / C++ | a filesystem path | the process's working directory; everything the pack names, against the `.ssab`'s own directory |
| Web | a URL | the page, then the `.ssab`'s own URL directory |

An engine or a packed build where `File::open` cannot reach the bytes — a Godot `.pck`, an APK, an
asset server — plugs in its own loader instead. See [Loading and assets](../guide/loading.md).

## What one `load` brings in

Not just the pack. A single call resolves, transitively:

* the pack itself;
* its atlases from **both** texture tables — the cell maps, *and* the texture files named by parts
  the artist gave a TextureChange (`TCHG`). Missing the second table is a bug that fails quietly:
  the part falls back to a 1×1 white texture and draws as a white silhouette with no error;
* the sibling `.ssab` files its **instance parts** play from — a character that plays another
  character's animation inside itself.

So a pack that looks like one file is often three or four, and shipping only the one you named is
the most common cause of a character that loads but renders wrong. Anything that did not arrive is
listed by `missing_assets()` / `ss_player_missing_*` / `missingAssets()` — check it once in
development.

## Next

* [Loading and assets](../guide/loading.md) — the loader seam, supplying textures yourself, and
  what a missing asset means
