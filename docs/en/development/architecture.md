# How the Player Is Put Together

For whoever works on SpriteStudio Player for wgpu, or changes it for a host of their own. It
describes the parts by what they do; the [Build Guide](build.md) builds and verifies a checkout,
uses a changed Player in a host and checks it on real hosts, and
[`CONTRIBUTING.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/CONTRIBUTING.md) has
the constraints.

What every SpriteStudio Player shares — how playback, motion blending, sound keys and part tracking
behave — is written once, in the SDK's
[Player Porting Guide](https://github.com/cri-middleware/SpriteStudio-SDK/tree/HEAD/libs/ssruntime/docs).

## Layers

The runtime is the SDK's `ssruntime` crate, linked as a path dependency into the `SpriteStudio-SDK`
submodule together with `ssab`, the `.ssab` schema. It advances the timeline and hands back each
frame as a draw plan and flat buffers; the `ssplayer-wgpu` crate draws that plan with wgpu, on a
device the host owns.

### Three types, split by how long their resources live

| Type | One per | Holds |
|---|---|---|
| `SSRenderer` | device | Pipelines, bind group layouts, the sampler, the shader modules, the atlas textures |
| `SSScene` | render target | The projection and the frame's geometry: one `FrameChunk` — arena, primitives, draw calls — that every player drawing into the target appends to |
| `SSPlayer` | character | The playback tree: one runtime `Context` per animation instance. No GPU resource |

The host lends the device, the queue, the renderer and the target every frame; none of the three
owns them.

Beside them, `SSPack` is one loaded `.ssab` shared by any number of players, CPU data only, and
`SSPlayer::load` reads it from the filesystem and decodes its atlases over short-lived threads. The
crate also decodes timeline events into owned values, plans which parts write mask coverage and how
(a port of `ssplayer-webgl`'s mask planner), places an animation in a box the host laid out
(`fit` / `align`), reports what a frame cost (`SceneStats`: counts, and GPU pass time where
timestamp queries exist), lends a view of one frame's runtime buffers (`FrameView`), and keeps
generation-tagged slots for the handles the hosts hand out.

### The faces

| Face | What it is |
|---|---|
| Rust | The API: the three types the host drives |
| C | `NativeHost` owns a wgpu stack built from a raw window handle, and the C entry points only translate the C ABI to it; the `ssplayer-wgpu-ffi` crate builds the library and regenerates `ssplayer_wgpu.h` |
| JS | The wasm-bindgen host on WebGPU, driven by `requestAnimationFrame`, and the TypeScript layer over it in the `@spritestudio/ssplayer-wgpu` package |
| Both C and JS | The swapchain that the two faces owning their surface configure, resize and draw into alike |
| Bevy | The `ssplayer-bevy` crate: components in Bevy's main world; the playback trees, one `SSRenderer` and one `SSScene` in its render world |

`examples/` has a host for each face: `hello` (the smallest Rust host), `viewer` and the Bevy
viewers, the Metal, DX12 and Vulkan C++ viewers, and `web`.

## Why it is built the way it is

Each reason here spans files; the code it concerns carries the detail in its comments.

**The player borrows; the host owns.** A game engine already has a device, a queue and a frame of
its own, so the player takes all three as loans (`SSRenderer`, `SSScene`, `SSPlayer`). That is why
one crate draws into a bare viewer, a browser canvas and Bevy alike, and why the types split by
resource lifetime: what is valid for the device, for one target, and for one character.

**A pack holds no GPU texture.** Textures belong to the host's asset lifecycle — in Bevy,
`Assets<Image>` and the render world's `GpuImage` — so an `SSPack` describes the textures it needs
and hands over their pixels, and whoever owns GPU memory registers them with the renderer.

**The CPU does not transform corners.** Each drawn part's slice of the runtime's flat arrays is
copied into the frame arena, with one primitive record per part saying where its geometry sits, and
the vertex stage reads both; the record's layout in Rust and in the WGSL must stay in step. There is
no per-vertex work on the CPU and no vertex buffer to rebuild.

**The C face is translation and nothing else.** Every C entry point validates its arguments,
converts them and calls `NativeHost` or `SSPlayer`; behaviour lives behind those two, where the Rust
API gets it too. The swapchain steps the C face and each web canvas share are kept in one place for
the same reason: a fix that reached one copy of two is what it prevents.

**Two mask backends.** The host's stencil (`MaskBackend::Stencil`) limits nesting to 8 bits; the
coverage bitmap the player rasterises for itself lifts that limit and needs no stencil attachment,
which a host such as Bevy's 2D pass does not have. Both stop at 96 writers across a player's
instance tree, as the Godot Player does. A mask feature goes into both.

**The drawing code came from the WebGL player, and the rules come from the porting guide.** The
blend table, the PartsColor program, the integration shaders and the mask planner were ported from
`ssplayer-webgl`, and the comments that say so name what each came from. What a `.ssab` should look
like is the porting guide's, and where the guide is silent, this Player's: it is the one the other
Players are read against (SpriteStudio-SDK `MAINTAINING_PLAYERS.md` §5). A change to how something
draws is checked against the guide, and the rule it settles goes into the guide in the same change.
