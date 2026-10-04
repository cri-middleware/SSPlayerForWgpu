# Releasing SpriteStudio Player for wgpu

Maintainers only. Contributors do not run any of this — see [CONTRIBUTING.md](./CONTRIBUTING.md).

## Cutting a release

Anyone with the Write role on this repository can cut a release: every step is a branch, a pull request, a tag, a workflow run or a Release, so nothing depends on whose machine runs it. You need `git`, `gh` (run `gh auth login` once), Node.js and a Rust toolchain. The commands are bash; on Windows, run them in Git Bash.

This releases the player — the `ssplayer-wgpu` crate, its C face and the npm package — as zips on a GitHub Release. The Bevy adapter `ssplayer-bevy` is versioned on its own and is not part of it.

A release is a tag on `main`, pushed first and built second. The version is written in one place, `[workspace.package] version` in `Cargo.toml`, which `ssplayer-wgpu` and `ssplayer-wgpu-ffi` inherit; `packages/ssplayer-wgpu/package.json` carries the copy npm needs. `scripts/release-version.sh` prints the version and refuses while the copy disagrees, the tag is `v` followed by it, and the release build refuses a tag that disagrees. `main` is the only long-lived branch: a release is a short-lived branch merged into it.

```bash
VERSION=7.0.0-beta.1                 # what is being released
BRANCH=release/$VERSION
```

### 1. Branch

```bash
git fetch origin
git switch -c "$BRANCH" origin/main
```

### 2. SDK, version and changelog

In one commit. Point the `SpriteStudio-SDK` submodule at the SDK release this one ships: `ssruntime` and `ssab` are path dependencies into it, so its commit is the SDK version. Then set the version in `Cargo.toml` and `package.json`, and date the version's heading in `CHANGELOG.md` as `## [X.Y.Z] - YYYY-MM-DD`, adding the heading if there is none.

```bash
SDK_TAG=v7.0.0-beta.1                            # the SDK release this ships
git -C SpriteStudio-SDK fetch --tags origin && git -C SpriteStudio-SDK checkout "$SDK_TAG"
# set version in Cargo.toml's [workspace.package] and in packages/ssplayer-wgpu/package.json,
# date the heading in CHANGELOG.md, then:
cargo update --workspace                         # the workspace's own entries in Cargo.lock
scripts/release-version.sh                       # must print $VERSION
git add SpriteStudio-SDK Cargo.toml Cargo.lock packages/ssplayer-wgpu/package.json CHANGELOG.md
git commit -m "chore(release): v$VERSION"
```

### 3. Pull request and QA build

Open the pull request into `main`; CI builds and tests it. Then dispatch the release workflow from the branch. With `upload_release` left false it builds the library for Linux (x86_64 and aarch64), Windows and macOS (universal) and the npm package, assembles and checks the zips, keeps them on the run (`ssplayer-wgpu-release-dist`) and creates no Release. Fix anything on the branch and repeat until it is green.

```bash
git push -u origin "$BRANCH"
gh pr create -B main --title "Release v$VERSION" --body ""
gh workflow run release.yml --ref "$BRANCH"
gh run watch                                      # the run takes a few seconds to appear
gh run download <run-id> -n ssplayer-wgpu-release-dist -D qa   # <run-id>: the one gh run watch shows
```

### 4. Merge and tag

Merge with a merge commit, never a squash or a rebase, so that `main` keeps the commits the QA build built rather than copies of them. Tag `main` only when its tree is the one the QA build built. It differs only when `main` gained commits after the branch was cut; merge `main` into the branch and go back to step 3.

```bash
gh pr merge "$BRANCH" --merge
git switch main && git pull --ff-only
git diff --stat HEAD^2 HEAD                       # must print nothing
TAG=v$(scripts/release-version.sh) && echo "$TAG"
git tag -a "$TAG" -m "$TAG" && git push origin "$TAG"
```

### 5. Build the Release

Dispatch from the tag. The run builds again and creates a **draft** Release carrying the five zips, `SHA256SUMS` and generated notes; nothing is public yet. `upload_release=true` from anything but a `v*` tag fails the run rather than skipping the Release quietly.

```bash
gh workflow run release.yml --ref "$TAG" -f upload_release=true
gh run watch
```

### 6. Publish

