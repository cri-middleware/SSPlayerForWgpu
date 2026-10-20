# Performance

How many characters can you put on screen? That depends on your content, so the player reports the
numbers to answer it with.

## What a character costs

Three things hold whatever you draw:

* **The CPU cost is per part, not per vertex.** There is no vertex buffer: the CPU writes one
  primitive per part and the vertex stage expands it. A character with 200 quad parts is 200 small
  writes; a mesh part adds a copy of its vertices and indices, with no per-vertex math.
* **An atlas is uploaded once per device.** The texture map is device-scoped, so the same pack shown
  five times is uploaded once.
* **A target's whole cast is one render pass.** Every character goes through one `update` and one
  pass, so a second character costs a playback tree and nothing on the GPU.

## A scene with nothing moving

When no character in a scene has anything to move, `update` keeps the frame it built last: nothing
is evaluated, rebuilt or uploaded, and `draw` draws the same frame again. A character has nothing to
move when it is stopped, has finished, or is paused with no independently played effect or instance
in it, and when no track fade or other track is still moving its pose (see
[Motion blending](./motion-blending.md)).

One character that moves rebuilds the whole frame, because every character in a scene shares one
buffer. A call that changes what a character draws — a track or part override, a seek, a new
placement — rebuilds it once. Writing the same placement, alpha, tint or visibility again costs
nothing, and neither does the scene's `set_viewport` with the viewport it already has, so a host may
write them every frame. `uploaded_bytes` reads `0` on an update that kept
the frame.

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
| `draw_calls` | draws recorded | ✅ | ✅ | ✅ |
| `uploaded_bytes` | bytes written to the frame's storage buffers | ✅ | ✅ | ✅ |
| `mergeable_draws` | how much of the draw list could merge | ✅ | — | ✅ |
| `redundant_texture_binds` | draws that re-bound the texture already bound | ✅ | — | ✅ |
| `coverage_layers` | mask coverage passes this frame | ✅ | — | — |
| `group_layers` | characters faded as one layer this frame | ✅ | ✅ | ✅ |
| `gpu_pass_time_ns` | GPU time for the last timed pass | ✅ | ✅ | ✅ |

The fastest way to see these on your own content is to drop a `.ssab` on
`cargo run -p ssplayer-wgpu-viewer`: both native viewers print them every second.

`gpu_pass_time_ns` needs a device created with the timestamp-query feature — in a browser, one the
browser granted `timestamp-query` to. Without it you get `None` / `0` / `undefined` and the other
counters still work. It arrives a frame or two late, because reading it sooner would mean stalling
the pipeline.

## What costs the most

| | |
|---|---|
| **The coverage mask backend** | one extra render pass **per masking character per frame**, and one more per 24 mask writers past the first 24 — the largest per-character cost here, and why [coverage is not used where a stencil exists](./masks.md) |
| **Many distinct atlases** | each one is a bind group change. `redundant_texture_binds` says whether draw order is working against you |
| **Group alpha** | with it on, one render pass and one composite over the target **per faded character per frame**, and a layer the size of the target for each one faded at once. `group_layers` counts them; a character at alpha 1 costs nothing. See [Whole-character tint and alpha](./part-overrides.md#whole-character-tint-and-alpha) |
| **Very many characters** | the per-character CPU walk is single-threaded. Copies of one character playing in step are a [crowd](./crowds.md), which costs one character's walk |
| **Off-screen characters** | `set_culled(true)` skips the timeline advance as well as the draw, so a culled character costs nothing. Use it for cast members that are off-screen. `set_visible(false)` is not a substitute: it skips only the draw, and the timeline keeps running |

## If the numbers look wrong

`mergeable_draws` and `redundant_texture_binds` are reported so you can check your own scene against
the assumptions the renderer makes. Draw-call merging, bind-group dedup, render bundles and
bindless textures were measured against a 256-character frame and are not implemented; the reasoning
is in the
[ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) under *Deliberate
non-goals*. If those two counters are large for your content, that is worth an issue.

## Next

* [Masks](./masks.md) — the backend choice that dominates the table above
* [Playback and transport](./playback.md) — `set_visible`, `set_culled` and what each one skips
