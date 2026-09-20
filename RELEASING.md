# Releasing ssplayer-wgpu

Maintainers only. Contributors do not run any of this — see [CONTRIBUTING.md](./CONTRIBUTING.md).

## Building the documentation site

```bash
./scripts/prepare-docs.sh            # once: .venv + the pins in docs/requirements.txt
./scripts/build-docs.sh              # English then Japanese, both --strict
./scripts/build-pages.sh serve=yes   # docs + rustdoc at /api/ + the WebGPU demo at /demo/
```

Both locales, English first: it clears `site/`, which contains `site/ja`. `mkdocs.base.yml` sets
`strict: true`, and nothing checks links on a pull request, so this build is the only gate.
`zensical serve` validates nothing and serves one locale at a time, so `build-pages.sh serve=yes`
is the only way to watch the language selector resolve. `pages.yml`'s build job **is** that script
with every option spelled out, so CI and the local gate cannot drift.

`nav` lives in **both** locale configs, so adding or renaming a page means editing both.

## Building the shipped artifacts

```bash
./scripts/build.sh                    # the library for this host, C face included, into bin/
./scripts/build.sh platform=web       # the wasm and the TypeScript layer, into packages/ssplayer-wgpu/
./scripts/build-preview.sh            # bin/ssplayer-preview.html, self-contained, opens from file://
./scripts/generate-licenses.sh        # the committed THIRD_PARTY_NOTICES.md
```

`build.sh` compiles the **library**; `deploy-examples.sh` produces the **data**. One script changes
when a fixture moves, the other when the library layout does. `platform=web` has no collect
directory — it fills `packages/ssplayer-wgpu/{wasm,dist}` in place, because for that face the
package directory *is* the artifact. `build-preview.sh` builds its own `wasm-pack --target
no-modules` bundle, a different target from the npm package's, because an ES module cannot load
from `file://`.

Check the npm tarball before publishing: `cd packages/ssplayer-wgpu && pnpm pack`. Its `prepack`
refuses a package whose advertised `dist/` or `wasm/` is missing.

<br>

---

# ssplayer-wgpu のリリース手順

維持者向けです。コントリビューターがこの手順を実行することはありません（[CONTRIBUTING.md](./CONTRIBUTING.md) を参照してください）。

## ドキュメントサイトのビルド

```bash
./scripts/prepare-docs.sh            # 一度だけ: .venv と docs/requirements.txt のピン
./scripts/build-docs.sh              # 英語 → 日本語の順に、どちらも --strict
./scripts/build-pages.sh serve=yes   # docs + /api/ の rustdoc + /demo/ の WebGPU デモ
```

必ず両ロケールを、英語を先に（英語ビルドは `site/ja` を含む `site/` を消します）。
`mkdocs.base.yml` が `strict: true` を設定します。プルリクエストではリンク検査が走らないため、
このビルドが唯一のゲートです。`zensical serve` は何も検証せず 1 ロケールずつしか配信しないので、
言語セレクタを確認できるのは `build-pages.sh serve=yes` だけです。`pages.yml` のビルドジョブは
そのスクリプト（オプションはすべて明示）なので、CI と手元のゲートがずれません。

`nav` は**両方**のロケール設定にあります。ページの追加・改名は両方を編集してください。

## 出荷物のビルド

```bash
./scripts/build.sh                    # このホスト向けライブラリ（C face 込み）を bin/ へ
./scripts/build.sh platform=web       # wasm と TypeScript 層を packages/ssplayer-wgpu/ へ
./scripts/build-preview.sh            # bin/ssplayer-preview.html。自己完結、file:// で開ける
./scripts/generate-licenses.sh        # コミット対象の THIRD_PARTY_NOTICES.md
```

`build.sh` は**ライブラリ**を、`deploy-examples.sh` は**データ**を作ります。フィクスチャが動いたら
後者、ライブラリの構成が変わったら前者、と変更先が 1 つに決まります。`platform=web` には収集先が
ありません。`packages/ssplayer-wgpu/{wasm,dist}` をその場で埋めます（この face ではパッケージ
ディレクトリ自体が成果物だからです）。`build-preview.sh` は独自に `wasm-pack --target no-modules`
でビルドします。npm パッケージとは別のターゲットなのは、ES モジュールが `file://` から読めないためです。

publish の前に npm tarball を確認してください: `cd packages/ssplayer-wgpu && pnpm pack`。
`prepack` が、宣言している `dist/` や `wasm/` を欠いたパッケージを拒否します。