Open the draft on the [Releases](https://github.com/cri-middleware/SSPlayerForWgpu/releases) page and check the attachments: `ssplayer-wgpu-linux-x86_64.zip`, `ssplayer-wgpu-linux-aarch64.zip`, `ssplayer-wgpu-windows-x86_64.zip`, `ssplayer-wgpu-macos-universal.zip`, `ssplayer-wgpu-web-wasm32.zip` and `SHA256SUMS`. Edit the notes, choose **Set as a pre-release** or **Set as the latest release** (a tag with a `-` can be either), and **Publish release**. Publishing also starts `pages.yml`, which deploys the [documentation site](https://cri-middleware.github.io/SSPlayerForWgpu/) with the rustdoc and the WebGPU demo. Watch the run through, then open the site: a failed run leaves the Release published and the site as it was. Once the cause is fixed, re-run it; a re-run builds the same tag.

```bash
gh run watch                                      # pages.yml; the run takes a few seconds to appear
```

### 7. Delete the branch

```bash
git push origin --delete "$BRANCH"
```

If the button on its pull request already deleted the branch, this only reports that it does not exist.

## Undoing a release

Until the Release is published, it can be undone. A run that failed for a transient reason is dispatched again from the same tag once its draft is deleted. A fault that needs new commits takes the tag down too: delete the draft, run `git push origin ":refs/tags/$TAG" && git tag -d "$TAG"`, fix it on the branch — restore it from its pull request if it was deleted — and go back to step 3.

**Once published, the tag and its assets are permanent** — hosts have downloaded the zips by that tag — so a fix is a new patch version.

## Building the release locally

`scripts/build-release.sh` is the release package, and `release.yml`'s package job runs exactly this script. It builds nothing — the four native platforms need three operating systems — so it assembles a run's artifacts instead: one zip per platform, the npm package as its own, and `SHA256SUMS` over them. Its checks are the ones nothing else does: every platform is present, the C header is the same in all of them, and each zip holds the library it is named for.

```bash
gh run download <run-id> -D artifacts             # a release.yml run, whole
scripts/build-release.sh                          # -> release-dist/
```

<br>

---

# SpriteStudio Player for wgpu のリリース手順

維持者向けです。コントリビューターがこの手順を実行することはありません（[CONTRIBUTING.md](./CONTRIBUTING.md) を参照してください）。

## リリースの手順

このリポジトリの Write ロールを持つ人なら誰でもリリースできます。どの手順もブランチ・プルリクエスト・タグ・ワークフローの実行・Release のいずれかなので、誰の手元で実行しても結果は変わりません。必要なのは `git`、`gh`（最初に一度 `gh auth login`）、Node.js、Rust ツールチェーンです。コマンドは bash で書いています。Windows では Git Bash で実行してください。

ここでリリースするのは Player 本体 — `ssplayer-wgpu` クレート、その C フェイス、npm パッケージ — で、GitHub Release の zip として出します。Bevy アダプタ `ssplayer-bevy` は独立した版を持ち、このリリースには含まれません。

リリースは `main` 上のタグです。先にタグを push し、ビルドはその後です。バージョンが書かれているのは `Cargo.toml` の `[workspace.package] version` の 1 か所だけで、`ssplayer-wgpu` と `ssplayer-wgpu-ffi` がそれを継承します。`packages/ssplayer-wgpu/package.json` は npm に必要な写しを持ちます。`scripts/release-version.sh` がバージョンを出力し、写しが食い違っている間は出力を拒否します。タグはその頭に `v` を付けたもので、食い違うタグはリリースビルドが拒否します。長く残るブランチは `main` だけで、リリースは短命のブランチを `main` へマージして行います。

```bash
VERSION=7.0.0-beta.1                 # リリースするバージョン
BRANCH=release/$VERSION
```

### 1. ブランチ

```bash
git fetch origin
git switch -c "$BRANCH" origin/main
```

### 2. SDK、バージョン、CHANGELOG

1 コミットで行います。`SpriteStudio-SDK` submodule を、同梱する SDK のリリースに合わせます。`ssruntime` と `ssab` はこの submodule へのパス依存なので、submodule のコミットがそのまま SDK のバージョンです。続けて `Cargo.toml` と `package.json` のバージョンを設定し、`CHANGELOG.md` のそのバージョンの見出しに日付を入れて `## [X.Y.Z] - YYYY-MM-DD` とします。見出しが無ければ足します。

```bash
SDK_TAG=v7.0.0-beta.1                            # 同梱する SDK のリリース
git -C SpriteStudio-SDK fetch --tags origin && git -C SpriteStudio-SDK checkout "$SDK_TAG"
# Cargo.toml の [workspace.package] と packages/ssplayer-wgpu/package.json のバージョンを設定し、
# CHANGELOG.md の見出しに日付を入れてから:
cargo update --workspace                         # Cargo.lock のワークスペース自身のエントリ
scripts/release-version.sh                       # $VERSION が出力されること
git add SpriteStudio-SDK Cargo.toml Cargo.lock packages/ssplayer-wgpu/package.json CHANGELOG.md
git commit -m "chore(release): v$VERSION"
```

### 3. プルリクエストと QA ビルド

`main` へのプルリクエストを作ります。CI がビルドとテストを行います。続けてブランチからリリースワークフローを実行します。`upload_release` を false のままにすると、Linux（x86_64 と aarch64）・Windows・macOS（ユニバーサル）向けのライブラリと npm パッケージをビルドし、zip を組み立てて検査し、run に残します（`ssplayer-wgpu-release-dist`）。Release は作りません。問題があればブランチで直し、緑になるまで繰り返します。

```bash
git push -u origin "$BRANCH"
gh pr create -B main --title "Release v$VERSION" --body ""
gh workflow run release.yml --ref "$BRANCH"
gh run watch                                      # run が一覧に出るまで数秒かかる
gh run download <run-id> -n ssplayer-wgpu-release-dist -D qa   # <run-id>: gh run watch に出るもの
```

### 4. マージとタグ

マージはマージコミットで行い、squash や rebase は使いません。QA ビルドしたコミットの写しではなく、そのものを `main` に残すためです。タグは、`main` のツリーが QA ビルドしたものと同じときにだけ打ちます。違うのは、ブランチを切った後に `main` へコミットが入った場合だけで、そのときは `main` をブランチへマージして手順 3 に戻ります。

```bash
gh pr merge "$BRANCH" --merge
git switch main && git pull --ff-only
git diff --stat HEAD^2 HEAD                       # 何も出力されないこと
TAG=v$(scripts/release-version.sh) && echo "$TAG"
git tag -a "$TAG" -m "$TAG" && git push origin "$TAG"
```

### 5. Release のビルド

タグからワークフローを実行します。もう一度ビルドし、5 つの zip と `SHA256SUMS`、自動生成のリリースノート付きで**下書き**の Release を作ります。まだ何も公開されていません。`v*` タグ以外からの `upload_release=true` は、Release を黙って飛ばさずに実行を失敗させます。

```bash
gh workflow run release.yml --ref "$TAG" -f upload_release=true
gh run watch
```

### 6. 公開

[Releases](https://github.com/cri-middleware/SSPlayerForWgpu/releases) ページで下書きを開き、添付を確かめます。`ssplayer-wgpu-linux-x86_64.zip`、`ssplayer-wgpu-linux-aarch64.zip`、`ssplayer-wgpu-windows-x86_64.zip`、`ssplayer-wgpu-macos-universal.zip`、`ssplayer-wgpu-web-wasm32.zip`、`SHA256SUMS` の 6 つです。リリースノートを編集し、**Set as a pre-release** か **Set as the latest release** を選んで（`-` を含むタグはどちらも選べます）、**Publish release** を押します。公開すると `pages.yml` が動き、[ドキュメントサイト](https://cri-middleware.github.io/SSPlayerForWgpu/) を rustdoc と WebGPU デモ付きでデプロイします。run を最後まで見届けてから、サイトを開いて確かめます。run が失敗しても Release は公開されたままで、サイトは元のまま残ります。原因を直したら run を再実行します。再実行は同じタグをビルドします。

```bash
gh run watch                                      # pages.yml。run が一覧に出るまで数秒かかる
```

### 7. ブランチを消す

```bash
git push origin --delete "$BRANCH"
```

プルリクエストのページのボタンで消してあれば、「ブランチが無い」と出るだけです。

## リリースの取り消し

Release を公開するまでは取り消せます。一時的な理由で失敗した run は、下書きを消してから同じタグでもう一度実行します。新しいコミットが要る不具合ならタグも取り下げます。下書きを消し、`git push origin ":refs/tags/$TAG" && git tag -d "$TAG"` を実行し、ブランチで直して（消していたらプルリクエストのページから復元して）手順 3 に戻ります。

**公開した後のタグとアセットは恒久的です。** ホストはそのタグで zip を取得しているので、修正は新しいパッチバージョンで出します。

## リリースのローカルビルド

`scripts/build-release.sh` がリリースのパッケージそのもので、`release.yml` の package ジョブはこのスクリプトを実行しているだけです。何もビルドしません（ネイティブの 4 プラットフォームには 3 つの OS が要ります）。代わりに run の成果物を組み立てます。プラットフォームごとの zip、npm パッケージの zip、それらに対する `SHA256SUMS` です。ほかのどこもしない検査を受け持ちます。全プラットフォームが揃っていること、C ヘッダがどれも同じであること、各 zip が名前どおりのライブラリを持っていることです。

```bash
gh run download <run-id> -D artifacts             # release.yml の run の成果物一式
scripts/build-release.sh                          # -> release-dist/
```
