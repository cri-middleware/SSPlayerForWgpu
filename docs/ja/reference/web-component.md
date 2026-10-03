# `<ss-player-webgpu>` { #ss-player-webgpu }

カスタム要素は**利便性のラッパー**です。1 要素＝1 キャンバス＝1 キャラクターで、1 体だけ表示する用途（広告クリエイティブ、ページ隅のマスコット、CMS テンプレート内のキャラクター）への最短経路です。

2 つ目の実装ではありません。内部ではページの共有コンテキストからサーフェスを取り、そこにプレイヤーを
1 つ足しているだけなので、**1 ページに 10 個置いても `GPUDevice` は 1 つ、パイプラインは 1 セット、アトラスのアップロードは 1 回**です。

## 登録する

```js
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';

defineSsPlayerWebGPUElement();                    // <ss-player-webgpu>
defineSsPlayerWebGPUElement('my-ss-player');      // ... 独自の名前で
```

登録は **import の副作用ではなく opt-in** なので、[JavaScript API](./js.md) だけを使うバンドルに要素は含まれません。呼び出しは冪等で、`tag` 引数は、既定名で別の SpriteStudio Player が登録済みのページのためにあります。

```html
<ss-player-webgpu src="/assets/Basic.ssab" autoplay style="width:360px;height:360px"></ss-player-webgpu>
```

## 属性

| 属性 | 意味 |
|---|---|
| `src` | `.ssab` の URL。reference / embedded どちらでも可 |
| `pack` | パック名の上書き。既定は `.ssab` 内の名前 |
| `animation` | 再生するアニメーション。既定はパックが宣言する最初のもの |
| `autoplay` | ロード後に再生開始。**boolean 属性**なので、無ければ再生しません |
| `speed-scale` | 既定 fps の倍率。`0` と負値は停止 |
| `fit` | `contain`（既定）と `cover` はオーサリング時のキャンバスを要素に合わせます。`none` は 1:1 で、原点を中央に置きます |
| `align` | 合わせたキャンバスの置き場所：`center`（既定）、`top`、`bottom`、`left`、`right`、`top-left`、`top-right`、`bottom-left`、`bottom-right` |
| `flip-x`、`flip-y` | キャラクターを反転します。boolean 属性で、揃え位置も反転に従います |
| `pause-when-hidden` | 画面外・バックグラウンド時に rAF ループを止める。**既定でオン** — `pause-when-hidden="false"` で解除 |

### 置いた後に書き換える

`src`・`pack`・`animation`・`speed-scale`・`fit`・`align`・`flip-x`・`flip-y` は監視されていて、再生中の要素で書き換えても反映されます。

* `src` か `pack` を変えると、`loadSrc()` と同じようにそのパックを読み込み、設定が済むと `load` イベントを送ります。
* `animation` を変えると、そのアニメーションに切り替えます。`autoplay` があれば先頭から再生し、無ければ 1 フレーム目で止まります。
* `autoplay` はアニメーションを設定するたびに読まれ、`pause-when-hidden` は要素が接続されたときに 1 度だけ読まれます。

### キャラクターを要素に合わせる

要素は、アニメーションの**オーサリング時のキャンバス**を、作者が決めた pivot で配置したうえで箱に合わせます。原点の上に立つよう作られたキャラクターも、頭がはみ出さずに箱に収まります。`fit` と `align` を指定しなければ `contain` と `center` です。

箱は、パックを**読み込んだときの**アニメーションのキャンバスで、その後に `animation` を切り替えても変えません。同じパックでもアニメーションごとにキャンバスが違うことがあり（Ringo の `dead` は幅 1300、ほかは 800）、モーションを変えるたびに大きさが変わるのは不自然だからです。新しい `src` か `pack` を読むと箱を決め直します。`fit="none"` はこれをすべて行わず、オーサリング時の 1 単位をキャンバスの 1 ピクセルとして、原点を中央に描きます。

### `autoplay` は `<video>` と同じく opt-in

