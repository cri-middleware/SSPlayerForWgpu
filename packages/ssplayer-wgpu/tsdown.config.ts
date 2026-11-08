import { defineConfig } from 'tsdown';

export default defineConfig({
  entry: ['src/index.ts', 'src/element.ts'],
  format: 'esm',
  platform: 'browser',
  target: 'es2022',
  dts: true,
  minify: true,
  deps: {
    // The wasm-bindgen glue stays OUT of the bundle. It self-locates its binary
    // with `new URL('ssplayer_wgpu_bg.wasm', import.meta.url)`, so bundling it
    // into dist/ would make it look for the wasm beside dist/index.js and force a
    // second copy of the binary into the tarball. Left external, `dist/*.js`
    // keeps a plain `../wasm/ssplayer_wgpu.js` import and there is exactly one of
    // each file — which is also what makes `exports["./wasm/*"]` a CDN-pinnable
    // path rather than a duplicate.
    neverBundle: [/^\.\.\/wasm\//],
  },
  outputOptions: {
    entryFileNames: '[name].js',
    chunkFileNames: 'chunk-[hash].js',
  },
});
