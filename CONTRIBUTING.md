# Contributing to SSPlayerForWgpu

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
[AGENTS.md](./AGENTS.md) carries the full list of invariants under *Hard
constraints*, and a change that breaks one will be asked to change rather than
the constraint. The four that catch newcomers most often:

- **The player never owns wgpu state.** `Device` / `Queue` / `Surface` /
  `SSRenderer` belong to the host and are lent per frame. Any API that takes
  ownership of them is wrong, and **`SSPlayer` must not gain a wgpu-typed
  field** — that rule is what keeps "1 canvas = 1 player" from coming back.
- **There is no vertex buffer.** The runtime's SoA arrays are memcpy'd into the
  scene's arena and expanded in the vertex stage from `@builtin(vertex_index)`;
  the CPU writes one 80-byte `Prim` per part and nothing per vertex. Add a
  feature by putting it on the `Prim`, not by reintroducing per-vertex CPU work.
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
the 29 line** (a Cargo workspace resolves exactly one wgpu, and a Slint UI
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
- **Rust 1.97+** and a GPU backend wgpu 29 supports (Vulkan / Metal / DX12 /
  WebGPU).
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
cargo clippy -p ssplayer-wgpu --features ffi --all-targets -- -D warnings
cargo build -p ssplayer-wgpu-ffi          # regenerates the C header
cargo test -p ssplayer-wgpu --features ffi --lib
```

No default command compiles `src/ffi.rs` or `src/host.rs`, which is why the C
face takes three lines of its own. **`ssplayer-wgpu-ffi` is the crate that
regenerates the header** — its `build.rs` runs cbindgen; building
`-p ssplayer-wgpu --features ffi` compiles the module but regenerates nothing.
`libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` is committed output, so if it
changes, commit it — CI checks it with `git diff --exit-code`.

[AGENTS.md](./AGENTS.md) carries the full matrix, including the auto-fix forms.

## Coding Standards

### Rust
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
  The SDK's [Player API conventions](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/20_design/40_api_conventions.md)
  are what that means in detail, and they apply across every official player.

---
By contributing to this project, you agree that your contributions will be licensed under the project's [LICENSE.md](./LICENSE.md).

<br>
<br>

---

# SSPlayerForWgpu への貢献について

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
不変条件の全体は [AGENTS.md](./AGENTS.md) の *Hard constraints* にあります。制約を破る変更は、制約側ではなく変更側を直していただくことになります。特に見落とされやすいのは次の 4 つです。

- **プレイヤーは wgpu の状態を所有しません。** `Device` / `Queue` / `Surface` / `SSRenderer` はホストのもので、フレームごとに貸し出されます。これらの所有権を取る API は誤りで、**`SSPlayer` に wgpu 型のフィールドを持たせてはいけません**。「1 canvas = 1 player」に戻らないのはこの規則のおかげです。
- **頂点バッファはありません。** ランタイムの SoA 配列をシーンのアリーナへ memcpy し、頂点ステージが `@builtin(vertex_index)` から展開します。CPU が書くのはパーツごとに 80 バイトの `Prim` 1 つだけで、頂点ごとには何も書きません。機能を足すときは `Prim` に載せてください。頂点ごとの CPU 処理を復活させないこと。
- **SDK との分離。** アニメーションのロジックは `SpriteStudio-SDK` submodule (`ssruntime`・`ssab`) にあります。アルゴリズムやコアロジックの変更は**あちらへの**プルリクエストと、こちらでの submodule 更新の組で行います。このリポジトリはレンダラーと、ホストに面した 3 つのフェイスです。
- **3 つのフェイスは歩調を揃えます。** API の本体は Rust で、C (`cbindgen`) と JavaScript (`wasm-bindgen`) はその薄いラッパーです。`ffi.rs` にロジックは置きません（引数検証・型変換・エラーコード対応のみ）。必要なものは `host.rs` か `player.rs` に置き、Rust API 側にも行き渡らせてください。

依存関係の変更を提案する前に、あと 2 点。**wgpu は 29 系に固定**されています（Cargo ワークスペースは wgpu を 1 つしか解決せず、Slint の UI がこのプレイヤーと 1 デバイス上で同居するためです）。また **`ssplayer-wgpu` にエンジンを登場させないでください** — `ssplayer-bevy` のようなアダプタは `libs/` 以下の別クレートです。

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
- **Rust 1.97 以降**と、wgpu 29 が対応する GPU バックエンド（Vulkan / Metal / DX12 / WebGPU）
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
cargo clippy -p ssplayer-wgpu --features ffi --all-targets -- -D warnings
cargo build -p ssplayer-wgpu-ffi          # regenerates the C header
cargo test -p ssplayer-wgpu --features ffi --lib
```

既定のコマンドでは `src/ffi.rs` も `src/host.rs` もコンパイルされないため、C フェイスだけ 3 行を要します。**ヘッダを再生成するのは `ssplayer-wgpu-ffi` クレート**で、その `build.rs` が cbindgen を実行します。`-p ssplayer-wgpu --features ffi` のビルドはモジュールをコンパイルしますが、再生成は行いません。`libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` はコミットされる生成物なので、変更が出たらコミットしてください。CI は `git diff --exit-code` で確認しています。

自動修正の形を含む一覧の全体は [AGENTS.md](./AGENTS.md) にあります。

## コーディング規約

### Rust
- 整形は `cargo fmt` が決めます。`rustfmt.toml` はリポジトリルートにあります。
- clippy は全ターゲットに対して `-D warnings` で走るため、lint は警告ではなくエラーです。
- 公開項目には doc コメントを付けてください。`libs/` 以下の各クレートの README が API 向けの文書で、説明対象のコードと一緒に動くことが期待されています。

### WGSL
- 頂点ステージの座標計算は、`mat4 * vec4` ではなく意図的にスカラーの累算になっています。「きれいに」書き直すと FMA の contraction を招き、ビット一致のゴールデン比較が壊れます。そのままにしてください。

### C・JavaScript フェイス
- 生成されたヘッダと wasm バインディングはソースではなく出力です。Rust 側を変更して再生成してください。
- 複数のフェイスに存在する名前は、どのフェイスでも同じ語幹を保ちます。その具体的な内容は SDK の [Player API conventions](https://github.com/cri-middleware/SpriteStudio-SDK/blob/HEAD/libs/ssruntime/docs/20_design/40_api_conventions.md) にあり、全公式 Player に適用されます。

---
このプロジェクトに貢献することで、あなたの貢献がプロジェクトの [LICENSE.ja.md](./LICENSE.ja.md) の下でライセンスされることに同意したものとみなされます。
