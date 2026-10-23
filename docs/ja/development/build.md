# 🛠️ ビルドガイド

SpriteStudio Player for wgpu を開発する人と、自分のホストのために改造する人に向けたページです。このリポジトリのものはすべて 1 台のマシンでビルドでき、このページは新しいクローンが必要とする順に並べたチェックリスト一式です。クレートの構成とその理由は[プレイヤーの構成](architecture.md)にあります。

## ソースを取得する

```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu
git submodule update --init        # --recursive を忘れた場合
```

`SpriteStudio-SDK` は **git サブモジュール**で、`ssruntime` / `ssab` はそこへの path 依存です。他の
Player が released zip 内の安定 C API を通して SDK と話すのに対し、この Player はそれらのクレートを直接リンクして FlatBuffers 型を読むため、未リリースの `.ssab` スキーマ変更に追随する必要があります。

!!! danger "サブモジュールの中でコミットしないでください"
    `ssruntime` や `ssab` に変更が必要な場合は、**SDK リポジトリ**への Pull Request を出し、その後こちらでサブモジュールを上げます：

    ```bash
    git -C SpriteStudio-SDK fetch origin && git -C SpriteStudio-SDK checkout origin/develop
    ./scripts/deploy-examples.sh
    cargo test --workspace
    git add SpriteStudio-SDK && git commit -m "chore: bump SDK submodule"
    ```

## 再生データを生成する（最初に 1 回）

```bash
./scripts/deploy-examples.sh
```

サブモジュールから `ssconverter-cli` をビルドし、SDK のテストプロジェクトを `assets/` に変換して、**両方の**エクスポートモードを `examples/web` に配置します。**テストは `assets/` を読む**ので、これは
`cargo test` の前提であって省略可能な手順ではありません。

SDK を上げたら、もう一度実行してください。`.ssab` の形式は SDK に従うので、別の SDK コミットで変換したデータはランタイムを panic させることがあります。`run-example.sh` と `build-pages.sh` はこれを自分で確かめます（`deploy-examples.sh when=stale`）。

## 全 Face をビルドする

| Face | コマンド |
|---|---|
| Rust ライブラリ（＋ C Face）を `bin/` へ収集 | `./scripts/build.sh` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| フォーマット | `cargo fmt --all -- --check` / `cargo fmt --all` |
| テスト | `cargo test --workspace` |
| **wasm / JS Face** | `cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web` |
| Lint（wasm / JS Face） | `cargo clippy -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web -- -D warnings` |
| **C Face**（＋ヘッダ再生成） | `cargo build -p ssplayer-wgpu-ffi` |
| C ハンドルの寿命テスト | `cargo test -p ssplayer-wgpu --features ffi --lib` |
| npm パッケージ（wasm ＋ TS 層） | `./scripts/build.sh platform=web` — `pnpm` と `wasm-pack` が必要 |
| TS 層の型チェック | `cd packages/ssplayer-wgpu && pnpm run typecheck` |
| 最小の Rust ホスト | `cargo run -p ssplayer-wgpu-hello` |
| ネイティブビューア | `cargo run -p ssplayer-wgpu-viewer` |
| Bevy ビューア | `cargo run -p ssplayer-bevy-overall` / `-p ssplayer-bevy-viewer` |
| Metal C++ ビューア（macOS） | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| DX12 C++ ビューア（Windows） | `cd examples\viewer-dx12-cpp; cmake -B build; cmake --build build --config Release` |
| Vulkan C++ ビューア（Linux） | `cd examples/viewer-vulkan-cpp && cmake -B build && cmake --build build && ./build/ssviewer-vulkan` |
| Web デモ | `./scripts/build.sh platform=web && (cd examples/web && python3 -m http.server --bind 127.0.0.1)` |

## 知っておくべき 3 つの死角

通常の `cargo build` / `clippy --all-targets` / `cargo test` は、以下を**まったくコンパイルしません**。どれも、誰かが意図してビルドするまで静かに腐ります：

1. **Web の Face** は `web` feature の裏にあり、ネイティブのどのコマンドでもコンパイルされません。触ったら wasm ターゲットをビルドし、Lint もかけてください。
2. **TypeScript 層**は wasm-pack が生成した `.d.ts` に対して型付けされているので、`#[wasm_bindgen]`
   メソッドの改名は cargo が通ったまま TS 層を壊します。Web Face の公開面を変えたら型チェックを走らせてください。
