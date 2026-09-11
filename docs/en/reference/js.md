---
title: JavaScript API reference
---

# JavaScript API reference

The package ships full type definitions (`dist/index.d.ts`), so an editor will complete everything
below. This page is the map.

```js
import { sharedContext, restoreSharedDevice, setWasmUrl } from '@spritestudio/ssplayer-wgpu';
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';
```

The element lives behind its own entry point, so a bundle that only wants the API never carries it.

## Module functions

| | |
|---|---|
| `sharedContext(): Promise<SsContextWeb>` | The page's one context — device, pipelines, atlas cache. Call it as often as you like; you get the same one |
| `restoreSharedDevice(ctx): Promise<void>` | Rebuild everything on a new device after a loss |
| `setWasmUrl(url)` | Point at the wasm binary explicitly, before the first `sharedContext()` |
| `defineSsPlayerWebGPUElement()` | Register `<ss-player-webgpu>`. Opt-in, not a side effect of the import |

## `SsContextWeb` — the page

| | |
|---|---|
| `createSurface(canvas)` | One per `<canvas>` |
| `addTexture(hash, rgba, width, height)` | Supply an atlas as raw RGBA8 |
| `addTextureFromBytes(hash, bytes): Promise` | ... from encoded bytes, decoded by the browser |
| `addTextureFromBitmap(hash, imageBitmap)` | ... from an `ImageBitmap` you already have |
| `hasTexture(hash)` | Whether the cache already holds one |
| `isDeviceLost()` / `restoreDevice()` | See [Device loss](../guide/device-loss.md) |
| `surfaceCount()` | Diagnostic |

## `SsSurfaceWeb` — one canvas

| | |
|---|---|
| `createPlayer()` | Add a character |
| `frame(dt)` | Update every player on this canvas and draw them in one pass. `dt` in **seconds** |
| `resize(width, height)` | Drawable pixels |
| `stats()` | `{ players, prims, drawCalls, uploadedBytes, mergeableDraws, redundantTextureBinds, gpuPassTimeNs? }` |
| `playerCount()` | Diagnostic |
| `destroy()` | Explicit and idempotent — dropping the JS reference would leave a swapchain behind |

## `SsPlayerWeb` — one character

**Loading** — the `load` family is the only asynchronous part of the API, and a player takes **one
load at a time**: starting a second before the first settles is rejected, so `await` it or chain
the calls.

| | |
|---|---|
| `load(src): Promise` | Pack, atlases, sibling packs — from a URL |
| `loadSsab(bytes, pack?, baseUrl?): Promise` | Register bytes and select that pack |
| `addSsab(bytes, pack?, baseUrl?): Promise` | Register bytes only |
| `missingAssets()` | What did not arrive. **Not a rejection** |
| `textureRefs(pack?)` / `embeddedTextures(pack?)` | What atlases a pack asks for |
| `animationNames()` / `partNames()` / `cellmapNames()` / `cellNames(cellmap)` | What the pack offers |
| `canvasSize()` | The authored canvas |

**Transport** — see [Playback and transport](../guide/playback.md).

| | |
|---|---|
| `setAnimation(name)` | Select. Synchronous, and the allocating call |
| `play()` / `playFrom(frame)` | **Head-out** — rewinds and re-arms the loop count |
| `pause()` / `resume()` / `stop()` | Hold, carry on in place, end |
| `seek(frame)` / `frameNo()` | Fractional frames are honoured |
| `setSpeedScale(s)` | A multiple of the authored rate. `0` and negatives stop |
| `setLoopCount(n)` / `loopCount()` / `remainingLoops()` | `n` cycles; `-1` or `0` forever |
| `setAnimationSection(start, end)` | After `setAnimation` |
| `setPlaybackDirection(dir, style)` / `playbackDirection()` / `playbackStyle()` | `0`/`1` each |
| `setFrameRate(fps)` / `frameRate()` | Replace the authored rate |
| `setSubFrame(on)` / `subFrameEnabled()` | Interpolate between authored frames |
| `setSkipFrames(on)` / `skipFrames()` | Cross several frames in one long tick |
| `startFrame()` / `endFrame()` / `totalFrames()` | The current range |
| `isPlaying()` / `isPausing()` / `isPlayingForward()` / `isFinished()` | State |
| `justLooped()` / `justFinished()` | Edges from the last `frame()` |

**Placement**

| | |
|---|---|
| `setTransform(x, y, scaleX, scaleY, rotation)` / `resetTransform()` | |
| `setOrder(n)` | Draw order within the canvas, ascending |
| `setVisible(v)` | Hidden characters **do not advance** and report no events |
| `setAlpha(a)` / `setTint(rgb)` | Whole-character |
| `destroy()` | Explicit and idempotent |

**Parts, overrides and events** — every override has a `ByIndex` twin; resolve once with
`findPartIndex(name)`.

| | |
|---|---|
| `setPartVisibilityOverride(part, hide, cascade)` / `clearPartVisibilityOverride(part)` | |
| `setPartColorOverride(part, rgba, blendOp, priority)` / `setPartColorOverrideCorners(...)` / `clearPartColorOverride(part)` | `blendOp` 0–3, `priority` 0–2 |
| `setPartCellOverride(part, cellmap, cell, priority)` / `clearPartCellOverride(part)` | |
| `clearAllPartOverrides()` | |
| `partTransform(part)` / `partTransformByIndex(i)` | `[m00, m01, m10, m11, tx, ty]` |
| `isPartHidden(part)` / `isPartSkinnedMesh(part)` / `partCount()` | `isPartSkinnedMesh` → `true`/`false`, or `null` for an unknown part |
| `events()` | Plain objects tagged by `kind`: `'userData'` / `'signal'` / `'audio'` |

## `<ss-player-webgpu>`

Documented in full on [its own page](./web-component.md); the attributes, in short:

| Attribute | Meaning |
|---|---|
| `src` | URL of the `.ssab` |
| `pack` | Pack-name override |
| `anim` | Animation to play; defaults to the first |
| `autoplay` | Boolean attribute — **absent means no**, and the element holds its first frame |
| `pause-when-hidden` | On by default; opt out with `"false"` |

`element.loadSrc(src, pack?, anim?)` switches packs at runtime and resolves to the animation names.

## Not on this face

**Motion blending** (the track stack) is Rust-only. See
[Scopes and names](../guide/concepts.md#what-each-face-can-do) for the full matrix.
