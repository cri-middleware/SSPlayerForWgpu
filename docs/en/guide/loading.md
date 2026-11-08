# Loading and assets

**Loading is one call on every face.** It resolves the pack, its atlases in either export mode, and
transitively the sibling `.ssab` files its instance parts play from. You never have to collect the
textures yourself.

=== "Rust"

    ```rust
    let mut player = SSPlayer::new();
    player.load("assets/overall/Instance.ssab")?;   // everything the pack depends on
    player.set_animation("anime_1")?;
    player.play();
    ```

=== "C"

    ```c
    ss_player_t *player = ss_scene_create_player(scene);
    ss_player_load(player, "assets/overall/Instance.ssab");
    ss_player_set_animation(player, "anime_1");
    ss_player_play(player);
    ```

=== "JavaScript"

    ```js
    const player = surface.createPlayer();
    await player.load('/assets/Instance.ssab');   // asynchronous: fetches the pack and its atlases
    player.setAnimation('anime_1');
    player.play();
    ```

=== "Bevy"

    ```rust
    commands.spawn(SsPlayer {
        pack: asset_server.load("Ringo/Ringo.ssab"),
        animation: "wait1".into(),
        ..default()
    });
    ```

## What a load actually fetches

Three things, not one — and the second is the one that bites:

1. **The pack**, from the path or URL you gave.
2. **Its atlases, from both texture tables.** A draw batch's texture key is a cell map's name hash
   for an ordinary part, but a *texture file name*'s hash for a part the artist gave a
   **TextureChange** (`TCHG`). A host that reads only the cell maps misses the second table, and it
   fails quietly: the part falls back to a 1×1 white texture, so it draws as a white silhouette
   with no error, no warning and no counter moving.
3. **Sibling packs**, for instance parts — a character that plays another character's animation
   inside itself. Resolved as `<the pack's own directory>/<name>.ssab`.

A pack may name the same file twice under two hashes. That is correct; deduplicate fetches **by
path, never by hash**.

## Paths are resolved against the `.ssab`, not against you

You give `load` one path or URL. Everything the pack names afterwards is resolved against **the
directory that file turned out to live in**, because that is what the exporter wrote those paths
relative to.

So: keep an exported pack and its atlases together, and pass the path to the `.ssab` — not to a
directory, and not to an atlas.

## What is on the GPU when `load` returns

On the Rust and C faces the borrow model lends no device until the frame starts, so a load reads
bytes and stops there. What it read is decoded and uploaded by the **first `update`**, on exactly
the timeline an embedded atlas has always taken. So immediately after a load, the renderer does not
know a texture's size yet; ask after the first frame.

On the JavaScript face the page's context holds the device from the start, so
`await player.load(...)` resolves with the atlases it fetched already decoded and uploaded.

## What did not arrive: `missing`, not an error

**A load fails only when the `.ssab` itself is unreadable.** One missing atlas must not cost you the
character, so everything else that did not arrive comes back as a list you can branch on.

=== "Rust"

    ```rust
    player.load("assets/overall/Instance.ssab")?;   // Err only if the .ssab itself failed
    for missing in player.missing_assets() {
        eprintln!("missing {:?}: {}", missing.kind, missing.name);
    }
    ```

=== "C"

    ```c
    if (ss_player_load(player, path) != SS_OK) {
        fprintf(stderr, "%s\n", ss_last_error());
    } else if (ss_player_missing_count(player) > 0) {
        ss_missing_asset_t m;
        for (uint32_t i = 0; i < ss_player_missing_count(player); i++) {
            if (ss_player_missing_at(player, i, &m) == SS_OK) {
                fprintf(stderr, "missing: %s\n", m.name);
            }
        }
    }
    ```

=== "JavaScript"

    ```js
    await player.load('/assets/Instance.ssab');   // rejects only on the .ssab itself
    for (const m of player.missingAssets()) {
      console.warn('missing', m.kind, m.name);
    }
    ```

Check it once while developing. A character that loads but renders as white silhouettes is almost
always a missing entry here.

## Bytes you already have

