# AGENTS.md

## What this repository is

`ssplayer-wgpu` — a wgpu renderer for SpriteStudio 7 animations, split out of the
SpriteStudio-SDK monorepo. The canonical design document is
**`SSPLAYER_WGPU_SPEC.md`**; read it before changing rendering behaviour. Playback
semantics (the "brain") are canonically documented in the SDK under
`SpriteStudio-SDK/libs/ssruntime/docs/` — do not duplicate them here.

## Layout

- `core/` — crate `ssplayer-wgpu`. Modules: `lib`, `frame`, `internal` (drawing),
  `player` (playback tree), `mask`, `web` (wasm-bindgen, `web` feature).
- `viewer/` — crate `ssplayer-wgpu-viewer`. Native winit host, development only.
- `SpriteStudio-SDK/` — git submodule. `ssruntime` and `ssab` are linked by path.
- `assets/` — generated, gitignored. Produced by `scripts/deploy-examples.sh`.

The two crates are versioned independently; there is no `workspace.package.version`.

## Verification

```bash
git submodule update --init
./scripts/deploy-examples.sh          # required once: the tests read assets/
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`core/tests/render.rs` renders offscreen and counts pixels; it skips when no GPU
adapter is available, so a "passing" run on a headless machine may have verified
nothing. When changing rasterisation, dump the readback to a PNG in the scratch
directory and look at it — pixel counts alone have hidden real regressions before.

## Constraints

- **wgpu is pinned to 29** for Slint compatibility (SPEC §2.2). Do not bump it
  without checking Slint's supported major; a workspace resolves one wgpu only.
- **`[workspace] exclude = ["SpriteStudio-SDK"]` is load-bearing.** Without it
  cargo claims the SDK crates as members of this workspace and fails to resolve
  their inherited dependencies.
- The player must not own `Device`/`Queue`/`Surface` (borrow model, SPEC §1). Any
  API that takes ownership of wgpu state is wrong.
- Keep `core` free of Slint and Bevy dependencies. Adapters are separate crates.

## SDK changes

If a change requires touching `ssruntime`/`ssab`, make it a pull request in the
SDK repository, then bump the submodule here. Do not commit inside the submodule
from this repository.