3. **C の Face** は `ffi` feature の裏にあり、そのビルドが cbindgen で `ssplayer_wgpu.h` を再生成します。**再生成されたヘッダは、それを動かした変更と一緒にコミットしてください。** そして差分を読むこと。cbindgen はクレート内のモジュールレベルの
   `pub const` をすべて拾います。

## ドキュメントサイトをビルドする

サイトは Markdown ＋ 2 つの接ぎ木で、文章側だけなら cargo は不要です：

```bash
./scripts/prepare-docs.sh          # .venv とピン留めした zensical
./scripts/build-docs.sh            # 両ロケール、--strict、site/ へ
./scripts/build-docs.sh serve=yes  # ... そのまま配信

./scripts/build-pages.sh serve=yes # pages.yml が公開するもの一式：docs ＋ rustdoc ＋ WebGPU デモ
```

| ルール | |
|---|---|
| **英語が正、`docs/ja` はそのミラー** | 両ツリーで同じパス。異なるのは文章とナビゲーションのラベルだけ |
| **ナビゲーションは 2 ファイルにある** | `mkdocs.yml` と `mkdocs.ja.yml`。ページの追加・移動・改名は両方の編集を意味します |
| **英語を先にビルドする** | 英語ビルドが `site/` を消し、日本語サイト・rustdoc・デモはその中に入るためです |
| **常に `--strict`** | リンク切れを警告で済ませず、手元でも CI でもビルドを失敗させます |

文章を書いている間の高速ループは `build-pages.sh api=no demo=no` です。

## 正典がどこにあるか

