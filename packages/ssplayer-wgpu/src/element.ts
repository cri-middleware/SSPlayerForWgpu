// A `<ss-player-webgpu>` custom element: renders a SpriteStudio `.ssab` to a
// WebGPU canvas. Attributes: `src` (.ssab URL, embedded or reference), `pack`
// (name override; default is the name inside the .ssab), `animation` (default =
// first), `autoplay` (start playing once loaded; absent = hold the first frame),
// `speed-scale`, `fit` / `align` / `flip-x` / `flip-y` (how the authored canvas
// fits the element's box; `fit="none"` draws 1:1 with the origin centred), and
// `pause-when-hidden` (default on — opt out with `pause-when-hidden="false"`).
// It dispatches the family's six events as DOM events: `animationstarted`,
// `animationfinished`, `animationlooped`, `userdata`, `signal` and `audio`; and
// `load` once a pack has loaded and its animation is set up, which is how a page
// that changed `src` learns the new pack's animation names.
//
// This is the **convenience wrapper**: one element = one canvas = one player,
// which is the shortest path for a single-character use like an ad creative
// (SPEC §10). It is not the shape of the API — underneath, the element takes a
// surface from the page's shared context and adds one player to it, so putting
// ten of these on a page still costs one `GPUDevice` and one atlas upload. For a
// canvas with several characters, drive `SsContextWeb` directly.
//
// Registration is opt-in (`defineSsPlayerWebGPUElement()`), not a side effect of
// the import: a bundle that only wants the context API must not be made to carry
// an element it never puts on a page.
import { sharedContext, restoreSharedDevice } from './index.js';
import type { SsContextWeb, SsSurfaceWeb, SsPlayerWeb } from '../wasm/ssplayer_wgpu.js';

/** The attributes that only move the character, so a change re-fits and nothing else. */
const FIT_ATTRIBUTES = ['fit', 'align', 'flip-x', 'flip-y'];

export class SsPlayerWebGPUElement extends HTMLElement {
  /**
   * What a page may change on a live element. `autoplay` is read when an
   * animation is set up, and `pause-when-hidden` once at connect.
   */
  static get observedAttributes(): string[] {
    return ['src', 'pack', 'animation', 'speed-scale', ...FIT_ATTRIBUTES];
  }

  canvas?: HTMLCanvasElement;
  ctx?: SsContextWeb;
  surface?: SsSurfaceWeb | null;
  player?: SsPlayerWeb | null;

  private _intersecting = true;
  private _restoring = false;
  private _gated = true;
  private _raf = 0;
  private _wantTicker = false;
  private _last = 0;
  private _io: IntersectionObserver | null = null;
  private _ro: ResizeObserver | null = null;
  private _onVisibilityChange: (() => void) | null = null;
  private _loads: Promise<unknown> = Promise.resolve();
  /**
   * The box the character is fitted into: the canvas of the animation the pack
   * was loaded on, kept across `animation` switches so the character does not
   * rescale when one animation's canvas is wider than another's. A new pack
   * picks it again.
   */
  private _frame: { x: number; y: number; width: number; height: number } | undefined;

  connectedCallback(): void {
    // A custom element is `display: inline` unless the page says otherwise, and an
    // inline box ignores width/height — so `<ss-player-webgpu style="width:360px">`
    // would collapse to a sliver and the canvas inside it would size off nothing.
    // Fill in only what the page left alone: an author who chose a display
    // (inline-block in a text run, a flex item, none) keeps it.
    if (!this.style.display && getComputedStyle(this).display === 'inline') {
      this.style.display = 'block';
    }

    const canvas = document.createElement('canvas');
    canvas.style.width = '100%';
    canvas.style.height = '100%';
    canvas.style.display = 'block';
    this.appendChild(canvas);
    this.canvas = canvas;
    this._applySize();
    // A page that sizes the element in `%` / `vw` / a flex track resizes it
    // whenever the window does, and a swapchain does not follow on its own: the
    // backing store would keep the size it had at connect time and the browser
    // would stretch it. Watching the element rather than the window also catches
    // a layout change the window never saw.
    if (typeof ResizeObserver !== 'undefined') {
      this._ro = new ResizeObserver(() => this._applySize());
      this._ro.observe(this);
    }

    // --- Visibility gating (SPEC §14) -------------------------------------
    // Assume on-screen until the observer says otherwise, so a host without
    // IntersectionObserver (or with the gating off) always runs.
    this._intersecting = true;
    this._restoring = false;
    this._raf = 0;
    this._wantTicker = false;
    this._ensureVisibilityWatch();

    void this._start();
  }

