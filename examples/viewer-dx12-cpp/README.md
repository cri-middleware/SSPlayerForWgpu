# viewer-dx12-cpp — DX12 verification viewer (C API)

A Win32 host that drives `ssplayer-wgpu` through its **C API** with the DX12
backend forced, so the C face (`ssplayer-wgpu-ffi`) is
exercised end to end: Win32 owns the window, hands its `HWND` to
`ss_scene_create`, and calls `ss_scene_update` / `ss_scene_render` every frame.
No Rust is involved on this side of the ABI.

It is the Windows twin of [`viewer-metal-cpp`](../viewer-metal-cpp/README.md),
and the two deliberately duplicate the playback loop rather than share it: what
actually differs is window and surface creation (`HWND` vs `CAMetalLayer`), and
the sample's value is that one directory copies whole. Everything
below is the Metal viewer's behaviour, unchanged.

**It opens with two characters in one window**, which is the point of the handle
split: `ss_scene_t` is the window, `ss_player_t` is a character drawn into it, and
they share one device, one renderer and one atlas upload.

The packs it starts on come from `assets/overall` — **reference-mode** `.ssab`
that name their atlases as sibling files, matching the Rust viewer. Either mode
is one call: `ss_player_load(player, path)` brings the pack, its atlases and the
sibling `.ssab` files its instance parts play from. Pass `assets\overall_emb` as `argv[1]`
for the embedded set.

## Drop a `.ssab` on the window

Dropping a `.ssab` — or a folder of them — plays it, whichever way it was
exported. Dropped packs join the list `Left`/`Right` walks rather than replacing
it, so a character can be compared against the demo set; drop a whole folder when
the pack has external instance references, since those resolve by pack name and
every referenced pack has to be registered first.

**Reference-mode packs work too, and that is the interesting part.** Their
atlases stay as sibling files, and where those sit is the `.ssab`'s own business:
`ss_player_load` resolves them against the directory the file was read from, and
a host whose assets are not files answers the same question itself through
`ss_player_texture_at` + `ss_scene_add_texture_png`.

## Build and run

Needs MSVC (Visual Studio 2019 or newer, "Desktop development with C++"), CMake
3.20+, and the `x86_64-pc-windows-msvc` Rust toolchain — the same target the
library builds for. From a *Developer Command Prompt*, or any shell where
`cmake` and `cargo` are on `PATH`:

```powershell
git submodule update --init          # once
.\scripts\deploy-examples.ps1        # once — generates assets\
cd examples\viewer-dx12-cpp
cmake -B build
cmake --build build --config Release
.\build\Release\ssviewer-dx12.exe    # or: ... ssviewer-dx12.exe "" Mask
```

With a single-config generator (`cmake -B build -G Ninja`) the executable lands
in `build\ssviewer-dx12.exe` instead, and the build defaults to Release.

CMake runs `cargo build -p ssplayer-wgpu-ffi` for you, which also
regenerates `libs\ssplayer-wgpu-ffi\include\ssplayer_wgpu.h` via cbindgen, and copies
`ssplayer_wgpu.dll` next to the executable — Windows has no rpath, so the DLL has
to sit beside the `.exe`. Use `--config Debug` (or `-DCMAKE_BUILD_TYPE=Debug`) to
link the debug library instead; cargo's debug build of wgpu is slow but works.

Arguments are optional: `ssviewer-dx12 [asset-dir] [pack-name]`. Pass `""` for
the asset directory to keep the built-in default and still pick a pack. Status is
printed to the console the viewer was launched from, so run it from a terminal.

| Key | Action |
|---|---|
| Space | pause / resume (`ss_player_pause` / `ss_player_resume`) |
| Left / Right | previous / next `.ssab` in the asset directory |
| Up / Down | previous / next animation in the current pack |
| R | restart the current animation |
| + / - | add / remove a player (1–6, all in the one scene) |
| Escape | quit |
| drop | play a dropped `.ssab`, or add every `.ssab` in a dropped folder |

## Verification checklist

Work through this list in order after a change to the window or surface code:

1. `ss_scene_create` returns non-null — that is the raw-handle → surface path and
   the DX12 adapter request.
2. Something draws, right way up, with `Basic.ssab`.
3. `Mask.ssab` clips, i.e. `enable_mask` really allocated a D3D12 stencil.
4. Resizing reconfigures cleanly (`ss_scene_resize` on `WM_SIZE`), including the
   drag itself — the modal loop is kept alive by a timer here, not by the main
   loop.
5. `+` up to six players still presents one frame per vblank.

If the backend is the thing you are isolating, `SS_BACKEND_VULKAN` in
`desc.backend` runs the same window through Vulkan on Windows and is a one-line
change.
