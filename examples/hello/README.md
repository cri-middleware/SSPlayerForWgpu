# hello — the smallest Rust host (winit + wgpu)

One window, one character, fitted into the window: the
[Rust quick start](../../docs/en/start/rust.md) as a program, step for step. Read
[`src/main.rs`](./src/main.rs) top to bottom — it is the whole integration.

## Build and run

```bash
git submodule update --init          # once
./scripts/deploy-examples.sh         # once — generates assets/
cargo run -p ssplayer-wgpu-hello
```

It plays the first animation of `assets/Ringo/Ringo.ssab` and refits it whenever the
window is resized.

## What it shows

| Step | Where |
|---|---|
| One `SSRenderer` per device, one `SSScene` per render target | `Hello::new` |
| A non-sRGB surface format, and the stencil attachment `MaskBackend::Stencil` needs | `Hello::new`, `stencil_view` |
| Loading a pack with its atlas in one call, selecting an animation, starting it | `Hello::new` |
| `set_viewport` and `fit_into` on every resize | `Hello::place` |
| `update` → `encode_mask_coverage` → `draw` into a pass the host opened | `Hello::frame` |

Everything else — several characters, pack switching, mask backends, motion
blending, live stats — is in [`../viewer`](../viewer).