  disconnectedCallback(): void {
    this._wantTicker = false;
    this._syncTicker();
    if (this._io) {
      this._io.disconnect();
      this._io = null;
    }
    if (this._ro) {
      this._ro.disconnect();
      this._ro = null;
    }
    if (this._onVisibilityChange) {
      document.removeEventListener('visibilitychange', this._onVisibilityChange);
      this._onVisibilityChange = null;
    }
    // The surface and the player live in the page's shared context, not in these
    // wrappers, so dropping the references would leak a swapchain per element
    // that ever existed. The context itself stays — other elements share it.
    this.player?.destroy();
    this.surface?.destroy();
    this.player = this.surface = null;
    // The canvas goes with them. Moving an element in the DOM disconnects and
    // reconnects it, and `connectedCallback` builds a canvas each time — leaving
    // this one would stack a dead swapchain's canvas under the live one on every
    // move.
    this.canvas?.remove();
    this.canvas = undefined;
  }

  /**
   * Match the backing store to the CSS box × `devicePixelRatio`, and tell the
   * surface — which reconfigures the swapchain and the projection, so the
   * animation keeps its authored size in CSS pixels instead of being stretched.
   *
   * Idempotent, because a `ResizeObserver` fires for changes that leave the
   * pixel size where it was.
   */
  private _applySize(): void {
    const canvas = this.canvas;
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round((this.clientWidth || 360) * dpr));
    const h = Math.max(1, Math.round((this.clientHeight || 360) * dpr));
    if (canvas.width === w && canvas.height === h) return;
    canvas.width = w;
    canvas.height = h;
    // Before `_start` finishes there is no surface yet; it configures itself
    // from the canvas it is handed, which is now the right size.
    this.surface?.resize(w, h);
    this._fit();
  }

  attributeChangedCallback(name: string, oldValue: string | null, value: string | null): void {
    // Before `_start` has a player there is nothing to change: it reads every
    // attribute itself once the player exists.
    if (!this.player || oldValue === value) return;
    if (FIT_ATTRIBUTES.includes(name)) {
      this._fit();
    } else if (name === 'speed-scale') {
      this.player.setSpeedScale(this._speedScale());
    } else if (name === 'src' || name === 'pack') {
      const src = this.getAttribute('src');
      if (src) void this.loadSrc(src, this.getAttribute('pack'), this.getAttribute('animation')).catch((e) => this._fail(e));
    } else if (name === 'animation' && value) {
      // Behind any load in flight: the player is lifted out of the context while
      // one runs, and the switch belongs to the pack that load leaves behind.
      this._loads = this._loads.then(() => this._switchAnimation(value)).catch((e) => this._fail(e));
    }
  }

  /**
   * Fit the character into the canvas by the `fit` / `align` / `flip-x` /
   * `flip-y` attributes, against the box picked at load. An unknown `fit` or
   * `align` throws, naming what it accepts, and fails the element rather than
   * drawing somewhere the page did not ask for.
   */
  private _fit(): void {
    const player = this.player;
    const canvas = this.canvas;
    if (!player || !canvas) return;
    try {
      player.fitInto(canvas.width, canvas.height, {
        fit: this.getAttribute('fit') || 'contain',
        align: this.getAttribute('align') || 'center',
        flipX: this.hasAttribute('flip-x'),
        flipY: this.hasAttribute('flip-y'),
        frame: this._frame,
      });
    } catch (e) {
      this._fail(e);
    }
  }

  /** `speed-scale` as a number; absent or unreadable is the authored speed. */
  private _speedScale(): number {
    const value = Number(this.getAttribute('speed-scale') ?? '1');
    return Number.isFinite(value) ? value : 1;
  }

  /**
   * Select another animation of the loaded pack. With `autoplay` it plays from
   * the head, as it did when the pack loaded; without, it holds its first frame.
   * The box stays the one the pack loaded on.
   */
  private _switchAnimation(name: string): void {
    const player = this.player;
    if (!player || player.currentAnimation() === name) return;
    player.setAnimation(name);
    if (this.hasAttribute('autoplay')) player.play();
  }

  /**
   * Re-dispatch what the last frame did as DOM events, in the order the Bevy
   * adapter raises its messages: the start edge, the timeline events, then the
   * completion and loop edges. The stems are the family's (`animationFinished`,
   * `userData`, …), lower-cased as DOM event names are; the timeline events
   * carry `player.events()`'s objects as `detail`. They do not bubble.
   */
  private _dispatchFrame(): void {
    const player = this.player;
    if (!player) return;
    if (player.justStarted()) this._emit('animationstarted');
    for (const event of player.events()) this._emit(String(event.kind).toLowerCase(), event);
    if (player.justFinished()) this._emit('animationfinished');
    if (player.justLooped()) this._emit('animationlooped');
  }

  private _emit(type: string, detail?: unknown): void {
    this.dispatchEvent(new CustomEvent(type, { detail }));
  }

  /**
   * Pause the rAF loop while the canvas is scrolled off-screen or the tab is in
   * the background — battery on mobile, and the viewability rule ad creatives are
   * measured against. Mirrors the WebGL player's `SSSurface` gating; no-op when
   * `pause-when-hidden="false"` or the APIs are missing (then it simply always runs).
   */
  private _ensureVisibilityWatch(): void {
    this._gated = (this.getAttribute('pause-when-hidden') || '').trim().toLowerCase() !== 'false';
    if (!this._gated) return;

    if (typeof IntersectionObserver !== 'undefined') {
      // Until the first callback we don't know where we are; assume hidden so an
      // off-screen element never paints an initial frame.
      this._intersecting = false;
      this._io = new IntersectionObserver((entries) => {
        this._intersecting = entries[entries.length - 1]!.isIntersecting;
        this._syncTicker();
      });
      this._io.observe(this.canvas!);
    }
    this._onVisibilityChange = () => this._syncTicker();
    document.addEventListener('visibilitychange', this._onVisibilityChange);
  }

  /** Whether the rAF loop should run right now. */
  private get _shouldRun(): boolean {
    if (!this._wantTicker || this._restoring || this.ctx?.isDeviceLost()) return false;
    // With the gating off nothing listens for visibility, so nothing would ever
    // restart the loop — don't let it stop on visibility either.
    return !this._gated || (this._intersecting && !document.hidden);
  }

  /** Start or stop the rAF loop to match `_shouldRun` (idempotent). */
  private _syncTicker(): void {
    if (this._shouldRun && !this._raf) {
      // Reset the clock so the first frame after a pause uses a normal delta
      // instead of the whole idle gap.
      this._last = performance.now();
      const loop = (now: number): void => {
        const dt = Math.min((now - this._last) / 1000, 0.05); // clamp long gaps
        this._last = now;
        try {
          this.surface!.frame(dt);
        } catch (e) {
          this._fail(e);
          return;
        }
        this._dispatchFrame();
        // The browser can take the device at any time (backgrounded tab, GPU
        // process crash). Rebuild rather than leaving a blank canvas forever.
        if (this.ctx!.isDeviceLost()) {
          this._raf = 0;
          void this._restoreDevice();
          return;
        }
        this._raf = this._shouldRun ? requestAnimationFrame(loop) : 0;
      };
      this._raf = requestAnimationFrame(loop);
    } else if (!this._shouldRun && this._raf) {
      cancelAnimationFrame(this._raf);
      this._raf = 0;
    }
  }

  /**
   * Recover from a lost WebGPU device: the context re-requests adapter+device and
   * rebuilds every GPU object on the page from its retained source, so playback
   * resumes on the frame it was interrupted on. Shared across elements — the
   * device is the page's, so the first one to notice recovers for everyone.
   */
  private async _restoreDevice(): Promise<void> {
    if (this._restoring) return;
    this._restoring = true;
    try {
      await restoreSharedDevice(this.ctx!);
    } catch (e) {
      this._fail(e);
      return;
    } finally {
      this._restoring = false;
    }
    this._syncTicker();
  }

  private async _start(): Promise<void> {
    try {
      this.ctx = await sharedContext();
      // Disconnected while the context was initialising: don't take a swapchain
      // nobody will release.
      if (!this.isConnected) return;
      this.surface = this.ctx.createSurface(this.canvas!);
      this.player = this.surface.createPlayer();
      // A setting holds from before the first animation and across a pack.
      this.player.setSpeedScale(this._speedScale());
      const src = this.getAttribute('src');
      if (src) await this.loadSrc(src, this.getAttribute('pack'), this.getAttribute('animation'));
      this._wantTicker = true;
      this._syncTicker();
    } catch (e) {
      this._fail(e);
    }
  }

  /**
   * Fetch + play a `.ssab`, embedded or reference. Can be called to switch packs.
   *
   * Calls are **serialised**: the player is lifted out of the shared context for
   * the duration of a load, so two in flight at once would race for the slot and
   * the context rejects the second (see `SsPlayerWeb::load`). A pack switch is
   * usually a user gesture — a dropdown, a scroll — and two of those in quick
   * succession must switch twice, not fail, so each waits for the one before it
   * and the last one called is the one left on screen.
   */
  loadSrc(src: string, pack?: string | null, animation?: string | null): Promise<string[] | undefined> {
    const run = this._loads.then(() => this._loadSrc(src, pack, animation));
    // What the next call waits on must settle, so the chain survives a load that
    // rejected. The caller still sees `run` itself, rejection and all.
    this._loads = run.catch(() => {});
    return run;
  }

  private async _loadSrc(src: string, pack?: string | null, animation?: string | null): Promise<string[] | undefined> {
    if (!this.player) return;
    // One call fetches the pack, its atlases and any sibling `.ssab` its instance
    // parts play from (SPEC §8.1, §7.1) — a URL answers both "what bytes" and
    // "what do its siblings resolve against". `pack` is a name override, which
    // `load` deliberately does not take; the rare host that needs one goes
    // through `loadSsab` with its own bytes.
    if (pack) {
      const url = new URL(src, location.href);
      const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer());
      await this.player.loadSsab(bytes, pack, url.href);
    } else {
      await this.player.load(src);
    }
    // Disconnected while the bytes were in flight: `disconnectedCallback` has
    // destroyed the player, and calling a destroyed handle is not a diagnostic
    // worth raising over an element that is no longer on the page.
    if (!this.player) return;
    const names = this.player.animationNames();
    // Selecting and starting are separate calls (SPEC §9.3): `setAnimation` is
    // the allocating step, `play` is transport. The old single `play(name)` is
    // deliberately not carried over.
    this.player.setAnimation(animation || names[0]!);
    this._frame = this.player.canvasRect() ?? undefined;
    this._fit();
    // `autoplay`, the HTML way: a boolean attribute is absent by default, so this
    // is opt-in — the same shape `<video autoplay>` has, and the same shape
    // `<ss-player>` (the WebGL element) already had. Without it the element holds
    // its first frame, poster-like; `draw` still renders it, since drawing is
    // independent of the transport.
    //
    // Sibling Players whose player is PLACED rather than constructed default the
    // other way (ForGodot / ForUnity autoplay on), because a scene node has no
    // code to call `play()` for it. An attribute is that code.
    if (this.hasAttribute('autoplay')) this.player.play();
    this._emit('load');
    return names;
  }

  private _fail(err: unknown): void {
    this._wantTicker = false;
    this._syncTicker();
    console.error('[ss-player-webgpu]', err);
    // textContent, not innerHTML: the message can carry a URL or a pack name that
    // came off an attribute, and an error path is a poor place to start parsing
    // markup.
    const box = document.createElement('div');
    box.style.cssText = 'font:14px system-ui;color:#c0392b;padding:1em';
    box.textContent = `⚠️ ${err}`;
    this.replaceChildren(box);
  }
}

/**
 * Register the `<ss-player-webgpu>` Custom Element. Opt-in (not called on import)
 * so the element stays tree-shakeable. Idempotent; pass a custom `tag` to avoid a
 * name clash with another SpriteStudio player build on the same page.
 */
export function defineSsPlayerWebGPUElement(tag = 'ss-player-webgpu'): void {
  if (typeof customElements === 'undefined') return;
  if (!customElements.get(tag)) customElements.define(tag, SsPlayerWebGPUElement);
}
