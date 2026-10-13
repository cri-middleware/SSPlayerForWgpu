---
title: Bevy API リファレンス
---

# Bevy API リファレンス

公開されているのは、プラグイン 1 つ・コンポーネント 4 つ（うち 1 つは任意）・状態コンポーネント 1 つ・フィードバックのメッセージ 4 つと命令のメッセージ 3 つだけです。生成ドキュメント：
<a href="../../api/ssplayer_bevy/index.html" target="_blank" rel="noreferrer">**`ssplayer_bevy` の rustdoc**</a>。

**Bevy 0.20.x のみ** — [Bevy ではじめる](../start/bevy.md#supported-bevy)を参照。

## プラグイン

```rust
app.add_plugins(SsPlayerPlugin);
```

extract / prepare / draw のシステムを追加し、レンダーワールド内でデバイススコープのレンダラーとターゲットスコープのシーンを所有します。他に必要なものはありません。

ただしカメラには 2 つ必要です：`Msaa::Off`（Player のパイプラインはシングルサンプル）と
`CompositingSpace::Srgb`（無いとキャラクターが目に見えて明るくなりすぎます）。

## コンポーネント

### `SsPlayer` — 何を再生するか

| フィールド | | |
|---|---|---|
| `pack: Handle<SsPack>` | `asset_server.load(...)` で得た `.ssab` | **変更すると、新しいパックが読み終わった時点で再生ツリーを再構築します** |
| `animation: String` | 選択するアニメーション。空文字は「まだ選ばない」 | 同上。ただし `transition_ms` を設定したときを除く。値が変わったときだけ働くので、システムが毎フレーム書いても構わない |
| `transition_ms: f32` | `animation` の替え方。`0`（既定）ならすぐ切り替え、0 より大きければその時間（ミリ秒）でクロスフェードする | 最初のアニメーションと、`pack` を替えた後の最初のアニメーションは、常にすぐ切り替わる |

トラック 0 のアニメーションを替える方法は `animation` だけです。トラック 0 にアニメーションを載せる `TrackCommand` は拒否されるので、`animation` が指す名前と、トラック 0 が再生しているものが食い違うことはありません。

`SsTransform` / `SsPlayback` / `SsPlaybackState` / `SsTrackState` は **required component** なので、`SsPlayer` だけを
spawn すれば 5 つとも既定値で付きます。

### `SsPack` — アセットとしての `.ssab`

`.ssab` は Bevy の `AssetServer` を通して、アプリが設定したソースから読み込まれます。ローダーは Player 自身の walk を回し、インスタンスパーツが再生する兄弟パックを読み、各アトラスを Bevy の `Image` として要求します。Player はアトラスのバイト列をそのまま扱うので、画像は非 sRGB で読みます。

* キャラクターは、パックとすべてのアトラスの読み込みが決着してから組み立てます。アトラス抜きのフレームは描かれません。読み込めなかったアトラスは白で描かれます。
* 同じハンドルを使うキャラクター同士は、パックのパースを 1 回、アトラスごとの GPU テクスチャを 1 枚だけ共有します。テクスチャはコピーせず Player に貸します。
* Bevy の `file_watcher` フィーチャーを有効にすると、`.ssab` や兄弟パックを変更したときはそれを再生しているキャラクターを組み直し、アトラスを変更したときはその場で貸し直します。
* `SsPack::pack()` はパース済みのパックで、名前と `animation_names()` を取れます。

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
| `speed_scale: f32` | `1.0` | 既定 fps の倍率。`0` と負値は停止 |
| `order: i32` | `0` | 描画順。小さいほど先。同値なら extract 順 |
| `alpha: f32` | `1.0` | キャラクター全体のフェード |
| `visible: bool` | `true` | false は描画をスキップします。タイムラインは進みます |
| `culled: bool` | `false` | true は描画だけでなく**タイムラインの前進もスキップ**します。画面外のキャラクター用 |
| `restart: u32` | `0` | **値を進める**（値が変われば何でも可）と先頭から再生し、ループ回数を張り直します |
| `direction: PlaybackDirection` | `Forward` | 再生ヘッドが進む向き |
| `style: PlaybackStyle` | `Normal` | 端に着いたら、`Normal` は先頭へ戻り、`PingPong` は折り返します |
| `loop_count: i32` | `-1` | 止まるまでの周回数。`-1` か `0` で無限。変えると残り回数を張り直します |
| `frame_rate: i32` | `0` | 既定 fps を置き換えます。`0` なら既定のまま。通常は `speed_scale` を使ってください |
| `frame_skip_enabled: bool` | `true` | オフにすると、実時間に合わせず tick ごとに 1 フレームずつ進めます |
| `sub_frame_enabled: bool` | `true` | オフにすると、描くフレームを整数に切り捨てます |
| `animation_section: Option<(i32, i32)>` | `None` | 再生ヘッドを現在のアニメーションの `start..=end` に制限します。設定すると再生ヘッドは区間の先頭へ移ります |

`restart` はフラグではなく世代カウンタです。ワンショットを何度でも再トリガーでき、後始末が要らず、
2 回連続で進めれば 2 回リスタートになります。

### `SsPlaybackState` — 再生が何をしたか（ポーリング）

| フィールド | |
|---|---|
| `frame_no: f32` | タイムラインが最後に着地したフレーム |
| `finished: bool` | アニメーションが終端に達したか |
| `playing_forward: bool` | 再生ヘッドが実際に進んでいる向き。ピンポンの戻りの脚でも false。音声はこれでゲートする |

### `SsTrackState` — トラックスタック（ポーリング） { #sstrackstate }

[`SsTrackCommand`](#motion-blending) が組み立てたスタックです。毎フレーム更新され、**1 フレーム遅れ**です。

| フィールド | |
|---|---|
| `tracks: Vec<SsTrackInfo>` | トラックごとに 1 つ（トラック 0 が先頭）。`main_source`・`alpha`・`blend`・`sync`・`frame`・`fade_progress`（フェードしていないときは `None`）、再生設定の `speed`・`frame_rate`・`frame_skip_enabled`・`direction`・`style`、`loop_count`（空のトラックでは `None`）、`finished`・`just_looped`・`outgoing_event_threshold` |
| `sources: Vec<SsSourceInfo>` | `TrackCommand::AddSource` で載せ、まだ再生中のソース。`source`（ハンドル）・`track`・`animation`・`weight`・`frame` |

### `SsCellmapTextures` — ランタイムテクスチャ差し替え（任意）
セルマップがバインドするテクスチャ丸ごとを、Bevy の画像に差し替えます（衣装・パレット変更）。

| フィールド | |
|---|---|
| `overrides: HashMap<String, Handle<Image>>` | セルマップ（または TextureChange の差し先）名 → バインドする画像 |

エントリを挿入するとオーバーライド、削除（またはコンポーネントごと削除）すると復元します。まだロード中の画像は、準備できたフレームで適用されるので、リクエストしたばかりのハンドルでも問題ありません。差し替えはキャラクター単位で、**`is_srgb: false` で読んだ** straight-alpha の Bevy 画像を前提とします。sRGB の画像は暗く描かれ、アダプタが 1 度だけ警告します。テクスチャを差し替えるキャラクターだけが spawn すれば十分です。[テクスチャ差し替え](../guide/texture-replacement.md)を参照。

## メッセージ（リアクティブ）

| メッセージ | 内容 |
|---|---|
| `SsEventMessage` | `entity`、`event: SsEvent` — ユーザーデータ・シグナル・オーディオ |
| `SsStarted` | `entity` — 開始**エッジ**：`playing` 中のアニメーション・パックの切り替え、初めて `playing` を有効にしたとき、`restart` の加算。再開では立ちません |
| `SsFinished` | `entity` — 完了**エッジ** |
| `SsLooped` | `entity` — ループ境界エッジ |

逆向き、つまりゲームプレイからキャラクターへ送る命令が 3 つあります。どれもキャラクターの状態ではなく 1 度だけ起こることなので、フィールドではなくメッセージにしています。パックの読み込み前に送ったものはアニメーションの設定が済んだ時点で反映され、同じ種類を 1 フレームに複数送ると最後のものが効きます。

| メッセージ | 内容 |
|---|---|
| `SsPlay` | `entity`、`start_frame: Option<f32>` — **play**：区間の先頭か `start_frame` へ戻し、ループ回数を張り直して再生を始めます。終了・停止したアニメーションも始まります。`None` は `restart` を進めるのと同じです。その後は `playing` に従います |
| `SsStop` | `entity` — **stop**：その位置で再生を終えます。そのフレームは描かれたままで、`playing` を切り替えても再開しません。再開は `SsPlay` か `restart` で行います |
| `SsSeek` | `entity`、`frame: f32` — 再生ヘッドをそこへ移します。小数フレーム可で、区間の外には出ません |

同じフレームに一緒に送ると、play・stop・seek の順に反映されます。

### モーションブレンディング { #motion-blending }

`SsTrackCommand { entity, command }` は、キャラクターのトラックスタックに対する操作を 1 つ行います。`command` は `TrackCommand` で、各値は `SSPlayer` の同名の呼び出しに対応します（`SetAnimation`・`Crossfade`・`FadeTrack`・`CancelFade`・`Clear`・`SetAlpha`・`SetBlend`・`SetSync`・`SetFrame`・`SetSpeed`・`SetFrameRate`・`SetFrameSkipEnabled`・`SetPlaybackDirection`・`SetLoopCount`・`SetOutgoingEventThreshold`・`AddSource`・`SetSourceWeight`・`SetSourceFrame`・`SetSourceEventThreshold`）。`SetAnimation` と `Crossfade` は、トラック 0 では拒否されます（トラック 0 は `SsPlayer::animation` が設定します）。`SetSpeed`・`SetFrameRate`・`SetFrameSkipEnabled`・`SetPlaybackDirection`・`SetLoopCount` も、トラック 0 では拒否されます（トラック 0 の再生設定とループ回数は `SsPlayback` で設定します）。上の 3 つと違い、**送ったものはすべて、送った順に反映されます**。パックの読み込み前に送ったものは、アニメーションの準備ができてから反映されます。拒否された操作はログに出ます。結果は [`SsTrackState`](#sstrackstate) に戻り、`AddSource` で載せたソースのハンドルもそこで分かります。

```rust
fn run(mut tracks: MessageWriter<SsTrackCommand>, character: Single<(Entity, &mut SsPlayer)>) {
    let (entity, mut player) = character.into_inner();
    tracks.write(SsTrackCommand { entity, command: TrackCommand::SetSync { track: 0, sync: TrackSync::Start } });
    // トラック 0 はコンポーネントでクロスフェードする。
    player.transition_ms = 300.0;
    player.animation = "run".into();
}

fn react(mut tracks: MessageWriter<SsTrackCommand>, character: Single<Entity, With<SsPlayer>>) {
    let entity = *character;
    for command in [
        TrackCommand::SetAnimation { track: 1, animation: "hit_react".into() },
        TrackCommand::SetLoopCount { track: 1, count: 1 },   // 1 回再生して、最後のフレームで止まる
    ] {
        tracks.write(SsTrackCommand { entity, command });
    }
}
```

**`transition_ms` が 0 のまま `SsPlayer::animation` を変えると、スタックはトラック 0 だけに戻ります**（`set_animation` と同じ）。オーバーレイやトラックの設定は、そのあと送り直してください。`transition_ms` を設定していれば、`crossfade` と同じくスタックはそのまま残ります。[モーションブレンディング](../guide/motion-blending.md)を参照。
```rust
fn jump(mut seeks: MessageWriter<SsSeek>, character: Single<Entity, With<SsPlayer>>) {
    seeks.write(SsSeek { entity: *character, frame: 18.0 });
}
```

```rust
fn on_finish(mut finished: MessageReader<SsFinished>, mut commands: Commands) {
    for m in finished.read() {
        commands.entity(m.entity).despawn();
    }
}
```

!!! warning "どちらのフィードバックも 1 フレーム遅れです"
    再生はレンダーワールドで、そのフレームの `Update` より後に動きます。今フレーム読める値は、前フレームに再生が生成したものです。音のキューや状態変化なら知覚できませんが、これを許容できないホストは再生をメインワールドへ移す必要がありますが、このアダプタはそれをしていません。

## 処理がどこで走るか

| スケジュール | 内容 |
|---|---|
| メインワールド | 上記コンポーネント — 公開されている全面 |
| `ExtractSchedule` | それらとフレームのデルタをレンダーワールドへコピー |
| `Render` / `RenderSystems::Prepare` | 再生ツリーへ適用し、`pack` か `animation` が変わったキャラクターを組み直す |
| `Core2d` / `Core2dSystems::MainPass` | 前進・構築・描画。**`main_transparent_pass_2d` の後**に順序付け |

最後の順序は、同じ set に入れた偶然ではなく明示的なエッジです。レンダーコンテキストを取る 2 つのシステムはそれぞれ自分のコマンドエンコーダを持ち、どちらのバッファが先にキューへ届くかは明示しなければ決まりません。これが無いと、キャラクターが Bevy のスプライトの上に出るか下に出るかが競合になります。

## このアダプタがまだやらないこと

| | |
|---|---|
| **ビューは 1 つ** | アダプタは全キャラクターを 1 つの `SSScene` に入れており、`SSScene::update` はタイムラインの前進とジオメトリ構築を一体で行うため、2 つ目のビューは全アニメーションを 1 フレームに 2 回進めてしまいます。2 つ目以降のビューは警告付きでスキップされます |
| **MSAA** | オフ必須。マルチサンプルのターゲットに対しては、クラッシュせず 1 度警告してスキップします |
| **パーツオーバーライド** | コンポーネントとして公開されていません。[スコープと名前の対応](../guide/concepts.md#what-each-face-can-do)を参照 |

## マスクとデバイスロスト

マスクは自動的に**カバレッジ**バックエンドになります。Bevy の 2D 深度テクスチャがステンシルを持たないためです。コストはマスクを使うキャラクター 1 体につき 1 パス／フレーム（[マスク](../guide/masks.md)）。

デバイスロストは Bevy 自身の復帰に乗りますが、**opt-in が必要**です。`RenderErrorPolicy::Recover` を返す
`RenderErrorHandler` を入れてください（[デバイスロスト](../guide/device-loss.md)）。
