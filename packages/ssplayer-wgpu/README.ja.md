# @spritestudio/ssplayer-wgpu

[SpriteStudio](https://www.webtech.co.jp/spritestudio/) のアニメーション（`.ssab`）を
**WebGPU** で再生するレンダラです。Rust / [wgpu](https://wgpu.rs/) 実装のプレイヤーを WebAssembly にコンパイルしたものです。

1ページに `GPUDevice` は1つ、1 canvas にサーフェス（`SsSurfaceWeb`）が1つ、そして **1枚の canvas に何体でも載り、
1パスで合成されます** — 10体並べても device もパイプラインもアトラスのアップロードも1回きりです。

English version: [README.md](./README.md)

## インストール

```bash
npm i @spritestudio/ssplayer-wgpu
```

WebGPU 対応ブラウザ（Chrome / Edge 113+、Safari 26+、対応プラットフォームの Firefox 141+）が必要です。非対応環境では `sharedContext()` が読めるエラーで reject するので、`catch` して静止画や [WebGL 版プレイヤー](https://www.npmjs.com/package/@spritestudio/ssplayer-webgl)へフォールバックできます。ESM 専用・ランタイム依存ゼロで、バンドラなら wasm を設定なしで拾います。バンドラを使わない場合は [wasm の読み込み](https://cri-middleware.github.io/SSPlayerForWgpu/ja/start/web/#wasm-の読み込み)を参照してください。

## 最初の 1 体

1 要素 = 1 canvas = 1 体です。

```js
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';

defineSsPlayerWebGPUElement(); // <ss-player-webgpu> を登録
```

```html
<ss-player-webgpu src="/assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

1 枚の canvas に複数体を載せるには、要素の中身の API を使います（ページに `sharedContext()`、canvas ごとにサーフェス、キャラクターごとにプレイヤー）。`.ssab` は SpriteStudio（7.5 以上）のプロジェクトから [SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) か SDK の `ssconverter-cli` で作ります。参照出力も埋め込み出力も読み込めます。

**次に読む:** [Web ではじめる](https://cri-middleware.github.io/SSPlayerForWgpu/ja/start/web/) ·
[`<ss-player-webgpu>`](https://cri-middleware.github.io/SSPlayerForWgpu/ja/reference/web-component/) ·
[JavaScript API リファレンス](https://cri-middleware.github.io/SSPlayerForWgpu/ja/reference/js/) · [アセットの準備](https://cri-middleware.github.io/SSPlayerForWgpu/ja/start/assets/)

## リンク

- [ドキュメント](https://cri-middleware.github.io/SSPlayerForWgpu/ja/) — このバージョンの同じページ（英語の Markdown）がパッケージの `docs/` にあり、オフラインで読めます（入口は `docs/README.md`）。同じページは Agent Skill として `skills/ssplayer-wgpu/` にもあり、pnpm 12.11 以降では `pnpm approve @spritestudio/ssplayer-wgpu` のあと、コーディングエージェントの skills ディレクトリにリンクされます
- [リポジトリ](https://github.com/cri-middleware/SSPlayerForWgpu) — Rust ライブラリ本体と、同じコアで動く Metal / DX12 / Vulkan の C/C++ ビューア
- [Issues](https://github.com/cri-middleware/SSPlayerForWgpu/issues)
- [SpriteStudio](https://www.webtech.co.jp/spritestudio/)

## ライセンス

BSD-3-Clause。[LICENSE.md](./LICENSE.md) を参照してください。
