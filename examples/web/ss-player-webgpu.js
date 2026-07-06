// A `<ss-player-webgpu>` custom element: renders a SpriteStudio `.ssab` to a
// WebGPU canvas via the wasm-bindgen wrapper (`SsPlayerWeb`, from core's `web`
// feature). Attributes: `src` (embedded .ssab URL), `pack` (name, default from
// the file), `anim` (animation name, default = first).
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
    this._start();
  }

  disconnectedCallback() {
    if (this._raf) cancelAnimationFrame(this._raf);
    this._raf = null;
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
        this._raf = requestAnimationFrame(loop);
      };
      this._raf = requestAnimationFrame(loop);
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
    console.error('[ss-player-webgpu]', err);
    this.innerHTML = `<div style="font:14px system-ui;color:#c0392b;padding:1em">⚠️ ${err}</div>`;
  }
}

customElements.define('ss-player-webgpu', SsPlayerWebGPU);
