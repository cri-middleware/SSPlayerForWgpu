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
import init, { SsContextWeb } from './pkg/ssplayer_wgpu.js';

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

/**
 * Give a freshly loaded pack its atlas textures (SPEC §8.1, §8.2).
 *
 * `loadSsab` registers playback data and nothing else — the player carries no
 * image decoder and cannot fetch, so until this resolves every part draws
 * against the 1x1 white fallback. Both `.ssab` forms end at the same call:
 *
 *   embedded  — the images ride inside the `.ssab`. `embeddedTextures()` hands
 *               out the encoded bytes; nothing is fetched.
 *   reference — only the `.ssab` knows where the exporter put each atlas, and
 *               only the page can read it. `textureRefs()` answers "what, and
 *               from where", relative to the `.ssab`'s own URL.
 *
 * `addTextureFromBytes` then decodes with `createImageBitmap` — off the main
 * thread, premultiplied, and using the decoder the browser already ships. That
 * is why the wasm carries none: bundling one cost ~29% of the transferred bytes
 * to do the same job worse. Same division of labour as `ssplayer-core`'s
 * `extractEmbeddedTextures` / `extractTexturePaths` + `loadTextures`.
 *
 * @param baseUrl the URL the `.ssab` itself was fetched from; reference-mode
 *                paths resolve against it. Omit for a pack known to be embedded.
 */
export async function supplyTextures(ctx, player, { pack, baseUrl } = {}) {
  const jobs = player.embeddedTextures(pack).map(({ hash, bytes }) => once(ctx, hash, () => ctx.addTextureFromBytes(hash, bytes)));

  if (baseUrl) {
    for (const ref of player.textureRefs(pack)) {
      if (ref.embedded) continue;
      jobs.push(once(ctx, ref.hash, async () => {
        const res = await fetch(new URL(ref.path, baseUrl));
        if (!res.ok) throw new Error(`atlas ${ref.path}: ${res.status} ${res.statusText}`);
        await ctx.addTextureFromBytes(ref.hash, new Uint8Array(await res.arrayBuffer()));
      }));
    }
  }

  await Promise.all(jobs);
}

/**
 * One decode per atlas per page, however many players ask for it.
 *
 * The renderer's texture map is device-scoped (SPEC §1.2), so `hasTexture`
 * settles anything a previous player already registered — this is the "registered
 * wins" rule the player-side flush applies for itself, applied from JS now that
 * the decoding is out here. The pending map covers the window the flag cannot:
 * two players added in the same tick would both see "not yet" and decode the
 * same atlas twice. `SSWebGLTextureCache` exists in the WebGL player for exactly
 * this, one layer down.
 */
const inFlight = new WeakMap();

function once(ctx, hash, supply) {
  if (ctx.hasTexture(hash)) return Promise.resolve();
  let pending = inFlight.get(ctx);
  if (!pending) inFlight.set(ctx, (pending = new Map()));
  let job = pending.get(hash);
  if (!job) {
    job = supply().finally(() => pending.delete(hash));
    pending.set(hash, job);
  }
  return job;
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
