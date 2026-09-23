---
title: Bevy ではじめる
---

# Bevy ではじめる

キャラクターをエンティティとして spawn すると、Bevy が自分の 2D パスで、自分が持っているデバイス上に描画します。`ssplayer-bevy` は 2 つ目の Player ではなく**アダプタ**です。描画しているのは
`ssplayer-wgpu` で、このクレートはその継ぎ目にすぎません。

**ここでは wgpu に一切触れません。** `SSRenderer` を触りたくなったら、それは
[Rust のパス](./rust.md)であってこのページではありません。

## 対応 Bevy { #supported-bevy }

**0.19.x のみ。**

Bevy と Player は 1 つの `wgpu` に解決される必要があります。`bevy_render` 0.19 は `wgpu ^29.0.3` を要求し、
Player は 29 系に固定されています。wgpu のメジャーが 2 つあるということは互換性のない型が 2 組あるということで、両者を橋渡しするものはありません — 不一致は実行時の驚きではなくコンパイルエラーになります。
npm ならこれを peer dependency と呼ぶところですが、cargo にその仕組みは無いので、対応範囲はここに明記し、
workspace が 1 つの `wgpu` に解決することで担保しています。

## 導入 { #install }

=== "リリース版"

    🚧 **crates.io には未公開です。** `ssplayer-wgpu` の公開待ちで、そちらは SDK クレートの公開待ちです。公開されればこうなります：

    ```toml
    [dependencies]
    ssplayer-bevy = "7.0.0-alpha.1"
    ```

=== "ソースから"

    ```toml
    [dependencies]
    ssplayer-bevy = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
    ```

    これが `ssplayer-wgpu` を引き込むので、両方に依存する必要はありません。ソースを取り込む場合はチェックアウト内の `libs/ssplayer-bevy` を指し、そこで
    `git submodule update --init --recursive` を実行してください。

## アプリ全体

```rust
use bevy::prelude::*;
use bevy::camera::CompositingSpace;
use bevy::render::view::Msaa;
use ssplayer_bevy::{SsPlayback, SsPlayer, SsPlayerPlugin, SsTransform};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(SsPlayerPlugin)
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    // MSAA はオフ：Player のパイプラインはシングルサンプル。
    // CompositingSpace::Srgb は下の「色」を参照 — 付けないと全体が明るくなりすぎます。
    commands.spawn((Camera2d, Msaa::Off, CompositingSpace::Srgb));

    commands.spawn((
        SsPlayer { src: "assets/Ringo/Ringo.ssab".into(), animation: "wait1".into() },
        SsTransform { scale_x: 0.6, scale_y: 0.6, ..default() },
        SsPlayback { order: 1, ..default() },
    ));
}
```

`SsTransform` と `SsPlayback` は required component なので、`SsPlayer` だけ spawn すれば残りは既定値で付きます。

| コンポーネント | 持つもの | 変更コスト |
|---|---|---|
| `SsPlayer` | `src`, `animation` | 再生ツリーを再構築します — 唯一アロケーションを伴うもの |
| `SsTransform` | `x`, `y`, `scale_x`, `scale_y`, `rotation` | ゼロ |
| `SsPlayback` | `playing`, `speed`, `order`, `alpha`, `visible`, `restart` | ゼロ |

Bevy の `Transform` では**ありません**。あちらは 3D アフィンで、Player の 5 つのスカラーへの分解は非可逆だからです。連動させたい場合はコピーするシステムを 1 つ書いてください。どちらを正とするかはあなたが決められます。

## 色：カメラに `CompositingSpace::Srgb` を付ける

SpriteStudio のアトラスは sRGB でオーサリングされ、Player はそのバイト列を渡されたターゲットへそのまま書き込みます（ネイティブや C++ ホストと同じく、色管理は一切しません）。Bevy の**既定の** 2D カメラは
**sRGB** ターゲットに描画するため、書き込み時にもう一度エンコードされ、キャラクターが目に見えて明るくなりすぎます。

`CompositingSpace::Srgb` を付けると Bevy は非 sRGB（`Rgba8Unorm`）ターゲットを選び、sRGB 空間で合成します。これが Player（および WebGL リファレンス）のモデルと一致します。付いていない sRGB ターゲットに描画した場合、アダプタが 1 度だけ警告します。

!!! warning "HDR は別の話"
    `Hdr` カメラは `Rgba16Float`（リニア光）で描画され、Player の sRGB 空間出力はトーンマッピング的に正しくありません。忠実な色が必要なら、SDR カメラ ＋ `CompositingSpace::Srgb` を使ってください。

## 再開・一時停止・反応

`SsPlayback::playing` は**巻き戻さずに**一時停止／再開します。したがって終了したアニメーションを再生し直すことはできません。先頭から再生したい場合は `SsPlayback::restart` を**インクリメント**します（値が変われば何でも構いません）。世代カウンタなので、ワンショットを何度でも再トリガーでき、後始末も不要です。

再生はレンダーワールドで動くため、結果は 2 つの経路で返ってきます：

```rust
fn play_sounds(mut events: MessageReader<SsEventMessage>) {
    for message in events.read() {
        if let SsEvent::Audio(audio) = &message.event {
            // Player 自身は音を鳴らしません。自前のオーディオへ渡してください。
            info!("play {:?} x{}", audio.sound_name, audio.loop_num);
        }
    }
}
```

| 経路 | 種類 | 用途 |
|---|---|---|
| `SsPlaybackState` コンポーネント | ポーリング | 現在の `frame_no`、終了したかどうか |
| `SsEventMessage` | `MessageReader` | タイムラインイベント（ユーザーデータ・シグナル・オーディオ） |
| `SsFinished` / `SsLooped` | `MessageReader` | 再生完了・ループ境界のエッジ |

どちらも**1 フレーム遅れ**です。再生はレンダーワールドで、そのフレームの `Update` より後に動くためです。

## マスクとデバイスロスト

Bevy の 2D 深度テクスチャは `Depth32Float` でステンシルを持たないため、アダプタは Player の**カバレッジビットマップ**バックエンドを自動的に選びます。Bevy のアタッチメントには何も要求せず、描画は
Bevy 自身のパス内で完結します。コストはマスクを使うキャラクター 1 体につき 1 パス／フレーム。[マスク](../guide/masks.md)を参照。

デバイスロストは Bevy 自身の復帰に乗って回復しますが、**opt-in が必要**です。Bevy の既定のエラーハンドラはデバイスロストで*終了*するため、`RenderErrorPolicy::Recover` を返す `RenderErrorHandler` を入れてください（[デバイスロスト](../guide/device-loss.md)）。

## サンプルを動かす

```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu
./scripts/deploy-examples.sh
cargo run -p ssplayer-bevy-overall    # 機能ツアー — まずこちら
cargo run -p ssplayer-bevy-viewer     # 複数キャラクターを自由に触るビューア
```

## 次に読む

* [Bevy API リファレンス](../reference/bevy.md) — 全コンポーネント・メッセージ・制限
* [再生と再生制御](../guide/playback.md) — コンポーネントが対応する語彙
* [エンジンへの組み込み](../wgpu/engines.md) — 他エンジン向けアダプタを書く場合の配線
