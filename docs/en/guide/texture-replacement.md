# Texture replacement

Swap the texture a **cell map** binds — a whole atlas at once — from your program, without touching
the `.ssab`. This is the avatar-customization path: costume changes, palette swaps, dynamic
expressions. Prepare a second atlas with the **same layout** as the original and bind it at runtime,
and every part drawing from that cell map picks it up.

It is a **Player-side override**, separate from the [part overrides](./part-overrides.md): those
change what *one part* draws; this replaces *a whole texture*.

## Naming what to replace

You address a texture by **cell map name** — the SpriteStudio name, without the `.ssce` extension —
or by a **TextureChange target**'s file name. Both are what a draw resolves through, and both are
enumerable so a UI can offer the choices rather than hard-coding them. Passing no pack selects the
**main pack**.

=== "Rust"

    ```rust
    for name in player.replaceable_cellmap_names(None).unwrap_or_default() {
        // "common", "Clothes", "spritemap1.png" … cell maps and TextureChange targets both
    }
    ```

=== "C"

    ```c
    uint32_t n = ss_player_cellmap_texture_count(player, /* pack */ NULL);
    for (uint32_t i = 0; i < n; i++) {
        const char *name = ss_player_cellmap_texture_name(player, NULL, i);
    }
    ```

=== "JavaScript"

    ```js
    const names = player.replaceableCellmapNames(null);  // string[]
    ```

=== "Bevy"

    ```rust
    // The names come from the pack; discover them through the Rust API on the
    // render-world player, or read them off your source `.ssae` in the editor.
    ```

## Replacing and restoring

A replacement is straight-alpha RGBA8, the same form a loaded atlas is; it is premultiplied on the
way to the GPU for you. **Restoring** points the cell map back at its original atlas — which was
never touched, so nothing is reloaded.

=== "Rust"

    ```rust
    // Override the main pack's "Clothes" cell map.
    player.set_cellmap_texture(None, "Clothes", &rgba, 1024, 1024)?;

    // …later, put the original back.
    player.restore_cellmap_texture(None, "Clothes");

    // Is it currently overridden?
    let swapped = player.is_cellmap_overridden(None, "Clothes");
    ```

    An engine that already owns the GPU texture skips the pixel copy: take a private key for the cell
    map and register your own `TextureView` under it.

    ```rust
    use ssplayer_wgpu::AlphaMode;

    let key = player.override_cellmap_texture_key(None, "Clothes").unwrap();
    renderer.set_texture(&device, key, &view, (1024, 1024), AlphaMode::Straight);
    ```

=== "C"

    ```c
    /* rgba = width*height*4 straight-alpha bytes. */
    ss_player_set_cellmap_texture(player, /* pack */ NULL, "Clothes", rgba, 1024, 1024);

    /* Passing rgba = NULL restores the original (width/height are then ignored). */
    ss_player_set_cellmap_texture(player, NULL, "Clothes", NULL, 0, 0);

    bool swapped = ss_player_is_cellmap_overridden(player, NULL, "Clothes");
    ```

=== "JavaScript"

    ```js
    // From encoded bytes (PNG), decoded by the browser:
    await player.setCellmapTextureFromBytes(null, 'Clothes', bytes);

    // …or from a premultiplied ImageBitmap you already have
    // (createImageBitmap(blob, { premultiplyAlpha: 'premultiply' })):
    player.setCellmapTexture(null, 'Clothes', imageBitmap);

    player.restoreCellmapTexture(null, 'Clothes');
    const swapped = player.isCellmapOverridden(null, 'Clothes');
    ```

=== "Bevy"

    ```rust
    use ssplayer_bevy::SsCellmapTextures;
    use bevy::platform::collections::HashMap;

    // Insert (or edit) the component. An image that is still loading is applied
    // the frame it becomes ready, so a freshly-requested handle is fine.
    let mut overrides = HashMap::new();
    overrides.insert("Clothes".to_string(), asset_server.load("sailor.png"));
    commands.entity(character).insert(SsCellmapTextures { overrides });

    // Remove an entry — or the whole component — to restore the original.
    ```

The swap is a **per-character** costume: another character playing the same pack is unaffected, and
an instance part's sub-animation (it plays from its own pack) does not inherit it — set it there too
if you want it.

## What to watch

!!! note "`set_cellmap_texture` vs `set_part_cell_override`"

    Two different swaps, and easy to confuse. **`set_cellmap_texture`** replaces a *whole cell map's
    texture*: every part drawing from that cell map changes at once, and each cell keeps its position.
    [**`set_part_cell_override`**](./part-overrides.md#cell) changes *which cell one part draws*, one
    part at a time. A costume recolour is the first; giving one hand a different weapon is the second.

!!! warning "Same size, same layout"

    The runtime's UVs stay in the **original** cell map's pixel space, and the player normalises them
    by the size of the texture it bound. A replacement of a different resolution therefore samples a
    different region — so swap in an atlas with the **same size and the same cell layout** as the
    original.

!!! tip "Replacing a TextureChange target"

    A part under an authored **TextureChange** binds through a different hash on the frames the change
    is active. Overriding the cell map name does not reach those frames — to replace what a
    TextureChange points at, pass **its file name** (the `…​.png` entries that appear in the
    enumeration above) instead.

On Rust and C the replacement reaches the GPU on the **next update** — the borrow model lends the
player no device before then — so the swap shows from the following frame. It survives selecting
another animation, because it lives with the character, not with the current animation tree.

## Next

* [Part overrides](./part-overrides.md) — changing what *one part* draws
* [Loading and assets](./loading.md) — where a pack's own atlases come from
