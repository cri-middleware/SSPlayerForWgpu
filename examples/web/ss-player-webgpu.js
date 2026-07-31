// A `<ss-player-webgpu>` custom element: renders a SpriteStudio `.ssab` to a
// WebGPU canvas. Attributes: `src` (.ssab URL, embedded or reference), `pack`
// (name override; default is the name inside the .ssab), `anim` (animation name,
// default = first), `pause-when-hidden`
// (default on — opt out with `pause-when-hidden="false"`).
//
// This is the **convenience wrapper**: one element = one canvas = one player,
// which is the shortest path for a single-character use like an ad creative
// (SPEC §10). It is not the shape of the API — underneath, the element takes a
// surface from the page's shared context and adds one player to it, so putting
// ten of these on a page still costs one `GPUDevice` and one atlas upload. For a
// canvas with several characters, drive `SsContextWeb` directly (see index.html).
//
// Build the wasm package first with ./build.sh (outputs ./pkg).
import { sharedContext, restoreSharedDevice } from './ss-context.js';

export class SsPlayerWebGPU extends HTMLElement {
  connectedCallback() {
    this.canvas = document.createElement('canvas');
    // Backing-store resolution = CSS size × devicePixelRatio for crisp output.
    const dpr = window.devicePixelRatio || 1;
    const w = this.clientWidth || 360;
    const h = this.clientHeight || 360;
    this.canvas.width = Math.round(w * dpr);
    this.canvas.height = Math.round(h * dpr);
    this.canvas.style.width = '100%';
    this.canvas.style.height = '100%';
    this.canvas.style.display = 'block';
    this.appendChild(this.canvas);

    // --- Visibility gating (SPEC §14) -------------------------------------
    // Assume on-screen until the observer says otherwise, so a host without
    // IntersectionObserver (or with the gating off) always runs.
    this._intersecting = true;
    this._restoring = false;
    this._raf = 0;
    this._wantTicker = false;
    this._ensureVisibilityWatch();

    this._start();
  }

  disconnectedCallback() {
    this._wantTicker = false;
    this._syncTicker();
    if (this._io) { this._io.disconnect(); this._io = null; }
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
  }

  /**
   * Pause the rAF loop while the canvas is scrolled off-screen or the tab is in
   * the background — battery on mobile, and the viewability rule ad creatives are
   * measured against. Mirrors the WebGL player's `SSSurface` gating; no-op when
   * `pause-when-hidden="false"` or the APIs are missing (then it simply always runs).
   */
  _ensureVisibilityWatch() {
    this._gated = (this.getAttribute('pause-when-hidden') || '').trim().toLowerCase() !== 'false';
    if (!this._gated) return;

    if (typeof IntersectionObserver !== 'undefined') {
      // Until the first callback we don't know where we are; assume hidden so an
      // off-screen element never paints an initial frame.
      this._intersecting = false;
      this._io = new IntersectionObserver((entries) => {
        this._intersecting = entries[entries.length - 1].isIntersecting;
        this._syncTicker();
      });
      this._io.observe(this.canvas);
    }
    this._onVisibilityChange = () => this._syncTicker();
    document.addEventListener('visibilitychange', this._onVisibilityChange);
  }

  /** Whether the rAF loop should run right now. */
  get _shouldRun() {
    if (!this._wantTicker || this._restoring || this.ctx?.isDeviceLost()) return false;
    // With the gating off nothing listens for visibility, so nothing would ever
    // restart the loop — don't let it stop on visibility either.
    return !this._gated || (this._intersecting && !document.hidden);
  }

  /** Start or stop the rAF loop to match `_shouldRun` (idempotent). */
  _syncTicker() {
    if (this._shouldRun && !this._raf) {
      // Reset the clock so the first frame after a pause uses a normal delta
      // instead of the whole idle gap.
      this._last = performance.now();
      const loop = (now) => {
        const dt = Math.min((now - this._last) / 1000, 0.05); // clamp long gaps
        this._last = now;
        try {
          this.surface.frame(dt);
        } catch (e) {
          this._fail(e);
          return;
        }
        // The browser can take the device at any time (backgrounded tab, GPU
        // process crash). Rebuild rather than leaving a blank canvas forever.
        if (this.ctx.isDeviceLost()) {
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
  async _restoreDevice() {
    if (this._restoring) return;
    this._restoring = true;
    try {
      await restoreSharedDevice(this.ctx);
    } catch (e) {
      this._fail(e);
      return;
    } finally {
      this._restoring = false;
    }
    this._syncTicker();
  }

  async _start() {
    try {
      this.ctx = await sharedContext();
      // Disconnected while the context was initialising: don't take a swapchain
      // nobody will release.
      if (!this.isConnected) return;
      this.surface = this.ctx.createSurface(this.canvas);
      this.player = this.surface.createPlayer();
      const src = this.getAttribute('src');
      if (src) await this.loadSrc(src, this.getAttribute('pack'), this.getAttribute('anim'));
      this._wantTicker = true;
      this._syncTicker();
    } catch (e) {
      this._fail(e);
    }
  }

  /** Fetch + play a `.ssab`, embedded or reference. Can be called to switch packs. */
  async loadSrc(src, pack, anim) {
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
    const names = this.player.animationNames();
    // Selecting and starting are separate calls (SPEC §9.3): `setAnimation` is
    // the allocating step, `play` is transport. The old single `play(name)` is
    // deliberately not carried over.
    this.player.setAnimation(anim || names[0]);
    this.player.play();
    return names;
  }

  _fail(err) {
    this._wantTicker = false;
    this._syncTicker();
    console.error('[ss-player-webgpu]', err);
    this.innerHTML = `<div style="font:14px system-ui;color:#c0392b;padding:1em">⚠️ ${err}</div>`;
  }
}

customElements.define('ss-player-webgpu', SsPlayerWebGPU);
