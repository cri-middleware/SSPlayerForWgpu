# 再生と再生制御

動詞は 4 つあり、互換ではありません。このページを取り違えると「なぜ先頭に戻った？」が起きるので、
短く率直に書きます。

## 4 つの動詞

| 動詞 | 何をするか |
|---|---|
| **play** | **先頭出し。** 区間の先頭へ巻き戻し、ループ回数を張り直します |
| **pause** | 現在フレームで保持。冪等で、再生中扱いのまま — 保持は停止ではありません |
| **resume** | pause を解除し、同じフレームから継続。冪等で、**停止中には何もしません** |
| **stop** | 再生終了。再生ヘッドはその場に残るので、止まったフレームを描き続けられます |

!!! danger "`pause()` の解除に `play()` は使えません"
    `play()` は巻き戻します。一時停止の解除に使うと、再生ヘッドが黙って区間の先頭へ戻り、ループ
    カウンタもリセットされます。カクついたように見え、ランタイムの不具合のように読めるバグです。
    **その場から続けるのは `resume()` です。**

    逆も同様です。`resume()` は何かを*開始*できません。停止中のアニメーションに対しては何もしません。
    開始は `play` の仕事で、`play` は巻き戻すからです。この分離は SDK の再生制御契約であり、
    ファミリーのどの Player でも同じです。

=== "Rust"

    ```rust
    player.set_animation("attack")?;   // 選択 — アロケーションを伴う
    player.play();                     // 先頭から再生
    player.pause();                    // 保持
    player.resume();                   // 同じフレームから継続
    player.stop();                     // 終了。再生ヘッドはその場
    player.play_from(12.0);            // 指定フレームから再生（ループ回数も張り直す）
    ```

=== "C"

    ```c
    ss_player_set_animation(player, "attack");
    ss_player_play(player);
    ss_player_pause(player);
    ss_player_resume(player);
    ss_player_stop(player);
    ss_player_play_from(player, 12.0f);
    ```

=== "JavaScript"

    ```js
    player.setAnimation('attack');
    player.play();
    player.pause();
    player.resume();
    player.stop();
    player.playFrom(12);
    ```

=== "Bevy"

    ```rust
    // SsPlayback::playing が pause/resume にあたります。その場で保持・継続し、
    // 終了したアニメーションを再生し直すことはできません。
    playback.playing = false;   // pause
    playback.playing = true;    // resume

    // 先頭から再生するには世代カウンタを進めます（値が変われば何でも可）。
    playback.restart += 1;
    ```

## 速度・方向・ピンポン

**速度は倍率であって方向ではありません。** `0.0` と負値は*停止*で、読み戻すと `0.0` になっています。
逆再生は別の設定です。

=== "Rust"

    ```rust
    use ssplayer_wgpu::{PlaybackDirection, PlaybackStyle};

    player.set_speed_scale(2.0);                                    // 2 倍速
    player.set_playback_direction(PlaybackDirection::Backward, PlaybackStyle::Normal);
    player.set_playback_direction(PlaybackDirection::Forward, PlaybackStyle::PingPong);
    player.set_frame_rate(30);                                      // 既定 fps を置き換える
    ```

=== "C"

    ```c
    ss_player_set_speed_scale(player, 2.0f);
    ss_player_set_playback_direction(player, 1 /* 逆再生 */, 0 /* 通常 */);
    ss_player_set_playback_direction(player, 0 /* 順再生 */, 1 /* ピンポン */);
    ss_player_set_frame_rate(player, 30);
    ```

=== "JavaScript"

    ```js
    player.setSpeedScale(2.0);
    player.setPlaybackDirection(1, 0);   // 逆再生・通常
    player.setPlaybackDirection(0, 1);   // 順再生・ピンポン
    player.setFrameRate(30);
    ```

=== "Bevy"

    ```rust
    playback.speed = 2.0;   // 方向とスタイルはコンポーネントとして公開されていません
    ```

| 値 | 方向 | スタイル |
|---|---|---|
| `0` | 順再生 | 通常 — 端で先頭に戻る／ループ回数に従って停止 |
| `1` | 逆再生 | ピンポン — 折り返して戻る。1 ループは往復 |

この `0` / `1` の対応は全 Player 共通です。C と JavaScript が整数を取り、Rust が enum を取るのはそのためです。

**フレームレートより速度を優先してください。** 速度はオーサリングされたタイミングを倍率で扱い、
フレームレートはそれを置き換えます。

## ループ

`count` 回再生して停止。`-1` **または `0`** で無限。ピンポンでは 1 ループが往復 1 回です。

