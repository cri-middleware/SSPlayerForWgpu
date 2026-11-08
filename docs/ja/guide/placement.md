# ボックスへのフィット

Player は与えられたターゲットに**オーサリング時のスケールのまま**描きます。ゲームではそれが望ましい動作です — キャラクターはアーティストが作ったサイズで、残りはカメラが決めます。ですが UI ではそうはいきません。パネルもカードもプレビューペインも、レイアウトが決めたサイズのボックスであり、アニメーションはその中に収まらなければなりません。

`fitInto` がその一手です。ボックスを渡せば配置を計算し、`setTransform` と同じ継ぎ目（ビューの中心からのピクセル、`y` は下向き）を通して書き込みます — 描画側に足すものはなく、毎フレームの追加コストもありません。

```
   fit: contain              fit: cover               fit: none
 ┌──────────────┐         ┌──────────────┐         ┌──────────────┐
 │░░┌────────┐░░│         │██┌────────┐██│         │              │
 │░░│        │░░│         │██│        │██│         │    ┌────┐    │
 │░░│  art   │░░│         │██│  art   │██│         │    │art │    │
 │░░│        │░░│         │██│        │██│         │    └────┘    │
 │░░└────────┘░░│         │██└────────┘██│         │              │
 └──────────────┘         └──────────────┘         └──────────────┘
  全体がボックスに          隙間なく埋める。はみ出し     オーサリング時の
  収まる。░ は余白          (█) はクリップされない      スケールで中央。
                                                     配置は自分で行う
```

## 語彙

`<Image resizeMode>` のものです。ホストが既に持っている語彙だからで、SpriteStudio の Player
ファミリー全体で共通です。

| フィールド | 意味 |
|---|---|
| `fit` | `contain`（既定）はボックス全体に収める、`cover` は埋めてはみ出す、`none` はフィットしない |
| `align` | コンテンツのどの点をビューの対応する点に合わせるか: `center`（既定）、`top`、`bottom`、`left`、`right`、`top-left`、`top-right`、`bottom-left`、`bottom-right` |
| `frame` | フィットさせるボックス。Player ローカル単位。**指定すればパックからは何も読みません** |
| `offset` | 整列後の位置からのピクセル数。フィットの**後**に適用するので、フィットのスケールは掛かりません。Rust ではタプル、C では `offset_x` / `offset_y`、JavaScript では `offsetX` / `offsetY` です |
| `scale` | フィットのスケールに**上乗せ**する倍率。置き換えではありません |
| `flipX`, `flipY` | 反転。整列はそれに追従します — 左寄せで反転したキャラクターも左端に接したままです。Rust と C では `flip_x` / `flip_y` です |
| `rotation` | そのまま素通しします。回転はフィットの計算には関与しません |

=== "Rust"

    ```rust
    use ssplayer_wgpu::{Align, Fit, Placement};

    // contain で中央 — 既定値。
    player.fit_into(width, height, &Placement::default());

    // パネルを埋めてはみ出させ、下端に合わせる。
    let placed = player.fit_into(width, height, &Placement {
        fit: Fit::Cover,
        align: Align::Bottom,
        ..Default::default()
    });
    println!("drawn at {}x", placed.scale_x);
    ```

=== "C"

    ```c
    ss_placement_t placement;
    ss_placement_default(&placement);   // ゼロ初期化しないこと: scale が 0 になります
    placement.fit = SS_FIT_COVER;
    placement.align = SS_ALIGN_BOTTOM;

    ss_placed_t placed;
    ss_player_fit_into(player, width, height, &placement, &placed);
    ```

=== "JavaScript"

    ```js
    const placed = player.fitInto(canvas.width, canvas.height, {
      fit: 'cover',
      align: 'bottom',
    });
    console.log(placed.scaleX);
    ```

=== "Bevy"

    ```rust
    // アダプタには配線されていません。`SsTransform` はワールド単位での配置で、
    // フィットにはビューのピクセルサイズが要りますが、それを知っているのはレンダー
    // ワールドだけです。
    ```

## フィットするボックス

