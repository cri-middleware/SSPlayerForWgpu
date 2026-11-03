# 再生と再生制御

動詞は 4 つあり、互換ではありません。頼んでいないのにアニメーションが先頭へ戻るなら、原因はこのページにあります。

## 4 つの動詞

| 動詞 | 何をするか |
|---|---|
| **play** | **先頭出し。** 区間（設定しなければアニメーション全体。後述）の先頭へ（逆再生なら末尾へ）巻き戻し、ループ回数を張り直し、独立再生のエフェクトとインスタンスの子も最初からやり直します |
| **pause** | 現在フレームで保持。冪等で、再生中扱いのまま — 保持は停止ではありません |
| **resume** | pause を解除し、同じフレームから継続。冪等で、**停止中には何もしません** |
| **stop** | 再生終了。再生ヘッドはその場に残るので、止まったフレームを描き続けられます |

!!! danger "`pause()` の解除に `play()` は使えません"
    `play()` は巻き戻します。一時停止の解除に使うと、再生ヘッドが黙って区間の先頭へ戻り、ループカウンタもリセットされます。カクついたように見え、ランタイムの不具合のように読めるバグです。**その場から続けるのは `resume()` です。**

    逆も同様です。`resume()` は何かを*開始*できません。停止中のアニメーションに対しては何もしません。開始は `play` の仕事で、`play` は巻き戻すからです。この分離は SDK の再生制御契約であり、ファミリーのどの Player でも同じです。

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

    // 残りは 1 度だけ起こることなので、メッセージで送ります。
    plays.write(SsPlay { entity, start_frame: Some(12.0) });   // 指定フレームから再生
    stops.write(SsStop { entity });                             // 停止
    ```

## 速度・方向・ピンポン

**速度は倍率であって方向ではありません。** `0.0` と負値は*停止*で、読み戻すと `0.0` になっています。逆再生は別の設定です。

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
    playback.speed_scale = 2.0;
    playback.direction = PlaybackDirection::Backward;
    playback.style = PlaybackStyle::PingPong;
    playback.frame_rate = 30;
    ```

| 値 | 方向 | スタイル |
|---|---|---|
| `0` | 順再生 | 通常 — 端で先頭に戻る／ループ回数に従って停止 |
| `1` | 逆再生 | ピンポン — 折り返して戻る。1 ループは往復 |

この `0` / `1` の対応は全 Player 共通です。C と JavaScript が整数を取り、Rust が enum を取るのはそのためです。

**フレームレートより速度を優先してください。** 速度はオーサリングされたタイミングを倍率で扱い、フレームレートはそれを置き換えます。

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

=== "Bevy"

    ```rust
    playback.loop_count = 3;   // 変えるとリスタートと同じく回数を張り直す
    ```

## アニメーションの一部だけを再生する

**区間**は現在のアニメーションのフレーム範囲に再生ヘッドを制限します。ループもピンポンもその中で折り返します。

必ず `set_animation` の**後**に設定してください。フレーム番号は、それを測ったアニメーションの中でしか意味を持たず、アニメーションを選び直すとツリーが再構築されて区間も一緒に消えます。

両端を含むので、両端が同じフレームの区間はその 1 フレームに留まります。アニメーションの範囲外の値（`-1` など）を渡すと、その側はアニメーションの端になります。両端が範囲内で終わりが始まりより前なら受け付けず、区間も再生ヘッドもそのままです。Rust は `Err(SSRuntimeError::InvalidParameter)` を返し、C は 0 以外のステータスを返して詳細を `ss_last_error()` に残し、JavaScript は例外を投げ、Bevy は警告を 1 度だけログに出します。

