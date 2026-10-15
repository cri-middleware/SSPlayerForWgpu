# Limitations & Scope

The constraints that change what you build. Each links to the page that explains it in full; planned
work lives in the [ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md),
and what each face exposes today is the matrix in
[Scopes and names](./guide/concepts.md#what-each-face-can-do).

| Constraint | Affects | Explained in |
|---|---|---|
| The C face **builds and owns its device**, and draws only to the window it was given — not into a render pass of yours, and not yet into a texture | C / C++ | [below](#no-device-sharing-across-the-c-abi) |
| **`wgpu` is pinned to the 30 line** — your application and the player resolve to one `wgpu` | Rust, Bevy | [Requirements](./start/rust.md#requirements) |
| **MSAA must be off**, and an sRGB target encodes the atlas a second time | Rust, Bevy | [What your render pass has to be](./wgpu/borrow-model.md#what-your-render-pass-has-to-be) |
| **One `update` per character per frame** — one cast cannot be drawn into two views | Rust, Bevy | [Several targets, one device](./wgpu/borrow-model.md#several-targets-one-device) |
| **WebGPU only**, with no OpenGL / WebGL backend — a browser without it needs a fallback | Web | [Requirements](./start/web.md#requirements) |
| **`Div2` / `Overlay2` draw as `Mix`** — no GPU blend state reproduces them | every face | [below](#div2-and-overlay2-draw-as-mix) |
| **`Mulalpha` / `Mul2` draw as `Mul`, and `Screen2` as `Screen`** | every face | [below](#div2-and-overlay2-draw-as-mix) |
| **Whole-character alpha applies part by part** — where two parts overlap, `set_alpha(0.5)` leaves 0.75 | every face | [below](#whole-character-alpha-applies-part-by-part) |
| **Integration shaders draw only on Normal parts with no mask**, and their extra cells (`map0` / `map1`) are not bound | every face | [below](#integration-shaders) |
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

From C, compositing with an engine's own drawing would take offscreen rendering plus a shared
texture, and this face **does not offer that yet**: a scene renders only to the window or layer it
was created on. Until it does, the route is a window or layer of the player's own — a child window,
or a `CAMetalLayer` placed in your layer tree. The [ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) item
says what an offscreen target needs.

## `Div2` and `Overlay2` draw as `Mix`

Every blend mode is drawn as a fixed-function GPU blend state. `Div2` and `Overlay2` have no such
state to map to, so a part set to either draws as `Mix`.

Matching them waits on a definition of their maths rather than on wgpu; the
[ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) item says what it needs.

Three more are drawn with a neighbour's state: `Mulalpha` and `Mul2` as `Mul`, and `Screen2` as
`Screen`. Nobody has compared those three against the editor, so treat them as approximate.

## Whole-character alpha applies part by part

`set_alpha` fades each part as it draws, not the character as a whole, so where two parts overlap
the one behind shows through the one in front: two halves of a character at 0.5 leave the overlap
at 0.75, and a fading character shows its own seams. Drawing the character whole and compositing it
once fixes that, and needs an offscreen path the player does not have yet; it is a
[ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) item.

Until then, fade something that is composited once: on the web, the canvas, with CSS `opacity`; on
the Rust face, a texture of your own that you drew the character into.

## Integration shaders

The `ss-*` integration shaders draw on Normal parts that no mask clips. A masked part, or a part of
another kind, draws without its shader — the masked case matches the WebGL player. A shader's extra
cells (`map0` / `map1`) are not bound, and a shader id the player does not know draws plain.
