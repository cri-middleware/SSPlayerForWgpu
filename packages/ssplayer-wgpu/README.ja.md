# @spritestudio/ssplayer-wgpu

[SpriteStudio](https://www.webtech.co.jp/spritestudio/) のアニメーション（`.ssab`）を
**WebGPU** で再生するレンダラです。Rust / [wgpu](https://wgpu.rs/) 実装のプレイヤーを WebAssembly にコンパイルしたものです。

1ページに `GPUDevice` は1つ、1 canvas に `SSScene` が1つ、そして **1枚の canvas に何体でも載り、
1パスで合成されます** — 10体並べても device もパイプラインもアトラスのアップロードも1回きりです。

English version: [README.md](./README.md)

## 動作要件

WebGPU 対応ブラウザ（Chrome / Edge 113+、Safari 26+、対応プラットフォームの Firefox 141+）。非対応環境では `sharedContext()` が読めるエラーで reject するので、`catch` して静止画や
[WebGL 版プレイヤー](https://www.npmjs.com/package/@spritestudio/ssplayer-webgl)へフォールバックできます。

## インストール

```bash
npm i @spritestudio/ssplayer-wgpu@beta
```

ESM 専用・ランタイム依存ゼロです。wasm バイナリを同梱しており、バンドラ（Vite / webpack 5 / Rollup）なら設定なしでアセットとして拾われます。URL 指定が要るケースは[wasm の読み込み](#wasm-の読み込み)を参照。

## クイックスタート — カスタム要素

1要素 = 1 canvas = 1体。広告クリエイティブのような単体用途の最短経路です。

```js
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';

defineSsPlayerWebGPUElement(); // <ss-player-webgpu> を登録
```

```html
<ss-player-webgpu src="/assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

| 属性 | 意味 |
|---|---|
| `src` | `.ssab` の URL（embedded / reference どちらでも）。 |
| `pack` | パック名の上書き。既定は `.ssab` 内の名前。 |
| `animation` | 再生するアニメーション。既定は先頭。 |
| `autoplay` | ロード後に再生を始める。boolean 属性なので**無ければ再生しません** — ポスターのように 1 フレーム目で止まります。 |
| `pause-when-hidden` | 画面外・バックグラウンド中に rAF ループを止める。既定は有効、`"false"` で無効化。 |
| `speed-scale` | 既定 fps の倍率。 |
| `fit`、`align`、`flip-x`、`flip-y` | オーサリング時のキャンバスを要素にどう合わせるか。既定は `contain` と `center`、`fit="none"` で 1:1。 |

`element.loadSrc(src, pack?, animation?, read?)` でスクリプトからパックを差し替えられ、アニメーション名一覧が返ります。`animationstarted`・`animationfinished`・`animationlooped`・`userdata`・`signal`・`audio` を DOM イベントとして送り、パックを読み込むと `load` を送ります。上の属性は置いた後に書き換えても反映されます。

登録は import の副作用ではなく **明示呼び出し** です。下の API だけを使うバンドルが、使わない要素を抱え込まないようにするためです。

## クイックスタート — 1 canvas に複数体

カスタム要素はこれのラッパです。`SsContextWeb` がページの device、surface が canvas、player が1体：

```js
import { sharedContext } from '@spritestudio/ssplayer-wgpu';

const ctx = await sharedContext();            // ページに1つ — device とパイプライン
const surface = ctx.createSurface(canvas);    // canvas ごとに1つ — 射影と頂点アリーナ

const a = surface.createPlayer();
await a.load('/assets/Basic.ssab');           // パック・アトラス・参照先 ssab を1コールで
a.setAnimation(a.animationNames()[0]);        // 選択と再生開始は別コール
a.play();

const b = surface.createPlayer();
await b.load('/assets/Effect.ssab');          // 既にアップ済みのアトラスは二度上がらない
b.setAnimation(b.animationNames()[0]);
b.setTransform(120, 0, 0.5, 0.5, 0);
b.setOrder(1);                                // canvas 内の描画順（昇順）
b.play();

let last = performance.now();
const loop = (now) => {
  const dt = Math.min((now - last) / 1000, 0.05); // 秒。長い中断はクランプ
  last = now;
  surface.frame(dt);                          // 全 player を更新し、1パスで描画
  requestAnimationFrame(loop);
};
requestAnimationFrame(loop);
```

非同期なのは `load()` だけです。パック本体に加えて、**2つのテクスチャテーブル**（セルマップと
TextureChange の参照先）双方のアトラスを、どちらのエクスポート形態でも解決し、さらに Instance パーツが再生する兄弟 `.ssab` まで辿ります。届かなかったものは例外ではなく `missingAssets()` から返ります。
`setAnimation()` と再生制御（`play` / `pause` / `resume` / `stop` / `seek` / `setSpeedScale`）は同期です。

wasm に画像デコーダは意図的に入れていません。ブラウザが既に持っているので、embedded のアトラスはエンコード済みバイト列で出て `createImageBitmap` で戻ってきます。

## wasm の読み込み

バインディングは `new URL('ssplayer_wgpu_bg.wasm', import.meta.url)` で自分の位置からバイナリを探し、
ESM バンドラならこれを解決できます。`import.meta.url` が残らないビルド（classic `<script>` / IIFE）や、バイナリを別の場所から配信する場合は、最初の `sharedContext()` より前に URL を渡してください。

```js
import { setWasmUrl, sharedContext } from '@spritestudio/ssplayer-wgpu';

setWasmUrl('https://cdn.jsdelivr.net/npm/@spritestudio/ssplayer-wgpu@<version>/wasm/ssplayer_wgpu_bg.wasm');
await sharedContext();
```

バージョンはパッケージと**必ず一致**させてください。JS グルーとバイナリは wasm-bindgen の ABI を共有しています。

CDN からネイティブの ES モジュールとして直接 import した場合は、グルーの `import.meta.url` が CDN の URL になるので、バイナリを自分で見つけます。URL の指定は要りません。[`examples/cdn`](https://github.com/cri-middleware/SSPlayerForWgpu/tree/main/examples/cdn) が、ビルドなしでこの形で読み込むサンプルです。

## デバイスロスト

ブラウザはいつでも `GPUDevice` を取り上げます（タブのバックグラウンド化、GPU プロセスのクラッシュ）。フレームループで `ctx.isDeviceLost()` を確認し、`restoreSharedDevice(ctx)` を呼べば、ページ上の全
canvas・全 player が保持済みのデータから作り直され、止まったところから再生が続きます。カスタム要素はこれを自前で行います。

## API

| オブジェクト | スコープ | 所有するもの |
|---|---|---|
| `SsContextWeb` | ページ | アダプタ、device、パイプライン、アトラスキャッシュ |
| `SsSurfaceWeb` | canvas 1枚 | スワップチェイン、射影、フレーム頂点アリーナ |
| `SsPlayerWeb` | 1体 | 再生ツリー、トランスフォーム、描画順 |

型定義はパッケージに同梱しています（`dist/index.d.ts`）。`surface.stats()` はプリミティブ数・ドローコール数・アップロードバイト数・マージ可能ドロー数を返し、計測に使えます。

ハンドルは明示的です。使い終わったら `player.destroy()` / `surface.destroy()` を呼んでください（JS 参照を捨てるだけではスワップチェインが残ります）。どちらも冪等です。

## `.ssab` の生成

`.ssab` は SpriteStudio のバイナリ形式で、SDK 同梱のコンバータが `.sspj` から生成します。reference
エクスポートはアトラスを `.ssab` の隣の外部 PNG として置き、embedded エクスポートは内部に持ちます。どちらも同じ `load()` で読み込めます。

## リンク

- [ドキュメント](https://cri-middleware.github.io/SSPlayerForWgpu/ja/) — このバージョンの同じページ（英語の Markdown）がパッケージの `docs/` にあり、オフラインで読めます（入口は `docs/README.md`）
- [リポジトリ](https://github.com/cri-middleware/SSPlayerForWgpu) — Rust ライブラリ本体と、同じコアで動く Metal / DX12 / Vulkan の C/C++ ビューア
- [Issues](https://github.com/cri-middleware/SSPlayerForWgpu/issues)
- [SpriteStudio](https://www.webtech.co.jp/spritestudio/)

## ライセンス

BSD-3-Clause。[LICENSE.md](./LICENSE.md) を参照してください。
