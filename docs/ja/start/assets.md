# アセットの準備

どの Face も再生するファイルは同じ **`.ssab`** で、アーティストが作業する `.sspj` から**事前に**コンバートしたものです。**この Player は実行時に一切変換せず、コンバータも同梱しません。** `.sspj` は SpriteStudio **7.6 以上**で作成したものが対象です。

## 自分のデータを変換する

| ツール | 用途 |
|---|---|
| [SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) | デスクトップでドラッグ＆ドロップ。ツールチェイン不要 |
| `ssconverter-cli`（SDK 同梱） | ビルドパイプライン、CI、watch ループ |

!!! tip "手元に `.sspj` がまだ無いときは"
    [SpriteStudio 公式のサンプルデータ](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/)がそのまま使えます。キャラクターなら [Ringo](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/#ringo)、エフェクトを含むものなら[パーティクル](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/#Perticle_sample) が手頃です。ダウンロードした `.sspj` は、上のどちらのツールにもそのまま渡せます。

選び方とパイプラインへの組み込みはポータル側が本家です：[アセットの変換](https://cri-middleware.github.io/SpriteStudio-Docs/ja/overview/conversion/)、[スタンドアロンツールでの変換](https://cri-middleware.github.io/SpriteStudio-Docs/ja/overview/convert-with-a-standalone-tool/)。ここでは繰り返しません。

このリポジトリのチェックアウトでは、サンプルは SDK 付属のテストアニメーションで動きます。`./scripts/deploy-examples.sh` が `assets/` に変換します。

## 2 つのエクスポートモード

`.ssab` がアトラスを指す方法は 2 通りありますが、**どちらも同じ 1 呼び出しのロードで読めます**。コード側で選び分ける必要はありません。

| モード | ディスク上 | 向いている場面 |
|---|---|---|
| **reference（外部参照）** | `Character.ssab` ＋ 兄弟の `.png` / `.webp` | アトラス共有、CDN、再コンバートせずに絵だけ差し替える |
| **embedded（埋め込み）** | `Character.ssab` 1 つ。アトラスのバイト列を内包 | 配布物を 1 ファイルにしたい、2 度目のフェッチを避けたい、テクスチャ欠落というバグ類型を消したい |

reference モードでは、画像パスは **`.ssab` 自身のディレクトリからの相対**で書かれています。エクスポートしたファイル一式は同じ場所に置いておけば、ローダーが見つけます。

!!! note "ブラウザでは embedded に追加コストはありません"
    Web Face は画像デコーダを持ちません。ブラウザが既にデコーダを持っているので、埋め込みアトラスはエンコードされたバイト列のまま渡され、`createImageBitmap` で戻ってきます。ネイティブでは
    `embedded` feature（既定で有効）がデコーダを担います。

## ファイルをどこに置くか

| Face | `load` が受け取るもの | 解決の基準 |
|---|---|---|
| Rust | ファイルパス | プロセスのカレントディレクトリ。パックが指す先は `.ssab` 自身のディレクトリ基準 |
| Bevy | `asset_server.load` で `SsPlayer::pack` に渡す asset パス | Bevy の他のアセットと同じくアプリの asset ルート基準。パックが指す先は `.ssab` 自身のディレクトリ基準 |
| C / C++ | ファイルパス | プロセスのカレントディレクトリ。パックが指す先は `.ssab` 自身のディレクトリ基準 |
| Web | URL | ページ基準、その後は `.ssab` 自身の URL ディレクトリ基準 |

`File::open` で届かない場所（Godot の `.pck`、APK、アセットサーバー）では、自前のローダーを差し込みます。[ロードとアセット](../guide/loading.md)を参照。

## 1 回の `load` が持ってくるもの

パックだけではありません。1 呼び出しで再帰的に解決されます：

* パック本体；
* **両方のテクスチャテーブル**にあるアトラス — セルマップと、アーティストがテクスチャチェンジ（`TCHG`）を設定したパーツが指すテクスチャファイル名。2 つ目を取りこぼすと**静かに壊れます**：1×1 の白テクスチャにフォールバックし、パーツが白いシルエットで描かれるだけでエラーは出ません；
* インスタンスパーツが再生する**兄弟 `.ssab`** — 別キャラクターのアニメーションを内部で再生するもの。

つまり 1 ファイルに見えるパックが実際には 3〜4 ファイルであることはよくあり、指定したファイルだけを配布物に入れてしまうのが「ロードは通るのに見た目が壊れる」最頻の原因です。届かなかったものは
`missing_assets()` / `ss_player_missing_*` / `missingAssets()` に並ぶので、開発中に一度確認してください。

## 次に読む

* [ロードとアセット](../guide/loading.md) — ローダーの継ぎ目、テクスチャの自前供給、`missing` の意味
