# viewer-metal-cpp — Metal verification viewer (C API)

A macOS/Cocoa host that drives `ssplayer-wgpu` through its **C API** with the
Metal backend forced, so the "③ ffi" face of [SPEC §11](../../SSPLAYER_WGPU_SPEC.md)
is exercised end to end: Cocoa owns the window and its `CAMetalLayer`, hands the
layer to `ss_scene_create`, and calls `ss_scene_update` / `ss_scene_render`
every frame. No Rust is involved on this side of the ABI.

**It opens with two characters in one window**, which is the point of the P7-7
handle split: `ss_scene_t` is the window, `ss_player_t` is a character drawn into
it, and they share one device, one renderer and one atlas upload. Before the
split a handle was both at once, so a second character meant a second surface on
the same window and the two presented over each other.

The packs it starts on come from `assets/overall` — **reference-mode** `.ssab`
that name their atlases as sibling files, matching the Rust viewer. Either mode
is one call: `ss_player_load(player, path)` brings the pack, its atlases and the
sibling `.ssab` files its instance parts play from
([SPEC §8.1](../../SSPLAYER_WGPU_SPEC.md)). Pass `assets/overall_emb` as `argv[1]`
for the embedded set.

## Drop a `.ssab` on the window

Dropping a `.ssab` — or a folder of them — plays it, whichever way it was
exported. Dropped packs join the list `Left`/`Right` walks rather than replacing
it, so a character can be compared against the demo set; drop a whole folder when
the pack has external instance references, since those resolve by pack name and
every referenced pack has to be registered first (SPEC §7.1).

**Reference-mode packs work too, and that is the interesting part.** Their
atlases stay as sibling files, and where those sit is the `.ssab`'s own business:
`ss_player_texture_at` reports the path the exporter wrote — relative to the file
— plus the hash to register it under and whether it is embedded at all. The
viewer joins that path with the directory the drop came from and hands the bytes
to `ss_scene_add_texture_png`, which decodes with the PNG decoder the library
already carries for embedded packs. So the pack answers *what it needs and from
where*, and the host only reads files ([SPEC §8.2](../../SSPLAYER_WGPU_SPEC.md)).
It is the same division of labour the sibling players use.

## Build and run

```sh
git submodule update --init          # once
./scripts/deploy-examples.sh         # once — generates assets/
cd examples/viewer-metal-cpp
cmake -B build
cmake --build build
./build/ssviewer-metal               # or: ./build/ssviewer-metal "" Mask
```

CMake runs `cargo build -p ssplayer-wgpu --features ffi` for you, which also
regenerates `libs/ssplayer-wgpu/include/ssplayer_wgpu.h` via cbindgen. Use
`-DCMAKE_BUILD_TYPE=Debug` to link the debug library instead.

Arguments are optional: `ssviewer-metal [asset-dir] [pack-name]`. Pass `""` for
the asset directory to keep the built-in default and still pick a pack.

| Key | Action |
|---|---|
| Space | pause / resume (`ss_player_pause` / `ss_player_play`) |
| Left / Right | previous / next `.ssab` |
| Up / Down | previous / next animation |
| R | restart the current animation |
| `+` / `-` | add / remove a character (`ss_scene_create_player` / `ss_player_destroy`) |
| Escape | quit |
| drop | play a dropped `.ssab`, or add every `.ssab` in a dropped folder |

## Linking against the library

The viewer links the **cdylib** (`libssplayer_wgpu.dylib`) and sets an rpath to
cargo's output directory, which keeps the sample short. Shipping an app instead?
Either copy the dylib next to the binary (and fix up the rpath), or link the
**staticlib** (`libssplayer_wgpu.a`, built by the same command) and add the
frameworks wgpu's Metal backend needs.

## Why this directory duplicates the loop

The DX12 and Vulkan viewers repeat this play loop rather than sharing it. Only
the window/surface creation differs between them, and a shared helper would cost
the property that makes a sample useful: copy one directory and it builds
(SPEC §2).