付けない場合、要素は**ロードして 1 フレーム目で止まります**（ポスターのように）。描画は再生制御とは独立しているので、キャラクターは画面に出ていて、動いていないだけです。これは HTML の慣習（`<video autoplay>`）に従ったもので、SpriteStudio Player for Web の WebGL パッケージの要素 `<ss-player>` とも揃っています。

（ファミリーのうち*配置する*タイプの Player（Godot / Unity）は逆に既定でオンです。シーンノードには
`play()` を呼ぶコードが無く、属性が**そのコードの代わり**になるためです。）

## プロパティとメソッド

要素は API を隠しません。自分が作ったものを公開するので、[JavaScript API](./js.md) にあるものはすべて到達可能です：

| | |
|---|---|
| `element.player` | `SsPlayerWeb` — パーツオーバーライド、イベント、再生制御など全部 |
| `element.surface` | このキャンバスの `SsSurfaceWeb` — `stats()`、`resize()` |
| `element.ctx` | ページ共有の `SsContextWeb` |
| `element.canvas` | 要素が作った `<canvas>` |

```js
const el = document.querySelector('ss-player-webgpu');
await el.loadSrc('/assets/Other.ssab');       // アニメーション名の配列が返る
el.player.setLoopCount(1);
el.player.setPartVisibilityOverride('hat', true, false);
```

### `loadSrc(src, pack?, animation?)`

`.ssab` を取得して再生します。実行時にパックを差し替える手段でもあります。戻り値はアニメーション名の配列なので、そのまま UI の選択肢にできます。

アニメーション（`animation`、無ければ最初のもの）を選択したあと、初回ロードとまったく同じように `autoplay` を尊重します。つまり `autoplay` の無い要素は、新しいパックでも 1 フレーム目で止まります。要素の属性は書き換えないので、DOM 上で移動した（接続し直した）要素は `src` 属性のパックを読み込みます。要素を移動するページでは、代わりに属性を設定してください。

`pack` を渡すと、要素自身がバイト列を取得してその名前で登録します。`load` は名前の上書きを受け取らないためです。

呼び出しは**直列化**されます。ロード中のプレイヤーは共有コンテキストから取り外されているため、前のロードが完了する前に始めた 2 本目はコンテキストに reject されます。パックの差し替えは多くの場合ユーザー操作で、続けざまに 2 回操作したら 2 回差し替わるべきなので、各呼び出しは 1 つ前を待ち、最後に呼んだものが画面に残ります。そのロード自体が失敗すれば戻り値の Promise は reject するので、catch は呼び出し側の仕事です（要素のエラー表示に流れるのは起動時の失敗だけです）。

## イベント

要素は、再生で起きたことを自分自身への DOM イベントとして送ります。バブルはしません。

| イベント | いつ | `detail` |
|---|---|---|
| `animationstarted` | `play` の後に最初に進んだフレーム | — |
| `animationfinished` | 再生を完了したフレーム。無限ループでは起きず、`stop()` でも起きません | — |
| `animationlooped` | ループ境界を跨いだフレーム | — |
| `userdata` | ユーザーデータのキーを跨いだ | `{ partIndex, integer?, rect?, point?, text? }` |
| `signal` | シグナルのキーを跨いだ | `{ partIndex, commandId, params }` |
| `audio` | オーディオのキーを跨いだ | `{ partIndex, soundName, loopNum, … }` |
| `load` | パックを読み込み、アニメーションの設定が済んだ | — |

最初の 6 つはファミリーの必須イベントで、どの SpriteStudio Player も同じ語幹（`animationFinished`、`userData` など）を使っています。DOM のイベント名なので小文字にしています。タイムラインのイベントは [`player.events()`](./js.md) が返すオブジェクトをそのまま運ぶので、tick が飛ばしたフレームのキーも届きます。要素は音を鳴らしません。`audio` イベントで音を鳴らすべきときは [イベント](../guide/events.md) を参照してください。

