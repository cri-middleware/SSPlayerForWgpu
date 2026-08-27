# Masks

A mask part clips what it covers — a character seen through a keyhole, a health bar filling, a
sprite revealed by a shape. The animator authors it; the player has to clip with something the GPU
offers, and **which something depends on your render target, not on the animation**.

The same `.ssab` looks the same either way; what changes is what you have to provide.

## The two backends

| Backend | Needs from you | Costs | Available on |
|---|---|---|---|
| **Stencil** | a depth/stencil attachment on every pass you draw into | nothing extra | Rust, C, JavaScript |
| **Coverage bitmap** | one call between update and draw | **one render pass per masking character per frame** | Rust, Bevy |

Coverage exists for a render pass that has no stencil to lend. Bevy's 2D depth texture is
`Depth32Float`, which carries none; an engine composing its own passes may expose no depth at all.
Where a stencil *is* available it is the cheaper answer, which is why coverage is not the default.

## What each face does

=== "Rust"

    You choose, in `SceneConfig`:

    ```rust
    // Stencil — you attach a matching depth/stencil buffer to every pass.
    SSScene::new(&device, &SceneConfig {
        color_format,
        stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8),
        mask: MaskBackend::Stencil,
    });

    // Coverage — nothing is asked of your attachments.
    SSScene::new(&device, &SceneConfig {
        color_format,
        stencil_format: None,
        mask: MaskBackend::Coverage,
    });

    // No masking at all: mask parts draw nothing, masked targets draw unclipped.
    SSScene::new(&device, &SceneConfig { color_format, stencil_format: None, mask: MaskBackend::None });
    ```

    `MaskBackend::Stencil` without a `stencil_format` cannot work, and is refused rather than
    silently drawing every masked part unclipped — the scene falls back to `None`.

=== "C"

    One flag on the scene descriptor. The stencil buffer is allocated for you.

    ```c
    ss_scene_desc_t desc = { /* ... */ .enable_mask = true };
    ```

    With `enable_mask = false` the scene runs without masking, exactly as if no stencil were
    attached. The coverage backend is not exposed on this face — it exists for hosts that own
    their own render pass, and this face owns yours.

=== "JavaScript"

    Nothing to configure. Each surface allocates its own depth/stencil texture and uses the
    stencil backend.

=== "Bevy"

    Nothing to configure — and it is the **coverage** backend, because Bevy's 2D depth carries no
    stencil. `SceneStats::coverage_layers` is what that costs.

## The coverage rule that matters

If you drive the coverage backend yourself (a Rust host, or an engine adapter), there is exactly
one ordering rule:

```rust
scene.update(&mut renderer, &device, &queue, dt, &mut players);
scene.encode_mask_coverage(&mut renderer, &device, &mut encoder);  // <- same encoder
let mut rpass = encoder.begin_render_pass(&descriptor);            // <- as this one
scene.draw(&renderer, &mut rpass);
```

**The bake and the pass that samples it go on one `CommandEncoder`.** Passes in one encoder run in
order, so the bitmap and the transform that samples it are both *this* frame's and cannot disagree.

That ordering is the whole guarantee: split the bake into another encoder, another system or
another frame and the mask a character is clipped by is the previous frame's — visible as the clip
trailing fast movement by one frame.

Under `MaskBackend::Stencil` the call is a harmless no-op, so it is safe to leave in a host that
can switch backends at runtime.

## Stacking characters in one pass

Every character **unwinds the stencil writers it applied** before the next one draws. Without that,
putting two characters in one render pass would let the first one's mask clip the second.

You get this without asking. It is worth pointing out only because of when it would otherwise
appear: one character per pass with `LoadOp::Clear(0)` cannot show the bug, so it would surface on
the day you merge your cast into a single pass.

## Limits

* The coverage bitmap carries **one bit per mask writer, 24 writers** — the same limit the other
  SpriteStudio players implement.
* Mask parts are clipping shapes, not colours: what they draw is the clip, never pixels.

## Next

* [Performance](./performance.md) — `coverage_layers`, and what a masking character costs
* [The borrow model](../wgpu/borrow-model.md) — why the choice belongs to your render target
