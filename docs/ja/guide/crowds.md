# 群衆描画

群衆（クラウド）は、1 体のキャラクターを多数の配置に描くものです。メンバーが何体いても、再生ヘッドは 1 つ、アニメーションの評価は 1 回、ジオメトリのアップロードは 1 回、ドローコールは 1 体分です。メンバーが増えるごとに足されるのは、インスタンスの木の 1 ノードにつき 1 つの配置レコード（`uploaded_bytes` に計上されます）と、そのピクセルを描く GPU の仕事だけなので、CPU コストは 1 体分に近いまま保たれます。同じ数のキャラクターなら、その数だけ更新が要ります。

=== "Rust"

    ```rust
    use ssplayer_wgpu::CrowdMember;

    player.set_crowd(&[
        CrowdMember { x: -200.0, ..CrowdMember::default() },
        CrowdMember { x: 0.0, alpha: 0.5, ..CrowdMember::default() },
        CrowdMember { x: 200.0, scale_x: -1.0, tint: 0x80c0ff, ..CrowdMember::default() },
    ]);
    player.set_crowd(&[]);   // 1 体に戻す
    ```

=== "C"

    ```c
    /* x, y, scale_x, scale_y, rotation, alpha, tint */
    ss_crowd_member_t members[2] = {
        { -200.0f, 0.0f, 1.0f, 1.0f, 0.0f, 1.0f, 0xFFFFFFu },
        {  200.0f, 0.0f, 1.0f, 1.0f, 0.0f, 0.5f, 0x80C0FFu },
    };
    ss_player_set_crowd(player, members, 2);
    ss_player_set_crowd(player, NULL, 0);   /* 1 体に戻す */
    ```

=== "JavaScript"

    ```js
    // メンバー 1 体につき 6 つの数値：x, y, scaleX, scaleY, rotation, alpha
    player.setCrowd(new Float32Array([
      -200, 0, 1, 1, 0, 1,
       200, 0, 1, 1, 0, 0.5,
    ]), new Uint32Array([0xffffff, 0x80c0ff]));   // tint。省略可
    player.setCrowd(new Float32Array());          // 1 体に戻す
    ```

=== "Bevy"

    ```rust
    use ssplayer_bevy::{CrowdMember, SsCrowd};

    commands.entity(character).insert(SsCrowd {
        members: vec![
            CrowdMember { x: -200.0, ..default() },
            CrowdMember { x: 200.0, alpha: 0.5, ..default() },
        ],
    });
    ```

## メンバーとは { #what-a-member-is }

| フィールド | 意味 |
|---|---|
| `x`, `y`, `scale_x`, `scale_y`, `rotation` | 立つ位置。`set_transform` と同じ表し方（`y` は下向き、回転はラジアン）で、キャラクター自身の配置の**下に**合成されます。キャラクターを動かすと群衆全体が動きます |
| `alpha` | 0..1。キャラクターのアルファに乗算されます |
| `tint` | `0xRRGGBB`。キャラクターの tint に乗算されます |

既定のメンバーはキャラクターと同じ位置に、フェードも tint も無しで立ちます。既定のメンバー 1 体だけの群衆は、群衆が無いときとまったく同じものを描きます。群衆のメンバーは 4096 体（`MAX_CROWD_MEMBERS`、`SS_MAX_CROWD_MEMBERS`）までです。Rust Face と Bevy はそれを超えた分を描かず、C Face はそれより長いリストを `SS_ERROR_INVALID_ARGUMENT` で拒否し、JavaScript は例外を投げます。

メンバーはコピーして取り込まれます。動かすには毎フレーム設定し直してください。Rust Face と Bevy では、変わっていない群衆には何のコストもかからず、群衆が大きくならない限りアロケーションも起きません。C Face と JavaScript Face では呼ぶたびにフレームを作り直すので、動いていない群衆は変わったときだけ設定してください。

## メンバーが共有するもの { #what-members-share }

立つ位置と、フェード・tint 以外のすべてです：

* **フレーム。** 全メンバーが同じアニメーションの同じフレームを、同じトラックとオーバーライドで再生します。アニメーションの別の位置を再生させたいなら、別のキャラクターにしてください。
* **イベント。** メンバーごとではなく 1 回だけ報告されます。
* **エフェクト。** 全メンバーのパーティクルは同じパーティクルで、それぞれのメンバーの位置に置かれます。
* **マスク。** カバレッジバックエンドでは、各メンバーが自分の配置に置かれたマスクで切り抜かれます。ステンシルバックエンド（C と JavaScript の Face が使うもの）では全メンバーのマスクがまとめて書かれるため、正しく切り抜けるのはメンバーどうしが重ならない場所だけです。
* **キャラクター自身の設定。** `set_order`・`set_visible`・`set_culled` と、キャラクターのアルファと tint は群衆全体に掛かります。1 体だけ隠すには、そのメンバーをリストから外してください。

`part_transform` はキャラクター自身の空間のままで、キャラクターの配置もメンバーの配置も含みません。あるメンバーに追従させるには、そのメンバーの配置を合成してください。

## メンバーが重なる場所 { #where-members-overlap }

キャラクターのドローはそれぞれ、全メンバー分を発行してから次のドローに進みます。そのため 2 体のメンバーが重なる場所では、1 体目の後のパーツが 2 体目の前のパーツを覆うことがあります。離して並べた群衆では起きません。重なったうえで各自の前後関係を保つ必要があるメンバーは、別のキャラクターにしてください。

[group alpha](./part-overrides.md#whole-character-tint-and-alpha) がオンなら、フェードした群衆は 1 体のキャラクターとして 1 枚のレイヤーでフェードします。メンバー自身のアルファは、引き続きパーツごとに掛かります。

## 次に読む { #next }

* [ボックスへのフィット](./placement.md) — 群衆を従えるキャラクターの配置
* [パフォーマンス](./performance.md) — 群衆のコストをフレーム統計で見る
