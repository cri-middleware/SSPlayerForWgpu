# パーツオーバーライド

オーバーライドは、`.ssab` に触れずに、アーティストがオーサリングした内容の上から 1 つのパーツの描画を
変更します。**表示**・**セル**・**カラー**の 3 種類と、パーツの現在位置を読む機能があります。

よくある用途：武器の差し替え、被弾時の赤点滅、アクセサリを隠す、ボーンに HP バーを追従させる。

## パーツを指定する

どのオーバーライドにも**名前指定**と**インデックス指定**の 2 形態があります。やることは同じで、
インデックス版は探索を省きます。

=== "Rust"

    ```rust
    let index = player.find_part_index("body").expect("そのパーツは無い");
    player.set_part_visibility_override_by_index(index, true, false);
    ```

=== "C"

    ```c
    int32_t index = ss_player_find_part_index(player, "body");
    ss_player_set_part_visibility_override_by_index(player, (uint32_t)index, true, false);
    ```

=== "JavaScript"

    ```js
    const index = player.findPartIndex('body');
    player.setPartVisibilityOverrideByIndex(index, true, false);
    ```

インデックスは `set_animation` のときに**一度だけ**引いて保持してください。名前の照合はハッシュですが、
探索自体はパーツ列を歩きます。パックが持つものは `part_names()` / `ss_player_part_count` ＋ 名前取得 /
`partNames()` で列挙できます。

## 表示

```
set_part_visibility_override(part, hide, cascade)
```

リグで効いてくるのが `cascade` です。これを付けると、強制非表示がそのパーツの子孫にも伝播します
（上腕だけでなく腕全体）。**キーフレームによる非表示は決してカスケードしないので、サブツリーを隠す方法は
これだけです。** クリアするとアーティストの制御に戻ります。

## セル

パックが持つ任意のセルマップの別セルに差し替えます：

=== "Rust"

    ```rust
    use ssplayer_wgpu::OverridePriority;

    player.set_part_cell_override("weapon", "items", "sword_02", OverridePriority::Permanent)?;
    player.clear_part_cell_override("weapon")?;
    ```

=== "C"

    ```c
    /* priority: 0 = 次のキーフレームまで, 1 = 次のアニメーションまで, 2 = 恒久 */
    ss_player_set_part_cell_override(player, "weapon", "items", "sword_02", 2);
    ss_player_clear_part_cell_override(player, "weapon");
    ```

=== "JavaScript"

    ```js
    player.setPartCellOverride('weapon', 'items', 'sword_02', 2 /* 恒久 */);
    player.clearPartCellOverride('weapon');
    ```

`cellmap_names()` と `cell_names(cellmap)` で選択肢を列挙できるので、UI にハードコードせずに提示できます。

## カラー

カラーオーバーライドは RGBA 値・**ブレンド演算**・**優先度**の 3 つです。パーツ全体に 1 色か、四隅ごとに
1 色（グラデーション）かの 2 形態があります。

=== "Rust"

    ```rust
    use ssplayer_wgpu::{BlendOp, OverridePriority};

    // 0xRRGGBBAA
    player.set_part_color_override("body", 0xff0000_80, BlendOp::Mix, OverridePriority::Permanent)?;

    // 四隅、それぞれ [r, g, b, a]
    player.set_part_color_override_corners("cape", corners, BlendOp::Mix, OverridePriority::Permanent)?;

    player.clear_part_color_override("body")?;
    player.clear_all_part_overrides();
    ```

=== "C"

    ```c
    /* rgba, blend_op（0 = mix）, priority（2 = 恒久） */
    ss_player_set_part_color_override(player, "body", 0xff000080u, 0, 2);
    ss_player_clear_part_color_override(player, "body");
    ss_player_clear_all_part_overrides(player);
    ```

=== "JavaScript"

    ```js
    player.setPartColorOverride('body', 0xff000080, 0 /* mix */, 2 /* 恒久 */);
    player.clearPartColorOverride('body');
    player.clearAllPartOverrides();
    ```

### ブレンド演算と優先度 { #blend-op-and-priority }

**ブレンド演算**は色とテクセルの合成方法です。**優先度は「どちらが勝つか」ではなく「どれだけの期間
生きるか」**を決めます。ここは最初に読むと取り違えやすいところです。

| ブレンド演算 | Rust | 数値（C / JS） |
|---|---|---|
| Mix | `BlendOp::Mix`（既定） | `0` |
| 乗算 | `BlendOp::Mul` | `1` |
| 加算 | `BlendOp::Add` | `2` |
| 減算 | `BlendOp::Sub` | `3` |

| 優先度 | Rust | 数値 | いつまで生きるか |
|---|---|---|---|
| 次のキーフレームまで | `OverridePriority::OverwriteOnNextKeyframe` | `0` | アニメーションがその属性を次に駆動するまで — つまりアニメーションが触れていないフレームでだけ現れます |
| 次のアニメーションまで | `OverridePriority::HoldUntilNextAnimation`（既定） | `1` | 別のアニメーションが選択されるまで |
| 恒久 | `OverridePriority::Permanent` | `2` | パックが変わるまで。アニメーション切り替えを跨いで残ります |

衣装の選択なら `Permanent`、モーションと共に終わる状態なら `HoldUntilNextAnimation`、アニメーションが
何か言った瞬間に譲るべき微調整なら `OverwriteOnNextKeyframe` を選びます。

## キャラクター全体の tint とアルファ

オーバーライドはパーツ単位です。キャラクター全体には、探索コストのかからない 2 つの制御があります。
Player が元々書いているパーツ単位のプリミティブに相乗りするためです。

=== "Rust"

    ```rust
    player.set_alpha(0.5);       // 0..1。キャラクター全体に乗算
    player.set_tint(0x80c0ff);   // 0xRRGGBB
    ```

=== "C"

    ```c
    ss_player_set_alpha(player, 0.5f);
    ss_player_set_tint(player, 0x80c0ffu);
    ```

=== "JavaScript"

    ```js
    player.setAlpha(0.5);
    player.setTint(0x80c0ff);
    ```

=== "Bevy"

    ```rust
    playback.alpha = 0.5;   // tint はコンポーネントとして公開されていません
    ```

## パーツのトランスフォームを読む { #reading-a-parts-transform }

「いま手はどこにあるか」（武器や UI マーカー、パーティクルの発生源を追従させたい）：

=== "Rust"

    ```rust
    if let Some(m) = player.part_transform("hand_r") {
        // [m00, m01, m10, m11, tx, ty] — アニメーションローカル空間の 2D アフィン
    }
    ```

=== "C"

    ```c
    float m[6];
    if (ss_player_part_transform(player, "hand_r", m) == SS_OK) { /* ... */ }
    ```

=== "JavaScript"

    ```js
    const m = player.partTransform('hand_r');   // [m00, m01, m10, m11, tx, ty]
    ```

この数値について 3 つ：

* **2D アフィンの 6 要素**で、`ssplayer-webgl` の `getPartTransform` と同じ形です。WebGL Player から
  移植したコードはそのまま読めます；
* **アニメーションローカルの Y-down 空間**です。キャラクター自身の `set_transform` も、ターゲットの投影も
  適用されていません。合成は自分で行います — だからこそ、結果を自分の UI が生きている空間に置けます；
* update の**後**に読んでください。update が生成したフレームを表す値です。非表示のパーツも、
  「そこにあったはずの位置」を返します。

## 次に読む

* [イベントとユーザーデータ](./events.md) — もう半分
* [モーションブレンディング](./motion-blending.md) — パーツではなくアニメーション同士のブレンド