A host that fetched the file itself — a drag-and-drop, a bundler, an archive, an engine's asset
system — registers the bytes directly instead. A whole dropped folder, atlases and sibling packs
beside the `.ssab`, is better served by one `load` with a reader: see [In a browser](#in-a-browser).

=== "Rust"

    ```rust
    player.load_ssab("Character", &bytes)?;   // register and make it the selected pack
    player.add_ssab("Effect", &other)?;       // register only, for instance parts to find
    ```

=== "C"

    ```c
    ss_player_load_ssab(player, "Character", data, len);
    ss_player_add_ssab(player, "Effect", other, other_len);
    ```

=== "JavaScript"

    ```js
    await player.loadSsab(bytes, null, new URL('/assets/Character.ssab', location.href).href);
    await player.addSsab(other, null, baseUrl);
    ```

The name you register a sibling pack under is the name its instance parts refer to — the one the
exporter wrote into the `.ssab` — so `loadSsab` and `addSsab` take `null` to keep it. Register
siblings before `set_animation`, which is when the tree looks them up. Registering a sibling pack
*before* the load also works: the walk skips a pack the player already has.

On Rust and C these calls only register the pack: a reference-mode pack's atlases are not read, and
`missing_assets()` stays empty. Supply them as in [Supplying textures yourself](#supplying-textures-yourself),
or on Rust hand `load_assets` a loader rooted where they sit. `loadSsab` and `addSsab` read them,
against `baseUrl`.

!!! warning "Registering a sibling never changes what is already playing"
    A playback tree is bound to the pack it was built from. Registering a new pack under a name the
    live tree is playing retires the old one rather than dropping it — what is on screen keeps
    playing, and the replacement is what the **next** `set_animation` builds from.

    Loading is different: `load_ssab` (`ss_player_load_ssab`, `loadSsab`) selects the new pack and
    drops the playing tree, so call `set_animation` again.

## Supplying textures yourself

Where the host owns the images — an engine with an asset system, a texture atlas you built, a CDN
fetch you already made — hand them over **keyed by the image, not by the cell map that names it**.
The texture map is **device-scoped**, so a pack shown five times is uploaded once.

!!! warning "A cell map's hash names a slot, not an image"
    Some tools name the atlas `spritemap1.png` in every project they export, so two packs from two
    projects ask for the same cell map hash and mean two different atlases. Register by that hash
    and the second pack looks already-supplied: its own pixels are dropped and its parts draw the
    first pack's atlas. Ask the player for the key instead — it answers with an identity for the
    image. After a `load`, two packs that really do share a file still share one upload; packs
    registered from bytes are keyed per pack, so siblings naming one file upload it once each.

=== "Rust"

    ```rust
    // `texture.slot` names the slot in the pack; the key is what the renderer is keyed by.
    let key = player.texture_key(pack, texture.slot);

    // A view you own. The player never takes ownership of a GPU texture.
    renderer.set_texture(&device, key, &view, (width, height), AlphaMode::Straight);

    // Or let it upload for you, from straight-alpha RGBA8.
    renderer.upload_texture(&device, &queue, key, &rgba, (width, height), Retention::Retain);
    ```

=== "C"

    ```c
    ss_texture_ref_t ref;
    /* NULL is the selected pack; repeat with each sibling pack's name. */
    for (uint32_t i = 0; i < ss_player_texture_count(player, NULL); i++) {
        if (ss_player_texture_at(player, NULL, i, &ref) != SS_OK || ref.embedded) continue;
        /* Read <the .ssab's directory>/ref.path yourself; ref.key is already the key. */
        /* Raw RGBA8, width * height * 4 bytes, straight alpha: */
        ss_scene_add_texture(scene, ref.key, rgba, width, height);
        /* or the encoded PNG bytes, decoded by the library: */
        /* ss_scene_add_texture_png(scene, ref.key, png_bytes, png_len); */
    }
    ```

=== "JavaScript"

    ```js
    // Every textureRefs / embeddedTextures entry carries the `key` to supply it under.
    ctx.addTexture(key, rgba, width, height);          // raw RGBA8
    await ctx.addTextureFromBytes(key, encodedBytes);  // PNG/WebP, decoded by the browser
    ctx.addTextureFromBitmap(key, imageBitmap);        // an ImageBitmap you already have
    ```

To know *what* to supply, ask the pack. Each entry says whether the atlas rides inside the `.ssab`
(embedded — nothing to do) or names a file the exporter wrote **relative to the pack's own
directory**:

| | Rust | C | JavaScript |
|---|---|---|---|
| Enumerate | `player.textures(pack)` | `ss_player_texture_count` / `ss_player_texture_at` | `player.textureRefs(pack)` |
| Embedded bytes | `player.embedded_images(pack)` | *(no call needed)* | `player.embeddedTextures(pack)` |

## Reading from somewhere `File::open` cannot reach

An exported Godot project keeps its files in a `.pck`, an Android build in an APK, Unreal's in a
`.pak`, a browser behind `fetch`. Rust hosts plug their own reader into the same walk with
`load_with`:

```rust
player.load_with(&mut my_loader, "res://characters/Ringo.ssab").await?;
```

`AssetLoader` is eight methods — `reset_base`, `set_base`, `read`, `supply_texture`, `has_texture` and
`warn` to write, plus `read_pack` and `texture_key`, which have defaults — and the walk's half of the
contract is fixed and tested:

1. `reset_base()` first, so one loader serves any number of loads;
2. `read` gets your `src` **verbatim** — the walk builds no paths and never looks inside it;
3. *then* `set_base(src)` once, and every path after that is a **bare name** relative to it.

Resolving is your job, because only you know whether you are holding a filesystem path, a URL or an
archive key.

!!! note "Do not reach for `std::fs` instead"
    It compiles on wasm32 and fails at runtime, and it cannot open a file that is inside an archive.
    The seam exists for exactly those two cases.

### In a browser { #in-a-browser }

The web face resolves URLs itself, so what you replace there is only the `fetch`. Pass `load` a
`read(url)`, and every file the walk asks for — the `.ssab`, its atlases, its sibling packs — comes
from you instead, by the absolute URL a fetch would have used. Answer an `ArrayBuffer`, a
`Uint8Array` or a `Blob`, or `null` for a file you do not have.

That is how a page plays a dropped folder: file its contents under URLs of your own choosing, and
load from there.

```js
let drops = 0;

async function playDropped(dropped) {   // [{ path, file }, …] from the drop event
  // One base per drop. Atlases are keyed by URL, so a second folder under the first
  // one's base would find a same-named atlas already loaded and draw the first one's.
  const base = new URL(`dropped/${++drops}/`, location.href);
  const files = new Map(dropped.map(({ path, file }) => [new URL(path, base).href, file]));
  await player.load(new URL('Ringo/Ringo.ssab', base).href, (url) => files.get(url));
}
```

`loadSsab`, `addSsab` and the element's `loadSrc` take the same `read` as their last argument. The
<a href="../demo/" target="_blank" rel="noreferrer">WebGPU demo</a> plays a folder dropped on it
this way.

## Next

* [Masks](./masks.md) — the one thing about drawing that depends on your render target
* [Device loss](./device-loss.md) — what happens to everything above when the GPU goes away
* [Preparing assets](../start/assets.md) — producing the `.ssab` in the first place
