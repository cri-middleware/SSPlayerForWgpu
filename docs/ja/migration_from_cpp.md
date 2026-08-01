# SS6Player for CPP からのマイグレーション

旧プレイヤ **[SS6Player for CPP](https://github.com/SpriteStudio/SS6PlayerForCpp)** から本プレイヤへ移行する場合は、以下の大きな変更点にご注意ください。本プレイヤは実装言語（C++ → Rust）、描画バックエンド（OpenGL 固定 → wgpu）、アセットフォーマット（`.ssfb2` → `.ssab`）のいずれも一新されています。単純な差し替えではなく、**アセットの再コンバートとホストコードの書き換え**が必要になります。

C / C++ のまま利用を続ける場合は、`ffi` フィーチャで生成される C API（`include/ssplayer_wgpu.h`）を使用してください。本書の対応表には Rust API と C API の両方を併記しています。

## 1. アセットフォーマットの変更（ssfb2 → ssab）

| 項目 | SS6Player for CPP | 本バージョン |
|---|---|---|
| ファイル形式 | **`.ssfb2`**（ssbp2 / FlatBuffers） | **`.ssab`**（SpriteStudio Anime Binary）＋ シーケンスは `.ssqb` |
| ファイル単位 | 1 プロジェクト = 1 ファイル | **アニメーションパックごとに 1 ファイル**。各 `.ssab` は自己完結し、そのパックが参照する ss*e だけを内包します |
| 名前引き | 文字列比較 | **uint32 ハッシュ**ベース |
| テクスチャ | 常に外部 PNG | **外部参照 / 埋め込み**の 2 形態に対応 |
| 変換ツール | SS6Converter | **`ssconverter-cli`**（SpriteStudio-SDK に同梱） |

**`.ssfb2` は本バージョンでは読み込めません。`.sspj` から改めて変換し直してください。**

```bash
# SpriteStudio-SDK 側でコンバータをビルド
cargo build -p ssconverter-cli --release

# .sspj → .ssab（＋参照テクスチャ）
ssconverter-cli --out-dir <出力先> path/to/Project.sspj

# テクスチャを .ssab に埋め込む場合
ssconverter-cli --embedded --out-dir <出力先> path/to/Project.sspj
```

この作業は本リポジトリの [`scripts/deploy-examples.sh`](../../scripts/deploy-examples.sh) が実例です。

## 2. 統合の形の変更（グローバルシステム → 借用モデル）

- **SS6Player for CPP**: `SpriteStudio6::SystemBootUp(settingSystem)` で始まるグローバルなシステム層を持ち、毎フレーム `SystemProcessMainPre()` / `SystemProcessMainPost()` / `SystemProcessRenderPost()` を呼ぶ必要がありました。OpenGL の状態はプレイヤ側が管理します。
- **本バージョン**: **グローバルな初期化も終了処理もありません**。wgpu の `Device` / `Queue` / `Surface` は**ホストが所有**し、フレームごとにプレイヤへ貸し出します（**借用モデル**）。プレイヤがそれらを保持することは設計上ありません。

| 役割 | SS6Player for CPP | 本バージョン |
|---|---|---|
| システム初期化 | `SystemBootUp` / `SystemShutDown` | （不要） |
| 毎フレームの前後処理 | `SystemProcessMainPre` / `MainPost` / `RenderPost` | （不要） |
| データ（プロジェクト） | `Project` | `SSPlayer` が内包（`load`） |
| キャラクター 1 体 | `Entity` | **`SSPlayer`** |
| デバイス単位の描画資源 | （内部・暗黙） | **`SSRenderer`** — ホストが 1 つだけ持つ |
| 描画先（ウィンドウ / RT） | （区別なし） | **`SSScene`** |
| シーケンス再生 | `Sequencer` | （未対応 — [§8](#8-未対応の機能仕様差分)） |

**1 つの `SSScene` に複数の `SSPlayer` を入れられます。** `Entity` ごとに `Draw()` を呼んでいた ForCPP と異なり、シーンにプレイヤを登録して 1 回のレンダーパスでまとめて描画します。描画順は `SSPlayer::set_order` で指定します。

### 2-1. 毎フレームの流れ（Rust）

```rust
use ssplayer_wgpu::{SSRenderer, SSScene, SSPlayer, SceneConfig};

// 初期化（Device はホストが用意する）
let mut renderer = SSRenderer::new(&device);                 // デバイス単位に 1 つ
let mut scene = SSScene::new(&device, &SceneConfig {         // 描画先ごとに 1 つ
    color_format: surface_format,
    stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8),
});
let mut player = SSPlayer::new();
player.load("assets/overall/overall.ssab")?;
player.set_animation("Action01_Body")?;
player.play();

// 毎フレーム
scene.set_viewport(&queue, width as f32, height as f32, /* y_down */ true);
scene.update(&mut renderer, &device, &queue, delta_secs, &mut [&mut player]);

let mut rpass = encoder.begin_render_pass(&desc);   // レンダーパスはホストが張る
scene.draw(&renderer, &mut rpass);
```

`SSScene::update` が ForCPP の `Entity::Update` に、`SSScene::draw` が `Entity::Draw` に相当します。**投影行列は `SSScene` が持つ**ため、`Draw(commandList, matrixProjection, matrixView, matrixWorld)` のように毎回渡す必要はありません。プレイヤ個別の位置・拡縮・回転は `SSPlayer::set_transform` で指定します。

### 2-2. 毎フレームの流れ（C API）

C 面ではシーンがサーフェスを持つため、レンダーパスの構築もシーン側で行います。

```c
ss_scene_t*  scene  = ss_scene_create(&desc);
ss_player_t* player = ss_scene_create_player(scene);
ss_player_load(player, "assets/overall/overall.ssab");
ss_player_set_animation(player, "Action01_Body");
ss_player_play(player);

/* 毎フレーム */
ss_scene_update(scene, delta_seconds);
ss_scene_render(scene);

/* 終了時 */
ss_player_destroy(player);   /* 二重呼び出しは安全な no-op */
ss_scene_destroy(scene);
```

失敗した呼び出しの理由は `ss_last_error()` で取得します。

## 3. データのロードとテクスチャ供給

- **SS6Player for CPP**: `.ssfb2` を自前で読み込んで `Project.BootUp(Data)`、続いて `Project.CountGetTexture()` の数だけ `texture/<名前>.png` を**アプリケーション側でロード**し、`Project.TextureSet(i, ...)` で 1 枚ずつ登録する必要がありました。
- **本バージョン**: **`SSPlayer::load(path)` の 1 呼び出し**で完結します。アトラス（外部参照・埋め込みの両モード）、TextureChange の差し替え先テクスチャ、インスタンスパーツが参照する兄弟 `.ssab` まで、パックが依存するものを再帰的に解決します。

```rust
player.load("assets/overall/overall.ssab")?;    // テクスチャの列挙・ロードは不要
```

移行時の注意点：

- **`load` は `Device` / `Queue` / `SSRenderer` を受け取りません。** 読み込んだ画像は最初の `SSScene::update` で GPU に上がるため、**`load` 直後はまだ GPU テクスチャが存在しません**（埋め込みアトラスと同じ経路です）。
- **読めなかったアセットは `Err` になりません。** `.ssab` 本体が読めなかったときだけ `Err` を返し、それ以外の取りこぼしは **`missing_assets()`**（C: `ss_player_missing_count` / `ss_player_missing_at`）に列挙されます。プログラムから分岐するのはこちらです。
- ファイルシステムを前提にできない環境（アーカイブ、パックファイル、wasm）では **`AssetLoader` を実装して `load_with`** を使ってください。`std::fs` を直接呼ぶ実装は wasm でビルドは通っても実行時に失敗します。
- 独自のアセット管理を使う場合は、ForCPP と同じく手動供給する経路も残っています（`add_ssab` / `SSRenderer::set_texture`、C: `ss_player_load_ssab` / `ss_scene_add_texture_png`）。

## 4. 再生制御 API の変更

**トラック番号（`indexTrack`）の概念はありません。** 1 つの `SSPlayer` が 1 つのアニメーションを再生します。

また、ForCPP の `AnimationPlay()` が 1 回の呼び出しで担っていた「アニメーション選択・再生開始・ループ回数・速度・再生区間・FPS」は、**それぞれ独立したメソッドに分離**されました。

```c
/* SS6Player for CPP */
int idx = entity.IndexGetAnimation("walk");
entity.AnimationPlay(0, idx, 0);        /* track 0, 無限ループ */
```

```rust
// 本バージョン
player.set_animation("walk")?;          // 名前で直接指定（インデックス取得は不要）
player.set_loop_count(-1);              // 無限ループ
player.play();
```

### 4-1. メソッド対応表

| SS6Player for CPP | 本バージョン（Rust） | C API | 備考 |
|---|---|---|---|
| `IndexGetAnimation(name)` + `AnimationPlay(track, index, …)` | `set_animation(name)` + `play()` | `ss_player_set_animation` / `ss_player_play` | **選択と再生開始を分離** |
| `AnimationStop(track)` | `stop()` | `ss_player_stop` | |
| `AnimationPause(track, true/false)` | `pause()` / `play()` | `ss_player_pause` / `ss_player_play` | `play()` が一時停止からの再開も兼ねます |
| `AnimationPlay(…, timesPlay)` | `set_loop_count(n)` | `ss_player_set_loop_count` | **値の対応は下記参照** |
| `AnimationPlay(…, rateTime)` / `RateTimeSet(track, rate)` | `set_speed(f32)` | `ss_player_set_speed` | |
| `rateTime` に負値を与えて逆再生 | `set_playback_direction(Backward, style)` | `ss_player_set_playback_direction` | **速度と方向が分離されました** |
| `KindStylePlay::PINGPONG` | `set_playback_direction(dir, PlaybackStyle::PingPong)` | 同上 | 方向とスタイルは対で指定します |
| `AnimationPlay(…, frameOffset)` / `CursorSet(track, cursor)` | `seek(f32)` | `ss_player_seek` | 小数フレーム可 |
| `CursorGet(track)` | `frame_no()` | `ss_player_frame_no` | |
| `labelRangeStart` / `labelRangeEnd` | `set_animation_section(start, end)` | `ss_player_set_animation_section` | **フレーム指定のみ。ラベル指定は未対応** |
| `AnimationPlay(…, framePerSecond)` | `set_frame_rate(fps)` | `ss_player_set_frame_rate` | |
| `StatusGetPlay(track)` | `is_playing()` | `ss_player_is_playing` | 一時停止中も `true` |
| `StatusGetPause(track)` | `is_paused()` | `ss_player_is_paused` | |
| `CountGetAnimation()` / `NameGetAnimation()` | `animation_names()` | `ss_player_animation_count` / `_name` | |
| `Entity::PackSet(index)` | `set_pack()` / `load()` が兼ねる | `ss_player_load` | |
| `Entity::RateSetOpacity(rate)` | `set_alpha(f32)` | `ss_player_set_alpha` | |
| `Entity::RateSetScaleLocal(x, y)` / `Draw` の `matrixWorld` | `set_transform(x, y, sx, sy, rot)` | `ss_player_set_transform` | |
| （描画順は呼び出し順） | `set_order(i32)` | `ss_player_set_order` | 1 シーンに複数プレイヤを置くため |
| （なし） | `set_visible(bool)` | `ss_player_set_visible` | |
| （なし） | `canvas_size()` / `part_transform(part)` | `ss_player_canvas_size` / `ss_player_part_transform` | |

### 4-2. ループ回数の値の違い

| 意味 | ForCPP `timesPlay` | 本バージョン `set_loop_count` |
|---|---|---|
| 無限ループ | `0` | `-1`（推奨）または `0` |
| 1 回だけ再生 | `1` | `1` |
| n 回再生 | `n` | `n` |
| 前回指定と同じ | `-1` | （相当なし。毎回明示してください） |

### 4-3. `set_animation` は再生設定をリセットします

`set_animation()` は再生ツリーを作り直すため、**ループ回数・速度・再生方向・再生区間はパックの既定値に戻ります。** ForCPP の `AnimationPlay` が「省略時は前回指定と同じ」という挙動だったのに対し、こちらは**毎回明示的に指定し直す**必要があります。

```rust
player.set_animation("walk")?;      // ← 先に呼ぶ
player.set_loop_count(-1);          // ← その後に設定する
player.set_speed(2.0);
player.set_playback_direction(PlaybackDirection::Backward, PlaybackStyle::Normal);
player.play();
```

## 5. イベント / コールバックの変更

- **SS6Player for CPP**: `Entity` の `CallBackUserData` / `CallBackSignal` / `CallBackPlayEnd` に**関数ポインタと `void*` を登録**する方式でした。コールバックは `Entity::Update` の内側から呼ばれ、`(entity, pointer, nameParts, idParts, indexAnimation, frameDecode, frameKeyData, userData, flagWayBack)` のように多数の引数を取ります。
- **本バージョン**: コールバックではなく、**更新後にポーリングする**方式です。借用モデルではプレイヤの更新中にホスト側のコードへ制御が戻ると、貸し出した `Device` / `Queue` の扱いが不定になるためです。

| SS6Player for CPP | 本バージョン（Rust） | C API |
|---|---|---|
| `CallBackUserData` | `events()` の `SsEvent::UserData` | `ss_player_event_count` / `ss_player_event_at` |
| `CallBackSignal` | `events()` の `SsEvent::Signal` | 同上（＋ `ss_player_event_signal_param_count` / `_at`） |
| `CallBackPlayEnd` | `finished_last_update()` | `ss_player_finished_last_update` |
| （なし・新規） | `looped_last_update()` | `ss_player_looped_last_update` |
| （なし・新規） | `events()` の `SsEvent::Audio` | `ss_player_event_at` |

```rust
scene.update(&mut renderer, &device, &queue, delta, &mut [&mut player]);

for event in player.events() {
    match event {
        SsEvent::UserData(e) => { /* e.part_index, 各値 */ }
        SsEvent::Signal(e)   => { /* e.params */ }
        SsEvent::Audio(e)    => { /* 効果音の再生など */ }
    }
}
if player.finished_last_update() { /* 再生終了 */ }
```

- `events()` は**その `update` で発生した分だけ**を保持し、次の `update` の先頭でクリアされます。毎フレーム読んでください。
- **飛ばされたフレームのイベントも取りこぼしません。** 処理落ちで複数フレーム進んだ場合も、通過したフレームのイベントがすべて列挙されます。
- `set_visible(false)` のプレイヤは更新自体が進まないため、イベントも発生しません。
- ForCPP の `TypeFunctionPlayEnd` にあった「エンティティの自動破棄」に相当する仕組みはありません。破棄が必要な場合は明示的に行ってください。
- **オーディオイベントは本バージョンの新機能**です（ForCPP には相当機能がありません）。

## 6. パーツ単位の操作

パーツは ForCPP の `idParts`（`IDGetParts(name)` で取得）に代わり、**`find_part_index(name)`** で得たインデックスで指定します。

| SS6Player for CPP | 本バージョン（Rust） | C API |
|---|---|---|
| `IDGetParts(name)` | `find_part_index(name)` | `ss_player_find_part_index` |
| `CountGetParts()` | `part_count()` | `ss_player_part_count` |
| `HideSet(idParts, flag, involveChildren)` | `set_part_visibility_override(part, hide, cascade)` | `ss_player_set_part_visibility_override` |
| `CellChangeParts(idParts, indexCellMap, indexCell, …)` | `set_part_cell_override(part, cellmap_name, cell_name, priority)` | `ss_player_set_part_cell_override` |
| （相当機能なし） | `set_part_color_override(part, blend_op, rgba, priority)` | `ss_player_set_part_color_override` |
| （相当機能なし） | `clear_all_part_overrides()` | `ss_player_clear_all_part_overrides` |
| `FlipSetParts(...)` | **相当機能なし**（[§8](#8-未対応の機能仕様差分)） | — |

ForCPP がセルマップ・セルを**インデックス**で指定していたのに対し、本バージョンは**名前**で指定します。また、上書きの有効期間を `OverridePriority` で選べます（次のキーフレームまで / 現在のアニメーション中 / 同じパックの再生中ずっと）。

## 7. 描画・プラットフォーム層

ForCPP の `src/Platform/OpenGL/` に相当する層は**存在しません**。wgpu が Vulkan / DX12 / Metal / GL / WebGPU を吸収するため、移植先ごとのプラットフォーム実装は不要です。

- ForCPP の `TypeDrawCommandList` を引き回す設計は無くなりました。Rust 面ではホストが張ったレンダーパスに `SSScene::draw` で描き込み、C 面では `ss_scene_render` がパスの構築まで行います。
- テクスチャは**プレイヤが所有しません**。`SSRenderer::set_texture` は供給側が所有する `TextureView` からバインドグループを作るだけなので、エンジン側のアセット管理と競合しません。
- パフォーマンス計測は `SSScene::stats()`（C: `ss_scene_stats`）で取得できます。

## 8. 未対応の機能・仕様差分

移行にあたって、ForCPP にあって本バージョンに無い機能は以下です。

- **マルチトラック / クロスフェード**: ForCPP の複数トラック再生と `Control::Animation::Track::Transition()`（TRS アニメーションのブレンド）に相当する API はありません。基盤となるブレンド機能は `libssruntime` 側に実装済みですが、本プレイヤからはまだ公開していません。レイヤー表現が必要な場合は、当面**複数の `SSPlayer` を併用**してください。
- **シーケンス再生（`.ssqb`）**: ForCPP の `Sequencer` クラスに相当する機能はありません。`.ssqb` 自体は SDK が変換まで対応しているため、`is_finished()` を監視して次のアニメーションを `set_animation()` する形で、ホスト側にステートマシンを組んで代替してください（SDK ROADMAP Phase 3 で `libssruntime` 側への内包を予定）。
- **エフェクトの乱数シード指定**: ForCPP の `Entity::SeedOffsetSet(seed)` に相当する API はありません。シードはスロット単位で固定されます。
- **パーツ単位の反転指定**: ForCPP の `FlipSetParts()` に相当する上書き API はありません。反転はアニメーションデータの指定に従います。
- **再生区間のラベル指定**: `labelRangeStart` / `labelRangeEnd` に相当する指定はなく、`set_animation_section()` は**フレーム番号のみ**を受け取ります。
- **Text / 9-slice パーツ**: ジオメトリが未実装です。
- **`.ssfb2` の読み込み**: 対応しません（[§1](#1-アセットフォーマットの変更ssfb2--ssab)）。

## 9. 対応環境

- Rust 1.97 以降
- **wgpu 29** 系がサポートするバックエンド（Vulkan / DX12 / Metal / GL / WebGPU）
- C / C++ から使う場合は `ffi` フィーチャ。ヘッダは `libs/ssplayer-wgpu/include/ssplayer_wgpu.h` に自動生成されます

詳細は [README](../../README.md) および設計正典 [`SSPLAYER_WGPU_SPEC.md`](../../SSPLAYER_WGPU_SPEC.md) を参照してください。
