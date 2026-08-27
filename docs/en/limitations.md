# Limitations & Scope

The constraints that change what you build. Each links to the page that explains it in full; planned
work lives in the [ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md),
and what each face exposes today is the matrix in
[Scopes and names](./guide/concepts.md#what-each-face-can-do).

| Constraint | Affects | Explained in |
|---|---|---|
| The C face **builds and owns its device**, and cannot draw into a render pass of yours | C / C++ | [below](#no-device-sharing-across-the-c-abi) |
| **`wgpu` is pinned to the 29 line** — your application and the player resolve to one `wgpu` | Rust, Bevy | [Requirements](./start/rust.md#requirements) |
| **MSAA must be off**, and an sRGB target encodes the atlas a second time | Rust, Bevy | [What your render pass has to be](./wgpu/borrow-model.md#what-your-render-pass-has-to-be) |
| **One `update` per character per frame** — one cast cannot be drawn into two views | Rust, Bevy | [Several targets, one device](./wgpu/borrow-model.md#several-targets-one-device) |
| **WebGPU only**, with no OpenGL / WebGL backend — a browser without it needs a fallback | Web | [Requirements](./start/web.md#requirements) |
| **Audio events are cues**; the player produces no sound | every face | [Events and user data](./guide/events.md) |

Coming from SS6Player for CPP? The calls with no equivalent here — label-named sections, per-part
flip, effect seeds — are listed in
[Things SS6 had that work differently](./migration/from-ss6-cpp.md#8-things-ss6-had-that-work-differently).

## No device sharing across the C ABI

The C face **builds and owns its graphics device** on the window handle you give it. It cannot draw
into a command list or render pass of yours.

wgpu types have no stable C representation, so there is nothing to pass across the boundary.
Embedding the player in an engine that owns a device is therefore a **Rust-only** capability,
delivered as [adapter crates](./wgpu/engines.md).

From C, the route to compositing is offscreen rendering plus a shared texture.
