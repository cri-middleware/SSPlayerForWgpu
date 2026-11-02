---
title: Rust API reference
---

# Rust API reference

The generated reference is published beside these pages:

<a href="../api/ssplayer_wgpu/index.html" target="_blank" rel="noreferrer">**📦 rustdoc for `ssplayer_wgpu`**</a>
&nbsp;·&nbsp;
<a href="../api/ssplayer_bevy/index.html" target="_blank" rel="noreferrer">**rustdoc for `ssplayer_bevy`**</a>

Locally, `cargo doc --no-deps --all-features -p ssplayer-wgpu --open` renders the same thing.

This page is the map: what lives where, and which feature turns it on.

## The public surface

```rust
pub use events::{AudioEvent, SignalEvent, SignalParam, SignalValue, SsEvent, UserDataEvent};
pub use fit::{Align, Fit, Placed, Placement, Rect};
pub use frame::FrameView;
pub use mask::{GeomKind, MaskOp, MaskWriter, MaskWriters, PartMaskFlags, ScopedWriter, StencilTest, plan_writers};
pub use metrics::SceneStats;
pub use pack::{EmbeddedImage, EmbeddedTexture, SSPack, TextureRef};
pub use player::{AssetLoader, AtlasKeys, BlendOp, CrowdMember, MAX_CROWD_MEMBERS, MissingAsset,
                 MissingKind, OverridePriority, PlaybackDirection, PlaybackStyle, SSPlayer};
pub use renderer::{AlphaMode, Retention, SSRenderer};
pub use scene::{BatchInfo, FrameChunk, IDENTITY_TRANSFORM, MaskBackend, NodeSlot, SSScene, SceneConfig};
pub use ssruntime::TrackBlend;
pub use ssruntime::TrackSync;
pub use ssruntime::error::SSRuntimeError;
#[cfg(feature = "web")]
pub use web::{SsContextWeb, SsPlayerWeb, SsSurfaceWeb};
pub const VERSION: &str;      // this player's version
pub const SDK_VERSION: &str;  // the SpriteStudio-SDK version it links, as a release tag, or "local"
```

Most applications touch six of these: `SSRenderer`, `SSScene`, `SceneConfig`, `SSPlayer`,
`MaskBackend` and `SsEvent`.

## The three types

| Type | Scope | Key calls |
|---|---|---|
| `SSRenderer` | one device | `new`, `set_texture`, `upload_texture`, `texture_size`, `restore_device` |
| `SSScene` | one render target | `new`, `set_viewport`, `update`, `update_with`, `encode_offscreen`, `set_group_alpha`, `draw`, `stats`, `restore_device` |
| `SSPlayer` | one character | `load`, `set_animation`, transport, overrides, `set_cellmap_texture`, `events`, `set_transform`, `set_crowd`, `restore_textures` |

`SSPlayer` holds **no wgpu-typed field**, so one scene carries any number of them and they share
one upload of a pack's atlases. See [The borrow model](../wgpu/borrow-model.md).

## Modules

How the source is laid out. Only `fit`, `host` and `ffi` are public modules; everything else you
reach from the crate root, where its types are re-exported (`ssplayer_wgpu::SSPlayer`, not
`ssplayer_wgpu::player::SSPlayer`).

| Module | Role |
|---|---|
| `renderer` | device-scoped: pipelines (built on first use), shaders, the texture map |
| `scene` | target-scoped: projection, the frame arena, the draw list, GPU timing |
| `player` | the playback node tree, loading, transport, per-part overrides |
| `pack` | `SSPack` — a `.ssab` shared between characters. CPU data only |
| `fit` | fitting the authored canvas into a box: `Fit`, `Align`, `Placement` |
| `assets` / `decode` | the loader seam, and bulk atlas decoding |
| `mask` | the stencil planner |
| `events` / `metrics` / `roster` | timeline events, frame counters, generation-tagged handles |
| `host` | a window-owning wgpu stack (`ffi` feature) — usable from Rust when you have a raw window handle rather than your own wgpu setup |
| `ffi` / `web` | the C and JavaScript faces |

## Features

| Feature | Default | What it adds |
|---|---|---|
| `embedded` | ✅ | Decodes embedded-mode atlases in-crate, and carries `SSPlayer::load`'s filesystem path. Turn it off where the host decodes instead |
| `web` | | `SsContextWeb` / `SsSurfaceWeb` / `SsPlayerWeb` via wasm-bindgen. Deliberately does **not** pull `embedded` — the browser already ships a decoder |
| `ffi` | | The C entry points and `NativeHost`, which builds wgpu from a raw window handle |

The crate is a plain `rlib`. The companion `ssplayer-wgpu-ffi` crate re-exports its `ffi` and `web`
modules and is the one built as `cdylib` + `staticlib` (the native C library) and as the wasm module
— so one source still serves a Rust dependency, a browser and a native shared library.

!!! note "`SSPlayer::load` needs `embedded` and a filesystem"
    The convenience path is `#[cfg(all(feature = "embedded", not(target_arch = "wasm32")))]`.
    Everywhere else — wasm, an archive, an engine's asset server — the entry point is
    `load_with(&mut loader, src)`. See [Loading and assets](../guide/loading.md).

## Errors

Loading returns `Result<(), SSRuntimeError>` and fails **only when the `.ssab` itself is
unreadable**. Everything else that did not arrive is in `missing_assets()`.

Most override and track calls return `Result<(), SSRuntimeError>`, which is `Err` when nothing is
loaded or the name does not resolve.

## Versioning

`ssplayer-wgpu` and `ssplayer-bevy` are versioned **independently**, because they track
different things: the player tracks the SDK, and the adapter tracks Bevy's release line.

wgpu is pinned to the **30** line repository-wide. See
[Engine integration](../wgpu/engines.md#the-wgpu-pin).
