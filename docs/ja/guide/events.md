# イベントとユーザーデータ

アーティストはタイムライン上にマーカーを置けます。再生ヘッドがそれを跨いだとき、Player があなたに渡します。
3 種類あります：

| 種類 | 持つもの | 典型的な用途 |
|---|---|---|
| **ユーザーデータ** | 最大 4 つの任意項目 — 整数・矩形・座標・文字列 | 12 フレーム目の当たり判定、湧き出し位置、状態名 |
| **シグナル** | 名前付きコマンドと型付きパラメータ | 「ドアを開けろ」と、その引数 |
| **オーディオ** | サウンド名・ハッシュ・再生回数 | 足音、ボイス |

!!! warning "Player は音を鳴らしません"
    オーディオイベントは *何を* *何回* 鳴らすかを伝えるだけです。キューは自前のオーディオスタックへ
    渡してください。

## 取得する

それを生成した update の後で読みます。返るのは**直前の tick が跨いだイベント**です。

=== "Rust"

    ```rust
    scene.update(&mut renderer, &device, &queue, dt, &mut [&mut player]);

    for event in player.events() {
        match event {
            SsEvent::UserData(u) => println!("user data on frame {}: {:?}", u.frame_no, u.text),
            SsEvent::Signal(s)   => println!("signal {} with {} params", s.command_id, s.params.len()),
            SsEvent::Audio(a)    => audio.play(&a.sound_name, a.loop_num),
        }
    }
    ```

=== "C"

    ```c
    ss_scene_update(scene, dt);

    uint32_t n = ss_player_event_count(player);
    for (uint32_t i = 0; i < n; i++) {
        SsEventData e;
        if (ss_player_event_at(player, i, &e) != SS_OK) continue;

        switch (e.kind) {
        case SS_EVENT_USER_DATA:
            if (e.has_integer) { /* e.integer */ }
            if (e.has_text)    { /* e.text — 保持するならコピーする */ }
            break;
        case SS_EVENT_SIGNAL:
            for (uint32_t p = 0; p < e.param_count; p++) {
                SsSignalParamData param;
                ss_player_event_signal_param_at(player, i, p, &param);
            }
            break;
        case SS_EVENT_AUDIO:
            play_sound(e.text, e.loop_num);
            break;
        }
    }
    ```

=== "JavaScript"

    ```js
    surface.frame(dt);

    for (const e of player.events()) {
      switch (e.kind) {
        case 'userData': console.log(e.integer, e.rect, e.point, e.text); break;
        case 'signal':   console.log(e.commandId, e.params); break;
        case 'audio':    audio.play(e.soundName, e.loopNum); break;
      }
    }
    ```

=== "Bevy"

    ```rust
    fn react(mut events: MessageReader<SsEventMessage>) {
        for message in events.read() {
            match &message.event {
                SsEvent::UserData(u) => { /* ... */ }
                SsEvent::Signal(s)   => { /* ... */ }
                SsEvent::Audio(a)    => { /* ... */ }
            }
        }
    }
    ```

    メッセージは**1 フレーム遅れ**です。再生はレンダーワールドで、そのフレームの `Update` より後に動きます。
    音のキュー程度なら知覚できない差です。

## 跨いだフレームのイベントはすべて発火します

これがリストの完全性を担保しています。

フレームスキップが効いている状態で重い tick が来ると、オーサリング上の複数フレームを一度に跨ぎ、ループが
その途中で折り返すこともあります。**跨いだフレームはすべてイベントを報告します** — 着地したフレームだけでは
ありません。tick が 5 から 9 へ飛んでも、7 フレーム目の足音は失われません。

各イベントが**オーサリングされたフレーム番号**を持つのもこのためです。それは、気づかれたフレームとは
限りません。

## 「空」にも意味があります

| 状況 | `events()` の中身 |
|---|---|
| 何も跨がなかった tick | 空 |
| 非表示のプレイヤー（`set_visible(false)`） | **空** — タイムラインが進んでいないので、直前に表示されていた tick を繰り返しません |
| 一時停止中のプレイヤー | 同じ理由で空 |

したがって、イベントごとに音を鳴らすホストで、キャラクターがカリングされたせいで同じ音が二重に鳴ることは
ありません。

## ユーザーデータの中身

4 つの任意項目があり、**未設定はゼロではありません**。この区別こそが Option になっている理由です。
テキストだけを入力したアーティストは整数を*未設定*にしたのであり、`0` は別の意味になります。

| 項目 | Rust | C | JavaScript |
|---|---|---|---|
| 整数 | `Option<i32>` | `has_integer` ＋ `integer` | `e.integer`（無ければ未定義） |
| 矩形 | `Option<[f32; 4]>` — `[x1, y1, x2, y2]`、**原点＋サイズではなく 2 隅** | `has_rect` ＋ `rect[4]` | `e.rect` |
| 座標 | `Option<[f32; 2]>` | `has_point` ＋ `point[2]` | `e.point` |
| テキスト | `Option<String>` | `has_text` ＋ `text` | `e.text` |

どのイベントも `part_index` とオーサリング時のパーツ名を持つので、「どのパーツが言ったか」を自分で
管理する必要はありません。

## シグナルのパラメータ

シグナルはコマンド ID と `(id, 値)` の並びです。値はブール・選択肢の**インデックス**・整数・浮動小数・
テキスト、あるいは **none**（パラメータは存在するが未設定）のいずれかです。

C Face では値が平坦化されており、`value_type` が `SsSignalParamData` のどのフィールドを読むかを示します。
Rust では `SignalValue` enum、JavaScript ではそのままの値です。

## 完了・ループのエッジ

イベントはタイムラインが言ったことを教えます。再生が何をしたかは別の 2 つが教えます：

| | Rust | C | JavaScript | Bevy |
|---|---|---|---|---|
| この tick で完了した | `just_finished()` | `ss_player_just_finished` | `justFinished()` | `SsFinished` メッセージ |
| ループ境界を跨いだ | `just_looped()` | `ss_player_just_looped` | `justLooped()` | `SsLooped` メッセージ |

どちらも**エッジ**で、それを行った tick でだけ true になり、次の `update()` で進みます。したがって
完了したワンショットが 2 度発火することはありません（非表示にして戻した場合も含めて）。エッジは読んでも
消費されず、進めるのは tick です。

## 次に読む

* [パーツオーバーライド](./part-overrides.md) — 「アニメーションの動きに反応する」もう半分
* [再生と再生制御](./playback.md) — イベントハンドラが普通に呼ぶ再生制御
