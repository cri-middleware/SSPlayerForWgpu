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
import init, { SsContextWeb } from './wasm/ssplayer_wgpu.js';

let contextPromise = null;

/** The page's context, initialising wasm + WebGPU on first use. */
export function sharedContext() {
  if (!contextPromise) {
    contextPromise = (async () => {
      if (!('gpu' in navigator)) throw new Error('このブラウザは WebGPU 非対応です（Chrome / Edge 113+ など）。');
      await init();
      return await SsContextWeb.create();
    })();
    // A failed init must not be cached as a permanent failure for the page.
    contextPromise.catch(() => { contextPromise = null; });
  }
  return contextPromise;
}

let restorePromise = null;

/**
 * Recover from a lost WebGPU device (SPEC §14).
 *
 * The loss is the *device's*, so it hits every canvas on the page at once and the
 * recovery is one call — but every canvas notices independently, so the calls are
 * collapsed into a single in-flight promise here. Without this, N elements would
 * each request their own replacement device and only the last would be kept.
 */
export function restoreSharedDevice(ctx) {
  if (!restorePromise) {
    restorePromise = ctx.restoreDevice().finally(() => { restorePromise = null; });
  }
  return restorePromise;
}
