# Bevy API リファレンス

公開されているのは、プラグイン 1 つ・コンポーネント 3 つ・状態コンポーネント 1 つ・メッセージ 3 つだけです。
生成ドキュメント：
<a href="../../api/ssplayer_wgpu_bevy/index.html" target="_blank" rel="noreferrer">**`ssplayer_wgpu_bevy` の rustdoc**</a>。

**Bevy 0.19.x のみ** — [Bevy ではじめる](../start/bevy.md#supported-bevy)を参照。

## プラグイン

```rust
app.add_plugins(SsPlayerPlugin);
```

extract / prepare / draw のシステムを追加し、レンダーワールド内でデバイススコープのレンダラーと
ターゲットスコープのシーンを所有します。他に必要なものはありません。

ただしカメラには 2 つ必要です：`Msaa::Off`（Player のパイプラインはシングルサンプル）と
`CompositingSpace::Srgb`（無いとキャラクターが目に見えて明るくなりすぎます）。

## コンポーネント

### `SsPlayer` — 何を再生するか

| フィールド | | |
|---|---|---|
| `src: String` | `.ssab` のパス | **変更すると再生ツリーを再構築します** |
| `animation: String` | 選択するアニメーション。空文字は「まだ選ばない」 | 同上 |

`SsTransform` / `SsPlayback` / `SsPlaybackState` は **required component** なので、`SsPlayer` だけを
spawn すれば 4 つとも既定値で付きます。

### `SsTransform` — どこに置くか

| フィールド | 既定 |
|---|---|
| `x`, `y` | `0.0` |
| `scale_x`, `scale_y` | `1.0` |
| `rotation`（ラジアン） | `0.0` |

毎フレーム変えても構いません。コストはゼロです。

### `SsPlayback` — 再生と合成

| フィールド | 既定 | |
|---|---|---|
| `playing: bool` | `true` | **その場で**一時停止・再開。巻き戻さないので、終了したアニメーションは再生できません |
| `speed: f32` | `1.0` | 既定 fps の倍率 |
| `order: i32` | `0` | 描画順。小さいほど先。同値なら extract 順 |
| `alpha: f32` | `1.0` | キャラクター全体のフェード |
| `visible: bool` | `true` | false は描画だけでなく**タイムラインの前進もスキップ**します |
| `restart: u32` | `0` | **値を進める**と先頭から再生し、ループ回数を張り直します |

`restart` はフラグではなく世代カウンタです。ワンショットを何度でも再トリガーでき、後始末が要らず、
2 回連続で進めれば 2 回リスタートになります。

### `SsPlaybackState` — 再生が何をしたか（ポーリング）

| フィールド | |
|---|---|
| `frame_no: f32` | タイムラインが最後に着地したフレーム |
| `finished: bool` | アニメーションが終端に達したか |

## メッセージ（リアクティブ）

| メッセージ | 内容 |
|---|---|
| `SsEventMessage` | `entity`、`event: SsEvent` — ユーザーデータ・シグナル・オーディオ |
| `SsFinished` | `entity` — 完了**エッジ** |
| `SsLooped` | `entity` — ループ境界エッジ |

```rust
fn on_finish(mut finished: MessageReader<SsFinished>, mut commands: Commands) {
    for m in finished.read() {
        commands.entity(m.entity).despawn();
    }
}
```

!!! warning "どちらのフィードバックも 1 フレーム遅れです"
    再生はレンダーワールドで、そのフレームの `Update` より後に動きます。今フレーム読める値は、前フレームに
    再生が生成したものです。音のキューや状態変化なら知覚できませんが、これを許容できないホストは再生を
    メインワールドへ移す必要があり、このクレートは意図的にそれをしていません。

## 処理がどこで走るか

| スケジュール | 内容 |
|---|---|
| メインワールド | 上記コンポーネント — 公開されている全面 |
| `ExtractSchedule` | それらとフレームのデルタをレンダーワールドへコピー |
| `Render` / `RenderSystems::Prepare` | 再生ツリーへ適用し、`src` が変わったパックをロード |
| `Core2d` / `Core2dSystems::MainPass` | 前進・構築・描画。**`main_transparent_pass_2d` の後**に順序付け |

最後の順序は、同じ set に入れた偶然ではなく明示的なエッジです。レンダーコンテキストを取る 2 つの
システムはそれぞれ自分のコマンドエンコーダを持ち、どちらのバッファが先にキューへ届くかは明示しなければ
決まりません。これが無いと、キャラクターが Bevy のスプライトの上に出るか下に出るかが競合になります。

## このアダプタがまだやらないこと

| | |
|---|---|
| **ビューは 1 つ** | `SSScene::update` はタイムラインの前進とジオメトリ構築を一体で行うため、2 つ目のビューは全アニメーションを 1 フレームに 2 回進めてしまいます。2 つ目以降のビューは警告付きでスキップされます |
| **MSAA** | オフ必須。マルチサンプルのターゲットに対しては、クラッシュせず 1 度警告してスキップします |
| **アセット** | ロードは同期で、`AssetServer` を経由せず直接ファイルシステムを読みます。ホットリロード・パックされたアセット・アトラス共有はありません |
| **パーツオーバーライド、モーションブレンディング** | コンポーネントとして公開されていません。[スコープと名前の対応](../guide/concepts.md#what-each-face-can-do)を参照 |

## マスクとデバイスロスト

マスクは自動的に**カバレッジ**バックエンドになります。Bevy の 2D 深度テクスチャがステンシルを持たない
ためです。コストはマスクを使うキャラクター 1 体につき 1 パス／フレーム（[マスク](../guide/masks.md)）。

デバイスロストは Bevy 自身の復帰に乗りますが、**opt-in が必要**です。`RenderErrorPolicy::Recover` を返す
`RenderErrorHandler` を入れてください（[デバイスロスト](../guide/device-loss.md)）。