| | |
|---|---|
| [`CONTRIBUTING.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/CONTRIBUTING.md) | 変更が守るべき制約と、その検証方法。自明でない変更の前に読んでください |
| [`ROADMAP.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) | 計画済み・未検証・意図的に採用しなかったもの |
| [SDK の Player 移植ガイド](https://github.com/cri-middleware/SpriteStudio-SDK/tree/HEAD/libs/ssruntime/docs) | 再生セマンティクス（Brain）はここが正典です。このリポジトリに複製してはいけません |
| このドキュメント | 読者向けのガイド。パッケージ README は npm と GitHub の玄関として残ります |

## 変更を検証する

CI と同じ順の一通りは：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                    # 先に deploy-examples.sh が必要
cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web
cargo clippy -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web -- -D warnings
cargo clippy -p ssplayer-wgpu --features ffi --all-targets -- -D warnings
cargo build -p ssplayer-wgpu-ffi                          # C ヘッダも再生成されます
cargo test -p ssplayer-wgpu --features ffi --lib
```

テストについて 3 点：

* **レンダリングテストは GPU アダプタが無い環境では自分をスキップします。** そういうマシンでの成功は何も検証していない可能性があります。CI はソフトウェア Vulkan デバイス（Mesa lavapipe）を入れ、アダプタが無ければ失敗させます。ラスタライズを変更したら、リードバックを PNG に落として目で見てください。
* **性能の変更は前後で計測し、そのとき本物のフレームループを回してください。** `update` とレンダーパスはフレームと同じように交互に実行します。別々のループで測ると、フレームを跨いでキャッシュするものが実際よりずっと良く見えます。`cargo test --test scale -- --ignored` がこのリポジトリのスループット計測で、両方を満たしています（CI のソフトウェア GPU では速度を判定できないため `#[ignore]`）。ホスト側から見えるカウンタは[パフォーマンス](../guide/performance.md)にあります。
* **ジオメトリに触る前にゴールデンを取ってください。** ゴールデンのコーパスのテスト（既定で ignore）は全フィクスチャ／アニメーション／フレーム／ステンシルの組み合わせを描画し、前回の実行とバイト比較します：

    ```bash
    SS_GOLDEN_DIR=/tmp/before cargo test --test golden -- --ignored
    # ... 変更する ...
    SS_GOLDEN_DIR=/tmp/after SS_GOLDEN_COMPARE=/tmp/before cargo test --test golden -- --ignored
    ```

    基準は GPU 固有なので、これはチェックイン済みのゴールデンセットではなく、1 台のマシン上での前後比較ツールです。比較すると `/tmp/after/summary.md` を書き、変わった画像ごとに前・後・差分を並べた画像を `/tmp/after/diff/` に書きます。CI はプルリクエストごとに同じことを lavapipe 上でベースの描画に対して行い、意図した変化も含めて、差があれば失敗します。意図した変化なら、CI がアップロードする並べた画像（`golden-diff`）がなぜ正しいのかを説明に書いてください。

## 変更したプレイヤーを使う

どの Face も、リリースと同じ形でチェックアウトを受け取ります。

- **Rust:** path 依存で使います（`ssplayer-wgpu = { path = "<checkout>/libs/ssplayer-wgpu" }`。`ssplayer-bevy` も同じ）。`ssruntime` と `ssab` はサブモジュール経由で一緒に付いてきます。プレイヤーはホストのデバイスを借りるので、ホストの `wgpu` はワークスペースの `Cargo.toml` が指定する版にそろえてください。
- **C / C++:** `./scripts/build.sh` がライブラリと `ssplayer_wgpu.h` を `bin/` に集めます（`bin/libs/<platform>/<arch>/`、`bin/include/`）。`examples/` の C++ ビューアと同じようにリンクしてください。
- **npm:** パッケージをビルドして pack し、その tarball をアプリにインストールします。

    ```bash
    ./scripts/build.sh platform=web                  # wasm-pack + tsdown -> packages/ssplayer-wgpu/{wasm,dist}
    (cd packages/ssplayer-wgpu && pnpm pack)         # -> spritestudio-ssplayer-wgpu-<version>.tgz
    ```

    そのあと、アプリで `npm install <checkout>/packages/ssplayer-wgpu/spritestudio-ssplayer-wgpu-<version>.tgz` を実行します。`prepack` は、うたっている `dist/` や `wasm/` が無い tarball を拒むので、pack できた tarball は欠けていません。

## 実際のホストで確かめる

CI と上のテストでは見えないものと、その確かめ方です。

- **Linux のデスクトップ。** CI は Vulkan の C++ ビューアをビルドし、lavapipe でヘッドレスに描きます。Wayland のコンポジタ、Xorg のセッション、対話的なリサイズには Linux のデスクトップが要ります。デスクトップのある VM で足ります。チェックアウトは `rsync -a --exclude target` でコピーしてください。サブモジュールも、VM に資格情報を置かずに運べます。
- **オフスクリーンの確認では黒でクリアしません。** 黒でクリアして「黒でない」画素を数えると、黒い絵が「何も描いていない」に見えます。黒一色のセルでフェードアウトするアセットはよくあります。別の色でクリアし、その色と違う画素を数えてください。レンダリングテスト自身の画素の数え方は黒でクリアして黒でないものを数えるので、そういうアセットには使えません。ビューポートは `set_animation()` の**後**に `canvas_size()` から読みます（前は `None` です）。低い解像度で画素数が 0 でも、すぐに不具合とみなさないでください。5×4 px のセルはそこで消えることがあります。
- **フレームのプロファイル。** ワークスペースの release プロファイルはシンボルを落とすので、プロファイルするものは `CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` と専用の `CARGO_TARGET_DIR` でビルドし、[samply](https://github.com/mstange/samply) で記録します（`--save-only --unstable-presymbolicate`）。LTO がフレームをインライン化して消すので、末端の self time を、いちばん近い呼び出し元と合わせて読みます。`deploy-examples.sh` はコンバータを `SpriteStudio-SDK/target` で探すので、`CARGO_TARGET_DIR` を外して実行してください。
- **前後の比較は交互に回します。** 両方をビルドし、3〜5 回交互に回して、比と中央値で比べます。絶対的な時間はマシンの負荷で 4 分の 1 ほど動き、GPU の単発の時間は GPU のクロックの状態で二峰になります。

## リリース

`release.yml` が全プラットフォームをビルドし、`scripts/build-release.sh` がマトリクスの成果物から zip、npm の tarball、チェックサムを組み立てます。ローカルにマトリクスの代替はありません（Windows と macOS で、それぞれの OS が必要です）。ローカル実行は `gh run download <run-id> -D artifacts` から始めます。

`pages.yml` がこのサイトを公開します（リリース時と、手動実行時）。
