# How the player is put together

For whoever works on SpriteStudio Player for wgpu, or changes it for a host of their own. It names
files and assumes a checkout; [For contributors](index.md) builds and verifies one, and
[`CONTRIBUTING.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/CONTRIBUTING.md) has
the constraints. This page is the map across them.

What every SpriteStudio Player shares — how playback, motion blending, sound keys and part tracking
behave — is written once, in the SDK's
[Player Porting Guide](https://github.com/cri-middleware/SpriteStudio-SDK/tree/HEAD/libs/ssruntime/docs).

## The map

The runtime is the SDK's `ssruntime` crate, linked as a path dependency into the `SpriteStudio-SDK`
submodule together with `ssab`, the `.ssab` schema. It advances the timeline and hands back each
frame as a draw plan and flat buffers; `libs/ssplayer-wgpu` draws that plan with wgpu, on a device
the host owns.

### Three types, split by how long their resources live

| Type | One per | Holds | File |
|---|---|---|---|
| `SSRenderer` | device | Pipelines, bind group layouts, the sampler, the shader modules, the atlas textures | `renderer.rs` |
| `SSScene` | render target | The projection and the frame's geometry: one `FrameChunk` — arena, primitives, draw calls — that every player drawing into the target appends to | `scene.rs` |
| `SSPlayer` | character | The playback tree: one runtime `Context` per animation instance. No GPU resource | `player.rs` |

The host lends the device, the queue, the renderer and the target every frame; none of the three
owns them.

### The rest of the crate

| File | What it does |
|---|---|
| `pack.rs` | `SSPack`, one loaded `.ssab` shared by any number of players. CPU data only |
| `assets.rs`, `decode.rs` | The filesystem behind `SSPlayer::load`, and atlas decoding spread over short-lived threads |
| `events.rs` | Timeline events, decoded into owned values |
| `mask.rs` | Which parts write mask coverage and how; a port of ssplayer-webgl's `MaskPlanner` |
| `fit.rs` | `fit` / `align`: placing an animation in a box the host laid out |
| `metrics.rs` | What a frame cost: counts, and GPU pass time where timestamp queries exist |
| `frame.rs` | A borrowed view of one frame's runtime buffers |
| `roster.rs` | Generation-tagged slots for the handles the hosts hand out |

### The faces

| Face | Files | What it is |
|---|---|---|
| Rust | `lib.rs` and the modules above | The API: three types the host drives |
| C | `host.rs`, `ffi.rs`, `libs/ssplayer-wgpu-ffi/` | `NativeHost` owns a wgpu stack built from a raw window handle; `ffi.rs` only translates the C ABI to it; the `-ffi` crate builds the library and regenerates `include/ssplayer_wgpu.h` |
| JS | `web.rs`, `packages/ssplayer-wgpu/` | The wasm-bindgen host on WebGPU, driven by `requestAnimationFrame`, and the TypeScript layer over it |
| Both C and JS | `surface.rs` | The swapchain the two faces that own their surface configure, resize and draw into alike |
| Bevy | `libs/ssplayer-bevy/` | Components in Bevy's main world; the playback trees, one `SSRenderer` and one `SSScene` in its render world |

`examples/` has a host for each face: `hello` (the smallest Rust host), `viewer` and the Bevy
viewers, the Metal, DX12 and Vulkan C++ viewers, and `web`.

## Why it is built the way it is

Each reason here spans files; the code it points at carries the detail.

**The player borrows; the host owns.** A game engine already has a device, a queue and a frame of
its own, so the player takes all three as loans (`SSRenderer`, `SSScene`, `SSPlayer` in `lib.rs`).
That is why one crate draws into a bare viewer, a browser canvas and Bevy alike, and why the types
split by resource lifetime: what is valid for the device, for one target, and for one character.

**A pack holds no GPU texture.** Textures belong to the host's asset lifecycle — in Bevy,
`Assets<Image>` and the render world's `GpuImage` — so an `SSPack` describes the textures it needs
and hands over their pixels, and whoever owns GPU memory registers them with the renderer
(`pack.rs`, `renderer.rs`, `ssplayer-bevy`'s `asset.rs`).

**The CPU does not transform corners.** Each drawn part's slice of the runtime's flat arrays is
copied into the frame arena, with one `Prim` record per part saying where its geometry sits, and the
vertex stage reads both (`Prim` in `scene.rs`, which must stay in step with the `Prim` comment in
the WGSL). There is no per-vertex work on the CPU and no vertex buffer to rebuild.

**The C face is translation and nothing else.** Every entry point in `ffi.rs` validates its
arguments, converts them and calls `NativeHost` or `SSPlayer`; behaviour lives in `host.rs` and
`player.rs`, where the Rust API gets it too. The swapchain steps the C face and each web canvas share
are in `surface.rs` for the same reason: a fix that reached one copy of two is what it prevents.

**Two mask backends.** The host's stencil (`MaskBackend::Stencil`) limits nesting to 8 bits; the
coverage bitmap the player rasterises for itself lifts that limit and needs no stencil attachment,
which a host such as Bevy's 2D pass does not have (`MaskBackend` in `scene.rs`). Both stop at 96
writers across a player's instance tree, as the Godot Player does. A mask feature goes into both.

**The drawing code came from the WebGL player, and the rules come from the porting guide.** The
blend table, the PartsColor program, the integration shaders and the mask planner were ported from
`ssplayer-webgl`, and the comments that say so name what each came from. What a `.ssab` should look
like is the porting guide's, and where the guide is silent, this Player's: it is the one the other
Players are read against (SpriteStudio-SDK `MAINTAINING_PLAYERS.md` §5). A change to how something
draws is checked against the guide, and the rule it settles goes into the guide in the same change.
