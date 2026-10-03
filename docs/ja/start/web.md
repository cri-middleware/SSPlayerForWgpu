---
title: Web ではじめる
---

# Web ではじめる

Player を WebAssembly にコンパイルし、**WebGPU** で描画します。1 ページに `GPUDevice` は 1 つ、キャンバス 1 枚につき `SsSurfaceWeb` が 1 つ、そして**1 枚のキャンバスに何体でも 1 レンダーパスで**描画できます。10 体並べてもデバイス 1 つ・パイプライン 1 セット・アトラスアップロード 1 回です。

!!! tip "wgpu を知っている必要はありません"
    wgpu はライブラリの実装が書かれている対象で、ブラウザではあなたのブラウザが持つ WebGPU に解決されます。**デバイスは `sharedContext()` が作ります**し、API に wgpu の型は登場しません。書くのは `load` /
    `setAnimation` / `play` です。

## 前提 { #requirements }

WebGPU 対応ブラウザ：**Chrome / Edge 113+、Safari 26+、Firefox 141+**（対応プラットフォーム上）。未対応環境では `sharedContext()` が読めるエラーで reject するので、フォールバック（静止画や
[SpriteStudio Player for Web の WebGL パッケージ](https://www.npmjs.com/package/@spritestudio/ssplayer-webgl)）は `catch` 一つで書けます。

## 導入 { #install }

=== "リリース版"

    🚧 **npm 未公開です。** 公開後は：

    ```bash
    npm i @spritestudio/ssplayer-wgpu@beta
    ```

    ESM 専用・ランタイム依存なし。wasm バイナリを同梱しており、バンドラ（Vite / webpack 5 / Rollup）は設定なしでアセットとして拾います。

=== "ソースから"

    パッケージをビルドしてチェックアウトから入れます。`rustup target add wasm32-unknown-unknown`、
    [`wasm-pack`](https://rustwasm.github.io/wasm-pack/)、`pnpm`（`corepack enable`）と、SDK サブモジュールへのアクセスが必要です。

    ```bash
    git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
    cd SSPlayerForWgpu
    ./scripts/build.sh platform=web          # wasm-pack + tsdown -> packages/ssplayer-wgpu/{wasm,dist}
    cd packages/ssplayer-wgpu && pnpm pack    # -> spritestudio-ssplayer-wgpu-<version>.tgz
    ```

    アプリ側で `npm i /path/to/spritestudio-ssplayer-wgpu-<version>.tgz`。`prepack` は `dist/` や `wasm/` が欠けた tarball を拒否するので、出来上がった tarball は常に完全です。

    **動いているところを見るだけ**なら、あと 1 コマンドです：

    ```bash
    ./scripts/deploy-examples.sh              # アニメーションデータ（1 回だけ）
    ./scripts/build.sh platform=web
    (cd examples/web && python3 -m http.server)
    ```

## 1 体だけ：カスタム要素

最短経路です（広告クリエイティブ、画面隅のマスコットなど）。1 要素＝1 キャンバス＝1 キャラクター。

```js
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';

defineSsPlayerWebGPUElement(); // <ss-player-webgpu> を登録
```

```html
<ss-player-webgpu src="/assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

| 属性 | 意味 |
|---|---|
| `src` | `.ssab` の URL。reference / embedded どちらでも可 |
| `pack` | パック名の上書き。既定は `.ssab` 内の名前 |
| `animation` | 再生するアニメーション。既定は最初のもの |
| `autoplay` | ロード後に再生開始。**boolean 属性なので、無ければ再生しません** — ポスターのように 1 フレーム目で止まります |
| `pause-when-hidden` | 画面外・バックグラウンド時に rAF ループを止める。既定でオン、`"false"` で解除 |
| `speed-scale` | 既定 fps の倍率 |
| `fit`、`align`、`flip-x`、`flip-y` | オーサリング時のキャンバスを要素にどう合わせるか。既定は `contain` と `center`、`fit="none"` で 1:1 |

`element.loadSrc(src, pack?, animation?)` でスクリプトからパックを差し替えられ、アニメーション名の配列が返ります。`animationstarted`・`animationfinished`・`animationlooped`・`userdata`・`signal`・`audio` を DOM イベントとして送り、パックを読み込むと `load` を送ります。上の属性は置いた後に書き換えても反映されます。要素の登録は import の副作用ではなく **opt-in** なので、API だけ使うバンドルに要素は含まれません。

要素の全体（レイアウト、可視性ゲート、デバイスロスト、要素から Player API に触る方法）は
[`<ss-player-webgpu>`](../reference/web-component.md) にまとめてあります。

## 複数体：要素の中身の API

```js
import { sharedContext } from '@spritestudio/ssplayer-wgpu';

const ctx = await sharedContext();            // ページに 1 つ — デバイス・パイプライン・アトラスキャッシュ
const surface = ctx.createSurface(canvas);    // キャンバスに 1 つ — 投影とフレームアリーナ

const a = surface.createPlayer();
await a.load('/assets/Basic.ssab');           // パック・アトラス・兄弟パックを 1 呼び出しで
a.setAnimation(a.animationNames()[0]);        // 選択と再生開始は別
a.play();

const b = surface.createPlayer();
await b.load('/assets/Effect.ssab');          // アップロード済みアトラスは 2 度上げません
b.setAnimation(b.animationNames()[0]);
b.setTransform(120, 0, 0.5, 0.5, 0);
b.setOrder(1);                                // キャンバス内の描画順（昇順）
b.play();

let last = performance.now();
const loop = (now) => {
  const dt = Math.min((now - last) / 1000, 0.05); // 秒。長い中断に備えてクランプ
  last = now;
  surface.frame(dt);                          // 全プレイヤーを更新し 1 パスで描画
  requestAnimationFrame(loop);
};
requestAnimationFrame(loop);
```

**プレイヤー API で非同期なのは `load` 系だけです。** パック本体、両方のテクスチャテーブルにあるアトラス、そしてインスタンスパーツが参照する兄弟 `.ssab` まで取得します。届かなかったものは例外ではなく `missingAssets()` から返ります。`setAnimation()` と再生制御はすべて同期です。

1 つのプレイヤーで同時に走らせられるロードは 1 本です。前のロードが完了する前に次を始めると、どちらかを黙って失う代わりに reject されます。`await` するか、要素と同じように呼び出しを直列につないでください。別々のプレイヤーなら従来どおり並行にロードできます。

ハンドルは明示的です。使い終わったら `player.destroy()` / `surface.destroy()` を呼んでください（JS 参照を捨てるだけではスワップチェーンが残ります）。どちらも冪等です。

## wasm の読み込み

バインディングは `new URL('ssplayer_wgpu_bg.wasm', import.meta.url)` で自分の場所を特定します。ESM バンドラはこれを解決できます。`import.meta.url` がビルドで失われる場合（classic `<script>` や IIFE バンドル）、あるいはバイナリを別の場所から配信する場合は、最初の `sharedContext()` より前に指定します：

```js
import { setWasmUrl, sharedContext } from '@spritestudio/ssplayer-wgpu';

setWasmUrl('https://cdn.example.com/ssplayer-wgpu/<version>/wasm/ssplayer_wgpu_bg.wasm');
await sharedContext();
```

**パッケージと同じバージョンに固定**してください。JS グルーとバイナリは wasm-bindgen の ABI を共有します。

## デバイスロスト

ブラウザはいつでも `GPUDevice` を取り上げられます（タブのバックグラウンド化、GPU プロセスのクラッシュ）。フレームループで `ctx.isDeviceLost()` を確認し、`restoreSharedDevice(ctx)` を呼んでください。ページ上の全キャンバス・全プレイヤーが保持済みのソースから再構築され、止まったところから再生が続きます。カスタム要素はこれを自前でやります。[デバイスロスト](../guide/device-loss.md)を参照。

## 次に読む

* [`<ss-player-webgpu>`](../reference/web-component.md) — 要素の全体
* [JavaScript API リファレンス](../reference/js.md) — 全クラスとメソッド
* [アセットの準備](./assets.md) — `.ssab` とアトラスをサーバー上のどこに置くか
* [再生と再生制御](../guide/playback.md) — `play` と `resume`、ループ、区間
