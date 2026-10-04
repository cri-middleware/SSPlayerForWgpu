# 制約と適用範囲

作るものの形が変わる制約だけを並べます。詳細はそれぞれのページにあります。計画中のものは
[ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) に、各 Face が現時点で公開している範囲は[スコープと名前の対応](./guide/concepts.md#what-each-face-can-do)の表にあります。

| 制約 | 対象 | 詳しくは |
|---|---|---|
| C Face は**自分でデバイスを構築し所有します**。あなたのレンダーパスへは描き込めません | C / C++ | [下記](#no-device-sharing-across-the-c-abi) |
| **`wgpu` は 30 系に固定**。アプリケーションと Player は 1 つの `wgpu` に解決する必要があります | Rust, Bevy | [前提](./start/rust.md#requirements) |
| **MSAA はオフ必須**。sRGB ターゲットではアトラスが二重にエンコードされます | Rust, Bevy | [レンダーパスに求められること](./wgpu/borrow-model.md#what-your-render-pass-has-to-be) |
| **1 キャラクターにつき 1 フレーム 1 回の `update`**。1 つのキャストを 2 つのビューには描けません | Rust, Bevy | [複数ターゲット、1 デバイス](./wgpu/borrow-model.md#several-targets-one-device) |
| **WebGPU のみ**。OpenGL / WebGL バックエンドは無く、非対応ブラウザにはフォールバックが要ります | Web | [前提](./start/web.md#requirements) |
| **`Div2` / `Overlay2` は `Mix` で描かれます**。対応する GPU のブレンドステートがありません | 全 Face | [下記](#div2-and-overlay2-draw-as-mix) |
| **オーディオイベントは合図**で、Player は音を鳴らしません | 全 Face | [イベントとユーザーデータ](./guide/events.md) |

SS6Player for CPP から移行する場合、相当するものが無い呼び出し（ラベル指定の再生区間、パーツ単位の反転、エフェクトのシード）は
[SS6 にあって扱いが変わったもの](./migration/from-ss6-cpp.md#8-things-ss6-had-that-work-differently)にまとまっています。

## C ABI 経由でデバイスを共有できません { #no-device-sharing-across-the-c-abi }

C Face は渡されたウィンドウハンドル上に**自分でグラフィックスデバイスを構築し、所有します**。あなたのコマンドリストやレンダーパスへ描き込むことはできません。

wgpu の型には安定した C 表現が無く、境界を越えて渡せるものが存在しません。したがってデバイスを持つエンジンへの組み込みは **Rust 限定**の機能であり、[アダプタクレート](./wgpu/engines.md)として提供されます。

C から合成したい場合の経路は、オフスクリーン描画＋テクスチャ共有です。

## `Div2` と `Overlay2` は `Mix` で描かれます { #div2-and-overlay2-draw-as-mix }

ブレンドモードはそれぞれ GPU の固定機能のブレンドステートとして描かれます。`Div2` と `Overlay2` には対応するステートが無いため、どちらかを設定したパーツは `Mix` として描かれます。

再現できるかは wgpu ではなく計算式の定義次第です。何が要るかは [ROADMAP](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) の項目にあります。
