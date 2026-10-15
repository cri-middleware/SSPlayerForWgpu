# 制約と適用範囲

作るものの形が変わる制約だけを並べます。詳細はそれぞれのページにあります。計画中のものは
[ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) に、各 Face が現時点で公開している範囲は[スコープと名前の対応](./guide/concepts.md#what-each-face-can-do)の表にあります。

| 制約 | 対象 | 詳しくは |
|---|---|---|
| C Face は**自分でデバイスを構築し所有し**、渡されたウィンドウにだけ描きます。あなたのレンダーパスへも、今のところテクスチャへも描き込めません | C / C++ | [下記](#no-device-sharing-across-the-c-abi) |
| **`wgpu` は 30 系に固定**。アプリケーションと Player は 1 つの `wgpu` に解決する必要があります | Rust, Bevy | [前提](./start/rust.md#requirements) |
| **MSAA はオフ必須**。sRGB ターゲットではアトラスが二重にエンコードされます | Rust, Bevy | [レンダーパスに求められること](./wgpu/borrow-model.md#what-your-render-pass-has-to-be) |
| **1 キャラクターにつき 1 フレーム 1 回の `update`**。1 つのキャストを 2 つのビューには描けません | Rust, Bevy | [複数ターゲット、1 デバイス](./wgpu/borrow-model.md#several-targets-one-device) |
| **WebGPU のみ**。OpenGL / WebGL バックエンドは無く、非対応ブラウザにはフォールバックが要ります | Web | [前提](./start/web.md#requirements) |
| **`Div2` / `Overlay2` は `Mix` で描かれます**。対応する GPU のブレンドステートがありません | 全 Face | [下記](#div2-and-overlay2-draw-as-mix) |
| **`Mulalpha` / `Mul2` は `Mul` で、`Screen2` は `Screen` で描かれます** | 全 Face | [下記](#div2-and-overlay2-draw-as-mix) |
| **キャラクター全体のアルファはパーツごとに掛かります**（group alpha がオフのとき）。2 つのパーツが重なる箇所は、`set_alpha(0.5)` で 0.75 になります | 全 Face | [下記](#whole-character-alpha-applies-part-by-part) |
| **群衆のメンバーは同じフレームを再生し**、重なる箇所ではドローごとに交互に描かれます。ステンシルバックエンドでは、マスクが正しく切り抜けるのはメンバーどうしが重ならない場所だけです | 全 Face | [群衆描画](./guide/crowds.md#what-members-share) |
| **統合シェーダはマスクの掛からない通常パーツにだけ描かれます**。追加のセル（`map0` / `map1`）はバインドされません | 全 Face | [下記](#integration-shaders) |
| **オーディオイベントは合図**で、Player は音を鳴らしません | 全 Face | [イベントとユーザーデータ](./guide/events.md) |

SS6Player for CPP から移行する場合、相当するものが無い呼び出し（ラベル指定の再生区間、パーツ単位の反転、エフェクトのシード）は
[SS6 にあって扱いが変わったもの](./migration/from-ss6-cpp.md#8-things-ss6-had-that-work-differently)にまとまっています。

## C ABI 経由でデバイスを共有できません { #no-device-sharing-across-the-c-abi }

C Face は渡されたウィンドウハンドル上に**自分でグラフィックスデバイスを構築し、所有します**。あなたのコマンドリストやレンダーパスへ描き込むことはできません。

wgpu の型には安定した C 表現が無く、境界を越えて渡せるものが存在しません。したがってデバイスを持つエンジンへの組み込みは **Rust 限定**の機能であり、[アダプタクレート](./wgpu/engines.md)として提供されます。

C からエンジン自身の描画と合成するにはオフスクリーン描画＋テクスチャ共有が要りますが、この Face は**まだそれを提供していません**。シーンが描くのは、作成時に渡したウィンドウまたはレイヤーだけです。それまでの経路は Player 専用のウィンドウかレイヤー — 子ウィンドウや、自前のレイヤーツリーに置いた `CAMetalLayer` — です。オフスクリーンのターゲットに何が要るかは [ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) の項目にあります。

## `Div2` と `Overlay2` は `Mix` で描かれます { #div2-and-overlay2-draw-as-mix }

ブレンドモードはそれぞれ GPU の固定機能のブレンドステートとして描かれます。`Div2` と `Overlay2` には対応するステートが無いため、どちらかを設定したパーツは `Mix` として描かれます。

再現できるかは wgpu ではなく計算式の定義次第です。何が要るかは [ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) の項目にあります。

ほかの 3 つは近いモードのステートを借りて描かれます。`Mulalpha` と `Mul2` は `Mul` として、`Screen2` は `Screen` として描かれます。この 3 つは近似で、エディタとの一致は確認していません。

## キャラクター全体のアルファはパーツごとに掛かります { #whole-character-alpha-applies-part-by-part }

既定では、`set_alpha` はキャラクターをまとめてではなく、パーツを描くたびにそれぞれをフェードさせます。そのため 2 つのパーツが重なる箇所では、奥のパーツが手前のパーツ越しに透けて見えます。キャラクターの左右半分を 0.5 にすると重なりは 0.75 になり、フェード中のキャラクターには自分の継ぎ目が見えます。

group alpha は、フェード中のキャラクターを丸ごと専用のレイヤーに描いてから 1 回で合成することで、これを直します。Rust Face では `set_group_alpha`、C では `ss_scene_set_group_alpha`、JavaScript では `setGroupAlpha`、Bevy では `SsSceneSettings::group_alpha` です。コストがあるため既定ではオフです。フェード中のキャラクター 1 体につき 1 フレームあたりレンダーパスが 1 つ、同時にフェードしている 1 体ごとにターゲットと同じ大きさのレイヤーが 1 枚要ります。レイヤーの中では、Add・Mul・Screen のパーツはキャラクターの背後ではなく透明に対してブレンドされます。[キャラクター全体の tint とアルファ](./guide/part-overrides.md#whole-character-tint-and-alpha)を参照してください。

## 統合シェーダ { #integration-shaders }

`ss-*` 統合シェーダは、マスクの掛からない通常パーツに描かれます。マスクの掛かるパーツや、通常以外の種類のパーツは、シェーダ無しで描かれます — マスクの場合は WebGL Player と同じ挙動です。シェーダの追加のセル（`map0` / `map1`）はバインドされず、Player の知らないシェーダ ID は通常どおりに描かれます。
