// The page's shared WebGPU context (SPEC §1.2, §10).
//
// `SsContextWeb` owns the adapter, the device and the device-scoped `SSRenderer`
// — pipelines and the atlas cache. There must be exactly **one per page**: two
// contexts means two `GPUDevice`s, and nothing is shared across a device, so the
// same character in two canvases would compile its pipelines twice and upload its
// atlas twice. Everything else (canvases, players) hangs off this one object.
//
// Importing this module does not initialise WebGPU; the first `sharedContext()`
// call does, and every later call gets the same promise.
import init, { SsContextWeb } from '../wasm/ssplayer_wgpu.js';
import type { InitInput } from '../wasm/ssplayer_wgpu.js';

// The wasm classes are the API — a canvas is `SsSurfaceWeb`, a character is
// `SsPlayerWeb` — so they are re-exported from the package root rather than
// making callers reach into `./wasm/`. The custom element is deliberately NOT
// here: it lives behind the `./element` entry point so a page that drives the
// context itself never pulls it in.
export { SsContextWeb, SsSurfaceWeb, SsPlayerWeb } from '../wasm/ssplayer_wgpu.js';
export type { InitInput, InitOutput } from '../wasm/ssplayer_wgpu.js';

let wasmSource: InitInput | Promise<InitInput> | undefined;

/**
 * Point the loader at the `.wasm` binary explicitly.
 *
 * Not needed under a bundler: the glue self-locates through
 * `new URL('ssplayer_wgpu_bg.wasm', import.meta.url)`, which Vite / webpack 5 /
 * Rollup all resolve to an emitted asset. It is needed where `import.meta.url`
 * does not survive the build (a classic `<script>` / IIFE bundle) or where the
 * binary is served from somewhere else — a CDN, an asset host:
 *
 * ```js
 * setWasmUrl('https://cdn.jsdelivr.net/npm/@spritestudio/ssplayer-wgpu@<version>/wasm/ssplayer_wgpu_bg.wasm');
 * ```
 *
 * Pin the SAME version as the package: the JS glue and the binary share a
 * wasm-bindgen ABI. Call before the first `sharedContext()`; afterwards there is
 * nothing left to point anywhere.
 */
export function setWasmUrl(url: string | URL | InitInput | Promise<InitInput>): void {
  wasmSource = url as InitInput | Promise<InitInput>;
}

let contextPromise: Promise<SsContextWeb> | null = null;

/** The page's context, initialising wasm + WebGPU on first use. */
export function sharedContext(): Promise<SsContextWeb> {
  if (!contextPromise) {
    contextPromise = (async () => {
      if (typeof navigator === 'undefined' || !('gpu' in navigator)) {
        throw new Error(
          '[ssplayer-wgpu] This browser has no WebGPU (needs Chrome/Edge 113+, Safari 26+, ' +
            'or Firefox 141+ on a supported platform). Catch this error to fall back.',
        );
      }
      // With no explicit source the glue self-locates via `import.meta.url`. In an
      // IIFE / classic <script> build there is no import.meta, so bundlers rewrite
      // it to a stub whose `.url` is undefined and the glue throws a bare
      // "Failed to construct 'URL': Invalid URL" that says nothing about wasm.
      // Say what the caller actually has to do instead.
      if (wasmSource === undefined && typeof (import.meta as { url?: unknown } | undefined)?.url !== 'string') {
        throw new Error(
          '[ssplayer-wgpu] No WebAssembly URL given and the runtime cannot locate itself in this ' +
            'build (a classic <script> / IIFE bundle has no import.meta.url). Call ' +
            "setWasmUrl('https://cdn.jsdelivr.net/npm/@spritestudio/ssplayer-wgpu@<version>/wasm/" +
            "ssplayer_wgpu_bg.wasm') first, pinning the SAME version as this package, since the two " +
            'share a wasm ABI.',
        );
      }
      await init(wasmSource === undefined ? undefined : { module_or_path: wasmSource });
      return await SsContextWeb.create();
    })();
    // A failed init must not be cached as a permanent failure for the page: a
    // transient fetch error would otherwise make every later call — a host retry,
    // or a second canvas created after the network recovered — return that same
    // rejected promise and never start.
    contextPromise.catch(() => {
      contextPromise = null;
    });
  }
  return contextPromise;
}

let restorePromise: Promise<void> | null = null;

/**
 * Recover from a lost WebGPU device (SPEC §14).
 *
 * The loss is the *device's*, so it hits every canvas on the page at once and the
 * recovery is one call — but every canvas notices independently, so the calls are
 * collapsed into a single in-flight promise here. Without this, N elements would
 * each request their own replacement device and only the last would be kept.
 */
export function restoreSharedDevice(ctx: SsContextWeb): Promise<void> {
  if (!restorePromise) {
    restorePromise = ctx.restoreDevice().finally(() => {
      restorePromise = null;
    });
  }
  return restorePromise;
}