=== "Rust"

    ```rust
    player.set_loop_count(3);        // 3 回で停止
    player.set_loop_count(-1);       // 無限
    let left = player.remaining_loops();
    ```

=== "C"

    ```c
    ss_player_set_loop_count(player, 3);
    int32_t left = ss_player_remaining_loops(player);
    ```

=== "JavaScript"

    ```js
    player.setLoopCount(3);
    const left = player.remainingLoops();
    ```

## アニメーションの一部だけを再生する

**区間**は現在のアニメーションのフレーム範囲に再生ヘッドを制限します。ループもピンポンもその中で
折り返します。

必ず `set_animation` の**後**に設定してください。フレーム番号は、それを測ったアニメーションの中でしか
意味を持たず、アニメーションを選び直すとツリーが再構築されて区間も一緒に消えます。

=== "Rust"

    ```rust
    player.set_animation("walk")?;
    player.set_animation_section(10, 24);
    player.play();                          // フレーム 10 から
    player.seek(18.0);                      // 区間内へジャンプ
    ```

=== "C"

    ```c
    ss_player_set_animation(player, "walk");
    ss_player_set_animation_section(player, 10, 24);
    ss_player_play(player);
    ss_player_seek(player, 18.0f);
    ```

=== "JavaScript"

    ```js
    player.setAnimation('walk');
    player.setAnimationSection(10, 24);
    player.play();
    player.seek(18);
    ```

## アニメーション切り替えで何が残るか

アニメーションの選択は、既存の再生コンテキストに新しいクリップを渡す形なので、再生状態の多くは
そのまま引き継がれます。これはファミリー共通の挙動で、この Player 独自のものではありません。

| `set_animation` で**残る** | **残らない** |
|---|---|
| 速度倍率、フレームレート上書き、再生方向、フレームスキップ | **区間** — フレーム番号は別クリップに対して意味を持たないため |
| ループ回数（他 Player と同様、この Player が意図的に復元します） | |

名前の下のパックそのものが差し替わっていた場合はツリーがゼロから作り直され、そのときは何も引き継がれません。
新しいパックは新しい持ち主だからです。

## 再生位置を問い合わせる

| | Rust | C | JavaScript |
|---|---|---|---|
| 現在フレーム | `frame_no()` | `ss_player_frame_no` | `frameNo()` |
| 範囲 | `start_frame()` / `end_frame()` / `total_frames()` | `ss_player_start_frame` / `_end_frame` / `_total_frames` | `startFrame()` / `endFrame()` / `totalFrames()` |
| 動いているか | `is_playing()` / `is_pausing()` | `ss_player_is_playing` / `_is_pausing` | `isPlaying()` / `isPausing()` |
| 終了したか | `is_finished()` | `ss_player_is_finished` | `isFinished()` |
| **直前 tick のエッジ** | `just_looped()` / `just_finished()` | `ss_player_looped_last_update` / `_finished_last_update` | `justLooped()` / `justFinished()` |

実務で使うのは最後の行です。`just_finished()` は**エッジ**で、再生を完了させた tick でだけ true になり、
読んだ時点で消費されます。したがって完了が 2 度発火することはなく、非表示にして戻したキャラクターが
処理済みの完了を再通知することもありません。

!!! note "動かなかった tick は何も報告しません"
    キャラクターを非表示にする（`set_visible(false)`）とタイムラインの前進自体がスキップされ、Player は
    そのフレームのイベント・ループフラグ・完了エッジをクリアします。直前に表示されていた tick の答えを
    繰り返しません。イベントごとに音を鳴らすホストで、同じ音が二重に鳴ることはありません。

## なめらかさ：サブフレームとフレームスキップ

| 設定 | 既定 | 内容 |
|---|---|---|
| サブフレーム | オン | オーサリングされたフレーム間を補間し、固定 fps ではなくディスプレイのリフレッシュに追従します。スローモーションでも滑らかです |
| フレームスキップ | オン | 長い tick が複数フレームを一度に跨いで正しい位置に着地します（遅れて引きずられません）。**跨いだフレームのイベントはすべて発火します** |

意図的にカクつかせた「2 コマ打ち」の見た目にしたいならサブフレームをオフに、経過時間に関わらず 1 update
＝1 フレームにしたいならフレームスキップをオフにします。

## 次に読む

* [イベントとユーザーデータ](./events.md) — タイムラインへの反応
* [モーションブレンディング](./motion-blending.md) — 2 つのアニメーションの同時再生とクロスフェード（Rust）
* [パーツオーバーライド](./part-overrides.md) — パーツが描くものを差し替える