`frame` を渡さない限り、ボックスはアニメーションの**オーサリング時のキャンバス**です。アーティストが構図を決めたその枠を、アニメーションの pivot で原点に対して配置したものです。

これは絵を実測しない、という意図的な選択です。実測するとロード時に全フレームを焼くことになり、再生中にサイズが変わってしまいます。オーサリング時のキャンバスは読み出しコストがゼロで、再生中に動かず、余白をどこに置くかを作者が決めた結果そのものです — 地面に立つよう作られたキャラクターは、頭上の余白をそのまま保ちます。

キャンバスはアニメーションごとに持つもので、同じパックの中でも違うことがあります — Ringo の `dead` は幅 1300 で、ほかのアニメーションは 800 です。そのため切り替えの後にフィットし直すと、キャラクターの大きさが変わることがあります。パック全体で大きさを揃えたいときは、最初の `setAnimation` の後にキャンバスを一度だけ読み、以後はそれを `frame` として渡してください。

=== "Rust"

    ```rust
    // フィットされるボックス。Player ローカル単位（y は下向き）。
    let rect = player.canvas_rect();

    // 自分で実測した、あるいは決め打ちの、もっと狭いボックスを渡すこともできます。
    player.fit_into(width, height, &Placement {
        frame: Some(ssplayer_wgpu::Rect { x: -100.0, y: -200.0, width: 200.0, height: 200.0 }),
        ..Default::default()
    });
    ```

=== "C"

    ```c
    ss_rect_t rect;
    if (ss_player_canvas_rect(player, &rect) == SS_OK) { /* … */ }

    placement.has_frame = true;
    placement.frame = (ss_rect_t){ -100.0f, -200.0f, 200.0f, 200.0f };
    ```

=== "JavaScript"

    ```js
    const rect = player.canvasRect();   // { x, y, width, height } または undefined

    player.fitInto(w, h, { frame: { x: -100, y: -200, width: 200, height: 200 } });
    ```

=== "Bevy"

    ```rust
    // Bevy では提供していません。オーサリング時のキャンバスはレンダーワールドの中で読まれ、
    // ゲームのコードからは届きません。自分のボックスからスケールを求めて `SsTransform` に書いてください。
    ```

!!! note "pivot はバウンディングボックスではありません"

    Player ローカルの `y` は**下向き**で、pivot はキャンバス内のどこに原点があるかを表します。原点を地面に置いて作られたキャラクター（エディタ上の `pivot 0 -0.5`）は、したがって `y` がすべて*負*のボックスを報告します — キャンバスは原点より上にあります。そのキャラクターがボックスの中央に浮かずに床に立つのは、これが理由です。

## 呼び直すタイミング

サイズは**レンダーターゲットのピクセル**なので、配置は渡したボックスに対してのみ正しい値です。ボックスが変わったら呼び直してください。

* リサイズ時。`resize` / `set_viewport` の前でも後でも構いません — 両者は独立です
* `setAnimation` の後。新しいアニメーションのキャンバスが以前と異なる場合

矩形に対する純粋な算術で、実測も GPU の仕事もアロケーションもありません。変化を追跡するよりそのほうが単純なら、毎フレーム呼んでも構いません。

## フィットしない場合

**潰れたボックス**（`width` か `height` が 0、または面積の無い `frame`）、**アニメーションが未設定**、`fit: none` の 3 つは同じ経路を通ります。フィットせず、原点はビューの中央、スケールは（呼び出し側の `scale` を掛ける前の値で）
1 です。どれもエラーではありません — 計測前のボックスをレイアウトしたホストは、ボックスにサイズがついた瞬間にアニメーションを取り戻せます。

**エラーになる**のは `fit` や `align` が未知の値だった場合です。C Face は `SS_ERROR_INVALID_ARGUMENT` を返し、JavaScript Face は受け付ける値を挙げて throw します。黙って中央に寄せてしまうと、見つけられないバグになるからです。

`cover` は意図的にボックスからはみ出し、Player は**自前のシザーを持ちません** — はみ出しのクリップはホスト側で、シザー矩形か専用のレンダーターゲットで行ってください。
