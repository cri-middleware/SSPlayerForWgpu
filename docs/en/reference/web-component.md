# `<ss-player-webgpu>` { #ss-player-webgpu }

The custom element is the **convenience wrapper**: one element = one canvas = one character, which
is the shortest path for a single-character use — an ad creative, a mascot in the corner of a page,
a character in a CMS template.

It is not a second implementation. Underneath, the element takes a surface from the page's shared
context and adds one player to it, so **ten of these on a page still cost one `GPUDevice`, one set
of pipelines and one atlas upload**.

## Registering it

```js
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';

defineSsPlayerWebGPUElement();                    // <ss-player-webgpu>
defineSsPlayerWebGPUElement('my-ss-player');      // ... or under a name of your own
```

Registration is **opt-in, not a side effect of the import**, so a bundle that only wants the
[JavaScript API](./js.md) never carries the element. The call is idempotent, and the `tag` argument
exists for a page that already has another SpriteStudio player registered under the default name.

```html
<ss-player-webgpu src="/assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

## Attributes

| Attribute | Meaning |
|---|---|
| `src` | URL of the `.ssab` — reference or embedded export, either works |
| `pack` | Pack-name override. Defaults to the name inside the `.ssab` |
| `animation` | Animation to play. Defaults to the first one the pack declares |
| `autoplay` | Start playing once loaded. A **boolean attribute**, so absent means no |
| `speed-scale` | A multiple of the authored rate. `0` and negatives stop |
| `fit` | `contain` (default) or `cover` fits the authored canvas into the element; `none` draws 1:1 with the origin at the centre |
| `align` | Where the fitted canvas sits: `center` (default), `top`, `bottom`, `left`, `right`, `top-left`, `top-right`, `bottom-left`, `bottom-right` |
| `flip-x`, `flip-y` | Mirror the character. Boolean attributes; the alignment follows them |
| `pause-when-hidden` | Stop the rAF loop while off-screen or backgrounded. **On by default** — opt out with `pause-when-hidden="false"` |

### Changing them on a live element

`src`, `pack`, `animation`, `speed-scale`, `fit`, `align`, `flip-x` and `flip-y` are watched:
setting one on an element that is already playing takes effect.

* A new `src` or `pack` loads that pack, as `loadSrc()` does, and the element dispatches `load`
  once it is set up.
* A new `animation` switches to it. With `autoplay` on, it plays from its head; without, it holds
  its first frame.
* `autoplay` is read whenever an animation is set up, and `pause-when-hidden` once, when the
  element connects.

### How the character fits the element

The element fits the animation's **authored canvas** into its box, placed by the pivot the artist
set, so a character authored standing on its origin lands inside the box rather than with its head
out of the top. It is `contain` and `center` unless `fit` and `align` say otherwise.

The box is the canvas of the animation the pack was **loaded on**, and it stays for every
`animation` switch after that: one pack's animations may differ in canvas (Ringo's `dead` is 1300
wide against 800), and a character that rescaled when it changed motion would look wrong. A new
`src` or `pack` picks the box again. `fit="none"` skips all of this and draws one authored unit per
canvas pixel with the origin at the centre.

### `autoplay` is opt-in, like `<video>`

Without it the element **loads and holds its first frame**, poster-like — drawing is independent of
the transport, so the character is on screen, not moving. That is the HTML convention (`<video
autoplay>`), and it matches `<ss-player>`, the element of SpriteStudio Player for Web's WebGL package.

(The *placed* players in the family, Godot and Unity, default the other way: a scene node has no
code to call `play()` for it, and an attribute **is** that code.)

## Properties and methods

The element does not hide the API — it exposes what it built, so anything on the
[JavaScript API](./js.md) is reachable:

| | |
|---|---|
| `element.player` | the `SsPlayerWeb` — part overrides, events, transport, everything |
| `element.surface` | the `SsSurfaceWeb` for this canvas — `stats()`, `resize()` |
| `element.ctx` | the page's shared `SsContextWeb` |
| `element.canvas` | the `<canvas>` the element created |

```js
const el = document.querySelector('ss-player-webgpu');
await el.loadSrc('/assets/Other.ssab');       // resolves to the animation names
el.player.setLoopCount(1);
el.player.setPartVisibilityOverride('hat', true, false);
```

### `loadSrc(src, pack?, animation?, read?)`

Fetches and plays a `.ssab`, and is how you switch packs at runtime. It resolves to the array of
animation names, so a UI can offer them.

It selects the animation (`animation`, or the first) and then honours `autoplay` exactly as the
initial load did — so an element without `autoplay` holds the new pack's first frame too. It does
not rewrite the element's attributes, so an element that is moved in the DOM (and so connects
again) loads what its `src` says; a page that moves elements sets the attribute instead.

Passing `pack` makes the element fetch the bytes itself and register them under that name, since
`load` takes no name override.

`read` stands in for `fetch`, as it does for `SsPlayerWeb.load`: it is how an element plays a
dropped folder. See [In a browser](../guide/loading.md#in-a-browser).

Calls are **serialised**: a player is lifted out of the shared context while it loads, so the
context rejects a second load started before the first settles. Switching packs is usually a user
gesture, and two in quick succession must switch twice rather than fail — so each waits for the one
before it, and the last one called is what stays on screen. The returned promise still rejects if
*that* load fails, and it is yours to catch: only a startup failure reaches the element's own error
box.

## Events

The element dispatches what playback did as DOM events on itself. They do not bubble.

| Event | When | `detail` |
|---|---|---|
| `animationstarted` | the first frame that ran after a `play` | — |
| `animationfinished` | the frame that completed playback. Never under an infinite loop, and not raised by `stop()` | — |
| `animationlooped` | the frame that crossed a loop boundary | — |
| `userdata` | a user-data key was crossed | `{ partIndex, integer?, rect?, point?, text? }` |
| `signal` | a signal key was crossed | `{ partIndex, commandId, params }` |
| `audio` | an audio key was crossed | `{ partIndex, soundName, loopNum, … }` |
| `load` | a pack has loaded and its animation is set up | — |

The first six are the family's required set, under the same stems every SpriteStudio Player uses
(`animationFinished`, `userData`, …), lower-cased as DOM event names are. The timeline events carry
the objects [`player.events()`](./js.md) returns, so a key crossed on a frame the tick skipped over
still arrives. The element never plays sound: see [Events](../guide/events.md) for when an `audio`
event is meant to.

```js
el.addEventListener('animationfinished', () => showNextButton());
el.addEventListener('userdata', (e) => console.log(e.detail.text));
```

## Layout and resolution

* A custom element is `display: inline` unless the page says otherwise, and an inline box ignores
  `width` / `height`. The element therefore **sets `display: block` only if the page left it
  `inline`** — an author who chose `inline-block`, a flex item, or `none` keeps that choice.
* The canvas fills the element (`100%` × `100%`).
* The backing store is **CSS size × `devicePixelRatio`**, so output is crisp on a HiDPI screen.
* Both are computed **when the element connects**, and a `ResizeObserver` on the element keeps them
  current: when the CSS box changes the element recomputes the backing store and calls
  [`SsSurfaceWeb.resize()`](./js.md) itself, so a panel that resizes needs no extra code. (Where
  `ResizeObserver` is unavailable the size stays at its connect-time value; `element.surface` still
  lets you drive `resize()` by hand.)
* Give the element a size. With none, it falls back to 360 × 360.

## Pausing when nobody is looking

`pause-when-hidden` is on by default and gates the animation frame loop on **two** signals: an
`IntersectionObserver` on the canvas, and the document's `visibilitychange`. It exists for battery
on mobile, and for the viewability rule ad creatives are measured against.

Two behaviours to expect:

* **Until the first observer callback the element assumes it is hidden**, so an element that starts
  off-screen never paints an initial frame it would immediately throw away.
* Where the APIs are missing — or with the gating turned off — the loop simply always runs, rather
  than never starting.

The frame delta is clamped to 50 ms, and the clock is reset after a pause, so returning to a
backgrounded tab does not advance the animation by the whole idle gap.

## Device loss, handled for you

The element checks for a lost device every frame and calls `restoreSharedDevice` itself. Since the
device belongs to the *page*, **the first element to notice recovers for every element on it**, and
playback resumes on the frame it was interrupted on. See [Device loss](../guide/device-loss.md).

## Errors

Anything thrown during startup, loading or a frame is caught: the element logs to the console under
`[ss-player-webgpu]` and replaces its own content with a plain error box. The message is written as
text, never as markup — it can carry a URL or a pack name that came off an attribute, and an error
path is a poor place to start parsing HTML.

The most common cause is a browser without WebGPU. If you want a designed fallback rather than the
default box, use [`sharedContext()`](./js.md) directly and `catch`.

## Lifecycle

Removing the element from the DOM destroys its player and its surface — **not** the page's context,
which the other elements share. That teardown is why the element cannot just drop its references: a
surface holds a swapchain, and one per element that ever existed would leak.

## When to use the API instead

Reach for [`SsContextWeb`](./js.md) directly when you want:

* **several characters in one canvas** — the element is one canvas, one character;
* your own frame loop, or the animation driven by something other than `requestAnimationFrame`;
* control over when the device is created, or a designed fallback where WebGPU is missing;
* a canvas you already own and manage.

Everything the element does is written on top of that API, and the two mix freely on one page.