```js
el.addEventListener('animationfinished', () => showNextButton());
el.addEventListener('userdata', (e) => console.log(e.detail.text));
```

## レイアウトと解像度

* カスタム要素はページが指定しない限り `display: inline` で、インラインボックスは `width` / `height` を無視します。そのため要素は、**ページが `inline` のままにしていた場合にだけ `display: block` を設定します**。`inline-block`・フレックスアイテム・`none` を選んだ作者の指定はそのまま残ります。
* キャンバスは要素いっぱい（`100%` × `100%`）に広がります。
* バッキングストアは **CSS サイズ × `devicePixelRatio`** なので、HiDPI でも鮮明です。
* どちらも**要素が接続された時点**で計算され、要素に張られた `ResizeObserver` が追従を保ちます。CSS
  ボックスが変わると要素自身がバッキングストアを再計算して [`SsSurfaceWeb.resize()`](./js.md) を呼ぶため、リサイズするパネルに置いても追加のコードは要りません。（`ResizeObserver` が使えない環境では接続時のサイズのまま固定されます。`element.surface` 経由で `resize()` を手動で呼ぶこともできます。）
* 要素にはサイズを与えてください。無い場合は 360 × 360 にフォールバックします。

## 誰も見ていないときに止める

`pause-when-hidden` は既定でオンで、アニメーションフレームループを **2 つの**シグナルでゲートします：キャンバスに対する `IntersectionObserver` と、ドキュメントの `visibilitychange`。モバイルでのバッテリーと、広告クリエイティブが測定されるビューアビリティ要件のためにあります。

想定しておく挙動が 2 つ：

* **最初のオブザーバコールバックが来るまでは「隠れている」とみなします。** 画面外から始まった要素が、すぐ捨てることになる初回フレームを描かないためです。
* API が存在しない場合（またはゲートを切った場合）は、まったく動かないのではなく**常に動きます**。

フレームのデルタは 50 ms でクランプされ、一時停止のあとは時計がリセットされます。バックグラウンドのタブに戻ったとき、アイドル時間の分だけアニメーションが飛ぶことはありません。

## デバイスロストは自動で処理されます

要素は毎フレーム、デバイスが失われていないか確認し、自分で `restoreSharedDevice` を呼びます。デバイスは*ページ*のものなので、**最初に気づいた要素がページ上の全要素の分を復帰させます**。再生は中断されたフレームから再開します。[デバイスロスト](../guide/device-loss.md)を参照。

## エラー

起動・ロード・フレーム処理で投げられたものはすべて捕捉されます。要素は `[ss-player-webgpu]` としてコンソールに出力し、自分の中身をシンプルなエラーボックスに置き換えます。メッセージはマークアップではなくテキストとして書き込まれます。属性由来の URL やパック名が混じりうるうえ、エラー経路は HTML をパースし始めるのに適した場所ではないからです。

最頻の原因は WebGPU 非対応ブラウザです。既定のボックスではなくデザインされたフォールバックが欲しい場合は、
[`sharedContext()`](./js.md) を直接使って `catch` してください。

## ライフサイクル

要素を DOM から外すと、そのプレイヤーとサーフェスが破棄されます。**ページのコンテキストは破棄されません** —
他の要素が共有しているためです。参照を捨てるだけでは済まないのはこの後始末のためで、サーフェスはスワップチェーンを持っており、存在したことのある要素の数だけリークするからです。

## API を直接使うべき場面

次の場合は [`SsContextWeb`](./js.md) を直接使ってください：

* **1 つのキャンバスに複数キャラクター** — 要素は 1 キャンバス 1 キャラクターです；
* 自前のフレームループ、または `requestAnimationFrame` 以外で駆動したい；
* デバイスをいつ作るかを制御したい、あるいは WebGPU 非対応時のフォールバックを自分で用意したい；
* すでに自分で管理しているキャンバスがある。

要素がやっていることはすべてその API の上に書かれており、1 ページで両者を混ぜても構いません。
