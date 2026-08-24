# 🔄 SS6Player for CPP からのマイグレーション

[← ドキュメント目次へ戻る](./index.md)

旧プレイヤ **[SS6Player for CPP](https://github.com/SpriteStudio/SS6PlayerForCpp)** から本プレイヤ（**SSPlayer for wgpu**）へ移行する場合の要点をまとめます。

本プレイヤは、実装言語（C++ → Rust コア）、描画バックエンド（OpenGL 固定 → wgpu / WebGPU / DirectX 12 / Metal / Vulkan）、アセットフォーマット（`.ssfb2` → `.ssab`）のいずれも一新されています。単純なライブラリ差し替えではなく、**アセットの再コンバートとホストコード（C++ または Rust）の書き換え**が必要です。

> **C / C++ で利用を継続する場合**  
> `ffi` 機能で提供される C API（[`include/ssplayer_wgpu.h`](../../libs/ssplayer-wgpu/include/ssplayer_wgpu.h)）を使用することで、既存の C++ アプリケーション（DirectX 12 / Metal / Vulkan 等）からシームレスに利用できます。本書では **Rust API** と **C API (C++)** の両方を併記しています。

---

## 1. アセットフォーマットとコンバートの変更

| 項目 | SS6Player for CPP | 本バージョン |
|---|---|---|
| **ファイル形式** | **`.ssfb2`**（FlatBuffers） | **`.ssab`**（SpriteStudio Anime Binary） |
| **ファイル単位** | 1 プロジェクト = 1 ファイル（大容量） | **アニメーションパック（ssee / ssce を含む）ごとに 1 ファイル** |
| **テクスチャ形式** | 常に外部 PNG | **外部参照（Reference）** または **埋め込み（Embedded）** |
| **コンバータ** | SS6Converter（GUI / CLI） | **`ssconverter-cli`**（SpriteStudio-SDK 同梱） |
| **名前引き** | 文字列比較 | **uint32 ハッシュ**ベース（高速） |

> ⚠️ **`.ssfb2` は本バージョンでは読み込めません。`.sspj` から改めて `.ssab` へコンバートしてください。**

### コンバート手順

SpriteStudio-SDK の `ssconverter-cli` を使用して `.sspj` をコンバートします。

```bash
# 1. コンバータのビルド（初回のみ）
git submodule update --init
cargo build -p ssconverter-cli --release

# 2. 外部テクスチャ参照モード（標準: .ssab + PNG 出力）
./target/release/ssconverter-cli --out-dir <出力先ディレクトリ> path/to/Project.sspj

# 3. テクスチャ埋め込みモード（アトラス画像を .ssab 内にパック）
./target/release/ssconverter-cli --embedded --out-dir <出力先ディレクトリ> path/to/Project.sspj
```

---

## 2. アーキテクチャとライフサイクルの変更（グローバルシステム → 借用モデル）

- **SS6Player for CPP**:
  - `SpriteStudio6::SystemBootUp()` で始まるグローバルなシステム層を持ち、毎フレーム `SystemProcessMainPre()` / `MainPost()` / `RenderPost()` を呼ぶ必要がありました。OpenGL のステートやバッファ、テクスチャをプレイヤ内部で専有・管理していました。
- **本バージョン**:
  - **グローバルな初期化・終了処理はありません**。
  - wgpu の `Device` / `Queue` / `Surface` は**ホストアプリケーションが所有**し、フレーム描画時にプレイヤへ貸し出します（**借用モデル / Borrow Model**）。プレイヤがこれらを保持・専有しないため、任意のゲームエンジンや描画フレームワーク（自作エンジン、DirectX 12/Metal ホスト、Bevy、Slint 等）へロックインなしで相乗りできます。

### クラス・概念の対応表

| 役割 | SS6Player for CPP | 本バージョン（Rust） | 本バージョン（C API） | 備考 |
|---|---|---|---|---|
| システム初期化 / 終了 | `SystemBootUp` / `SystemShutDown` | （不要） | （不要） | ホスト側で wgpu/Device を用意 |
| デバイス単位の描画資源 | （内部・暗黙） | **`SSRenderer`** | （`ss_scene_t` 内包） | パイプライン・サンプラー・テクスチャ（Device ごとに 1 つ） |
| 描画ターゲット（画面 / RT） | （区別なし） | **`SSScene`** | **`ss_scene_t`** | 投影行列・頂点アリーナ・ステンシル管理 |
| キャラクター 1 体 | `Entity` | **`SSPlayer`** | **`ss_player_t`** | 再生木・トランスフォーム（**GPU 資源非所有**） |
| アセット保持 | `Project` | `SSPlayer`（内部で `SSPack` を保持） | `ss_player_t` | `load` で自動ロード |

### 1 つのシーンに複数のキャラクターを乗せる（N 体まとめ描画）

ForCPP では `Entity::Draw()` を個別に呼んでいましたが、本バージョンでは **`SSScene` に複数の `SSPlayer` を登録し、1 つのレンダーパスでまとめて描画** します。

```
[SS6Player for CPP]
Entity A -> Draw() -> (個別バッファ転送・ドロー)
Entity B -> Draw() -> (個別バッファ転送・ドロー)

[本バージョン]
SSScene -> update(&renderer, &device, &queue, dt, &[playerA, playerB])
        -> draw(&renderer, &mut render_pass)  // 1 レンダーパス・共有頂点アリーナで描画
```

---

## 3. アセットロードとテクスチャ供給

- **SS6Player for CPP**: `.ssfb2` を読み込んで `Project.BootUp()` した後、`Project.CountGetTexture()` の数だけ画像ファイルを**ホスト側で 1 枚ずつ読み込み**、`Project.TextureSet()` で登録する必要がありました。
- **本バージョン**: **`load(path)` の 1 呼び出し** で、パック本体・アトラス画像（外部/埋め込み）・テクスチャチェンジ（TCHG）対象画像・インスタンスパーツが参照する兄弟 `.ssab` まで自動的に再帰解決します。

```rust
// Rust:
player.load("assets/character/character.ssab")?;
```

```c
/* C API (C++): */
ss_player_load(player, "assets/character/character.ssab");
```

> [!NOTE]
> - `load` 呼び出し時点では `Device` や `Queue` を渡しません。読み込まれた画像データは最初の `update` 実行時に GPU テクスチャへ自動転送されます。
> - ロードできなかった画像や兄弟パックの有無は **`missing_assets()`**（C: `ss_player_missing_count` / `ss_player_missing_at`）で確認できます。

---

## 4. 再生制御 API の変更

### 4-1. トラック（Track）概念の廃止とアニメーション名直接指定

- **SS6Player for CPP**: `IndexGetAnimation("walk")` でインデックスを取得し、`AnimationPlay(indexTrack, indexAnimation, ...)` に渡していました。
- **本バージョン**: トラックの概念はありません。`set_animation("walk")` で**アニメーション名を直接指定**し、`play()` で再生します。

```c
/* SS6Player for CPP */
int idx = entity.IndexGetAnimation("walk");
entity.AnimationPlay(0, idx, 0); // track 0, 無限ループ
```

```rust
// 本バージョン (Rust)
player.set_animation("walk")?;   // 名前で直接指定
player.set_loop_count(-1);       // -1 = 無限ループ
player.play();
```

```c
/* 本バージョン (C API) */
ss_player_set_animation(player, "walk");
ss_player_set_loop_count(player, -1);
ss_player_play(player);
```

### 4-2. メソッド・プロパティ対応表

| SS6Player for CPP | 本バージョン（Rust） | C API | 備考 |
|---|---|---|---|
| `IndexGetAnimation(name)` + `AnimationPlay(track, index, …)` | `set_animation(name)` + `play()` | `ss_player_set_animation` / `ss_player_play` | **アニメーション選択と再生開始を分離** |
| `AnimationStop(track)` | `stop()` | `ss_player_stop` | 停止（再生ヘッドはその場に残り、巻き戻さない） |
| `AnimationPause(track, true/false)` | `pause()` / `resume()` | `ss_player_pause` / `ss_player_resume` | 一時停止の解除は `resume()`。`play()` は先頭出しなので再開には使えない |
| `AnimationPlay(…, timesPlay)` | `set_loop_count(n)` | `ss_player_set_loop_count` | `-1` / `0` = 無限, `n` = n回 |
| `RateTimeSet(track, rate)` | `set_speed(f32)` | `ss_player_set_speed` | 正の再生速度倍率（1.0 = 標準） |
| `rateTime` に負値を与えて逆再生 | `set_playback_direction(Backward, style)` | `ss_player_set_playback_direction` | **速度と再生方向を分離** |
| `KindStylePlay::PINGPONG` | `set_playback_direction(dir, PlaybackStyle::PingPong)` | 同上 | 往復再生 |
| `AnimationPlay(…, frameOffset)` / `CursorSet(track, cursor)` | `seek(f32)` | `ss_player_seek` | フレーム位置指定（小数可） |
| `CursorGet(track)` | `frame_no()` | `ss_player_frame_no` | 現在フレーム番号 |
| `labelRangeStart` / `labelRangeEnd` | `set_animation_section(start, end)` | `ss_player_set_animation_section` | **フレーム番号で指定** |
| `AnimationPlay(…, framePerSecond)` | `set_frame_rate(fps)` | `ss_player_set_frame_rate` | fps 指定 |
| `StatusGetPlay(track)` | `is_playing()` | `ss_player_is_playing` | 再生中か（一時停止中も `true`） |
| `StatusGetPause(track)` | `is_paused()` | `ss_player_is_paused` | 一時停止中か（未再生時と `stop()` 後は `false`） |
| `CountGetAnimation()` / `NameGetAnimation(i)` | `animation_names()` | `ss_player_animation_count` / `_name` | アニメーション名一覧 |
| `Entity::RateSetOpacity(rate)` | `set_alpha(f32)` | `ss_player_set_alpha` | プレイヤ全体の不透明度 (0.0〜1.0) |
| `Entity::RateSetScaleLocal()` / `Draw` の `matrixWorld` | `set_transform(x, y, sx, sy, rot)` | `ss_player_set_transform` | 位置・拡大縮小・回転（ラジアン） |
| （描画順は呼び出し順） | `set_order(i32)` | `ss_player_set_order` | 同一シーン内での前後描画順 |
| （なし） | `set_visible(bool)` | `ss_player_set_visible` | 非表示設定（非表示時は更新スキップ） |
| `CanvasGet()` | `canvas_size()` | `ss_player_canvas_size` | 基準キャンバスサイズ (w, h) |

### 4-3. ループ回数の設定値

| 意味 | SS6Player `timesPlay` | 本バージョン `set_loop_count` |
|---|---|---|
| 無限ループ | `0` | **`-1`（推奨）** または `0` |
| 1 回だけ再生 | `1` | `1` |
| n 回再生 | `n` | `n` |

> [!IMPORTANT]
> **`set_animation()` は再生設定を初期化します**  
> `set_animation()` を呼ぶと、再生ツリーが再構築され、ループ回数・速度・再生方向・区間はデータ既定値に戻ります。`set_loop_count` や `set_speed` は **`set_animation()` の後に設定** してください。

---

## 5. イベント / コールバックの変更

- **SS6Player for CPP**: `Entity` の `CallBackUserData` / `CallBackSignal` / `CallBackPlayEnd` に関数ポインタとユーザーポインタ（`void*`）を登録し、`Entity::Update` の実行中にコールバックされていました。
- **本バージョン**: **更新後にポーリング（取得）する方式** に変更されました。借用モデルでは、プレイヤ更新中にホストへ制御を戻すと貸出中のリソースが競合するリスクがあるためです。

### イベント対応表

| SS6Player for CPP | 本バージョン（Rust） | C API | ペイロード |
|---|---|---|---|
| `CallBackUserData` | `events()` の `SsEvent::UserData` | `ss_player_event_at` | パーツ名/ID、int/float/string/rect/point |
| `CallBackSignal` | `events()` の `SsEvent::Signal` | 同上（＋ `ss_player_event_signal_param_at`） | コマンドID、パラメータ配列 |
| `CallBackPlayEnd` | `finished_last_update()` | `ss_player_finished_last_update` | 再生終了判定（無限ループ時は `false`） |
| （なし・新規） | `looped_last_update()` | `ss_player_looped_last_update` | ループ境界通過判定 |
| （なし・新規） | `events()` の `SsEvent::Audio` | `ss_player_event_at` | サウンドアセット名等 |

### イベント取得例

```rust
// Rust:
scene.update(&mut renderer, &device, &queue, dt, &mut [&mut player]);

// 1. フレーム内イベントの走査
for event in player.events() {
    match event {
        SsEvent::UserData(e) => {
            println!("UserData: part={}, frame={}", e.part_name, e.frame_no);
            if let Some(val) = e.value_int { /* ... */ }
            if let Some(txt) = &e.value_string { /* ... */ }
        }
        SsEvent::Signal(e) => {
            println!("Signal: command={}", e.command);
        }
        SsEvent::Audio(e) => {
            println!("Audio: sound={}", e.name);
        }
    }
}

// 2. ループ・終了判定
if player.looped_last_update() {
    println!("アニメーションが1ループしました");
}
if player.finished_last_update() {
    println!("全ループ再生が終了しました");
}
```

```c
/* C API (C++): */
ss_scene_update(scene, dt);

/* イベントの取得 */
uint32_t count = ss_player_event_count(player);
for (uint32_t i = 0; i < count; ++i) {
    ss_event_t event;
    if (ss_player_event_at(player, i, &event) == SS_OK) {
        if (event.kind == SS_EVENT_USER_DATA) {
            /* event.part_name, event.value_int 等を参照 */
        }
    }
}

if (ss_player_finished_last_update(player)) {
    /* 再生終了処理 */
}
```

> [!NOTE]
> - `events()` は直前の `update` で発生した分だけを保持し、次の `update` で自動クリアされます。
> - 処理落ち（可変デルタタイム）で複数フレーム進んだ場合でも、通過したフレームのイベントは**取りこぼさずすべて列挙**されます。

---

## 6. パーツ単位の操作（パーツオーバーライド）

パーツの特定は、従来の `idParts`（`IDGetParts(name)`）から **`find_part_index(name)`** に変更されました。パーツの表示非表示やセル差し替え、カラー上書きはオーバーライド API を使用します。

| SS6Player for CPP | 本バージョン（Rust） | C API |
|---|---|---|
| `IDGetParts(name)` | `find_part_index(name)` | `ss_player_find_part_index` |
| `CountGetParts()` | `part_count()` | `ss_player_part_count` |
| `HideSet(idParts, flag, cascade)` | `set_part_visibility_override(part, hide, cascade)` | `ss_player_set_part_visibility_override` |
| `CellChangeParts(id, map, cell, …)` | `set_part_cell_override(part, cellmap_name, cell_name, priority)` | `ss_player_set_part_cell_override` |
| （相当機能なし） | `set_part_color_override(part, blend_op, rgba, priority)` | `ss_player_set_part_color_override` |
| （相当機能なし） | `clear_all_part_overrides()` | `ss_player_clear_all_part_overrides` |
| `PartGetTransform(id, ...)` | `part_transform(part)` | `ss_player_part_transform` |

> ForCPP ではセルマップやセルをインデックス番号で指定していましたが、本バージョンでは**セルマップ名・セル名の文字列**で直感的に指定できます。また、上書きの有効期間（キーフレームまで / アニメーション中 / パック中恒久）を `OverridePriority` で制御できます。

---

## 7. 実践コード対比（Before / After）

### C++: SS6Player for CPP（Before）

```cpp
#include <ss6player_platform.h>
#include <Entity/ss6player_entity.h>
#include <Project/ss6player_project.h>

// 初期化
SpriteStudio6::SettingSystem setting;
SpriteStudio6::SystemBootUp(setting);

SpriteStudio6::Project project;
project.BootUp(ssfb2Data);
// テクスチャの手動読み込み・登録
for (int i = 0; i < project.CountGetTexture(); ++i) {
    project.TextureSet(i, LoadTexture(project.NameGetTexture(i)));
}

SpriteStudio6::Entity entity;
entity.BootUp();
entity.ProjectSet(&project);
entity.PackSet(0);

int animIdx = entity.IndexGetAnimation("Attack");
entity.AnimationPlay(0, animIdx, 1); // 1回再生
entity.RateSetScaleLocal(1.5f, 1.5f);

// 毎フレーム
SpriteStudio6::SystemProcessMainPre();
entity.Update(deltaTime);
SpriteStudio6::SystemProcessMainPost();

entity.Draw(commandList, matProj, matView, matWorld);
SpriteStudio6::SystemProcessRenderPost();

// 破棄
entity.ShutDown();
project.ShutDown();
SpriteStudio6::SystemShutDown();
```

### C++: SSPlayer for wgpu（After / C API 経由）

```cpp
#include <ssplayer_wgpu.h>

// 初期化（DX12 / Metal / Vulkan のネイティブウィンドウからシーンを作成）
ss_scene_desc_t desc = {};
desc.metal_layer     = metalLayer;  // macOSの場合（Windowsは hwnd / hinstance）
desc.width           = 1280;
desc.height          = 720;
desc.clear_color[0]  = 0.1f;
desc.clear_color[1]  = 0.1f;
desc.clear_color[2]  = 0.1f;
desc.clear_color[3]  = 1.0f;

ss_scene_t* scene = ss_scene_create(&desc);
ss_player_t* player = ss_scene_create_player(scene);

// ロード（テクスチャや依存ファイルも自動解決）
ss_player_load(player, "assets/character/character.ssab");
ss_player_set_animation(player, "Attack");
ss_player_set_loop_count(player, 1);
ss_player_set_transform(player, 640.0f, 360.0f, 1.5f, 1.5f, 0.0f);
ss_player_play(player);

// 毎フレーム
ss_scene_update(scene, deltaTime);
ss_scene_render(scene);

// イベント確認
if (ss_player_finished_last_update(player)) {
    // 攻撃アニメーション完了
}

// 破棄
ss_player_destroy(player);
ss_scene_destroy(scene);
```

### Rust: SSPlayer for wgpu（After / Rust ネイティブ）

```rust
use ssplayer_wgpu::{SSRenderer, SSScene, SSPlayer, SceneConfig};

// 初期化（Device / Queue / SurfaceFormat はホストが用意）
let mut renderer = SSRenderer::new(&device);
let mut scene = SSScene::new(&device, &SceneConfig {
    color_format: surface_format,
    stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8),
});

let mut player = SSPlayer::new();
player.load("assets/character/character.ssab")?;
player.set_animation("Attack")?;
player.set_loop_count(1);
player.set_transform(640.0, 360.0, 1.5, 1.5, 0.0);
player.play();

// 毎フレーム
scene.set_viewport(&queue, 1280.0, 720.0, true);
scene.update(&mut renderer, &device, &queue, delta_time, &mut [&mut player]);

let mut rpass = encoder.begin_render_pass(&render_pass_desc);
scene.draw(&renderer, &mut rpass);
drop(rpass);

// イベント確認
if player.finished_last_update() {
    // 攻撃アニメーション完了
}
```

---

## 8. 未対応の機能・仕様差分

移行にあたり、ForCPP に存在し本バージョンで仕様が異なる機能は以下の通りです。

1. **マルチトラック / アニメーションブレンド**:
   ForCPP の複数トラック同時再生および `Transition()`（TRS アニメーションのブレンド）は、コア計算層（`ssruntime`）で実装中ですが、プレイヤ API としては未公開です。当面は**複数の `SSPlayer` を作成して描画順（`set_order`）で重ねる**ことで代用してください。
2. **シーケンサー（`.ssqb`）**:
   ForCPP の `Sequencer` クラス（複数アニメーションの連続再生）は、現在 `finished_last_update()` を検知して次のアニメーションを `set_animation()` するホスト側コードで代用してください。
3. **パーツ単位の反転（Flip）**:
   ForCPP の `FlipSetParts()` のような個別パーツ反転オーバーライドはありません。反転はデータ側のキーフレーム設定に従います。
4. **エフェクトのシード値指定**:
   ForCPP の `SeedOffsetSet()` に相当する API はありません。エフェクトのシードは再生スロットごとに自動決定されます。
5. **再生区間のラベル指定**:
   `labelRangeStart` / `labelRangeEnd` などのラベル文字列指定は未対応です。フレーム番号による `set_animation_section(start, end)` を使用してください。

---

## 9. まとめとリンク

SSPlayer for wgpu への移行により、以下のメリットが得られます：
- **マルチ GPU バックエンド対応**: WebGPU / Vulkan / DirectX 12 / Metal を単一コードベースで高速描画
- **CPU 負荷の激減**: Programmable Vertex Pulling により頂点生成コストを大幅削減
- **シンプルなライフサイクル**: 借用モデルによるクリーンなエンジン統合と自動アセット解決

### 関連リンク
- [README](../../README.md)
- [クレート README (`libs/ssplayer-wgpu`)](../../libs/ssplayer-wgpu/README.md)
- [ROADMAP](../../ROADMAP.md)
- [C API サンプル: Metal C++ Viewer](../../examples/viewer-metal-cpp/README.md)
- [C API サンプル: DX12 C++ Viewer](../../examples/viewer-dx12-cpp/README.md)
- [C API サンプル: Vulkan C++ Viewer](../../examples/viewer-vulkan-cpp/README.md)
- [WebGPU / JS ガイド](../../examples/web/README.md)
