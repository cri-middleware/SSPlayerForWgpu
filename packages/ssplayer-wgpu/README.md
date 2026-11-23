# @spritestudio/ssplayer-wgpu

A **WebGPU** renderer for [SpriteStudio](https://www.webtech.co.jp/eng/spritestudio/)
animations (`.ssab`), compiled to WebAssembly from the Rust/[wgpu](https://wgpu.rs/) player.
SpriteStudio is a 2D animation tool: an artist adds bones, mesh deformation and effects to their own
illustrations, and this package plays the result.

One `GPUDevice` per page, one surface (`SsSurfaceWeb`) per canvas, and **any number of characters drawn into one
canvas in a single render pass** — putting ten characters on screen costs one device, one set of
pipelines and one atlas upload.

▶️ **[Live demo](https://cri-middleware.github.io/SSPlayerForWgpu/demo/)** — this package running
in your browser: the `<ss-player-webgpu>` element, and several characters on one canvas. Drop a
folder holding your own `.ssab` onto the page to play it.

日本語版は [README.ja.md](./README.ja.md)。

## Install

```bash
npm i @spritestudio/ssplayer-wgpu
```

It needs a browser with WebGPU — Chrome / Edge 113+, Safari 26+, or Firefox 141+ on a supported
platform — and `sharedContext()` rejects with a readable error where it is missing, so a fallback
(a static image, the [WebGL player](https://www.npmjs.com/package/@spritestudio/ssplayer-webgl)) is a
`catch` away. The package is ESM-only with no runtime dependencies, and a bundler picks up its wasm
with no configuration; without one, see [Loading the wasm](https://cri-middleware.github.io/SSPlayerForWgpu/start/web/#loading-the-wasm).

## First frame

One element is one canvas is one character:

```js
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';

defineSsPlayerWebGPUElement(); // registers <ss-player-webgpu>
```

```html
<ss-player-webgpu src="/assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

For many characters in one canvas, use the API the element wraps: `sharedContext()` for the page, a
surface per canvas, a player per character. The `.ssab` comes from a SpriteStudio project (7.6 or
later) through [SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) or the SDK's
`ssconverter-cli`; reference and embedded exports both load. No `.sspj` of your own yet? SpriteStudio's
[official sample data](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/) works as-is — the page is in Japanese, the downloads
are not. To make your own, you use SpriteStudio itself — see [its site](https://www.webtech.co.jp/eng/spritestudio/).

**Read next:** [Get started on the web](https://cri-middleware.github.io/SSPlayerForWgpu/start/web/) ·
[`<ss-player-webgpu>`](https://cri-middleware.github.io/SSPlayerForWgpu/reference/web-component/) ·
[JavaScript API reference](https://cri-middleware.github.io/SSPlayerForWgpu/reference/js/) · [Preparing assets](https://cri-middleware.github.io/SSPlayerForWgpu/start/assets/)

## Links

- [Documentation](https://cri-middleware.github.io/SSPlayerForWgpu/) — and the same pages for this
  version, as Markdown, in this package's `docs/` (start at `docs/README.md`), readable offline,
  and in `skills/ssplayer-wgpu/` as an Agent Skill;
  [Docs for Your Assistant](https://cri-middleware.github.io/SpriteStudio-Docs/ai/docs/#-as-an-agent-skill)
  says how your package manager links it into your coding agent's skills directory.
- [Repository](https://github.com/cri-middleware/SSPlayerForWgpu) — the Rust library, plus native
  C/C++ viewers for Metal, DX12 and Vulkan built on the same core.
- [Issues](https://github.com/cri-middleware/SSPlayerForWgpu/issues)
- [SpriteStudio](https://www.webtech.co.jp/eng/spritestudio/)

## License

BSD-3-Clause. See [LICENSE.md](./LICENSE.md).
