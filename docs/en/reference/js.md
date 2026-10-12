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
| `addTexture(key, rgba, width, height)` | Supply an atlas as raw RGBA8, under a `textureRefs` entry's `key` |
| `addTextureFromBytes(key, bytes): Promise` | ... from encoded bytes, decoded by the browser |
| `addTextureFromBitmap(key, imageBitmap)` | ... from an `ImageBitmap` you already have |
| `hasTexture(key)` | Whether the cache already holds one |
| `isDeviceLost()` / `restoreDevice()` | See [Device loss](../guide/device-loss.md) |
| `surfaceCount()` | Diagnostic |
| `version` / `sdkVersion` | This player's version, and the SpriteStudio-SDK release it links: its tag, or `local` for an SDK that is not a release. Read-only; `element.ctx` reaches them from the custom element |

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
| `load(src, read?): Promise` | Pack, atlases, sibling packs — from a URL. `read(url)` answers in place of `fetch`; see [In a browser](../guide/loading.md#in-a-browser) |
| `loadSsab(bytes, pack?, baseUrl?, read?): Promise` | Register bytes and select that pack |
| `addSsab(bytes, pack?, baseUrl?, read?): Promise` | Register bytes only |
| `missingAssets()` | What did not arrive. **Not a rejection** |
| `textureRefs(pack?)` / `embeddedTextures(pack?)` | What atlases a pack asks for |
| `animationNames()` / `partNames()` / `cellmapNames()` / `cellNames(cellmap)` | What the pack offers |
| `currentAnimation()` | The one set up, or `undefined` |
| `canvasSize()` | The authored canvas |

**Transport** — see [Playback and transport](../guide/playback.md).

| | |
|---|---|
| `setAnimation(name)` | Select. Synchronous, and the allocating call |
| `play()` / `playFrom(frame)` | **Head-out** — rewinds and re-arms the loop count |
| `pause()` / `resume()` / `stop()` | Hold, carry on in place, end |
| `seek(frame)` / `frameNo()` | Fractional frames are honoured |
| `setSpeedScale(s)` / `speedScale()` | A multiple of the authored rate. `0` and negatives stop |
| `setLoopCount(n)` / `loopCount()` / `remainingLoops()` | `n` cycles; `-1` or `0` forever |
| `setAnimationSection(start, end)` | After `setAnimation` |
| `setPlaybackDirection(dir, style)` / `playbackDirection()` / `playbackStyle()` | `0`/`1` each |
| `setFrameRate(fps)` / `frameRate()` | Replace the authored rate |
| `setSubFrameEnabled(on)` / `subFrameEnabled()` | Interpolate between authored frames |
| `setFrameSkipEnabled(on)` / `frameSkipEnabled()` | Cross several frames in one long tick |
| `startFrame()` / `endFrame()` / `totalFrames()` | The current range |
| `isPlaying()` / `isPausing()` / `isPlayingForward()` / `isFinished()` | State |
| `justStarted()` / `justLooped()` / `justFinished()` | Edges from the last `frame()`. `justStarted` is not raised by `resume()` |

**Placement**

| | |
|---|---|
| `setTransform(x, y, scaleX, scaleY, rotation)` / `resetTransform()` | |
| `setOrder(n)` / `order()` | Draw order within the canvas, ascending |
| `setVisible(v)` / `isVisible()` | Hidden characters draw nothing but keep playing and reporting events |
| `setCulled(c)` / `isCulled()` | Culled characters **do not advance** and report no events |
| `setAlpha(a)` / `setTint(rgb)` | Whole-character. Read back with `alpha()` / `tint()` |
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
| `isPartHidden(part)` / `isPartSkinnedMesh(part)` / `partCount()` | `isPartHidden` and `isPartSkinnedMesh` → `true`/`false`, or `undefined` for an unknown part |
| `events()` | Plain objects tagged by `kind`: `'userData'` / `'signal'` / `'audio'`. Each carries `frameNo`, `partName` and `source`, the handle of the source that fired it |

**Texture replacement** — swap the whole texture a cell map binds (a costume or palette change).
`pack` null selects the main pack. See [Texture replacement](../guide/texture-replacement.md).

| | |
|---|---|
| `replaceableCellmapNames(pack?)` | The replaceable names: cell maps and TextureChange targets both |
| `setCellmapTexture(pack, name, imageBitmap)` | Replace from a **premultiplied** `ImageBitmap` |
| `setCellmapTextureFromBytes(pack, name, bytes): Promise` | …or from encoded bytes, decoded by the browser |
| `restoreCellmapTexture(pack, name)` | Put the original atlas back |
| `isCellmapOverridden(pack, name)` | Whether one is currently overridden |

### Motion blending { #motion-blending }

The track stack: tracks fold onto the setup pose in ascending order, track 0 being what
`setAnimation` establishes. See [Motion blending](../guide/motion-blending.md).

| | |
|---|---|
| `setTrackAnimation(track, name)` | Lay an animation on a track, replacing what it played |
| `crossfade(track, name, ms)` | Fade a track over to another animation |
| `fadeTrack(track, target, ms)` / `cancelTrackFade(track)` | Drive a track's alpha somewhere / stop the fade where it stands |
| `trackFadeProgress(track)` | 0 to 1; `undefined` when the track is not fading |
| `setTrackAlpha(track, a)` / `trackAlpha(track)` | A track's strength |
| `setTrackBlend(track, b)` / `trackBlend(track)` | `0` mix or `1` additive |
| `setTrackSync(track, s)` / `trackSync(track)` | `0` free or `1` start — set it before laying |
| `setTrackFrame` / `trackFrame` / `setTrackSpeed` | Scrub a track; speed 0 parks it |
| `trackCount()` / `clearTrack(track)` | How many tracks; silence one (not track 0) |
| `trackMainSource(track)` | The source carrying a track's animation |
| `addTrackSource(track, name, weight)` | Lay another animation beside it, mixed by weight; returns its handle |
| `setSourceWeight` / `setSourceFrame` / `setSourceEventThreshold`, and the reads without `set` | One source's weight, playhead and event threshold |

A read of a track or source that does not exist answers `undefined`; a command on one throws.

## `<ss-player-webgpu>` { #ss-player-webgpu }

Documented in full on [its own page](./web-component.md); the attributes, in short:

| Attribute | Meaning |
|---|---|
| `src` | URL of the `.ssab` |
| `pack` | Pack-name override |
| `animation` | Animation to play; defaults to the first |
| `autoplay` | Boolean attribute — **absent means no**, and the element holds its first frame |
| `pause-when-hidden` | On by default; opt out with `"false"` |
| `speed-scale` | A multiple of the authored rate |
| `fit`, `align`, `flip-x`, `flip-y` | How the authored canvas fits the element: `contain` and `center` by default, `fit="none"` for 1:1 |

`element.loadSrc(src, pack?, animation?, read?)` switches packs from script and resolves to the animation names. It dispatches `animationstarted`, `animationfinished`, `animationlooped`, `userdata`, `signal` and `audio` as DOM events, and `load` once a pack has loaded; the attributes above can be changed on a live element. Everything `SsPlayerWeb` offers, motion blending included, is on `element.player`.
