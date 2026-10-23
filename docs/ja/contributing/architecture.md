# プレイヤーの構成

SpriteStudio Player for wgpu を開発する人と、自分のホストのために改造する人に向けたページです。ファイル名を挙げて説明し、チェックアウトがある前提で書いています。ビルドと検証は[開発者向け](index.md)に、制約は [`CONTRIBUTING.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/CONTRIBUTING.md) にあります。このページは、それらをまたぐ地図です。

SpriteStudio の Player に共通すること（再生・モーションブレンド・サウンドキー・パーツトラッキングの振る舞い）は、SDK の [Player Porting Guide](https://github.com/cri-middleware/SpriteStudio-SDK/tree/HEAD/libs/ssruntime/docs) に一度だけ書いてあります。

## 地図

ランタイムは SDK の `ssruntime` クレートで、`.ssab` のスキーマの `ssab` と一緒に、`SpriteStudio-SDK` サブモジュールへの path 依存でリンクします。ランタイムはタイムラインを進め、各フレームを描画計画と平らなバッファとして返します。`libs/ssplayer-wgpu` はその計画を、ホストが持つデバイスの上で wgpu で描きます。

### リソースの寿命で分けた 3 つの型

| 型 | 1 つあたり | 持つもの | ファイル |
|---|---|---|---|
| `SSRenderer` | デバイス | パイプライン、バインドグループレイアウト、サンプラー、シェーダーモジュール、アトラスのテクスチャ | `renderer.rs` |
| `SSScene` | 描画先 | 投影とフレームのジオメトリ。その描画先に描くすべてのプレイヤーが追記する `FrameChunk`（アリーナ・プリミティブ・描画呼び出し）を 1 つ持つ | `scene.rs` |
| `SSPlayer` | キャラクター | 再生のツリー。アニメのインスタンスごとにランタイムの `Context` を 1 つ持つ。GPU のリソースは持たない | `player.rs` |

デバイス・キュー・レンダラー・描画先は、ホストが毎フレーム貸します。3 つの型のどれも、それらを持ちません。

### クレートのそのほか

| ファイル | 役割 |
|---|---|
| `pack.rs` | `SSPack`。読み込んだ `.ssab` 1 つを、何体のプレイヤーでも共有する。CPU のデータだけを持つ |
| `assets.rs`、`decode.rs` | `SSPlayer::load` の裏のファイルシステムと、短命のスレッドに分けるアトラスのデコード |
| `events.rs` | タイムラインのイベントを、所有する値にデコードする |
| `mask.rs` | どのパーツがマスクのカバレッジをどう書くか。ssplayer-webgl の `MaskPlanner` の移植 |
| `fit.rs` | `fit` / `align`。ホストが割り付けた箱にアニメを収める |
| `metrics.rs` | フレームのコスト。回数と、タイムスタンプクエリがあれば GPU のパスの時間 |
| `frame.rs` | 1 フレーム分のランタイムのバッファを借りて見る |
| `roster.rs` | ホストが渡すハンドルのための、世代付きのスロット |

### Face

| Face | ファイル | 内容 |
|---|---|---|
| Rust | `lib.rs` と上のモジュール | API。ホストが動かす 3 つの型 |
| C | `host.rs`、`ffi.rs`、`libs/ssplayer-wgpu-ffi/` | `NativeHost` が生のウィンドウハンドルから wgpu のスタックを作って持つ。`ffi.rs` は C ABI をそれに翻訳するだけ。`-ffi` クレートがライブラリをビルドし、`include/ssplayer_wgpu.h` を再生成する |
| JS | `web.rs`、`packages/ssplayer-wgpu/` | `requestAnimationFrame` で動く WebGPU の wasm-bindgen のホストと、その上の TypeScript の層 |
| C と JS の両方 | `surface.rs` | 自分の面を持つ 2 つの Face が同じように設定・リサイズ・描画するスワップチェーン |
| Bevy | `libs/ssplayer-bevy/` | Bevy の main world のコンポーネントと、render world の再生ツリー・`SSRenderer` 1 つ・`SSScene` 1 つ |

`examples/` には Face ごとのホストがあります。`hello`（いちばん小さい Rust のホスト）、`viewer` と Bevy のビューア、Metal・DX12・Vulkan の C++ ビューア、`web` です。

## この作りになっている理由

ここに挙げる理由はどれも複数のファイルにまたがります。詳細は、それぞれが指すコードにあります。

**プレイヤーは借り、ホストが持ちます。** ゲームエンジンは自分のデバイス・キュー・フレームをすでに持っているので、プレイヤーはその 3 つを借ります（`lib.rs` の `SSRenderer`・`SSScene`・`SSPlayer`）。1 つのクレートで素のビューア・ブラウザの canvas・Bevy のどれにも描けるのはこのためで、型をリソースの寿命（デバイスで有効なもの、描画先 1 つで有効なもの、キャラクター 1 体のもの）で分けているのもこのためです。

**パックは GPU のテクスチャを持ちません。** テクスチャはホストのアセットの寿命に属します（Bevy なら `Assets<Image>` と render world の `GpuImage`）。そのため `SSPack` は必要なテクスチャを記述して画素を渡し、GPU のメモリを持つ側がそれをレンダラーに登録します（`pack.rs`、`renderer.rs`、`ssplayer-bevy` の `asset.rs`）。

**CPU は頂点の座標変換をしません。** 描く各パーツについて、ランタイムの平らな配列のその部分をフレームのアリーナにコピーし、ジオメトリの位置を示す `Prim` のレコードをパーツごとに 1 つ書きます。頂点ステージは両方を読みます（`scene.rs` の `Prim`。WGSL の `Prim` のコメントと歩調をそろえる必要があります）。CPU に頂点ごとの仕事は無く、作り直す頂点バッファもありません。

**C の Face は翻訳だけをします。** `ffi.rs` のどの入口も、引数を検証し、変換し、`NativeHost` か `SSPlayer` を呼ぶだけです。振る舞いは `host.rs` と `player.rs` にあり、Rust の API も同じものを使います。C の Face と各 web の canvas が共有するスワップチェーンの手順が `surface.rs` にあるのも同じ理由で、2 つの写しの片方にだけ修正が届くことを防ぎます。

**マスクの backend は 2 つあります。** ホストのステンシル（`MaskBackend::Stencil`）は入れ子を 8 ビットに制限します。プレイヤーが自分でラスタライズするカバレッジのビットマップはその制限を外し、ステンシルのアタッチメントも要りません。Bevy の 2D パスのように、それを持たないホストがあるためです（`scene.rs` の `MaskBackend`）。どちらも、プレイヤーのインスタンスツリー全体で書き手は 96 までで、Godot の Player と同じです。マスクの機能は両方に入れます。

**描画のコードは WebGL 版から来ていて、規則はポーティングガイドから来ます。** ブレンドの表、PartsColor のプログラム、統合シェーダー、マスクのプランナーは `ssplayer-webgl` から移植したもので、そう書いたコメントが移植元を名指しています。`.ssab` がどう描かれるべきかを決めるのはポーティングガイドで、ガイドに書いていないところはこの Player です。ほかの Player はこの Player を基準に読まれます（SpriteStudio-SDK の `MAINTAINING_PLAYERS.md` §5）。描き方の変更はガイドに照らして確かめ、それで決まった規則は同じ変更でガイドに書き足します。
