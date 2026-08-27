# Performance

Start from the assumption that **there is no performance problem until a number says otherwise**.
This page is about getting that number, and about the handful of things that genuinely cost.

## Why it is usually fast

Three structural facts, none of which you have to do anything to get:

* **There is no vertex buffer.** The CPU writes one 80-byte primitive per part and the vertex stage
  expands it from `@builtin(vertex_index)`. Nothing is written per vertex, so a character with 200
  parts is 200 small writes, not 800 vertices.
* **One atlas upload per device.** The texture map is device-scoped, so the same pack shown five
  times is uploaded once — five characters, one upload.
* **One render pass for the whole cast.** Every character of a target goes through one `update` and
  one pass; a second character costs a playback tree.

The remaining cost is expressiveness, not throughput.

## Reading the counters

=== "Rust"

    ```rust
    let s = scene.stats();
    println!("{} players, {} prims, {} draws, {} bytes", s.players, s.prims, s.draw_calls, s.uploaded_bytes);
    ```

=== "C"

    ```c
    ss_scene_stats_t s;
    ss_scene_stats(scene, &s);
    ```

=== "JavaScript"

    ```js
    const s = surface.stats();   // { players, prims, drawCalls, uploadedBytes, ... }
    ```

| Counter | Means | Rust | C | JS |
|---|---|:--:|:--:|:--:|
| `players` | characters that contributed geometry | ✅ | ✅ | ✅ |
| `prims` | parts, shape fans and effect particles in the frame | ✅ | ✅ | ✅ |
| `draw_calls` | draws recorded. The gap from `prims` is merging headroom | ✅ | ✅ | ✅ |
| `uploaded_bytes` | bytes written to the frame's storage buffers | ✅ | ✅ | ✅ |
| `mergeable_draws` | how much of the draw list *could* merge | ✅ | — | ✅ |
| `redundant_texture_binds` | draws that re-bound the texture already bound | ✅ | — | ✅ |
| `coverage_layers` | mask coverage passes this frame — the cost of the coverage backend | ✅ | — | — |
| `gpu_pass_time_ns` | GPU time for the last timed pass | ✅ | ✅ | ✅ |

Both native viewers print these every second, so the fastest way to see them on your own content is
to drop a `.ssab` on `cargo run -p ssplayer-wgpu-viewer`.

## GPU timing

`gpu_pass_time_ns` needs the device to have been created with the timestamp-query feature — on the
web, the browser must have granted `timestamp-query`. Where it is missing you get `None` / `0` /
`undefined`, and the other counters still work.

It is read back **a frame or two late, by design**: making it current would mean stalling the
pipeline, which would change the thing being measured. Do not add a stall to make it current.

## Measuring a change

Two rules, both learned the hard way:

1. **Measure before and after.** A structural change that "must be faster" often is not.
2. **Drive a real frame loop.** Interleave `update` with the render pass, exactly as a frame does.
   Timing them in separate loops makes anything that caches across frames look far better than it
   is.

`cargo test --test scale -- --ignored` is the repository's own throughput harness and does both.
It is `#[ignore]`d because CI's software GPU cannot judge speed.

!!! warning "Beware green rendering tests"
    The offscreen rendering tests skip themselves when no GPU adapter is present, so a pass on a
    machine without one may have verified nothing. When you change rasterisation, dump the readback
    to a PNG and look at it — pixel counts alone have hidden real regressions.

## What actually costs

| | |
|---|---|
| **The coverage mask backend** | one extra render pass **per masking character per frame**. This is the biggest per-character cost in the library, and it is why [coverage is not the default](./masks.md) where a stencil exists |
| **Many distinct atlases** | each is a bind group change. `redundant_texture_binds` tells you whether draw order is fighting you |
| **Very many characters** | the per-character CPU walk is single-threaded, by design — see below |
| **Hiding vs. not drawing** | `set_visible(false)` skips the timeline advance entirely, so a culled character costs nothing. Use it for off-screen cast members |

## What was measured and deliberately declined

These are not oversights, and adding them has been considered against a 256-character frame:

* draw-call merging, bind-group dedup, render bundles, indirect draws for shapes;
* bindless textures — additionally unavailable on WebGPU;
* threading the per-frame character walk. (Atlas *decoding* at load time **is** threaded, in a
  scope that is gone before the call returns.)

The reasoning for each is in the
[ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) under *Deliberate
non-goals*. `mergeable_draws` and `redundant_texture_binds` exist so that you can check the premise
yourself on your own content: if they are large for your scene, that is worth a bug report.

## Next

* [Masks](./masks.md) — the backend choice that dominates the cost table above
* [The borrow model](../wgpu/borrow-model.md) — where each cost lives, and why
