# SS6Player for CPP からの移行

[SS6Player for CPP](https://github.com/SpriteStudio/SS6PlayerForCpp) からの移行は、3 つが同時に変わります：実装言語（C++ → Rust コア）、描画バックエンド（OpenGL 固定 → wgpu：WebGPU / Vulkan / DX12 / Metal）、アセットフォーマット（`.ssfb2` → `.ssab`）。

単純なライブラリ差し替えではありません。**アセットの再コンバート**と**ホストコードの書き換え**を見込んでください。ただしホストコードは相当小さくなります。このページはその新旧対応表です。

!!! tip "C++ のまま使い続けても構いません"
    [C API](../reference/c.md) は後付けではなく一級の Face で、既存の DX12 / Metal / Vulkan アプリケーションがそのまま叩けます。以下では Rust と C の両方を併記します。

## 1. アセット

| | SS6Player for CPP | 本 Player |
|---|---|---|
| 形式 | `.ssfb2`（FlatBuffers） | **`.ssab`**（SpriteStudio Anime Binary） |
| 粒度 | 1 プロジェクト＝1 ファイル（大きい） | **アニメーションパックごとに 1 ファイル** |
| テクスチャ | 常に外部 PNG | **外部参照（reference）** または **埋め込み（embedded）** |
| コンバータ | SS6Converter | **`ssconverter-cli`**（SDK 同梱）または [SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) |
| 名前引き | 文字列比較 | **`uint32` ハッシュ** |

!!! danger "`.ssfb2` は読み込めません"
    元の `.sspj` から再コンバートしてください（[アセットの準備](../start/assets.md)）。

## 2. ライフサイクル：グローバルシステムから借用モデルへ

**SS6Player for CPP** はグローバルなシステム層を持っていました。`SystemBootUp()` に始まり、毎フレーム
`SystemProcessMainPre()` / `MainPost()` / `RenderPost()` を呼び、OpenGL のステート・バッファ・テクスチャは
Player の内部が持っていました。

**本 Player にはグローバルな初期化も終了処理もありません。** Rust からは、wgpu の `Device` / `Queue` / `Surface` はアプリケーションのもので、フレームごとに貸し出します。だからこそ、どんなレンダラーやエンジンにもロックインなしで相乗りできます（[借用モデル](../wgpu/borrow-model.md)）。C からは、`ss_scene_create` が渡したウィンドウからそれらを作り、`ss_scene_destroy` が解放します。

| 役割 | SS6Player for CPP | Rust | C API |
|---|---|---|---|
| システム初期化 / 終了 | `SystemBootUp` / `SystemShutDown` | *(不要)* | *(不要)* |
| デバイス単位の描画資源 | *(内部・暗黙)* | `SSRenderer` | *(`ss_scene_t` に内包)* |
| 描画ターゲット | *(区別なし)* | `SSScene` | `ss_scene_t` |
| キャラクター 1 体 | `Entity` | `SSPlayer` | `ss_player_t` |
| アセット保持 | `Project` | `SSPlayer`（内部に `SSPack`） | `ss_player_t` |

### 複数キャラクターを 1 パスで

`Entity::Draw()` はキャラクター単位でした。本 Player では、**そのターゲットの全キャラクターが 1 回の
update と 1 つのレンダーパスを通ります**：

```text
[SS6Player for CPP]
Entity A -> Draw()   （個別のバッファ転送とドロー）
Entity B -> Draw()   （個別のバッファ転送とドロー）

[本 Player]
SSScene -> update(&mut renderer, &device, &queue, dt, &mut [a, b])
        -> draw(&renderer, &mut rpass)      // 1 パス、共有ジオメトリアリーナ
```

## 3. ロード

**以前**：`.ssfb2` を読んで `Project.BootUp()`、そして `Project.CountGetTexture()` の回数だけ画像を自分で読み込み、`Project.TextureSet()` で登録。

**現在**：1 呼び出しで、パック本体・アトラス（外部／埋め込み、テクスチャチェンジ対象を含む）・インスタンスパーツが再生する兄弟 `.ssab` まで解決されます。

=== "Rust"

    ```rust
    player.load("assets/character/character.ssab")?;
    ```

=== "C"

    ```c
    ss_player_load(player, "assets/character/character.ssab");
    ```

2 点だけ：`load` は **`Device` も `Queue` も受け取りません**（読んだものは最初の `update` で GPU に届きます）。そしてアトラスの欠落は**エラーではなく** `missing_assets()` / `ss_player_missing_count` のエントリです（[ロードとアセット](../guide/loading.md)）。

## 4. 再生

### アニメーションは名前で選ぶ

`IndexGetAnimation("walk")` ＋ `AnimationPlay(track, index, …)` が、`set_animation("walk")` ＋ `play()` になります。

=== "Rust"

    ```rust
    player.set_animation("walk")?;
    player.set_loop_count(-1);       // -1 = 無限
    player.play();
    ```

=== "C"

    ```c
    ss_player_set_animation(player, "walk");
    ss_player_set_loop_count(player, -1);
    ss_player_play(player);
    ```

=== "SS6Player for CPP"

    ```cpp
    int idx = entity.IndexGetAnimation("walk");
    entity.AnimationPlay(0, idx, 0);   // track 0, 無限ループ
    ```

!!! note "「トラック」の意味が変わりました"
    SS6 のトラックは 1 エンティティ上の並列再生スロットでした。本 Player にも[トラックスタック](../guide/motion-blending.md)がありますが、こちらは*ブレンド*のスタック（レイヤー・重み・クロスフェード）で、再生制御ではなく C の Face の `ss_player_set_track_animation` など（Rust API では `set_track_animation` など）から触ります。

### 対応表

| SS6Player for CPP | Rust | C API | 備考 |
|---|---|---|---|
| `IndexGetAnimation` ＋ `AnimationPlay` | `set_animation(name)` ＋ `play()` | `ss_player_set_animation` / `_play` | **選択と再生開始は別** |
| `AnimationStop(track)` | `stop()` | `ss_player_stop` | 再生ヘッドはその場に残ります |
| `AnimationPause(track, true/false)` | `pause()` / `resume()` | `ss_player_pause` / `_resume` | **`play()` は巻き戻すので解除には使えません** |
| `AnimationPlay(…, timesPlay)` | `set_loop_count(n)` | `ss_player_set_loop_count` | `-1` / `0` で無限 |
| `RateTimeSet(track, rate)` | `set_speed_scale(f32)` | `ss_player_set_speed_scale` | 正の倍率。`0` と負値は停止 |
| `rateTime` に負値 | `set_playback_direction(Backward, …)` | `ss_player_set_playback_direction` | **速度と方向は別** |
| `KindStylePlay::PINGPONG` | `set_playback_direction(dir, PingPong)` | 同じ呼び出し | |
| `AnimationPlay(…, frameOffset)` / `CursorSet` | `seek(f32)` / `play_from(f32)` | `ss_player_seek` / `_play_from` | 小数フレーム可 |
| `CursorGet(track)` | `frame_no()` | `ss_player_frame_no` | |
| `labelRangeStart` / `labelRangeEnd` | `set_animation_section(start, end)` | `ss_player_set_animation_section` | **フレーム番号で指定**。ラベル文字列は未対応 |
| `AnimationPlay(…, framePerSecond)` | `set_frame_rate(fps)` | `ss_player_set_frame_rate` | |
| `StatusGetPlay(track)` | `is_playing()` | `ss_player_is_playing` | 一時停止中も true |
| `StatusGetPause(track)` | `is_pausing()` | `ss_player_is_pausing` | |
| *(なし)* | `is_playing_forward()` | `ss_player_is_playing_forward` | 再生ヘッドが**実際に**進んでいる向き。ピンポンの戻りの脚では、方向が Forward のままでも false。**音声はこれでゲートしてください** |
| `CountGetAnimation` / `NameGetAnimation` | `animation_names()` | `ss_player_animation_count` / `_name` | |
| `Entity::RateSetOpacity` | `set_alpha(f32)` | `ss_player_set_alpha` | |
| `RateSetScaleLocal` / `Draw` の `matrixWorld` | `set_transform(x, y, sx, sy, rot)` | `ss_player_set_transform` | 回転はラジアン |
| *(描画順は呼び出し順)* | `set_order(i32)` | `ss_player_set_order` | シーン内で昇順 |
| *(なし)* | `set_visible(bool)` | `ss_player_set_visible` | 非表示は描画だけをスキップ。タイムラインは進みます |
| *(なし)* | `set_culled(bool)` | `ss_player_set_culled` | カリングは**タイムラインの前進もスキップ**。画面外のキャラクター用 |
| *(なし)* | `canvas_size()` | `ss_player_canvas_size` | |

### ループ回数

| 意味 | SS6 の `timesPlay` | `set_loop_count` |
|---|---|---|
| 無限 | `0` | **`-1`**（推奨）または `0` |
| 1 回 | `1` | `1` |
| n 回 | `n` | `n` |

!!! warning "`set_animation` が保つもの・捨てるもの"
    速度・フレームレート上書き・再生方向・フレームスキップは `set_animation` を**跨いで残ります**。そして**ループ回数**は、他の Player と同様、本 Player が意図的に復元します。

    残らないのは**再生区間**だけです。フレーム番号は別のクリップに対して意味を持たないためです。つまり `set_animation` のたびに `set_animation_section` を張り直し、それ以外はリセットされたと思い込まないでください。

## 5. イベント：コールバックからポーリングへ

**以前**：`CallBackUserData` / `CallBackSignal` / `CallBackPlayEnd` に関数ポインタを登録し、
`Entity::Update` の*最中*に呼ばれていました。

**現在**：update の後に読みます。借用モデルでは、更新の途中でアプリケーションへ制御を戻すと、まさに貸し出されているリソースを巡って競合するおそれがあるためです。

| SS6Player for CPP | Rust | C API |
|---|---|---|
| `CallBackUserData` | `events()` の `SsEvent::UserData` | `ss_player_event_at`（`kind == SS_EVENT_USER_DATA`） |
| `CallBackSignal` | `events()` の `SsEvent::Signal` | 同上 ＋ `ss_player_event_signal_param_at` |
| `CallBackPlayEnd` | `just_finished()` | `ss_player_just_finished` |
| *(新規)* | `just_looped()` | `ss_player_just_looped` |
| *(新規)* | `events()` の `SsEvent::Audio` | `ss_player_event_at`（`kind == SS_EVENT_AUDIO`） |

=== "Rust"

    ```rust
    scene.update(&mut renderer, &device, &queue, dt, &mut [&mut player]);

    for event in player.events() {
        match event {
            SsEvent::UserData(e) => println!("user data: part={} frame={}", e.part_name, e.frame_no),
            SsEvent::Signal(e)   => println!("signal: {}", e.command_id),
            SsEvent::Audio(e)    => println!("audio: {}", e.sound_name),
        }
    }

    if player.just_looped()   { /* 1 ループした */ }
    if player.just_finished() { /* 設定分すべて再生し終えた */ }
    ```

=== "C"

    ```c
    ss_scene_update(scene, dt);

    uint32_t count = ss_player_event_count(player);
    for (uint32_t i = 0; i < count; ++i) {
        ss_event_data_t e;
        if (ss_player_event_at(player, i, &e) != SS_OK) continue;
        if (e.kind == SS_EVENT_USER_DATA) {
            if (e.has_integer) { /* e.integer */ }
            if (e.has_text)    { /* e.text */ }
        }
    }

    if (ss_player_just_finished(player)) { /* ... */ }
    ```

イベントはそれを生成した update のもので、次の update でクリアされます。**長い tick で跨いだフレームの分もすべて報告される**ので、フレームレートが落ちても取りこぼしません（[イベントとユーザーデータ](../guide/events.md)）。

## 6. パーツ

パーツの特定は `IDGetParts(name)` から `find_part_index(name)` に変わり、パーツ単位の変更は寿命が明示された*オーバーライド*になりました。

| SS6Player for CPP | Rust | C API |
|---|---|---|
| `IDGetParts(name)` | `find_part_index(name)` | `ss_player_find_part_index` |
| `CountGetParts()` | `part_count()` | `ss_player_part_count` |
| `HideSet(id, flag, cascade)` | `set_part_visibility_override(part, hide, cascade)` | `ss_player_set_part_visibility_override` |
| `CellChangeParts(id, map, cell, …)` | `set_part_cell_override(part, cellmap, cell, priority)` | `ss_player_set_part_cell_override` |
| *(相当なし)* | `set_part_color_override(part, rgba, blend_op, priority)` | `ss_player_set_part_color_override` |
| *(相当なし)* | `clear_all_part_overrides()` | `ss_player_clear_all_part_overrides` |
| *(相当なし)* | `part_transform(part)` | `ss_player_part_transform` |

セルマップとセルはインデックスではなく**文字列**で指定します。また、各オーバーライドは[優先度](../guide/part-overrides.md#blend-op-and-priority)を持ち、それがどれだけの期間生きるかを決めます。

## 7. プログラム全体の対比

=== "SS6Player for CPP"

    ```cpp
    #include <ss6player_platform.h>
    #include <Entity/ss6player_entity.h>
    #include <Project/ss6player_project.h>

    SpriteStudio6::SettingSystem settingSystem;
    SpriteStudio6::SystemBootUp(settingSystem);

    SpriteStudio6::Project project;
    project.BootUp(dataSsfb2);
    for (int i = 0; i < project.CountGetTexture(); i++) {
        /* 画像は自分で読み込んでから： */
        project.TextureSet(i, myTexture);
    }

    SpriteStudio6::Entity entity;
    entity.BootUp(project);
    entity.PackSet(0);
    entity.AnimationPlay(0, entity.IndexGetAnimation("walk"), 0);

    /* 毎フレーム */
    SpriteStudio6::SystemProcessMainPre();
    entity.Update(deltaTime, SpriteStudio6::Library::KindMasking::FOLLOW_DATA);
    entity.Draw(commandList, matrixProjection, matrixView, matrixWorld);
    SpriteStudio6::SystemProcessMainPost();
    SpriteStudio6::SystemProcessRenderPost();
    ```

=== "C"

    ```c
    #include <ssplayer_wgpu.h>

    ss_scene_desc_t desc = {
        .window      = { .kind = SS_WINDOW_WIN32, .window = hwnd },
        .backend     = SS_BACKEND_AUTO,
        .width       = 960, .height = 540,
        .clear_color = { 0.1f, 0.1f, 0.12f, 1.0f },
        .enable_mask = true,
    };
    ss_scene_t *scene = ss_scene_create(&desc);

    ss_player_t *player = ss_scene_create_player(scene);
    ss_player_load(player, "assets/character/character.ssab");   /* テクスチャ込み */
    ss_player_set_animation(player, "walk");
    ss_player_set_loop_count(player, -1);
    ss_player_play(player);

    /* 毎フレーム */
    ss_scene_update(scene, delta_seconds);
    ss_scene_render(scene);

    ss_scene_destroy(scene);   /* プレイヤーも一緒に */
    ```

=== "Rust"

    ```rust
    use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

    let mut renderer = SSRenderer::new(&device);
    let mut scene = SSScene::new(&device, &SceneConfig {
        color_format: surface_format,
        stencil_format: Some(wgpu::TextureFormat::Depth24PlusStencil8),
        mask: MaskBackend::Stencil,
    });
    scene.set_viewport(&queue, width as f32, height as f32, false);

    let mut player = SSPlayer::new();
    player.load("assets/character/character.ssab")?;
    player.set_animation("walk")?;
    player.set_loop_count(-1);
    player.play();

    // 毎フレーム
    scene.update(&mut renderer, &device, &queue, dt, &mut [&mut player]);
    scene.draw(&renderer, &mut rpass);
    ```

## 8. SS6 にあって扱いが変わったもの { #8-things-ss6-had-that-work-differently }

| | |
|---|---|
| **エンティティごとの複数トラック** | [トラックスタック](../guide/motion-blending.md)に置き換わりました。単に並列再生するのではなくブレンドします。C の Face では `ss_player_set_track_animation`・`ss_player_crossfade` などで操作します |
| **シーケンサー（`.ssqb`）** | 相当機能はありません。`just_finished()` を検知して次を `set_animation` してください |
| **パーツ単位の反転（`FlipSetParts`）** | ありません。反転はオーサリングされたキーフレームに従います |
| **エフェクトのシード（`SeedOffsetSet`）** | ありません。シードは再生スロットごとに決まります |
| **ラベルによる区間指定** | ありません。`set_animation_section` にフレーム番号を渡してください |

## 得られるもの

* **WebGPU / Vulkan / DX12 / Metal** を 1 つのコードパスで；
* 大幅に小さいホストコード（グローバルシステム無し、テクスチャループ無し、ロードは 1 呼び出し）；
* 1 レンダーパスに複数キャラクター、アトラスアップロードは共有；
* [モーションブレンディング](../guide/motion-blending.md)（1 体のキャラクターでアニメーションを重ね、クロスフェードする）。

## 次に読む

* [C / C++ ではじめる](../start/cpp.md) — C++ のまま続ける場合
* [Rust ではじめる](../start/rust.md) — 移行する場合
* [制約と適用範囲](../limitations.md)
