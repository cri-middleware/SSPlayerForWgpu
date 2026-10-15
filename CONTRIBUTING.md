# Contributing to SSPlayerForWgpu

> [!IMPORTANT]
> We are not accepting pull requests until a Contributor License Agreement
> (CLA) is in place. Issues remain open for bug reports and requests.

Thank you for your interest in contributing to SSPlayerForWgpu! We welcome all
kinds of contributions, including bug reports, feature requests, documentation
improvements, and code contributions.

## Table of Contents
- [Code of Conduct](#code-of-conduct)
- [How Can I Contribute?](#how-can-i-contribute)
- [Development Environment](#development-environment)
- [Coding Standards](#coding-standards)

## Code of Conduct
Please refer to [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

## How Can I Contribute?

### Technical Constraints & Guiding Principles
A change that breaks one of this player's invariants will be asked to change
rather than the invariant. The five that catch newcomers most often:

- **The player never owns wgpu state.** `Device` / `Queue` / `Surface` /
  `SSRenderer` belong to the host and are lent per frame. Any API that takes
  ownership of them is wrong, and **`SSPlayer` must not gain a wgpu-typed
  field** — that rule is what keeps "1 canvas = 1 player" from coming back.
- **There is no vertex buffer.** The runtime's SoA arrays are memcpy'd into the
  scene's arena and expanded in the vertex stage from `@builtin(vertex_index)`;
  the CPU writes one `Prim` per part and nothing per vertex. Add a feature by
  putting it on the `Prim`, not by reintroducing per-vertex CPU work.
- **No per-frame allocations in the playback hot path.** `FrameChunk::reset`
  empties every buffer and keeps its capacity, and GPU buffers only grow; a new
  per-frame buffer goes on the chunk, or on the player's `Walk` when only that
  player's walk uses it.
- **SDK separation.** The animation logic lives in the `SpriteStudio-SDK`
  submodule (`ssruntime`, `ssab`). Algorithm and core-logic changes are a pull
  request *there* followed by a submodule bump here; this repository is the
  renderer and the three host-facing faces.
- **The three faces stay in step.** Rust is the API; C (`cbindgen`) and
  JavaScript (`wasm-bindgen`) are thin wrappers over it. `ffi.rs` holds no
  logic — argument validation, type conversion and error-code mapping only — so
  anything it would need belongs in `host.rs` or `player.rs`, where the Rust API
  gets it too.

Two more worth knowing before proposing a dependency change: **wgpu is pinned to
the 30 line** (a Cargo workspace resolves exactly one wgpu, and a Slint UI
composes with this player on one device), and **no engine may appear in
`ssplayer-wgpu`** — adapters such as `ssplayer-bevy` are separate crates under
`libs/`.

### Reporting Bugs
If you find a bug, please open an issue including:
- A clear, descriptive title.
- Steps to reproduce.
- Your environment (OS, GPU and driver, wgpu backend, and which face you were
  using — Rust, Bevy, C/C++ or WebGPU).
- Expected vs. actual behavior.

A rendering bug is much easier to act on with a screenshot and the `.ssab` that
produced it. If it reproduces on one of the SDK's test projects, say which — that
is a fixture we already have.

*Note: if reproducing a bug requires proprietary `.sspj` project data, please do
not attach it to a public issue — contact the maintainers privately instead.*

### Pull Requests
1. Fork the repository and create your branch from the default branch.
2. Follow the existing code style and the constraints above.
3. Run the verification below, and say in the description which parts you ran —
   a GPU-dependent test result depends on the adapter you have.
4. Submit a Pull Request with a clear description of the change.

If you depart from a documented constraint on purpose, **write the reason next to
the code.** A departure with no stated reason is indistinguishable from a mistake
to whoever reads it next.

## Development Environment

### Prerequisites
- **Rust 1.99+** and a GPU backend wgpu 30 supports (Vulkan / Metal / DX12 /
  WebGPU). On Linux with no GPU driver, Mesa's software Vulkan device (lavapipe,
  `mesa-vulkan-drivers`) is enough. The tests that draw return early without an
  adapter and report `ok`, so `tests/gpu_adapter.rs` fails instead to say so.
- **The SDK submodule.** `ssruntime` and `ssab` are path dependencies into
  `SpriteStudio-SDK/`, so clone with `--recursive` (or run
  `git submodule update --init`).
- For the web face only: `wasm-pack`, `pnpm` (`corepack enable`) and
  `rustup target add wasm32-unknown-unknown`.

### Build and run
```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu

./scripts/deploy-examples.sh    # converts the SDK's test projects into assets/
./scripts/build.sh              # the library and the C face, collected into bin/
cargo run -p ssplayer-wgpu-viewer
```

**`deploy-examples.sh` is a prerequisite, not an optional step.** The tests read
`assets/`, so `cargo test` fails without it.

### Verification
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                    # needs deploy-examples.sh first
cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web
cargo clippy -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web -- -D warnings
cargo clippy -p ssplayer-wgpu --features ffi --all-targets -- -D warnings
cargo build -p ssplayer-wgpu-ffi          # regenerates the C header
cargo test -p ssplayer-wgpu --features ffi --lib
```

No default command compiles `src/ffi.rs` or `src/host.rs`, which is why the C
face takes three lines of its own; `src/web.rs` needs the `web` feature on wasm32,
which is why the web face takes two. **`ssplayer-wgpu-ffi` is the crate that
regenerates the header** — its `build.rs` runs cbindgen; building
`-p ssplayer-wgpu --features ffi` compiles the module but regenerates nothing.
`libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` is committed output, so if it
changes, commit it — CI checks it with `git diff --exit-code`.

For `clippy --fix`, `git add` your own edits first and pass `--allow-staged`, never
`--allow-dirty`, so clippy's edits land unstaged and review apart from yours.

The rest of the matrix:

| Check | Command |
|---|---|
| Fetch the SDK | `git submodule update --init` |
| Generate the fixtures (once, and after an SDK bump) | `./scripts/deploy-examples.sh` — the tests read `assets/`, whose `.ssab` format follows the SDK |
| Build the library for this host | `./scripts/build.sh` |
| A sample from a fresh checkout | `./scripts/run-example.sh` (`example=viewer` / `viewer-bevy` / `overall-bevy` / `web`) — converts and builds only what is missing or what an SDK bump left stale |
| Smallest Rust host | `cargo run -p ssplayer-wgpu-hello` |
| Native viewer | `cargo run -p ssplayer-wgpu-viewer` |
| Metal C++ viewer (macOS) | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| DX12 C++ viewer (Windows) | `cd examples\viewer-dx12-cpp; cmake -B build; cmake --build build --config Release; .\build\Release\ssviewer-dx12.exe` |
| Vulkan C++ viewer (Linux) | `cd examples/viewer-vulkan-cpp && cmake -B build && cmake --build build && ./build/ssviewer-vulkan` — `--x11` / `--wayland` picks the window path |
| Preview shell | `./scripts/build-preview.sh`, then open `bin/ssplayer-preview.html` as a file, not over a server |
| Preview with a pack in it | `cargo run -p ssplayer-preview-gen -- pack=assets/Ringo/Ringo.ssab open=yes` |
| npm package | `./scripts/build.sh platform=web` — needs `pnpm` (`corepack enable`) and `wasm-pack` |
| js layer typecheck | `cd packages/ssplayer-wgpu && pnpm run typecheck`, after a `platform=web` build |
| Web demo | `./scripts/run-web.sh`, or `./scripts/build.sh platform=web && (cd examples/web && python3 -m http.server)` after changing the Rust |
| Third-party notices current | `./scripts/generate-licenses.sh`, then `git diff --exit-code THIRD_PARTY_NOTICES.md` |

**Before and after anything that changes how geometry reaches the GPU, take a golden dump.**
`tests/golden.rs::golden_corpus` is `#[ignore]`d and renders all 134 fixture/animation/frame/stencil
combinations: `SS_GOLDEN_DIR=/tmp/before cargo test --test golden -- --ignored`, then re-run with
`SS_GOLDEN_COMPARE=/tmp/before`. The reference is GPU-specific, so it must come from the same
machine — it is a before/after tool, not a checked-in golden set.

**Measure before and after any performance change, and drive a real frame loop while doing it.**
`SSScene::stats()` reports primitives, draw calls, bytes uploaded, how much of the draw list is
mergeable, and GPU pass time when the device has `Features::TIMESTAMP_QUERY`; both viewers print
it. `tests/scale.rs` interleaves `update` with the render pass, because timing them in separate
loops flatters anything that caches across frames. GPU time is read back a frame or two late by
design; do not add a stall to make it current.

### Documentation

Pages live as both `docs/en/<path>` and `docs/ja/<path>`, and `nav` lives in both locale configs,
so adding or renaming a page means editing both. The English pages also ship inside the release zips
and the npm package, behind the front doors in `scripts/release-docs/`; renaming a page they link to
means editing those too, and `build-release.sh` refuses a link that goes nowhere. The crates carry
them as well: `libs/ssplayer-wgpu/docs` and `libs/ssplayer-bevy/docs` are symlinks to `docs/en`, which
`cargo package` packs as the files they point at. On Windows, enable symlinks (`git config
core.symlinks true`, with Developer Mode) before packaging a crate, or the checkout holds a one-line
text file there.
If you edit a page, build both locales before you finish: nothing builds the docs on a pull request, so that local build is the only gate before the merge (CI builds them weekly).

```bash
./scripts/prepare-docs.sh            # once: .venv + the pins in docs/requirements.txt
./scripts/build-docs.sh              # English then Japanese, both --strict
./scripts/build-pages.sh serve=yes   # docs + rustdoc at /api/ + the WebGPU demo at /demo/
```

Both locales, English first: it clears `site/`, which contains `site/ja`, so an English-only build
leaves no Japanese site at all. `mkdocs.base.yml` sets `strict: true`; `zensical serve` validates
nothing and serves one locale at a time, so `build-pages.sh serve=yes` is the only way to watch the
language selector resolve. `pages.yml`'s build job **is** that script, so CI builds exactly what a
local run with the same options builds.

## Coding Standards

### Rust
- Rendering parity is judged against the WebGL player (`../SSPlayerForWeb/packages/ssplayer-webgl/`), which the drawing code was ported from; consult it before inventing new behaviour.
- Doc comments and the crate READMEs under `libs/` are written in English.
- `cargo fmt` decides formatting; `rustfmt.toml` is in the repository root.
- Clippy runs with `-D warnings` over all targets, so a lint is an error.
- Public items carry doc comments. The crate READMEs under `libs/` are the
  API-facing documents and are expected to move with the code they describe.

### WGSL
- The vertex stage's position maths is deliberately scalar accumulation rather
  than `mat4 * vec4`. Rewriting it "cleanly" invites FMA contraction and costs
  the bit-exact golden comparison, so leave it as it is.

### The C and JavaScript faces
- The generated header and the wasm bindings are outputs, not sources: change
  the Rust and regenerate.
- A name that exists on more than one face keeps the same stem on all of them.
  The SDK's [Player API conventions](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/20_design/30_api_conventions.md)
  are what that means in detail, and they apply across every official player.

---
By contributing to this project, you agree that your contributions will be licensed under the project's [LICENSE.md](./LICENSE.md).

<br>
<br>

---

# SSPlayerForWgpu への貢献について

> [!IMPORTANT]
> コントリビューターライセンス契約（CLA）の準備が整うまで、プルリクエストは受け付けていません。バグ報告や要望は引き続き Issue で受け付けています。

SSPlayerForWgpu にご関心をお寄せいただき、ありがとうございます！ バグ報告、機能提案、ドキュメントの改善、コードの提供など、あらゆる形での貢献を歓迎します。

## 目次
- [行動規範](#行動規範)
- [貢献する方法](#貢献する方法)
- [開発環境のセットアップ](#開発環境のセットアップ)
- [コーディング規約](#コーディング規約)

## 行動規範
[CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md) を参照してください。

## 貢献する方法

### 技術的制約と設計原則
不変条件を破る変更は、不変条件側ではなく変更側を直していただくことになります。特に見落とされやすいのは次の 5 つです。

- **プレイヤーは wgpu の状態を所有しません。** `Device` / `Queue` / `Surface` / `SSRenderer` はホストのもので、フレームごとに貸し出されます。これらの所有権を取る API は誤りで、**`SSPlayer` に wgpu 型のフィールドを持たせてはいけません**。「1 canvas = 1 player」に戻らないのはこの規則のおかげです。
- **頂点バッファはありません。** ランタイムの SoA 配列をシーンのアリーナへ memcpy し、頂点ステージが `@builtin(vertex_index)` から展開します。CPU が書くのはパーツごとに `Prim` 1 つだけで、頂点ごとには何も書きません。機能を足すときは `Prim` に載せてください。頂点ごとの CPU 処理を復活させないこと。
- **再生中は毎フレームのヒープ確保をしません。** `FrameChunk::reset` はバッファを空にしても容量を残し、GPU バッファは伸びるだけです。毎フレーム使うバッファを足すときは chunk に置いてください。1 体のプレイヤーのウォークだけが使うものは、そのプレイヤーの `Walk` に置きます。
- **SDK との分離。** アニメーションのロジックは `SpriteStudio-SDK` submodule (`ssruntime`・`ssab`) にあります。アルゴリズムやコアロジックの変更は**あちらへの**プルリクエストと、こちらでの submodule 更新の組で行います。このリポジトリはレンダラーと、ホストに面した 3 つのフェイスです。
- **3 つのフェイスは歩調を揃えます。** API の本体は Rust で、C (`cbindgen`) と JavaScript (`wasm-bindgen`) はその薄いラッパーです。`ffi.rs` にロジックは置きません（引数検証・型変換・エラーコード対応のみ）。必要なものは `host.rs` か `player.rs` に置き、Rust API 側にも行き渡らせてください。

依存関係の変更を提案する前に、あと 2 点。**wgpu は 30 系に固定**されています（Cargo ワークスペースは wgpu を 1 つしか解決せず、Slint の UI がこのプレイヤーと 1 デバイス上で同居するためです）。また **`ssplayer-wgpu` にエンジンを登場させないでください** — `ssplayer-bevy` のようなアダプタは `libs/` 以下の別クレートです。

### バグの報告
バグを見つけた場合は、以下を含めて issue を作成してください。

- 明確で説明的なタイトル
- 再現手順
- 環境（OS、GPU とドライバ、wgpu のバックエンド、使用していたフェイス — Rust / Bevy / C・C++ / WebGPU）
- 期待する挙動と実際の挙動

描画のバグは、スクリーンショットと再現する `.ssab` があると対応がはるかに容易になります。SDK のテストプロジェクトで再現する場合はどれかをお知らせください。こちらに既にある素材です。

*注意: 再現に非公開の `.sspj` プロジェクトデータが必要な場合、公開 issue には添付せず、メンテナに個別にご連絡ください。*

### プルリクエスト
1. リポジトリを fork し、デフォルトブランチからブランチを作成します。
2. 既存のコードスタイルと、上記の制約に従ってください。
3. 下記の検証を実行し、**どこまで実行したか**を説明に書いてください。GPU に依存するテスト結果は、お手元のアダプタ次第で変わります。
4. 変更内容を明確に説明したプルリクエストを作成してください。

文書化された制約から意図的に外れる場合は、**その理由をコードの隣に書いてください。** 理由の書かれていない逸脱は、次に読む人には単なる間違いと区別がつきません。

## 開発環境のセットアップ

### 前提条件
- **Rust 1.99 以降**と、wgpu 30 が対応する GPU バックエンド（Vulkan / Metal / DX12 / WebGPU）。GPU ドライバの無い Linux では、Mesa のソフトウェア Vulkan（lavapipe、`mesa-vulkan-drivers`）で足ります。描画するテストはアダプタが無いと早期 return して `ok` と報告するので、代わりに `tests/gpu_adapter.rs` が失敗してそれを知らせます
- **SDK submodule。** `ssruntime` と `ssab` は `SpriteStudio-SDK/` への path 依存なので、`--recursive` 付きで clone する（または `git submodule update --init` を実行する）必要があります
- web フェイスの場合のみ: `wasm-pack`、`pnpm`（`corepack enable`）、`rustup target add wasm32-unknown-unknown`

### ビルドと実行
```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu

./scripts/deploy-examples.sh    # SDK のテストプロジェクトを assets/ へ変換
./scripts/build.sh              # ライブラリと C フェイスを bin/ へ収集
cargo run -p ssplayer-wgpu-viewer
```

**`deploy-examples.sh` は省略可能な手順ではなく前提条件です。** テストは `assets/` を読むため、実行しないと `cargo test` が失敗します。

### 検証
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                    # needs deploy-examples.sh first
cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web
cargo clippy -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web -- -D warnings
cargo clippy -p ssplayer-wgpu --features ffi --all-targets -- -D warnings
cargo build -p ssplayer-wgpu-ffi          # regenerates the C header
cargo test -p ssplayer-wgpu --features ffi --lib
```

既定のコマンドでは `src/ffi.rs` も `src/host.rs` もコンパイルされないため、C フェイスは 3 行を要します。`src/web.rs` は wasm32 向けに `web` feature を付けたときだけコンパイルされるため、web フェイスは 2 行を要します。**ヘッダを再生成するのは `ssplayer-wgpu-ffi` クレート**で、その `build.rs` が cbindgen を実行します。`-p ssplayer-wgpu --features ffi` のビルドはモジュールをコンパイルしますが、再生成は行いません。`libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` はコミットされる生成物なので、変更が出たらコミットしてください。CI は `git diff --exit-code` で確認しています。

`clippy --fix` を使うときは、先に自分の編集を `git add` してから `--allow-staged` を付けてください。`--allow-dirty` は使いません。clippy の修正が unstaged に残るので、自分の編集と分けてレビューできます。

ビューア・プレビュー・npm パッケージ・ゴールデンダンプ・性能計測を含むマトリクスの残りは、英語版の [Verification](#verification) にあります。

### ドキュメント

ページは `docs/en/<path>` と `docs/ja/<path>` の両方にあり、`nav` も両方のロケール設定にあります。ページの追加・改名は両方を編集してください。英語のページはリリースの zip と npm パッケージにも入り、`scripts/release-docs/` の入口ページから案内されます。入口ページが指すページを改名したら入口ページも直してください（行き先の無いリンクは `build-release.sh` が拒否します）。crate にも入ります。`libs/ssplayer-wgpu/docs` と `libs/ssplayer-bevy/docs` は `docs/en` への symlink で、`cargo package` はその指す先をファイルとして詰めます。Windows で crate をパッケージする前は symlink を有効にしてください（`git config core.symlinks true` と開発者モード）。有効でないと、checkout にはそこに 1 行のテキストファイルが置かれます。ページを編集したら、仕上げる前に両ロケールをビルドしてください。プルリクエストでは docs はビルドされないため、マージ前のゲートは手元のビルドだけです（CI がビルドするのは週次です）。

```bash
./scripts/prepare-docs.sh            # 一度だけ: .venv と docs/requirements.txt のピン
./scripts/build-docs.sh              # 英語 → 日本語の順に、どちらも --strict
./scripts/build-pages.sh serve=yes   # docs + /api/ の rustdoc + /demo/ の WebGPU デモ
```

必ず両ロケールを、英語を先にビルドします。英語ビルドは `site/ja` を含む `site/` を消すため、英語だけビルドすると日本語サイトは残らず消えます。`mkdocs.base.yml` が `strict: true` を設定します。`zensical serve` は何も検証せず 1 ロケールずつしか配信しないので、言語セレクタを確認できるのは `build-pages.sh serve=yes` だけです。`pages.yml` のビルドジョブはそのスクリプトなので、CI が建てるものは、同じオプションで手元で建てたものと一致します。

## コーディング規約

### Rust
- 描画の一致は WebGL 版 Player（`../SSPlayerForWeb/packages/ssplayer-webgl/`）を基準に判断します。描画コードはそこから移植したものなので、新しい挙動を作る前に参照してください。
- doc コメントと `libs/` 以下の各クレートの README は英語で書きます。
- 整形は `cargo fmt` が決めます。`rustfmt.toml` はリポジトリルートにあります。
- clippy は全ターゲットに対して `-D warnings` で走るため、lint は警告ではなくエラーです。
- 公開項目には doc コメントを付けてください。`libs/` 以下の各クレートの README が API 向けの文書で、説明対象のコードと一緒に動くことが期待されています。

### WGSL
- 頂点ステージの座標計算は、`mat4 * vec4` ではなく意図的にスカラーの累算になっています。「きれいに」書き直すと FMA の contraction を招き、ビット一致のゴールデン比較が壊れます。そのままにしてください。

### C・JavaScript フェイス
- 生成されたヘッダと wasm バインディングはソースではなく出力です。Rust 側を変更して再生成してください。
- 複数のフェイスに存在する名前は、どのフェイスでも同じ語幹を保ちます。その具体的な内容は SDK の [Player API conventions](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/20_design/30_api_conventions.md) にあり、全公式 Player に適用されます。

---
このプロジェクトに貢献することで、あなたの貢献がプロジェクトの [LICENSE.md](./LICENSE.md)（参考訳: [LICENSE.ja.md](./LICENSE.ja.md)）の下でライセンスされることに同意したものとみなされます。
