# 開発者向け

このリポジトリのものはすべて 1 台のマシンでビルドできます。新しいクローンが必要とする順に並べた、
チェックリスト一式です。

## ソースを取得する

```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu
git submodule update --init        # --recursive を忘れた場合
```

`SpriteStudio-SDK` は **git サブモジュール**で、`ssruntime` / `ssab` はそこへの path 依存です。他の
Player が released zip 内の安定 C API を通して SDK と話すのに対し、この Player はそれらのクレートを直接
リンクして FlatBuffers 型を読むため、未リリースの `.ssab` スキーマ変更に追随する必要があります。

!!! danger "サブモジュールの中でコミットしないでください"
    `ssruntime` や `ssab` に変更が必要な場合は、**SDK リポジトリ**への Pull Request を出し、その後こちらで
    サブモジュールを上げます：

    ```bash
    git -C SpriteStudio-SDK fetch origin && git -C SpriteStudio-SDK checkout origin/develop
    cargo test --workspace
    git add SpriteStudio-SDK && git commit -m "chore: bump SDK submodule"
    ```

## 再生データを生成する（最初に 1 回）

```bash
./scripts/deploy-examples.sh
```

サブモジュールから `ssconverter-cli` をビルドし、SDK のテストプロジェクトを `assets/` に変換して、
**両方の**エクスポートモードを `examples/web` に配置します。**テストは `assets/` を読む**ので、これは
`cargo test` の前提であって省略可能な手順ではありません。

## 全 Face をビルドする

| Face | コマンド |
|---|---|
| Rust ライブラリ（＋ C Face）を `bin/` へ収集 | `./scripts/build.sh` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| フォーマット | `cargo fmt --all -- --check` / `cargo fmt --all` |
| テスト | `cargo test --workspace` |
| **wasm / JS Face** | `cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web` |
| **C Face**（＋ヘッダ再生成） | `cargo build -p ssplayer-wgpu-ffi` |
| C ハンドルの寿命テスト | `cargo test -p ssplayer-wgpu --features ffi --lib` |
| npm パッケージ（wasm ＋ TS 層） | `./scripts/build.sh platform=web` — `pnpm` と `wasm-pack` が必要 |
| TS 層の型チェック | `cd packages/ssplayer-wgpu && pnpm run typecheck` |
| ネイティブビューア | `cargo run -p ssplayer-wgpu-viewer` |
| Bevy ビューア | `cargo run -p ssplayer-bevy-overall` / `-p ssplayer-bevy-viewer` |
| Metal C++ ビューア（macOS） | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| DX12 C++ ビューア（Windows） | `cd examples\viewer-dx12-cpp; cmake -B build; cmake --build build --config Release` |
| Vulkan C++ ビューア（Linux） | `cd examples/viewer-vulkan-cpp && cmake -B build && cmake --build build && ./build/ssviewer-vulkan` |
| Web デモ | `./scripts/build.sh platform=web && (cd examples/web && python3 -m http.server)` |

## 知っておくべき 3 つの死角

通常の `cargo build` / `clippy --all-targets` / `cargo test` は、以下を**まったくコンパイルしません**。
どれも、誰かが意図してビルドするまで静かに腐ります：

1. **`web.rs`** は `web` feature の裏にあり、ネイティブのどのコマンドでもコンパイルされません。触ったら
   wasm ターゲットをビルドしてください。
2. **TypeScript 層**は wasm-pack が生成した `.d.ts` に対して型付けされているので、`#[wasm_bindgen]`
   メソッドの改名は cargo が通ったまま TS 層を壊します。Web Face の公開面を変えたら型チェックを走らせて
   ください。
3. **`host.rs` / `ffi.rs`** は `ffi` feature の裏にあり、そのビルドが cbindgen で
   `libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` を再生成します。**再生成されたヘッダは、それを動かした
   変更と一緒にコミットしてください。** そして差分を読むこと。cbindgen はクレート内のモジュールレベルの
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
| **常に `--strict`** | Pull Request でドキュメントをビルドするものが無いので、これが無いとリンク切れは次の公開時に初めて表面化します |

文章を書いている間の高速ループは `build-pages.sh api=no demo=no` です。

## 正典がどこにあるか

| | |
|---|---|
| [`AGENTS.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/AGENTS.md) | 変更が守るべき不変条件と、その検証方法。自明でない変更の前に読んでください |
| [`ROADMAP.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) | 計画済み・未検証・意図的に採用しなかったもの |
| [SDK の `AGENTS.md`](https://github.com/cri-middleware/SpriteStudio-SDK) | 再生セマンティクス（Brain）はここが正典です。このリポジトリに複製してはいけません |
| このドキュメント | 読者向けのガイド。パッケージ README は crates.io / npm / GitHub の玄関として残ります |

## 変更を検証する

CI と同じ順の一通りは：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                    # 先に deploy-examples.sh が必要
cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web
cargo build -p ssplayer-wgpu-ffi                          # C ヘッダも再生成されます
```

テストについて 3 点：

* **レンダリングテストは GPU アダプタが無い環境では自分をスキップします。** そういうマシンでの成功は
  何も検証していない可能性があります。CI はソフトウェア Vulkan デバイス（Mesa lavapipe）を入れ、
  アダプタが無ければ失敗させます。ラスタライズを変更したら、リードバックを PNG に落として目で見てください。
* **性能の変更は前後で計測し、そのとき本物のフレームループを回してください。** `update` とレンダーパスは
  フレームと同じように交互に実行します。別々のループで測ると、フレームを跨いでキャッシュするものが実際より
  ずっと良く見えます。`cargo test --test scale -- --ignored` がこのリポジトリのスループット計測で、両方を
  満たしています（CI のソフトウェア GPU では速度を判定できないため `#[ignore]`）。ホスト側から見える
  カウンタは[パフォーマンス](../guide/performance.md)にあります。
* **ジオメトリに触る前にゴールデンを取ってください。** `tests/golden.rs::golden_corpus`（既定で ignore）は
  全フィクスチャ／アニメーション／フレーム／ステンシルの組み合わせを描画し、前回の実行とバイト比較します：

    ```bash
    SS_GOLDEN_DIR=/tmp/before cargo test --test golden -- --ignored
    # ... 変更する ...
    SS_GOLDEN_COMPARE=/tmp/before cargo test --test golden -- --ignored
    ```

    基準は GPU 固有なので、これはチェックイン済みのゴールデンセットではなく、1 台のマシン上での前後比較
    ツールです。

## リリース

`release.yml` が全プラットフォームをビルドし、`scripts/build-release.sh` がマトリクスの成果物から zip と
チェックサムを組み立てます。ローカルにマトリクスの代替はありません（Linux / Windows / macOS で 3 つの OS が
必要です）。ローカル実行は `gh run download <run-id> -D artifacts` から始めます。

`pages.yml` がこのサイトを公開します（リリース時と、手動実行時）。
