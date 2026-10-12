---
title: C API リファレンス
---

# C API リファレンス

`ssplayer_wgpu.h` の全エントリポイントを用途別に並べたものです。関数ごとの詳細と安全性の注意はヘッダ自身に書かれています。このページはその見取り図です。

ヘッダは[リリース zip かソースビルド](../start/cpp.md#install)から入手します。C99 で、依存はありません。

## 約束事

| | |
|---|---|
| 戻り値 | 失敗しうる関数は `int32_t`：`SS_OK`（0）、`SS_ERROR_INVALID_ARGUMENT`（-1）、`SS_ERROR_GPU`（-2） |
| エラーテキスト | `ss_last_error()` — 呼び出しスレッドで最後に起きた失敗。ログ用であり、分岐用ではありません |
| バージョン | `ss_version()` — このライブラリのバージョン。`ss_sdk_version()` — リンクしている SpriteStudio-SDK のリリースのタグ。リリースでない SDK なら `local`。どちらも静的な文字列で、解放しません |
| null ハンドル | どの関数も null ハンドルを許容し、クラッシュではなく `SS_ERROR_INVALID_ARGUMENT`（またはゼロ値）を返します |
| 文字列 | 入力は UTF-8 の `const char *`。**出力**の文字列はシーンが所有し、同種の次の呼び出しまで有効です — 保持するならコピーを |
| サイズ | 常に**描画ピクセル**。ポイントではありません。ディスプレイ倍率は自分で掛けてください |

## ライフタイム

| | |
|---|---|
| `ss_scene_create(desc)` | ウィンドウ上にシーンを構築。失敗時は null |
| `ss_scene_destroy(scene)` | 破棄 — **その中の全プレイヤーも** |
| `ss_scene_create_player(scene)` | キャラクターを追加 |
| `ss_player_destroy(player)` | 1 体削除。ハンドルの箱はシーンが消えるまで残るので、二重破棄は安全な no-op |
| `ss_scene_player_count(scene)` | 診断用 |

ハンドルは世代付きです。古い `ss_player_t *` は「破棄済み」に解決し、そのスロットを引き継いだ別のキャラクターには決して解決しません。

## フレーム

| | |
|---|---|
| `ss_scene_update(scene, delta_seconds)` | 全プレイヤーを進め、このフレームのジオメトリを構築 |
| `ss_scene_render(scene)` | キャスト全体を 1 レンダーパスで描画し、present |
| `ss_scene_resize(scene, width, height)` | 描画ピクセル |
| `ss_scene_stats(scene, out)` | [パフォーマンス](../guide/performance.md)を参照 |
| `ss_scene_is_device_lost(scene)` / `ss_scene_restore_device(scene)` | [デバイスロスト](../guide/device-loss.md)を参照 |

## ロード

| | |
|---|---|
| `ss_player_load(player, path)` | 1 呼び出しでパック・アトラス・兄弟パック |
| `ss_player_load_ssab(player, name, data, len)` | バイト列を登録し、そのパックを選択対象にする |
| `ss_player_add_ssab(player, name, data, len)` | 登録のみ（インスタンスパーツ用） |
| `ss_player_missing_count(player)` / `ss_player_missing_at(player, i, out)` | 届かなかったもの。**エラーではありません** |
| `ss_player_texture_count(player, pack)` / `ss_player_texture_at(player, pack, i, out)` | パックが要求するアトラスと、エクスポータが書いた位置 |
| `ss_scene_add_texture(scene, key, rgba, w, h)` | ストレートアルファ RGBA8 でアトラスを供給（`SsTextureRef::key` で） |
| `ss_scene_add_texture_png(scene, key, data, len)` | エンコード済み PNG を渡してライブラリにデコードさせる |

テクスチャは**シーン**に登録します。テクスチャマップがデバイススコープだからです。ウィンドウ内の全プレイヤーから見え、同じパックを 5 体で表示してもアップロードは 1 回です。

## アニメーションの選択と情報

| | |
|---|---|
| `ss_player_set_animation(player, name)` | 再生ツリーを構築。アロケーションを伴う呼び出し |
| `ss_player_animation_count(player)` / `ss_player_animation_name(player, i)` | パックが持つもの |
| `ss_player_current_animation(player)` | 設定中のアニメーション。無ければ null |
| `ss_player_part_name(player, i)` | パーツ名（パーツインデックス順）。末尾を過ぎると null |
| `ss_player_cellmap_count` / `ss_player_cellmap_name(player, i)` / `ss_player_cell_count(player, cellmap)` / `ss_player_cell_name(player, cellmap, i)` | セルのオーバーライドに渡す名前 |
| `ss_player_canvas_size(player, &w, &h)` | オーサリング時のキャンバス。周囲のレイアウト用 |
| `ss_player_total_frames(player)` / `ss_player_start_frame` / `ss_player_end_frame` | 現在の範囲 |

## 再生制御

| | |
|---|---|
| `ss_player_play(player)` | **先頭出し** — 巻き戻してループ回数を張り直す |
| `ss_player_play_from(player, frame)` | 指定フレームから同じことを |
| `ss_player_pause(player)` / `ss_player_resume(player)` | 保持と、その場からの継続 |
| `ss_player_stop(player)` | 終了。再生ヘッドはその場に残る |
| `ss_player_seek(player, frame)` | ジャンプ。小数フレーム可 |
| `ss_player_set_speed_scale(player, speed)` / `ss_player_speed_scale` | 既定 fps の倍率。`0` と負値は停止 |
| `ss_player_set_frame_rate(player, fps)` / `ss_player_frame_rate` | 既定 fps を置き換える |
| `ss_player_set_loop_count(player, n)` / `ss_player_loop_count` / `ss_player_remaining_loops` | `n` 回。`-1` / `0` で無限 |
| `ss_player_set_animation_section(player, start, end)` | 再生範囲を制限。`set_animation` の**後**に |
| `ss_player_set_playback_direction(player, dir, style)` | `0`/`1` = 順/逆、通常/ピンポン |
| `ss_player_playback_direction` / `ss_player_playback_style` | 読み戻し |
| `ss_player_set_sub_frame_enabled(player, on)` / `ss_player_sub_frame_enabled` | フレーム間を補間 |
| `ss_player_set_frame_skip_enabled(player, on)` / `ss_player_frame_skip_enabled` | 長い tick で複数フレームを跨ぐ |

[再生と再生制御](../guide/playback.md)も参照してください。特に `pause` の解除に `play` を使えない理由を。

## 再生が何をしたかを問い合わせる

| | |
|---|---|
| `ss_player_is_playing` / `ss_player_is_pausing` / `ss_player_is_playing_forward` | 状態 |
| `ss_player_is_finished` | **持続的**：設定されたループをすべて再生し終えた。無限ループでは常に false |
| `ss_player_just_started` | **エッジ**：`ss_player_play` / `ss_player_play_from` の後の最初の update でだけ true。`ss_player_resume` では立ちません |
| `ss_player_just_finished` | **エッジ**：再生を完了させた update でだけ true。`stop` では立ちません |
| `ss_player_frame_no` | 再生ヘッドの位置 |
| `ss_player_just_looped` | **エッジ**：直前の update がループ境界を跨いだ。何周跨いでも 1 回の報告 |

## イベント

| | |
|---|---|
| `ss_player_event_count(player)` | 直前の update が跨いだ数 — **スキップしたフレームの分も含みます** |
| `ss_player_event_at(player, i, out)` | `SsEventData` 1 件。`kind` がどのフィールドに値があるかを示します |
| `ss_player_event_signal_param_count(player, i)` / `ss_player_event_signal_param_at(player, i, p, out)` | シグナルのパラメータ |

`kind` は `SS_EVENT_USER_DATA`（0）/ `SS_EVENT_SIGNAL`（1）/ `SS_EVENT_AUDIO`（2）。シグナルパラメータの
`value_type` は `SS_SIGNAL_VALUE_NONE` / `_BOOL` / `_INDEX` / `_INTEGER` / `_FLOATING` / `_TEXT`（0–5）です。[イベントとユーザーデータ](../guide/events.md)を参照。

## キャラクターの配置

| | |
|---|---|
| `ss_player_set_transform(player, x, y, sx, sy, rotation)` | 位置・スケール・回転 |
| `ss_player_set_order(player, order)` / `ss_player_order(player)` | シーン内の描画順（昇順） |
| `ss_player_set_visible(player, visible)` / `ss_player_is_visible(player)` | 非表示のキャラクターは描画されませんが、再生は続きイベントも出します |
| `ss_player_set_culled(player, culled)` / `ss_player_is_culled(player)` | カリング中のキャラクターは**進まず**、イベントも出しません |
| `ss_player_set_alpha(player, a)` / `ss_player_set_tint(player, rgb)` | キャラクター全体。`ss_player_alpha` / `ss_player_tint` で読み出せます |

## パーツとオーバーライド

各オーバーライドには名前探索を省く `_by_index` 版があります。`ss_player_find_part_index` で一度だけ引いて保持してください。

| | |
|---|---|
| `ss_player_part_count` / `ss_player_find_part_index` | 列挙と解決 |
| `ss_player_part_transform(player, part, out)`（＋ `_by_index`） | float 6 個：`[m00, m01, m10, m11, tx, ty]`、アニメーションローカルの Y-down |
| `ss_player_is_part_hidden(player, part)`（＋ `_by_index`） | 表示中のフレームで `1` 非表示、`0` 表示、`-1` は不明なパーツかアニメーション未設定 |
| `ss_player_is_part_skinned_mesh(player, part)`（＋ `_by_index`） | `1` skinned、`0` not、`-1` は不明なパーツかパック未バインド |
| `ss_player_set_part_visibility_override(player, part, hide, cascade)`（＋ `_by_index` / `clear_`） | キーフレームの非表示はカスケードしないので、サブツリーを隠す唯一の方法です |
| `ss_player_set_part_color_override(player, part, rgba, blend_op, priority)`（＋ `_corners` / `_by_index` / `clear_`） | `blend_op` 0–3、`priority` 0–2 |
| `ss_player_set_part_cell_override(player, part, cellmap, cell, priority)`（＋ `_by_index` / `clear_`） | 別の画像を描く |
| `ss_player_clear_all_part_overrides(player)` | まとめてリセット |

`blend_op` と `priority` の意味は[パーツオーバーライド](../guide/part-overrides.md)を参照。

## ランタイムテクスチャ差し替え

セルマップがバインドするテクスチャ丸ごとを、プログラムから差し替えます（衣装・パレット変更）。
`pack` に null でメインパックが選ばれます。

| | |
|---|---|
| `ss_player_cellmap_texture_count(player, pack)` / `ss_player_cellmap_texture_name(player, pack, i)` | 差し替え可能な名前：セルマップと TextureChange の差し先の両方 |
| `ss_player_set_cellmap_texture(player, pack, name, rgba, w, h)` | straight-alpha RGBA8 で差し替え。**`rgba` に NULL で元に戻す** |
| `ss_player_is_cellmap_overridden(player, pack, name)` | 現在オーバーライドされているか |

差し替えテクスチャは元アトラスとサイズ・セルレイアウトを合わせてください。次のシーン更新で
GPU に届きます。[テクスチャ差し替え](../guide/texture-replacement.md)を参照。

## モーションブレンディング { #motion-blending }

トラックスタックです。トラックはセットアップポーズの上に昇順に重なり、トラック 0 は `ss_player_set_animation` が確立するものです。[モーションブレンディング](../guide/motion-blending.md)を参照。

| | |
|---|---|
| `ss_player_set_track_animation(player, track, name)` | トラックにアニメーションを載せる（それまでの再生内容を置き換える） |
| `ss_player_crossfade(player, track, name, ms)` | トラックを別のアニメーションへフェードで切り替える。トラック 0 では、呼んだ時点から現在のアニメーションになる |
| `ss_player_fade_track(player, track, target, ms)` / `ss_player_cancel_track_fade(player, track)` | トラックの alpha を目標値へ動かす／フェードをその場で止める |
| `ss_player_track_fade_progress(player, track)` | 0〜1。フェードしていないときは **-1** |
| `ss_player_set_track_alpha` / `ss_player_track_alpha` | トラックの強さ |
| `ss_player_set_track_blend` / `ss_player_track_blend` | `SS_TRACK_BLEND_MIX`（0）か `SS_TRACK_BLEND_ADDITIVE`（1） |
| `ss_player_set_track_sync` / `ss_player_track_sync` | `SS_TRACK_SYNC_FREE`（0）か `SS_TRACK_SYNC_START`（1）。載せる前に設定する |
| `ss_player_set_track_frame` / `ss_player_track_frame` / `ss_player_set_track_speed` | トラックのスクラブ。速度 0 で止まる |
| `ss_player_track_speed` / `ss_player_set_track_frame_rate` / `ss_player_track_frame_rate` / `ss_player_set_track_frame_skip_enabled` / `ss_player_track_frame_skip_enabled` | トラックごとの再生設定。トラック 0 の設定は再生制御の呼び出し |
| `ss_player_set_track_playback_direction(player, track, direction, style)` / `ss_player_track_playback_direction` / `ss_player_track_playback_style` | `ss_player_set_playback_direction` と同じ `0` / `1` |
| `ss_player_set_track_loop_count` / `ss_player_track_loop_count` | トラックのアニメーションのループ回数。載せた後で設定する。`-1` は無限、**0** はアニメーションが無い |
| `ss_player_is_track_finished` / `ss_player_track_just_looped` | トラックの終了（状態）とループ（その update だけ） |
| `ss_player_set_track_outgoing_event_threshold` / `ss_player_track_outgoing_event_threshold` | クロスフェードの出ていく側が、どこまでイベントを発火し続けるか。既定の無限大では発火しない |
| `ss_player_track_count(player)` / `ss_player_clear_track(player, track)` | トラックの数／トラックを空にする（トラック 0 は不可） |
| `ss_player_track_main_source(player, track)` | トラックのアニメーションを担うソース。無いときは **`SS_INVALID_SOURCE`** |
| `ss_player_add_track_source(player, track, name, weight, &source)` | 別のアニメーションを横に載せ、weight で混ぜる |
| `ss_player_set_source_weight` / `_frame` / `_event_threshold` と、`set_` の無い取得 | ソース 1 つの weight・再生位置・イベントのしきい値 |

操作は状態コードを返します。存在しないトラックやソースを読むと **0** が返るので、先に `ss_player_track_count` で確かめてください。例外は上で印を付けた 2 つで、0 が意味を持つ値だからです。`SsEventData.source` は、イベントを発火したソースを表します。

## この Face に無いもの

自分が持つレンダーパスへの描画は Rust だけです。この Face はデバイスを自分で構築し所有します。全体像は[スコープと名前の対応](../guide/concepts.md#what-each-face-can-do)、デバイス共有を提供しない理由は[制限事項](../limitations.md#no-device-sharing-across-the-c-abi)を参照してください。
