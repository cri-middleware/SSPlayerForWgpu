# Engine integration

Embedding the player in an engine that already owns a `wgpu::Device` is a **Rust-only** capability,
and it is delivered as an *adapter crate* rather than as a feature of the player.

## Why adapters are separate crates

`ssplayer-wgpu` depends on no engine, and must not. Two reasons, both practical:

* an engine dependency in the player crate would put every adopter behind that engine's release
  schedule, whether or not they use it;
* an adapter can follow its engine's line — including its wgpu major, if that ever diverges from
  the player's — without dragging the player along.

So `ssplayer-bevy` lives beside the player, versioned independently, and any future adapter
does the same.

## The wgpu pin

**wgpu is pinned to the 29 line, repository-wide.**

A Cargo workspace resolves exactly one `wgpu`, and two majors of wgpu are two unrelated sets of
types with no glue between them. So the pin is not a preference, it is the compatibility contract:

| Constraint | |
|---|---|
| `bevy_render` 0.19 | requires `wgpu ^29.0.3` |
| Slint's `unstable-wgpu-NN` | requires an **exact** wgpu major, and a Vtuber application composes a Slint UI and this player on one device |

Check Slint's supported major before bumping. A mismatch is a compile error rather than a runtime
surprise, which is the one good thing about it.

## What an adapter has to do

The Bevy adapter is the worked example; anything else follows the same five points.

1. **Own an `SSRenderer` at device scope** — one per engine device, not per view or per character.
2. **Own an `SSScene` per render target**, built against that target's colour format, and rebuild
   it when the format changes (an HDR camera, a different swapchain).
3. **Run `update` and `draw` inside the engine's own pass**, with an *explicit* ordering edge
   against whatever else draws into the target. Two systems that each open an encoder race
   otherwise.
4. **Choose the mask backend from what the pass can lend**, not from the animation: a stencil
   attachment means `Stencil`; no stencil means `Coverage`, whose bake must go on the same encoder
   as the content pass. See [Masks](../guide/masks.md).
5. **Plug the engine's asset system into `load_with`** rather than reading the filesystem, if it has
   one — see [Loading and assets](../guide/loading.md).

Then map the engine's lifecycle onto the player's: device loss (rebuild outward), window resize
(reproject), and whatever the engine calls "this entity was despawned".

## Case study: Bevy

Bevy renders in two worlds, and the adapter is split along that seam: the three components live in
the main world, extraction copies them and the frame delta into the render world, and the playback
trees, the renderer and the scene all live on the render side. The player never owns Bevy's device —
the draw system lends it for the frame, which is the same borrow model the winit viewer and the C++
hosts honour.

Three decisions worth copying:

* **Coverage masking**, because Bevy's 2D depth is `Depth32Float` and carries no stencil. Nothing
  is asked of Bevy's attachments and the draw stays inside Bevy's own pass.
* **An explicit `after(main_transparent_pass_2d)` edge**, not merely membership in the same set.
* **Its own transform component** rather than Bevy's `Transform`, because decomposing a 3D affine
  into the player's five scalars is lossy. An application that wants the two linked writes the one
  system that copies across — and keeps the choice of which is authoritative.

See [the Bevy reference](../reference/bevy.md).

## Slint, egui, macroquad, your own renderer

There is nothing to adapt: these hand you a device and a render pass, which is exactly what the
[borrow model](./borrow-model.md) asks for. Keep one `SSRenderer` beside your device, one `SSScene`
per target, and call `update` / `draw` in your frame.

The Slint case is the reason the wgpu pin exists at all: composing a Slint UI and this player on one
device requires Slint's `unstable-wgpu-<major>` feature to name the same major the player resolves.

## What is not possible

**An engine cannot share its device with the player through the C ABI.** wgpu types do not cross a C
boundary — there is no stable representation of a `Device` or a `Queue` to pass — so engine
embedding is a Rust-only capability by construction, not by omission.

From C the route to compositing is offscreen rendering plus a shared texture: let the player own its
device, render into a texture, and bring that texture into your engine by whatever interop it
already has. See [Limitations](../limitations.md#no-device-sharing-across-the-c-abi).
