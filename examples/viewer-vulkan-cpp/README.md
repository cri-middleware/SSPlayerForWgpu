# viewer-vulkan-cpp — Vulkan verification viewer (C API)

> **Status: built in CI, not yet run on a desktop.** It was written on macOS,
> where neither the X11 nor the Wayland code can be compiled. CI builds it on Linux
> for every pull request, but the runner has no display, so it has never opened a
> window; the first run on a Linux desktop is tracked in the
> [ROADMAP](../../ROADMAP.md). The [verification checklist](#verification-checklist)
> below is what that run checks.

A Linux host that drives `ssplayer-wgpu` through its **C API** with the Vulkan
backend forced, so the C face (`ssplayer-wgpu-ffi`) is exercised end to end: this process owns
the window, hands its raw handle to `ss_scene_create`, and calls
`ss_scene_update` / `ss_scene_render` every frame. No Rust is involved on this
side of the ABI.

It is the Linux twin of [`viewer-dx12-cpp`](../viewer-dx12-cpp/README.md) and
[`viewer-metal-cpp`](../viewer-metal-cpp/README.md), and the three deliberately
duplicate the playback loop rather than share it: what actually differs is window
and surface creation, and the sample's value is that one directory copies whole.

**It opens with two characters in one window**, which is the point of the handle
split: `ss_scene_t` is the window, `ss_player_t` is a character drawn into it, and
they share one device, one renderer and one atlas upload.

The packs it starts on come from `assets/overall` — **reference-mode** `.ssab`
that name their atlases as sibling files, matching the Rust viewer. Either mode
is one call: `ss_player_load(player, path)` brings the pack, its atlases and the
sibling `.ssab` files its instance parts play from. Pass `assets/overall_emb` as
the first argument for the embedded set.

## X11 and Wayland are both in here

`SS_WINDOW_XLIB` and `SS_WINDOW_WAYLAND` are separate arms of `surface_target` in
`ffi.rs`, and **running the X11 viewer on a Wayland session verifies only the
Xlib arm** — Xwayland is an X server, so nothing about it reaches the Wayland
one. There is no single window that covers both, so this binary carries both and
prints which one is live in its status block.

| Invocation | Window path |
|---|---|
| `ssviewer-vulkan` | Wayland when `WAYLAND_DISPLAY` is set, else X11 |
| `ssviewer-vulkan --wayland` | `SS_WINDOW_WAYLAND` — `wl_surface*` + `wl_display*` |
| `ssviewer-vulkan --x11` | `SS_WINDOW_XLIB` — `Window` XID + `Display*` + screen |

**Both need running, and on Ubuntu one machine can do it**: the default GNOME
session is Wayland, and the login screen's gear menu offers *Ubuntu on Xorg* for
a native X11 one. Under the Wayland session, `--x11` still runs — through
Xwayland — which is worth doing too, since that is what most X11 clients on a
modern desktop actually get.

The window code is deliberately raw: Xlib and `wayland-client` + xdg-shell, no
GLFW, no SDL. A toolkit would be a fifth of the lines, but then the sample would
be showing how to use the toolkit rather than what the C API needs, and the
directory would stop being copyable on its own.

## Build and run

Needs the Rust toolchain, CMake 3.20+, a C++17 compiler, and the development
files for whichever window systems you want built in:

```bash
sudo apt install build-essential cmake pkg-config \
                 libx11-dev libwayland-dev wayland-protocols
```

Both window systems are optional at configure time — CMake warns and drops the
one it cannot find, and fails only if neither is there. Wayland needs all three
of `libwayland-dev` (the client library), `wayland-protocols` (the xdg-shell XML)
and `pkg-config` (which is how `wayland-scanner` and the XML directory are
located).

You also need a Vulkan driver. Check what the machine has before blaming the
viewer:

```bash
sudo apt install mesa-vulkan-drivers vulkan-tools
vulkaninfo --summary        # must list at least one device
```

Then:

```bash
git submodule update --init          # once
./scripts/deploy-examples.sh         # once — generates assets/
cd examples/viewer-vulkan-cpp
cmake -B build
cmake --build build
./build/ssviewer-vulkan              # or: ./build/ssviewer-vulkan --x11 "" Mask
```

CMake runs `cargo build -p ssplayer-wgpu-ffi` for you, which also
regenerates `libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` via cbindgen. The
executable gets an rpath into `target/`, so `libssplayer_wgpu.so` does not have
to be copied anywhere. Configure with `-DCMAKE_BUILD_TYPE=Debug` to link the
debug library instead; cargo's debug build of wgpu is slow but works.

Arguments are optional: `ssviewer-vulkan [--x11|--wayland] [asset-dir] [pack-name]`.
Pass `""` for the asset directory to keep the built-in default and still pick a
pack. Status is printed to the terminal it was launched from.

| Key | Action |
|---|---|
| Space | pause / resume (`ss_player_pause` / `ss_player_resume`) |
| Left / Right | previous / next `.ssab` in the asset directory |
| Up / Down | previous / next animation in the current pack |
| R | restart the current animation |
| + / - | add / remove a player (1–6, all in the one scene) |
| Escape | quit |

Dropping a `.ssab` on the window — which the Metal and DX12 viewers support — is
not implemented here. On X11 that is the XDnD handshake written out by hand, on
Wayland it is `wl_data_device`, and neither says anything about the C API. Pass a
directory as the first argument instead.

## Verification checklist

Nothing on this list is recorded as checked. Work through it in order, **and do
the whole list once per window path** — they share every line
of the playback loop and no line of the window code.

1. `ss_scene_create` returns non-null. That is the raw-handle → surface path
   (`surface_target`'s Xlib or Wayland arm) and the Vulkan adapter request.
2. Something draws, right way up, with `Basic.ssab`.
3. `Mask.ssab` clips, i.e. `enable_mask` really allocated a Vulkan stencil.
4. Resizing reconfigures cleanly. On X11 that is `ConfigureNotify`; on Wayland it
   is `xdg_toplevel.configure` followed by the `xdg_surface.configure` that
   applies it, and with no title bar (see below) the way to trigger it is
   Super+middle-drag or the compositor's own resize shortcut.
5. `+` up to six players still presents one frame per vblank.
6. The status block's *Window* line names the path you meant to be testing.
   Under a Wayland session with no flag it must say Wayland; with `--x11` it must
   say X11, and that run is going through Xwayland.

## Known rough edges

These are deliberate, and each one is a place a real application would do more:

* **No window decorations on Wayland under GNOME.** There is no
  `xdg-decoration` here, and GNOME does not decorate a client that has not asked
  to be decorated. Escape quits, Super+drag moves. KDE would draw a title bar for
  the same binary.
* **Wayland keys follow physical QWERTY positions.** The key handler reads raw
  evdev codes rather than pulling in `xkbcommon` and compiling a keymap, so on a
  non-QWERTY layout `R` and `+`/`-` are wherever those keys sit on the hardware.
  Arrows, Space and Escape are unaffected. X11 reads keysyms and has no such
  problem.
* **No HiDPI handling.** The scene is created at the size the compositor
  reports, with no display scale applied — a scaled desktop will show a window
  drawn at the wrong pixel density. `ss_scene_desc_t` takes pixels, so a real
  host multiplies by its own scale factor.
* **Minimising on Wayland stops the loop** until the window is shown again. FIFO
  present waits on a frame callback, and a hidden surface stops getting them.
  That is the protocol working, not a hang. X11 handles this explicitly by
  blocking on the next event instead of presenting.

## If the only adapter is lavapipe

A VM usually has no Vulkan driver of its own, and `mesa-vulkan-drivers` then
leaves lavapipe — Mesa's software device — as the only ICD. That is enough to
verify everything this viewer is for: surface creation, swapchain configuration,
resize and present all run for real, and so does every draw. It is not enough to
say anything about performance, and it does not exercise RADV / NVIDIA / ANV
behaviour, which is the part only real hardware answers.

To pin the run to lavapipe on a machine that has both (`x86_64` shown; the file
is `lvp_icd.aarch64.json` on ARM):

```bash
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json ./build/ssviewer-vulkan
```

The status block's `GPU pass` line reads `n/a (no timestamp query)` there, since
lavapipe has no timestamp query — that is expected, not a failure.
