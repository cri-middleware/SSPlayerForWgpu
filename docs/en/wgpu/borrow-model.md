# The borrow model

!!! info "Who this page is for"
    A Rust application that owns a `wgpu::Device`, or anyone writing an engine adapter. If you came
    from the [C](../start/cpp.md) or [web](../start/web.md) path, none of this is required reading —
    those faces own their device for you.

**The player never owns the wgpu `Device`, `Queue`, `Surface` or `SSRenderer`. You own them, and
lend them per frame.**

That single decision is why one library serves a standalone window, a browser canvas, a C++
application and an engine that already has a device of its own — without any of them being a
special case, and without asking you to hand your renderer over.

## What it looks like

```rust
// Yours, for the whole run.
let (device, queue) = ...;

// Yours, lent per frame.
scene.update(&mut renderer, &device, &queue, dt, &mut players);
scene.draw(&renderer, &mut rpass);   // a pass YOU opened, on an encoder YOU own
```

The player borrows what it needs for the duration of a call and returns it. Nothing it holds
outlives the frame.

## Three scopes, because resources have three lifetimes

| Scope | Type | Holds | Rebuilt when |
|---|---|---|---|
| Device | `SSRenderer` | pipelines, bind group layouts, sampler, atlas textures | the device is lost |
| Render target | `SSScene` | the projection uniform, the frame's geometry arena, the draw list | the target's **format** changes |
| Character | `SSPlayer` | the playback tree, transform, order, visibility, alpha | never — it holds no GPU resource |

The rule that keeps this honest: **`SSPlayer` must not gain a wgpu-typed field.** The moment a
character holds a texture, a buffer or a device, "one canvas, one character" comes back — and with
it a device per canvas, a pipeline set per character, and an atlas uploaded once per character
instead of once per device.

One device per application or page. Never `request_device` per canvas or per window.

## Data ownership, in both directions

The borrow model is not only about wgpu handles:

| Data | Owned by | Borrowed by |
|---|---|---|
| The `.ssab` | the player | the runtime |
| Frame data / the SoA buffers | the runtime | the player — **never held across frames** |
| GPU textures | you, or the renderer as a convenience | the renderer's bind groups |

`SSRenderer::set_texture` takes a `TextureView` **you** own and builds a bind group from it;
`upload_texture` is a convenience that happens to create the texture itself. That asymmetry is
deliberate: owning GPU textures would put a second lifecycle in competition with an engine's asset
system — Bevy's `Assets<Image>` and `RenderAssets<GpuImage>`, Unity's, Unreal's — and the loser of
that competition is always the player.

`SSPack` holds CPU data only.

## What your render pass has to be

| | |
|---|---|
| **Colour format** | must equal `SceneConfig::color_format`. Pipelines are baked against it, so a change means a new `SSScene` |
| **MSAA** | off. The pipelines are single-sampled |
| **Stencil** | required if, and only if, you chose `MaskBackend::Stencil` — in the format you declared, attached to **every** pass you draw into |
| **Colour space** | the player writes atlas bytes verbatim and does **no colour management**. Into an sRGB target that encodes twice and everything is too bright; give it a non-sRGB target, or composite in sRGB space |

## Slotting into an existing render graph

The order within a frame is fixed, and only one part of it is subtle:

```rust
// 1. CPU: advance every character of this target and build its geometry.
scene.update(&mut renderer, &device, &queue, dt, &mut players);

// 2. If this scene masks with the coverage backend, bake the bitmaps —
//    on the SAME encoder as the content pass below.
scene.encode_mask_coverage(&mut renderer, &device, &mut encoder);

// 3. Record draws into your pass, wherever it belongs in your graph.
let mut rpass = encoder.begin_render_pass(&descriptor);
scene.draw(&renderer, &mut rpass);
```

Step 2 is the subtle one: passes within a single `CommandEncoder` run in order, so the mask bitmap
and the transform that samples it are both this frame's. Splitting them across encoders, systems or
frames reintroduces the one-frame mask lag this player does not have.

If other systems draw into the same target, the ordering between their encoder and yours must be an
**explicit edge**, not a shared set — two systems that each take a render context get their own
encoder, and which buffer reaches the queue first is otherwise a race. (The Bevy adapter's draw
system is ordered explicitly after `main_transparent_pass_2d` for exactly this reason.)

## Several targets, one device

`SSScene` is per render target, and they share the device-scoped renderer:

```rust
let mut renderer = SSRenderer::new(&device);        // once
let mut main   = SSScene::new(&device, &main_config);
let mut minimap = SSScene::new(&device, &minimap_config);
```

Both draw the same packs from one atlas upload. A regression test
(`tests/pack.rs::scenes_share_one_renderer`) pins that.

!!! note "One `update` per character per frame"
    `SSScene::update` fuses the timeline advance with the geometry build. Passing the same character
    to two scenes' `update` in one frame advances it **twice**. Drawing one cast into two views is
    therefore not supported yet — the Bevy adapter skips later views and warns.

## Why there is no vertex buffer

The runtime's structure-of-arrays data is copied into the scene's arena and expanded by the vertex
stage from `@builtin(vertex_index)`. The CPU writes one 80-byte primitive per part and nothing per
vertex.

What that means for you: the cost of a character scales with its part count, not with the geometry
those parts expand into, and there is no vertex buffer for you to supply, share or resize.

## Device loss

Rebuild outward — renderer, then scenes, then characters — because a character registers its
atlases *into* the renderer. See [Device loss](../guide/device-loss.md).

## Next

* [Engine integration](./engines.md) — what an adapter has to do, and the wgpu version pin
* [Performance](../guide/performance.md) — the counters this model makes cheap
