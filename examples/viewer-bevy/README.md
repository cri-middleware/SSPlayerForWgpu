# viewer-bevy — Bevy host

The reference host for the **Bevy adapter** (`ssplayer-wgpu-bevy`), and its
development viewer — the Bevy sibling of [`../viewer`](../viewer) (native winit)
and the [`../viewer-metal-cpp`](../viewer-metal-cpp) / [`../viewer-dx12-cpp`](../viewer-dx12-cpp)
/ [`../viewer-vulkan-cpp`](../viewer-vulkan-cpp) C++ hosts.

Like the native viewer, **it shows a cast, not a character.** Bevy owns the
device; the adapter draws every `SsPlayer` entity into Bevy's own 2d pass, so N
characters share one target and one pass. Everything on screen is driven purely
through the three components the adapter exposes — `SsPlayer`, `SsTransform`,
`SsPlayback` — which is the shortest honest answer to "how do I put these in my
Bevy game?"

For a scripted, one-character walk through the whole sample set (Basic, Shape,
Effect, Mesh, …), run [`../overall-bevy`](../overall-bevy) instead
(`cargo run -p ssplayer-wgpu-overall-bevy`); this one is the interactive harness
for poking at a live cast.

## Build and run

```bash
git submodule update --init          # once
./scripts/deploy-examples.sh         # once — generates assets/
cargo run -p ssplayer-wgpu-viewer-bevy
```

It starts with two characters on `Basic`, in one pass. Every `.ssab` under
`assets/overall` is in the picker.

## Controls

The pack / animation / `V` / `[` `]` keys act on the **focused** member; focus
starts at "all", so they drive the whole cast until you press `Tab`.

| Key | Action | | Key | Action |
|---|---|---|---|---|
| Space | pause / resume | | `+` / `-` | add / remove a character (1–8) |
| Left / Right | previous / next pack | | Tab | cycle focus (all / #1 / #2 …) |
| Up / Down | previous / next animation | | V | toggle visibility |
| R | restart the animation | | `[` / `]` | alpha down / up |
| L | layout: Row / Stack | | O | reverse draw order |
| F | flip Y (vertical mirror) | | Escape | quit |
| drop | play a dropped `.ssab`, or every `.ssab` in a dropped folder | | | |

**Stack is the layout that shows the interesting behaviour.** Characters only
interact where they overlap, so a Row of them looks like several separate ones.
Put a `Mask` pack over a `Basic` pack in Stack and check that the first one's mask
does not clip the second, and use `O` to confirm draw order decides who is in
front. The on-screen HUD lists each member's pack, animation, order, alpha and
read-back frame.

## What is here vs. the native viewer

The cast, layout, pack/animation switching, focus, visibility, alpha, order and
drop all behave as they do in [`../viewer`](../viewer). **Two of the native
viewer's keys are deliberately absent, because the adapter's surface does not reach
what they drive** — a useful reminder of where the Bevy seam currently stops:

- **K (mask backend)** — Bevy's 2d depth is `Depth32Float`, which carries no
  stencil, so the adapter always selects the player's coverage-bitmap backend.
  There is no stencil path to toggle to. (Masks still work; that is the point of
  the coverage backend.)
- **The track keys (C / S / A / 1 / 2 / 0 / , / .)** — track mixing is a method on
  the render-world player, and the adapter exposes only `SsPlayer` / `SsTransform`
  / `SsPlayback`, none of which carries a track stack yet.

`F` is also an approximation: the native viewer flips the projection's handedness,
but the adapter owns the projection (it follows the camera), so here `F` mirrors
the character vertically with a negative scale instead. And the HUD omits the
GPU / primitive counters the native viewer prints — the adapter does not surface
`SSScene::stats()` back to the main world.

## Where the data lives, and how a real app ships it

This example carries **no `assets/` folder of its own**. Two loaders are in play,
and only one of them is Bevy's:

- The **SpriteStudio player** loads each `.ssab` and its atlases through its own
  loader, rooted on the `src` path — independent of Bevy's `AssetServer`. This
  viewer also parses the `.ssab` catalogue itself, in the main world, to list each
  pack's animation names (the render-world player that could answer is out of the
  input systems' reach) — exactly what the native viewer does.
- **Bevy's `AssetServer`** is used only for the sound files, so the app points
  `AssetPlugin { file_path }` at the repository's shared, generated `assets/` tree
  (`CARGO_MANIFEST_DIR/../../assets`) — the same tree `deploy-examples` fills and
  every viewer here reads. It is gitignored; never commit it.

Bevy itself has no Unity/Godot-style project format and no default asset pack
(`.pck` / `.pak`): a shipped Bevy game is **the executable plus an `assets/`
directory beside it**, which `AssetPlugin` reads (default `./assets`, overridable
as above). A real host would copy the packs, atlases and sounds it needs into its
own `assets/` next to the binary; this development viewer instead points at the
shared repo tree so there is one copy of the data, out of git.

## Related

- [`../../libs/ssplayer-wgpu-bevy/README.md`](../../libs/ssplayer-wgpu-bevy/README.md) — the adapter: components, colour, masking, device loss
- [`../../libs/ssplayer-wgpu/README.md`](../../libs/ssplayer-wgpu/README.md) — the player crate's API
- [`../viewer`](../viewer) — the same cast in a native winit host
