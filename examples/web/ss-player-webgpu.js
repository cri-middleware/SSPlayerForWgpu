// A `<ss-player-webgpu>` custom element: renders a SpriteStudio `.ssab` to a
// WebGPU canvas via the wasm-bindgen wrapper (`SsPlayerWeb`, from core's `web`
// feature). Attributes: `src` (embedded .ssab URL), `pack` (name, default from
// the file), `anim` (animation name, default = first), `pause-when-hidden`
// (default on — opt out with `pause-when-hidden="false"`).
//
// Build the wasm package first with ./build.sh (outputs ./pkg).
import init, { SsPlayerWeb } from './pkg/ssplayer_wgpu.js';

// The wasm module is initialised once and shared by every element on the page.
let wasmReady = null;
function ensureWasm() {
  if (!wasmReady) wasmReady = init();
  return wasmReady;
}

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
    if (!this._wantTicker || this._restoring || this.player?.isDeviceLost()) return false;
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
          this.player.frame(dt);
        } catch (e) {
          this._fail(e);
          return;
        }
        // The browser can take the device at any time (backgrounded tab, GPU
        // process crash). Rebuild rather than leaving a blank canvas forever.
        if (this.player.isDeviceLost()) {
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
   * Recover from a lost WebGPU device: the wrapper re-requests adapter+device and
   * rebuilds every GPU object from its retained source, so playback resumes on the
   * frame it was interrupted on.
   */
  async _restoreDevice() {
    if (this._restoring) return;
    this._restoring = true;
    try {
      await this.player.restoreDevice();
    } catch (e) {
      this._fail(e);
      return;
    } finally {
      this._restoring = false;
    }
    this._syncTicker();
  }

  async _start() {
    if (!('gpu' in navigator)) {
      this._fail('このブラウザは WebGPU 非対応です（Chrome / Edge 113+ など）。');
      return;
    }
    try {
      await ensureWasm();
      this.player = await SsPlayerWeb.create(this.canvas);
      const src = this.getAttribute('src');
      if (src) await this.loadSrc(src, this.getAttribute('pack'), this.getAttribute('anim'));
      this._wantTicker = true;
      this._syncTicker();
    } catch (e) {
      this._fail(e);
    }
  }

  /** Fetch + play an embedded `.ssab`. Can be called to switch animations. */
  async loadSrc(src, pack, anim) {
    if (!this.player) return;
    const bytes = new Uint8Array(await (await fetch(src)).arrayBuffer());
    const name = pack || src.split('/').pop().replace(/\.ssab$/, '');
    this.player.loadSsab(name, bytes);
    const names = this.player.animationNames();
    this.player.play(anim || names[0]);
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
