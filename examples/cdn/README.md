# CDN samples — no build step

Two self-contained pages that load `@spritestudio/ssplayer-wgpu` straight from jsDelivr, with no
bundler and no `npm install`:

| File | What it shows |
| --- | --- |
| [`element.html`](./element.html) | `<ss-player-webgpu>`: one module script registers the element, and the markup plays one character |
| [`api.html`](./api.html) | The API the element wraps: two characters in one canvas, one device for the page |

Both import the package as native ES modules:

```html
<script type="module">
  import { defineSsPlayerWebGPUElement } from 'https://cdn.jsdelivr.net/npm/@spritestudio/ssplayer-wgpu@7.0.0-beta.1/dist/element.js';

  defineSsPlayerWebGPUElement();
</script>

<ss-player-webgpu src="Ringo.ssab" autoplay></ss-player-webgpu>
```

**There is no wasm URL to give.** Imported as a module, the glue finds `ssplayer_wgpu_bg.wasm`
beside itself on the CDN through `import.meta.url`, so `setWasmUrl()` is not needed. The package is
ESM-only: there is no classic-`<script>` build.

**Pin an exact version.** The JS glue and the wasm share a wasm-bindgen ABI, and both come from the
one package URL, so the one version pins both. `scripts/set-versions.mjs` rewrites the version in
these pages on every release.

## Run it

Both pages `fetch()` an `.ssab`, so serve them over http(s): opening the file directly (`file://`)
fails on CORS. They need a browser with WebGPU.

1. **Supply an `.ssab`.** Use an *embedded* one, which carries its textures, so a page needs no
   sibling PNGs. The repository's sample converts with:

   ```bash
   ./scripts/deploy-examples.sh
   cp assets/Ringo_emb/Ringo.ssab examples/cdn/
   ```

2. **Serve the directory**, then open `http://localhost:8000/element.html` and
   `http://localhost:8000/api.html`:

   ```bash
   (cd examples/cdn && python3 -m http.server)
   ```
