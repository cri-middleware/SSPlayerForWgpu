# Limitations & Scope

What this player does **not** do, and why — the technical constraints, not the to-do list. Planned
and unverified work lives in the
[ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md); what each face
exposes today is the matrix in [Scopes and names](./guide/concepts.md#what-each-face-can-do).

## Scope

* **SpriteStudio 7 `.ssab` only.** `.ssfb` / `.ssfb2` from the SS6 players cannot be read; reconvert
  from the `.sspj`. See [Migrating from SS6Player for CPP](./migration/from-ss6-cpp.md).
* **Conversion is ahead of time.** Nothing here converts a `.sspj` at runtime and no converter is
  bundled. See [Preparing assets](./start/assets.md).
* **The player produces no sound.** An audio event says what to play and how many times; feeding an
  audio stack is the application's job.

## Graphics

### No device sharing across the C ABI

The C face **builds and owns its graphics device** on the window handle you give it. It cannot draw
into a command list or render pass of yours.

wgpu types have no stable C representation, so there is nothing to pass across the boundary.
Embedding the player in an engine that owns a device is therefore a **Rust-only** capability,
delivered as [adapter crates](./wgpu/engines.md).

From C, the route to compositing is offscreen rendering plus a shared texture.

### wgpu is pinned to the 29 line

A Cargo workspace resolves exactly one `wgpu`, and two majors are two unrelated sets of types.
`bevy_render` 0.19 requires `wgpu ^29.0.3`, and Slint's `unstable-wgpu-NN` requires an exact major —
so the pin is a compatibility contract shared by everything in the repository, not a preference.

### MSAA must be off

The player's pipelines are single-sampled. A multisampled target is refused rather than drawn
wrongly (the Bevy adapter warns once and skips).

### No colour management

Atlas bytes are written verbatim into whatever target you provide, exactly as the WebGL reference
player does. Into an sRGB target that encodes them a second time and everything is too bright — give
the player a non-sRGB target, or composite in sRGB space. An HDR (`Rgba16Float`, linear-light)
target is not tone-mapping-correct for this output.

### No OpenGL / WebGL backend

The renderer pulls its vertices from storage buffers bound to the vertex stage, which OpenGL exposes
only from 4.3 / GLES 3.1 and WebGL not at all. No GL adapter is selected or verified anywhere: the
instance takes wgpu's primary backends (Vulkan, Metal, DX12, WebGPU), and the C API's backend
selector offers DX12, Metal and Vulkan.

**Where WebGPU is missing, the answer is [`ssplayer-webgl`](https://github.com/cri-middleware/SSPlayerForWeb)**,
which is maintained and is this player's rendering-parity reference. A browser without WebGPU is a
`catch` away from that fallback — see [Get started on the web](./start/web.md#requirements).

### Masks: 24 writers

The coverage-bitmap backend carries one bit per mask writer, 24 of them — the same limit the other
SpriteStudio players implement. See [Masks](./guide/masks.md).

## Playback

### One `update` per character per frame

`SSScene::update` fuses the timeline advance with the geometry build, so passing the same character
to two scenes in one frame would advance its animation **twice**. Drawing one cast into two views
(a main view and a minimap of the same characters) is therefore not available; the Bevy adapter
skips later views with a warning.

Several scenes on one device are fine — it is one *character* in two scenes that is not.

### Instance children are not layered

A part that plays another pack's animation runs its own playback context, driven by its slot on the
root's frame. The [track stack](./guide/motion-blending.md) applies to the character whose player
you are holding, not to its instance children.

### Crossfading does not rebuild the instance tree

That is what makes it a fade rather than a switch. Crossfading to an animation whose **instance
parts differ** is out of scope — use `set_animation` and accept the cut.

### No label-named playback sections

Sections are set by frame number (`set_animation_section(start, end)`). The SS6 player's
`labelRangeStart` / `labelRangeEnd` label strings have no equivalent.

### No per-part flip override

Flipping follows the authored keyframes; there is no `set_part_flip_override`.

### No effect seed control

An effect's random seed is decided per playback slot. There is no equivalent of SS6's
`SeedOffsetSet()`.

## Distribution

* **Releases ship static libraries only** for the C face — `libssplayer_wgpu.a` /
  `ssplayer_wgpu.lib`. Shipping the shared library alongside would add code signing, SONAMEs and
  import-library pairing to every release for a second way to do the same thing. `build.sh` still
  produces the shared library for local work.
* **Nothing is published yet.** crates.io, npm and the release zips each wait on something named on
  their [Get started](./start/rust.md) page — while the SDK repository is private, every route is a
  build from source with access to it.