=== "Rust"

    ```rust
    player.set_animation("walk")?;
    player.set_animation_section(10, 24)?;
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

=== "Bevy"

    ```rust
    // 区間はその時点のアニメーションに掛かるので、2 つは一緒に変える。
    // 設定すると再生ヘッドは区間の先頭へ移る。
    character.animation = "walk".into();
    playback.animation_section = Some((10, 24));

    // seek は 1 度だけ起こることなのでメッセージ。seeks は MessageWriter<SsSeek>。
    seeks.write(SsSeek { entity, frame: 18.0 });
    ```

## アニメーションやパックを切り替えて何が残るか

再生の設定はアニメーションではなくプレイヤーのものです。`set_animation` でも新しいパック（`load`、`set_pack`）でも引き継がれ、最初のアニメーションより前に設定することもできます。これはファミリー共通の挙動で、この Player 独自のものではありません。

| `set_animation` と新しいパックで**残る** | **残らない** |
|---|---|
| 速度倍率、フレームレート上書き、再生方向、フレームスキップ | **区間** — フレーム番号は別クリップに対して意味を持たないため |
| ループ回数 | |

## 再生位置を問い合わせる

| | Rust | C | JavaScript |
|---|---|---|---|
| 現在フレーム | `frame_no()` | `ss_player_frame_no` | `frameNo()` |
| 範囲 | `start_frame()` / `end_frame()` / `total_frames()` | `ss_player_start_frame` / `_end_frame` / `_total_frames` | `startFrame()` / `endFrame()` / `totalFrames()` |
| 動いているか | `is_playing()` / `is_pausing()` | `ss_player_is_playing` / `_is_pausing` | `isPlaying()` / `isPausing()` |
| 終了したか | `is_finished()` | `ss_player_is_finished` | `isFinished()` |
| **直前 tick のエッジ** | `just_started()` / `just_looped()` / `just_finished()` | `ss_player_just_started` / `_just_looped` / `_just_finished` | `justStarted()` / `justLooped()` / `justFinished()` |

実務で使うのは最後の行です。`just_finished()` は**エッジ**で、再生を完了させた tick でだけ true になり、次の `update()` で進みます。したがって完了が 2 度発火することはなく、カリングして戻したキャラクターが処理済みの完了を再通知することもありません。読んでも消費されず、同じフレームで 2 回読めば両方とも true を返します。エッジを進めるのは *tick* であって、*読み取り* ではありません。

!!! note "動かなかった tick は何も報告しません"
    キャラクターをカリングする（`set_culled(true)`）とタイムラインの前進自体がスキップされ、Player はそのフレームのイベントと開始・ループ・完了のエッジをクリアします。直前に走った tick の答えを繰り返しません。イベントごとに音を鳴らすホストで、同じ音が二重に鳴ることはありません。カリング中に呼んだ `play` は失われず、最初に走った tick がその開始を報告します。

    キャラクターを非表示にする（`set_visible(false)`）のは別物で、飛ばすのは描画だけです。タイムラインは進み続け、毎 tick 通常どおり報告するので、表示を切り替えて点滅させてもタイミングは崩れません。

## なめらかさ：サブフレームとフレームスキップ

| 設定 | 既定 | 内容 |
|---|---|---|
| サブフレーム | オン | オーサリングされたフレーム間を補間し、固定 fps ではなくディスプレイのリフレッシュに追従します。スローモーションでも滑らかです |
| フレームスキップ | オン | 長い tick が複数フレームを一度に跨いで正しい位置に着地します（遅れて引きずられません）。**跨いだフレームのイベントはすべて発火します** |

意図的にカクつかせた「2 コマ打ち」の見た目にしたいならサブフレームをオフに、経過時間に関わらず 1 update ＝ 1 フレームにしたいならフレームスキップをオフにします。

| | Rust | C | JavaScript | Bevy |
|---|---|---|---|---|
| サブフレーム | `set_sub_frame_enabled(false)` | `ss_player_set_sub_frame_enabled(player, false)` | `setSubFrameEnabled(false)` | `SsPlayback::sub_frame_enabled` |
| フレームスキップ | `set_frame_skip_enabled(false)` | `ss_player_set_frame_skip_enabled(player, false)` | `setFrameSkipEnabled(false)` | `SsPlayback::frame_skip_enabled` |

## 次に読む

* [イベントとユーザーデータ](./events.md) — タイムラインへの反応
* [モーションブレンディング](./motion-blending.md) — 2 つのアニメーションの同時再生とクロスフェード
* [パーツオーバーライド](./part-overrides.md) — パーツが描くものを差し替える
