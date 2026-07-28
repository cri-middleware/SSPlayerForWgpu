# viewer-metal-cpp — Metal verification viewer (C API)

A macOS/Cocoa host that drives `ssplayer-wgpu` through its **C API** with the
Metal backend forced, so the "③ ffi" face of [SPEC §11](../../SSPLAYER_WGPU_SPEC.md)
is exercised end to end: Cocoa owns the window and its `CAMetalLayer`, hands the
layer to `ss_player_create`, and calls `ss_player_update` / `ss_player_render`
every frame. No Rust is involved on this side of the ABI.

The packs it plays come from `assets/overall_emb`, i.e. **embedded-mode** `.ssab`
whose atlases ride inside the file, so nothing has to supply textures
([SPEC §8.1](../../SSPLAYER_WGPU_SPEC.md)).

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
| Escape | quit |

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
