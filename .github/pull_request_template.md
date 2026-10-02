<!--
Thanks for contributing! See CONTRIBUTING.md for the constraints this project works under
(the player owns no wgpu state, no vertex buffer, the three faces stay in step).
CONTRIBUTING.md に本プロジェクトの制約が記載されています。日本語でも英語でも構いません。
-->

## What does this change? / 変更内容

<!-- What it does and why. Link any related issue: Fixes #123 -->

## How was it verified? / 検証方法

<!--
Which checks ran, and on what. If you could not run them, say so —
that is useful information, not a problem.
実行した確認と環境。実行できなかった場合はその旨をお書きください。
-->

- [ ] `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace` after `./scripts/deploy-examples.sh` — `tests/render.rs` skips itself without a GPU adapter
- [ ] The wasm and `ffi` builds CONTRIBUTING lists, if the API changed
- [ ] Ran an example or the preview / サンプルかプレビューで確認した
- [ ] Not run — see notes below / 未実行（理由は下記）

**Environment / 環境:** OS, GPU and driver, wgpu backend, face (Rust / Bevy / C / WebGPU)

## Checklist

- [ ] Scoped to one thing. Unrelated fixes are easier to review as their own PR.
- [ ] Follows the style of the files it touches.
- [ ] Docs updated if behaviour changed.
- [ ] Anything that exists twice changed twice — a page and its translation, a `.sh` and its `.ps1`.
- [ ] `SSPlayer` gains no wgpu-typed field; `Device` / `Queue` / `Surface` / `SSRenderer` stay the host's.
- [ ] No vertex buffer: one `Prim` per part, nothing per vertex on the CPU.
- [ ] The three faces stay in step: `ffi.rs` holds no logic, and `include/ssplayer_wgpu.h` is regenerated and committed.
- [ ] wgpu stays on the 30 line, and no engine appears in `ssplayer-wgpu`.

## Anything reviewers should know? / レビュー時の注意点

<!-- Trade-offs you made, alternatives you rejected, parts you are unsure about. -->
