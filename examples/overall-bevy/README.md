# overall-bevy — the Bevy feature tour

The **Bevy counterpart of the sibling players' `overall` / `webgl-example`
demos**: one character stepped through the shared sample set, so every SpriteStudio
feature the player supports can be seen in sequence, in a Bevy app, driven entirely
through the adapter's three components (`SsPlayer`, `SsTransform`, `SsPlayback`).

Where [`../viewer-bevy`](../viewer-bevy) is the free-form development viewer (a
cast you poke at), this is the scripted, labelled tour — the one to run first to
see what the adapter can do.

## Build and run

```bash
git submodule update --init          # once
./scripts/deploy-examples.sh         # once — generates assets/
cargo run -p ssplayer-wgpu-overall-bevy
```

## What it steps through

The `Ringo` character (wait / run / dance) and the `overall/*` feature packs —
Basic, Shape, PartsColor, Mesh, BlendMode, Effect, Mask, Instance, TextureChange,
Sound, UserData, Signal — plus a masked-instance scene. The on-screen HUD names
the current scene, the timeline frame, the speed, and the last timeline event.

| Key | Does |
|---|---|
| `[` / `]` | Previous / next scene |
| `Space` | Play / pause |
| `Up` / `Down` | Speed (0.1–3.0) |
| `R` | Restart from the head |

The `Sound` scene plays audio, which this host does — the player only names the
cue; `UserData` and `Signal` raise timeline events the HUD echoes. Masking uses the
adapter's coverage backend, since Bevy's 2d depth carries no stencil.

## Where the data lives

This example carries **no `assets/` folder of its own**. The player loads each
`.ssab` and its atlases through its own loader (rooted on the `src` path), while
Bevy's `AssetServer` — pointed at the repository's shared, generated `assets/` tree
via `AssetPlugin { file_path }` — supplies only the sound files. That tree is what
`deploy-examples` fills and every viewer here reads; it is gitignored. See
[`../viewer-bevy/README.md`](../viewer-bevy/README.md) for the fuller note on how a
shipped Bevy app carries its data (executable plus an `assets/` directory beside
it — Bevy has no Unity/Godot-style project format).

## Related

- [`../../libs/ssplayer-wgpu-bevy/README.md`](../../libs/ssplayer-wgpu-bevy/README.md) — the adapter: components, colour, masking, device loss
- [`../viewer-bevy`](../viewer-bevy) — the interactive development viewer
