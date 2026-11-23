[**English**](./README.md) | [**日本語**](./README.ja.md)

# SpriteStudio Player for wgpu

**Rust のためのプロ品質 2D アニメーション。ネイティブウィンドウからブラウザ、C++ ホストまでを 1 つの wgpu レンダラーで。**

▶️ **[ブラウザで WebGPU ライブデモを試す](https://cri-middleware.github.io/SSPlayerForWgpu/demo/)** — WebGPU で描画される SpriteStudio アニメーションをブラウザで見られます。インストールは要りません。

**[SpriteStudio](https://www.webtech.co.jp/spritestudio/)** で制作したアニメーション (`.ssab`) を再生する高性能プレイヤーです。[wgpu](https://wgpu.rs/) の上に構築されています。すでにお持ちのスプライト素材
(PNG / WebP) を専用エディタでアニメーションさせ、完成したアニメーションをアプリケーションで再生します。動画ファイルも、作り直しも、エンジンへのロックインもありません。

## ✨ SpriteStudio を wgpu で使うメリット

- **キャラクターもシーン全体も“生きる”——アニメーションのコードなしで。** 呼吸・まばたき、なびく髪や衣装、斬撃・魔法のエフェクト、カットシーンや UI まで——ボーン・メッシュ変形・パーティクルエフェクトは専用エディタ（SpriteStudio）でビジュアルに作り込まれ、1 つの `.ssab` として書き出されます。プレイヤーはそれを再生し、タイムラインのイベントとパーツ単位のオーバーライドでゲームロジックにつながります。
- **モーション同士をブレンド。** アニメーションをトラックに重ね、トラック間でクロスフェードできます。歩きから走りへ、飛ばずになめらかにつながります。詳しくは[モーションブレンディング](./docs/ja/guide/motion-blending.md)を参照してください。
- **GPU はホストのもの、プレイヤーは借りるだけ。** Rust のホストが wgpu の `Device` / `Queue` / `Surface` を所有し、フレームごとに貸し出します（C の Face は、渡したウィンドウから自前で作ります）。そのため自前のアプリでも egui・Bevy でも、どの wgpu アプリケーションの内側にも同居でき、多数のキャラクターを載せたシーンでもアトラスのアップロードは 1 回で共有されます。
- **Rust・Bevy から C/C++・ブラウザまで。** **Bevy** では `ssplayer-bevy` の 3 つのコンポーネントをエンティティに付けるだけで、キャラクターが Bevy 自身の 2D 描画に組み込まれます。C・C++・Objective-C からは、静的ライブラリ 1 本とヘッダ 1 枚です。`HWND`・`NSView`・`CAMetalLayer`・X11 の `Window`・`wl_surface` のいずれかを渡し、毎フレーム 2 つの関数を呼ぶだけ。ブラウザでは WebAssembly ビルドとして動きます。どこで動いても、1 本の wgpu のコードパスがそのプラットフォームのネイティブなグラフィックス API（Windows では DirectX 12、macOS では Metal、Linux では Vulkan、ブラウザでは WebGPU）で描きます。
- **パース不要の読み込みと SIMD。** `.ssab` は FlatBuffers のバイナリをそのまま読み、ポーズは SDK の `#![no_std]` Rust コアが SIMD で計算します。全公式プレイヤーと同じコアです。

## 📚 ドキュメント

- [**ドキュメントサイト**](https://cri-middleware.github.io/SSPlayerForWgpu/ja/) — まずはここから。ホストごとの導入経路 (Rust / Bevy / C・C++ / WebGPU)、共通ガイドと各 Face の API リファレンスがあります。ページの原稿はリポジトリ内の [`docs/ja/`](./docs/ja/index.md) です
- [**English documentation**](https://cri-middleware.github.io/SSPlayerForWgpu/) — 同じサイトの英語版（リポジトリ内では [`docs/en/`](./docs/en/index.md)）
- [**制約と適用範囲**](./docs/ja/limitations.md) — 現在描けるもの、ホスト側に委ねているもの
- [**SS6Player for CPP からの移行**](./docs/ja/migration/from-ss6-cpp.md)
- [**ROADMAP**](./ROADMAP.md) — 予定しているもの、意図的に見送ったもの
- [**AI アシスタント向け**](https://cri-middleware.github.io/SSPlayerForWgpu/ja/llms.txt) — このドキュメントを Markdown で提供します。`llms.txt` はページの目次、`llms-full.txt` は全ページを 1 ファイルにまとめたものです。npm パッケージは同じページを `docs/` に、Agent Skill として `skills/` にも持っています（[skill の入れ方](https://cri-middleware.github.io/SpriteStudio-Docs/ja/ai/docs/)）
- [**SpriteStudio Docs（ポータル）**](https://cri-middleware.github.io/SpriteStudio-Docs/ja/) — SDK と全公式 Player の入口。コーディングアシスタントに SpriteStudio を教える[プラグイン](https://cri-middleware.github.io/SpriteStudio-Docs/ja/ai/)もあります

## 📦 インストール

**アプリケーションで使う場合**はこのまま下へ。ホストを選んで、[使い方](#-使い方)へ。最初の 1 体から先（イベント、パーツ単位の上書き、モーションブレンディング、デバイスロストからの復帰）も同じ道の続きで、[ドキュメント](./docs/ja/index.md)にあります。
**プレイヤー自体を開発する場合**は [CONTRIBUTING.md](./CONTRIBUTING.md) と[ビルドガイド](./docs/ja/development/build.md)（変更したプレイヤーをホストで使う方法と、実際のホストでの確かめ方を含む）へ。層とその理由は[プレイヤーの構成](./docs/ja/development/architecture.md)にあります。

### Rust

クレートを crates.io から追加します:

```bash
cargo add ssplayer-wgpu
```

まだリリースしていない変更を使うときは、代わりにこのリポジトリへの git 依存にします。submodule のチェックアウトは cargo が行うので、他のツールは要りません:

```toml
[dependencies]
ssplayer-wgpu = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
```

**Bevy** アプリの中で使う場合は、代わりにアダプタへ依存してください。プレイヤーを引き込んだ上で、キャラクターを Bevy 自身の 2D 描画に組み込むための 3 つのコンポーネント
(`SsPlayer` / `SsTransform` / `SsPlayback`) を追加します:

```bash
cargo add ssplayer-bevy
```

まだリリースしていない変更を使うときは、git 依存にします:

```toml
[dependencies]
ssplayer-bevy = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
```

対象とする Bevy のラインは一度に 1 本だけです（どのラインかは[アダプタの README](./libs/ssplayer-bevy)
に記載）。Bevy とプレイヤーが単一の `wgpu` に解決される必要があるためで、食い違いは実行時の驚きではなくコンパイルエラーになります。

### C と C++

C Face は cdylib / staticlib と、生成されたヘッダです。各 [`ssplayer-wgpu` の Release](https://github.com/cri-middleware/SSPlayerForWgpu/releases?q=ssplayer-wgpu&expanded=true) の **Assets** に、静的ライブラリがプラットフォームごとの zip で付いています。`ssplayer-wgpu-windows-x86_64.zip`（MSVC、`/MD` ランタイム）と `ssplayer-wgpu-macos-universal.zip`（arm64 + x86_64。静的ライブラリの横に、署名・公証済みの共有ライブラリも入っています）で、どちらも `include/`・`lib/`・`LICENSE.md`・`LICENSE.ja.md`・`THIRD_PARTY_NOTICES.md` と、ドキュメントの Markdown（`README.md`・`docs/`）を含み、`SHA256SUMS` が添えてあります。リンクの方法、静的ランタイム（`/MT`）向けのビルド、チェックアウトからのビルド（Linux や Windows の DLL）は [C / C++ ではじめる](./docs/ja/start/cpp.md) にあります。[`examples/`](#-サンプル) 以下の C++ ビューアが、コピーして使える最短の実例です。

### ブラウザ (WebGPU)

ブラウザ向けの Face は npm パッケージ
[`@spritestudio/ssplayer-wgpu`](./packages/ssplayer-wgpu) です。wasm と、その上に載る小さな
TypeScript 層で構成されています。npm からインストールします:

```bash
npm i @spritestudio/ssplayer-wgpu
```

必要なものを import します:

```js
import { sharedContext } from '@spritestudio/ssplayer-wgpu';           // ページ共有のデバイス
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';  // <ss-player-webgpu>
```

`SsContextWeb` / `SsSurfaceWeb` / `SsPlayerWeb` が API の全体です（ページごとに 1 デバイス、canvas ごとに
1 サーフェス、1 パスに任意個数のキャラクター）。カスタム要素はその上に載る 1 キャラクター用の簡便なラッパーで、使わないバンドルが抱え込まずに済むよう独立したエントリポイントに分けてあります。
JavaScript 側から読むべきガイドは [Web ではじめる](./docs/ja/start/web.md) です。

このリポジトリからのビルドは[ビルドガイド](./docs/ja/development/build.md)にあります。

### その `.ssab` はどこから来るのか

アニメーションは SpriteStudio（**7.6 以上**）で制作し、事前に変換します。このプレイヤーは実行時には何も変換せず、コンバータも同梱しません。デスクトップの
[SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) か、`ssconverter-cli` を使ってください。`ssconverter-cli` は [SpriteStudio-SDK の Release](https://github.com/cri-middleware/SpriteStudio-SDK/releases) の **Assets** にある OS ごとの zip で、プロジェクトを 1 つのコマンドで `.ssab` のフォルダに変換します。

```bash
ssconverter-cli overall.sspj -o assets/overall
```

ツールの選び方とビルドパイプラインへの組み込み方は[単体ツールで変換する](https://cri-middleware.github.io/SpriteStudio-Docs/ja/overview/convert-with-a-standalone-tool/)にあります。自分の `.sspj` がまだ無い場合は、SpriteStudio の[公式サンプルデータ](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/)がそのまま使えます。キャラクターなら [Ringo](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/#ringo) が手頃で、上の 2 つのツールのどちらにもそのまま渡せます。自分のアニメーションを作るには SpriteStudio 本体を使います（[製品サイト](https://www.webtech.co.jp/spritestudio/)）。チェックアウトが要る以下のサンプルは、SDK 自身のテストアニメーションで動きます。

## ⚡ 使い方

Rust では、デバイスごとに `SSRenderer` を 1 つ、レンダーターゲットごとに `SSScene` を 1 つ、そこへ任意個数の
`SSPlayer` を 1 パスで描画します:

```rust
let mut renderer = SSRenderer::new(&device);
let mut scene = SSScene::new(&device, &SceneConfig { color_format, stencil_format, mask: MaskBackend::Stencil });
scene.set_viewport(&queue, width as f32, height as f32, /* y_down */ false);

let mut player = SSPlayer::new();
player.load("assets/overall/Basic.ssab")?;   // 変換済みの任意の .ssab。パック・アトラス・兄弟パックを 1 呼び出しで
player.set_animation("anime_1")?;
player.fit_into(width as f32, height as f32, &Placement::default());   // キャンバス全体を表示に収める
player.play();

scene.update(&mut renderer, &device, &queue, delta_secs, &mut [&mut player]);
scene.draw(&renderer, &mut rpass);           // ホストが開いたパスへ
```

API の全体 — 3 つのスコープ、ロード、トランスポート、パーツ単位の上書き、イベント、モーションブレンディング、デバイスロストからの復帰 — は [Rust API リファレンス](./docs/ja/reference/rust.md)と、そこからリンクしたガイドにあります。
C と JavaScript の Face は名前が違うだけで同じ形をしており、以下の各サンプルがそれぞれの環境について自分で説明しています。

**次に読む:** [はじめる](./docs/ja/start/rust.md)（[Bevy](./docs/ja/start/bevy.md)・[C / C++](./docs/ja/start/cpp.md)・[ブラウザ](./docs/ja/start/web.md)） · [再生](./docs/ja/guide/playback.md) · [制約と適用範囲](./docs/ja/limitations.md)

## 🎬 サンプル

環境ごとにホストが 1 つ（Rust は最小のホストと開発用ビューアの 2 つ、ウィンドウをエンジンが所有する Bevy も 2 つ）、それぞれ自己完結しています。どれもその環境の利用ガイドを兼ねた README を持っており、ウィンドウの作り方、デバイスの貸し方、アセットの入れ方が書いてあります。

| サンプル | 環境 | 実行方法 |
|---|---|---|
| [`examples/hello`](./examples/hello) | Rust, winit — 最小のホスト。キャラクター 1 体 | `cargo run -p ssplayer-wgpu-hello` |
| [`examples/viewer`](./examples/viewer) | Rust, winit — 開発用ビューア。複数キャラクターを並べたもの | `cargo run -p ssplayer-wgpu-viewer` |
| [`examples/web`](./examples/web) | ブラウザ上の WebGPU | `./scripts/run-web.sh` |
| [`examples/cdn`](./examples/cdn) | ブラウザ上の WebGPU。npm パッケージを jsDelivr から直接読み込む（ビルド不要） | `cp assets/Ringo_emb/Ringo.ssab examples/cdn/ && (cd examples/cdn && python3 -m http.server --bind 127.0.0.1)` |
| [`examples/viewer-metal-cpp`](./examples/viewer-metal-cpp) | C++ / Metal, macOS | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| [`examples/viewer-dx12-cpp`](./examples/viewer-dx12-cpp) | C++ / DX12, Windows | `cd examples\viewer-dx12-cpp; cmake -B build; cmake --build build --config Release; .\build\Release\ssviewer-dx12.exe` |
| [`examples/viewer-vulkan-cpp`](./examples/viewer-vulkan-cpp) | C++ / Vulkan, Linux — X11 と Wayland を 1 つのバイナリに | `cd examples/viewer-vulkan-cpp && cmake -B build && cmake --build build && ./build/ssviewer-vulkan` |
| [`examples/viewer-bevy`](./examples/viewer-bevy) | Bevy — アダプタの開発用ビューア。複数キャラクターを並べたもの | `cargo run -p ssplayer-bevy-viewer` |
| [`examples/overall-bevy`](./examples/overall-bevy) | Bevy — 機能を順に見せるツアー | `cargo run -p ssplayer-bevy-overall` |

clone したばかりの状態からは、スクリプト 1 本で動かせます。初回はデータを変換してからサンプルを起動し、指定しなければ `hello` を動かします（Windows では `.ps1`）:

```bash
./scripts/run-example.sh example=viewer   # or viewer-bevy / overall-bevy / web
./scripts/run-web.sh                      # ブラウザデモ: パッケージが無ければビルドし、サーブして開く
```

表のコマンドで手動で動かす場合は、先に再生データを生成してください:

```bash
./scripts/deploy-examples.sh    # SDK から ssconverter-cli をビルドし、テストプロジェクトを変換して assets/ へ
```

C++ ビューアは CMake 経由でライブラリを駆動し、その CMake が cargo 自体を実行するので、別途ビルド手順は要りません。

## 🔗 関連リポジトリ

- [SpriteStudio Docs](https://cri-middleware.github.io/SpriteStudio-Docs/ja/) — SDK と全公式 Player のドキュメントポータル
- [SpriteStudio-SDK](https://github.com/cri-middleware/SpriteStudio-SDK) — `ssruntime` と `ssconverter` を提供するコア SDK
- [SSPlayerForWeb](https://github.com/cri-middleware/SSPlayerForWeb) — Web 向け Player 群（WebGPU が無い環境向けの WebGL レンダラーを含む）
- [SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) — Player なしで `.sspj` を `.ssab` へ変換できるデスクトップ GUI ツール

## 📄 ライセンス

BSD-3-Clause です。[LICENSE.md](./LICENSE.md)（正文）を参照してください。参考訳は
[LICENSE.ja.md](./LICENSE.ja.md) にあります。

本ライブラリは自身も Rust クレート（wgpu とそのバックエンド、FlatBuffers、`image` デコーダなど）をリンクしており、それらのライセンスは皆さんが配布する成果物にも義務を課します。謝辞に何を含めるべきかは
[THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md) にあります。依存関係が変わったときは
`scripts/generate-licenses.sh` で再生成してください。
