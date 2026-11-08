# viewer-metal-cpp — Metal verification viewer (C API)

A macOS/Cocoa host that drives `ssplayer-wgpu` through its **C API** with the
Metal backend forced, so the C face (`ssplayer-wgpu-ffi`)
is exercised end to end: Cocoa owns the window and its `CAMetalLayer`, hands the
layer to `ss_scene_create`, and calls `ss_scene_update` / `ss_scene_render`
every frame. No Rust is involved on this side of the ABI.

**It opens with two characters in one window**, which is the point of the handle
split: `ss_scene_t` is the window, `ss_player_t` is a character drawn into it, and
they share one device, one renderer and one atlas upload. Fusing the two would
make a second character need a second surface on the same window, and the two
would present over each other.

The packs it starts on come from `assets/overall` — **reference-mode** `.ssab`
that name their atlases as sibling files, matching the Rust viewer. Either mode
is one call: `ss_player_load(player, path)` brings the pack, its atlases and the
sibling `.ssab` files its instance parts play from. Pass `assets/overall_emb` as `argv[1]`
for the embedded set.

## Drop a `.ssab` on the window

Dropping a `.ssab` — or a folder of them — plays it, whichever way it was
exported. Dropped packs join the list `Left`/`Right` walks rather than replacing
it, so a character can be compared against the demo set. The sibling packs a
pack's instance parts play from are found beside it, as `ss_player_load` finds
them.

**Reference-mode packs work too, and that is the interesting part.** Their
atlases stay as sibling files, and where those sit is the `.ssab`'s own business:
`ss_player_load` resolves them against the directory the file was read from, and
a host whose assets are not files answers the same question itself through
`ss_player_texture_at` + `ss_scene_add_texture_png`.

## Build and run

```sh
git submodule update --init          # once
./scripts/deploy-examples.sh         # once — generates assets/
cd examples/viewer-metal-cpp
cmake -B build
cmake --build build
./build/ssviewer-metal               # or: ./build/ssviewer-metal "" Mask
```

CMake runs `cargo build -p ssplayer-wgpu-ffi` for you, which also
regenerates `libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` via cbindgen. Use
`-DCMAKE_BUILD_TYPE=Debug` to link the debug library instead.

Arguments are optional: `ssviewer-metal [asset-dir] [pack-name]`. Pass `""` for
the asset directory to keep the built-in default and still pick a pack.

| Key | Action |
|---|---|
| Space | pause / resume (`ss_player_pause` / `ss_player_resume`) |
| Left / Right | previous / next `.ssab` |
| Up / Down | previous / next animation |
| R | restart the current animation |
| `+` / `-` | add / remove a character (`ss_scene_create_player` / `ss_player_destroy`) |
| Escape | quit |
| drop | play a dropped `.ssab`, or add every `.ssab` in a dropped folder |

## Linking against the library

The viewer links the **cdylib** (`libssplayer_wgpu.dylib`) straight from cargo's
output directory, with no rpath: cargo leaves the dylib's install name at its
absolute path under `target/`, which is what the binary records and loads it from.
That keeps the sample short, and only runs on the machine that built it. Shipping
an app instead? Either copy the dylib next to the binary and give it an `@rpath`
install name (the release zip's dylib already has one), or link the
**staticlib** (`libssplayer_wgpu.a`, built by the same command) and add the
frameworks wgpu's Metal backend needs.

## Why this directory duplicates the loop

The DX12 and Vulkan viewers repeat this play loop rather than sharing it. Only
the window/surface creation differs between them, and a shared helper would cost
the property that makes a sample useful: copy one directory and it builds.
