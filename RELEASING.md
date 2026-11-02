# Releasing SpriteStudio Player for wgpu

Maintainers only. Contributors do not run any of this — see [CONTRIBUTING.md](./CONTRIBUTING.md).

## Cutting a release

Anyone with the Write role on this repository can cut a release: it is one run of the **Release** workflow on `main`, so nothing depends on whose machine starts it. You need `git` and `gh` 2.87 or later (run `gh auth login` once), for steps 2 and 3, the toolchain [CONTRIBUTING.md](./CONTRIBUTING.md) sets up, and for step 7, `npm` signed in to an account that can publish to the `@spritestudio` scope. The repository needs GitHub Pages enabled with GitHub Actions as its source, and a `*-v*` tag rule on its `github-pages` environment, or publishing a Release deploys nothing; both are settings a repository admin makes. The package on npm needs a trusted publisher for this repository's `release.yml`, allowed to stage. npm sets one up only for a package it already has, so the package's first version goes out by hand (step 7). The macOS library is signed with the organisation's Developer ID and notarized, so the repository needs the six organisation secrets the sibling repositories sign with: `APPLE_DEV_ID_APP_CERT`, `APPLE_DEV_ID_APP_CERT_PASS`, `APPLE_DEV_ID_APP_NAME`, `ASC_API_KEY_PASS`, `ASC_API_KEY_ID` and `ASC_API_ISSUER_ID`. Without them a release stops before it builds. The commands are bash: in zsh, run `setopt interactive_comments` first, so that the `#` comments are not taken as arguments; on Windows, use Git Bash.

This releases the player — the `ssplayer-wgpu` crate, its C face and the npm package `@spritestudio/ssplayer-wgpu` — as zips, the package's tarball and the [preview shell](./PREVIEW.md) on a GitHub Release, and that tarball on npm. The packages here are versioned independently, and a release names the version it sets, as `<package>@<version>`; the Bevy adapter `ssplayer-bevy` has a version of its own and no release route yet. The player's version is `[workspace.package] version` in `Cargo.toml`, which `ssplayer-wgpu` and `ssplayer-wgpu-ffi` inherit, with the copy npm needs in `packages/ssplayer-wgpu/package.json`. `scripts/set-versions.mjs` writes all of them, and turns the `## [Unreleased]` notes in `libs/ssplayer-wgpu/CHANGELOG.md` into that version's section; `scripts/release-version.sh` prints the version and refuses while the copies disagree. What each version gained is written in that changelog and nowhere else: a Release carries the zips, the tarball, the preview shell and a link to it. A contributor's pull request carries no version and no changelog entry — both are written here. There is no release branch: what `main` holds is what goes out, so `main` holds only what can ship.

Every block runs as written in a new shell: it works out what it needs from git and GitHub. One value is set by hand, written as a placeholder to fill in: `<version>`, the version from step 1, as in `7.0.0-beta.2`, in steps 4 and 5 and their retry under [Undoing a release](#undoing-a-release). The block that assembles a run's artifacts takes the run as `<run-id>` the same way, and a block that acts on a staged version takes it as `<stage-id>`. Left as it is, a placeholder stops its line with a syntax error.

An agent can run every block. A person sorts the scan's summary in step 4 and decides two steps: the merge in step 2, which puts the notes on `main`, and step 8, which publishes the Release for good. Step 7, which asks for a 2FA code, is a person's too.

### 1. Decide the version

Look at what the player gained since its previous tag. The SDK it ships is the commit of the `SpriteStudio-SDK` submodule — `ssruntime` and `ssab` are path dependencies into it — and the player names it by that commit's version. The first release names the version the player already has, `7.0.0-beta.1`.

```bash
git fetch origin --tags
prev=$(git -c versionsort.suffix=- tag --list 'ssplayer-wgpu-v*' --sort=-v:refname | head -n 1)
echo "last released: ${prev:-never}"
git log --oneline ${prev:+"$prev.."}origin/main
```

Steps 4 and 5 take the decision in place of their `<version>`.

### 2. Write the changelog

Write what the player gained under `## [Unreleased]` in `libs/ssplayer-wgpu/CHANGELOG.md`, in Keep a Changelog's sections (`### Added`, `### Changed`, `### Fixed`, `### Removed`), regenerate `THIRD_PARTY_NOTICES.md` for the dependencies the release ships, and merge both into `main` with a pull request. The release turns `## [Unreleased]` into `## [<version>] - <date>`, and stops before any platform builds when there is nothing there.

On the same branch, bring the dependencies up to date first, in a commit of its own, since the notices are made from them: `cargo update` moves `Cargo.lock`, which the libraries and the wasm are built from, and `pnpm update` the npm package's tools, to the newest versions their ranges allow. Cargo reads `ssruntime` and `ssab` from the submodule, so the block initializes it first. A new major version changes code, and goes in an ordinary pull request of its own rather than here.

```bash
git switch -c release-notes origin/main
git submodule update --init SpriteStudio-SDK &&
  cargo update && pnpm --dir packages/ssplayer-wgpu update
git commit -m "chore(deps): update within their ranges" -- Cargo.lock packages/ssplayer-wgpu/pnpm-lock.yaml packages/ssplayer-wgpu/package.json   # nothing to commit when nothing moved
# write under ## [Unreleased] in libs/ssplayer-wgpu/CHANGELOG.md
./scripts/generate-licenses.sh                    # -> THIRD_PARTY_NOTICES.md
git add libs/ssplayer-wgpu/CHANGELOG.md THIRD_PARTY_NOTICES.md
git commit -m "docs: changelog for the next release"
git push -u origin release-notes
gh pr create -B main --fill
```

**A person runs the merge**, which puts the notes on `main`: choose **Create a merge commit** on the pull request's page, or run

```bash
gh pr merge release-notes --merge --delete-branch
```

### 3. Build the site

`pages.yml` builds the site only once a Release is published, from its tag, and a re-run builds that same tag: a fault in the docs, the rustdoc or the demo found then can only be fixed in `main` and deployed from there. So build the site now, from the `main` the release will be cut from. `scripts/build-pages.sh` builds what `pages.yml` deploys, in the same order. A failure is fixed by a pull request into `main` before step 4.

```bash
git switch main && git pull --ff-only && git submodule update --init &&
  scripts/build-pages.sh                            # -> site/, the site pages.yml would deploy
```

### 4. Look at it first

A dry run builds every platform with the version applied and checks the result, and commits, tags, releases and stages nothing. It signs and notarizes the macOS library as the release will, so a secret that has expired fails here first; without the secrets it leaves the library ad-hoc signed and says so in a warning. Its zips, tarball and preview shell stay on the run (`ssplayer-wgpu-release-dist`); the block downloads them into `qa/<run ID>`, and fails when the run did.

```bash
VERSION=<version>                                 # the version from step 1
RUN=$(gh workflow run release.yml --ref main -f versions="ssplayer-wgpu@${VERSION:?}" -f dry_run=true | sed 's|.*/||')
gh run watch "${RUN:?}" --exit-status
gh run download "${RUN:?}" -n ssplayer-wgpu-release-dist -D "qa/$RUN"
gh run view "${RUN:?}" --exit-status > /dev/null  # the block fails when the run did
```

Scan `main` too, for code that carries someone else's copyright or license. `provenance.yml` runs ScanCode, which finds copyright and license notices, and keeps its report and a summary on the run (`provenance`); the block downloads them into `qa/<run ID>` and prints the summary. **A person sorts what the summary lists.** A notice of the earlier SpriteStudio Players and SDK marks our own code. A notice of a library this repository bundles must be listed in `THIRD_PARTY_NOTICES.md`. Anything else is rewritten, unless its license, read at its source, is compatible with BSD-3-Clause and the code keeps the notice that license asks for. Make any fix by a pull request into `main` before step 5, and run the block again.

```bash
git switch main && git pull --ff-only &&
  RUN=$(gh workflow run provenance.yml --ref main | sed 's|.*/||') &&
  gh run watch "${RUN:?}" --exit-status &&
  gh run download "${RUN:?}" -n provenance -D "qa/$RUN" &&
  cat "qa/$RUN/summary.md"
```

### 5. Run the release

```bash
VERSION=<version>                                 # the version from step 1
RUN=$(gh workflow run release.yml --ref main -f versions="ssplayer-wgpu@${VERSION:?}" | sed 's|.*/||')
gh run watch "${RUN:?}" --exit-status
```

The run first checks the changelog, then builds the library for Windows and macOS (universal), the npm package and the preview shell, all with the version applied, signs and notarizes the macOS library, then assembles and checks the zips, the package's tarball and the shell, and only then commits the version and the dated changelog to `main`, tags `ssplayer-wgpu-v<version>`, creates a **draft** Release carrying the platform zips, the tarball, the preview shell, `SHA256SUMS` and a link to the changelog, and stages that tarball on npm, where it waits for step 7. A failure before the commit leaves `main` untouched, and so does a `main` that moved during the run, which rejects the push. Then see what it made:

```bash
git fetch origin --tags
git log --oneline -1 origin/main                  # chore(release): ssplayer-wgpu@<version>
gh release list --json tagName,isDraft -q '.[] | select(.isDraft) | .tagName'   # its draft
```

npm cannot stage a package it does not have yet, so the run that releases the package's first version — the first release — adds `-f publish_npm=false`, and step 7 publishes it by hand.

### 6. Check the draft

Nothing on the draft needs writing, and `scripts/release-channel.sh` has already marked it: a pre-release when a full release is already tagged, ready to become Latest otherwise. Check the attachments: `ssplayer-wgpu-windows-x86_64.zip`, `ssplayer-wgpu-macos-universal.zip`, `spritestudio-ssplayer-wgpu-<version>.tgz`, `ssplayer-preview.html` and `SHA256SUMS`. The macOS zip holds `lib/libssplayer_wgpu.dylib` beside the static library, and the run has already checked its signature and its ticket with Apple; on a Mac, `spctl --assess --type open --context context:primary-signature --verbose=2` on it answers `source=Notarized Developer ID`, which is what Gatekeeper decides on a user's Mac. Look at them before anything on npm is approved: until then, a fault is undone by deleting the draft and rejecting what was staged (see [Undoing a release](#undoing-a-release)), and after it, only a new version fixes it. The block lists the attachments, then what the tarball holds — what npm is given.

```bash
tag=$(gh release list --json tagName,isDraft -q '[.[] | select(.isDraft) | .tagName | select(startswith("ssplayer-wgpu-v"))][0] // empty')
gh release view "${tag:?}" --json assets -q '.assets[].name'
dir=$(mktemp -d)
gh release download "${tag:?}" -p '*.tgz' -D "$dir" && tar -tzf "$dir"/*.tgz
```

### 7. Approve on npm

**A person runs this step**: approving, like publishing by hand, asks for a 2FA code. A version the run staged stays out of sight on npm until a maintainer approves it, and approving applies the dist-tag it was staged under. Approve before the Release is published, so that by the time the Release announces the version, `npm install` and the CDN snippets in the READMEs and `examples/cdn` find it. Approve on npmjs.com, or here:

```bash
npm login                                         # once
npm stage list @spritestudio/ssplayer-wgpu        # each staged version, with its stage id
npm stage approve <stage-id>
```

A version the run did not stage — every run with `publish_npm=false`, the first release among them, and any after a failed **Stage on npm** — is published by hand instead, from the very tarball on the draft. `scripts/npm-dist-tag.sh` picks the dist-tag: `latest` for a full release, and for a prerelease while the package's `latest` is not yet a full release; the prerelease's word (`beta`, `rc`) after that.

```bash
tag=$(gh release list --json tagName,isDraft -q '[.[] | select(.isDraft) | .tagName | select(startswith("ssplayer-wgpu-v"))][0] // empty')
version=${tag#ssplayer-wgpu-v}
dist=$(scripts/npm-dist-tag.sh @spritestudio/ssplayer-wgpu "$version")
dir=$(mktemp -d)
gh release download "${tag:?}" -p '*.tgz' -D "$dir" &&
  npm publish "$dir"/*.tgz --tag "${dist:?}" --access public &&
  npm view @spritestudio/ssplayer-wgpu dist-tags
```

### 8. Publish

Then **a person publishes the Release**: a published Release is permanent. Publishing also starts `pages.yml`, which deploys the [documentation site](https://cri-middleware.github.io/SSPlayerForWgpu/) with the rustdoc and the WebGPU demo, unless the Release is a pre-release, which leaves the site as it is.

```bash
tag=$(gh release list --json tagName,isDraft -q '[.[] | select(.isDraft) | .tagName | select(startswith("ssplayer-wgpu-v"))][0] // empty')
gh release edit "${tag:?}" --draft=false
```

Then watch the deploy. The block finds the Release published last itself, and waits up to a minute for its run, which appears a few seconds after it.

```bash
tag=$(gh release list --json tagName,publishedAt,isDraft -q '[.[] | select(.isDraft | not)] | sort_by(.publishedAt) | last | .tagName')
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
  RUN=$(gh run list -w pages.yml -b "${tag:?}" -L 1 --json databaseId -q '.[0].databaseId')
  [ -n "$RUN" ] && break
  sleep 5
done
gh run watch "${RUN:?}" --exit-status
```

Then open the site. A failed run leaves the Release published and the site as it was. When the cause was a setting outside the repository, re-run it with `gh run rerun <run-id>`, the run the block watched; a fix in the repository goes into `main` by pull request and deploys with `gh workflow run pages.yml --ref main`.

## Patching an earlier line

Fixes go to the latest release (SECURITY.md). When one has to reach an earlier version of a package on request, as `ssplayer-wgpu` 7.0.2 after 7.1.0, `main` carries the newer line, so it is released from a `maint/` branch cut from that version's tag. `release.yml` runs from a `maint/` branch as it does from `main`, and pushes the version commit back to that branch. `set-versions.mjs` compares with the branch's own version, so the patch is no step backwards, and a package whose Release already exists is skipped, so only the patched one is tagged and drafted.

1. **Branch.** The first block cuts a `maint/` branch from the line's latest full release. Commit or cherry-pick the fix onto it, with notes under the package's `## [Unreleased]` in its CHANGELOG.md.
2. **Release.** The second block runs the release from the branch, as step 5 does from `main`. The tarball is staged on npm under `latest-X.Y`, which `scripts/npm-dist-tag.sh` picks since npm's `latest` is a newer full release, so a bare `npm install` keeps getting the newest; approve it as step 7 does.
3. **Publish.** `scripts/release-channel.sh` calls the Release `old`, and `pages.yml` deploys nothing for it. Publish it with the last block, not from the Releases page, whose default would make it Latest.
4. **Back to main.** Cherry-pick the fix onto `main` if it still needs it; the last block deletes the branch.

```bash
git fetch origin --tags
LINE=<x.y>                                        # the line, as major.minor
BASE=$(git tag --list "ssplayer-wgpu-v${LINE:?}.*" --sort=-v:refname | grep -E -- '-v[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1)
git switch -c "maint/ssplayer-wgpu-v${LINE:?}" "${BASE:?}" && git push -u origin "maint/ssplayer-wgpu-v$LINE"
```

```bash
LINE=<x.y>
VERSION=<version>                                 # the patch
RUN=$(gh workflow run release.yml --ref "maint/ssplayer-wgpu-v${LINE:?}" -f versions="ssplayer-wgpu@${VERSION:?}" | sed 's|.*/||')
gh run watch "${RUN:?}" --exit-status
```

```bash
git fetch origin --tags
LINE=<x.y>
TAG=$(git describe --exact-match --tags --match "ssplayer-wgpu-v${LINE:?}.*" "origin/maint/ssplayer-wgpu-v${LINE:?}")
gh release edit "${TAG:?}" --draft=false --latest=false &&
  git push origin --delete "maint/ssplayer-wgpu-v$LINE"
```

## Undoing a release

**A version npm has made public is permanent**, whether approved or published by hand: it cannot be withdrawn, only superseded. So is a published Release, whose tag and assets hosts have downloaded. A fault found after either is fixed by a new patch version. A staged version is not public yet, and `npm stage reject <stage-id>` removes it (a person, with 2FA). Before that:

*   A run that failed before its commit reached `main` is run again with the same version, once the cause is fixed in `main`. The **Re-run** button would replay the old commit, and its push would be rejected.

    ```bash
    VERSION=<version>                                 # the version from step 1
    RUN=$(gh workflow run release.yml --ref main -f versions="ssplayer-wgpu@${VERSION:?}" | sed 's|.*/||')
    gh run watch "${RUN:?}" --exit-status
    ```

*   A run that failed after its commit reached `main` is run again with `versions` empty, before anything else is merged into `main`: it releases the version `main` now carries, reusing a tag it had pushed, drafting the Release if it is missing and staging the tarball if npm does not have that version, staged or published.

    ```bash
    RUN=$(gh workflow run release.yml --ref main | sed 's|.*/||')
    gh run watch "${RUN:?}" --exit-status
    ```

*   A fault in the draft takes the tag down too: delete the draft and its tag, have a person reject what the run staged — a retry skips a version npm has staged, so the faulty tarball would stay the one waiting for approval — merge the fix into `main`, correcting the version's changelog section if it needs it, and run again with `versions` empty, which tags the same version on the fixed `main` and stages it again. The block finds the tag among the drafts, so it never touches a published Release.

    ```bash
    tag=$(gh release list --json tagName,isDraft -q '[.[] | select(.isDraft) | .tagName | select(startswith("ssplayer-wgpu-v"))][0] // empty')
    gh release delete "${tag:?}" --cleanup-tag --yes  # deletes the tag too, here and on GitHub
    ```

    ```bash
    npm stage list @spritestudio/ssplayer-wgpu        # the staged version, with its stage id
    npm stage reject <stage-id>                       # a person, with 2FA
    ```

## Building the release locally

`scripts/build-release.sh` is the release package, and `release.yml`'s package job runs exactly this script. It builds nothing — the two native platforms need an operating system each — so it assembles a run's artifacts instead: one zip per native platform, the npm package as the tarball npm is given (`npm pack`, so it needs `npm`), the preview shell as the web job built it, and `SHA256SUMS` over them. Its checks are the ones nothing else does: every platform is present, the C header is the same in all of them, every platform links the same SDK release — a release, not `local` — the npm package carries the version being released, the macOS shared library is universal, named `@rpath/libssplayer_wgpu.dylib` and signed with a Developer ID, the preview shell is stamped with that version, the SDK release the other artifacts link and the SDK commit the checkout pins, carries its license notices and has its pack hole empty, each zip holds the library it is named for, and the tarball holds what the package advertises. It reads that signature out of the Mach-O, since `codesign` does not run on Linux, so it proves who signed rather than that the signature holds; `scripts/release-macos.sh`, in the macOS job, is what verifies it and the notarization. `<tag>` is the release being rebuilt, `ssplayer-wgpu-v<version>`, and `<run-id>` the `release.yml` run that built it.

```bash
TAG=<tag>
RUN=<run-id>
git checkout "${TAG:?}" &&
  gh run download "${RUN:?}" -D artifacts &&    # a release.yml run, whole
  scripts/build-release.sh                        # -> release-dist/
```

<br>

---

# SpriteStudio Player for wgpu のリリース手順

維持者向けです。コントリビューターがこの手順を実行することはありません（[CONTRIBUTING.md](./CONTRIBUTING.md) を参照してください）。

## リリースの手順

このリポジトリの Write ロールを持つ人なら誰でもリリースできます。リリースは `main` での **Release** ワークフローの実行 1 回なので、誰の手元から始めても結果は変わりません。必要なのは `git` と `gh` 2.87 以降（最初に一度 `gh auth login`）、手順 2・3 のための、[CONTRIBUTING.md](./CONTRIBUTING.md) が用意するツールチェーン、それに手順 7 のための `npm` で、`@spritestudio` scope に publish できるアカウントでログインしておきます。リポジトリ側では、GitHub Pages を有効にしてソースを GitHub Actions にし、`github-pages` environment に `*-v*` のタグのルールを足しておく必要があります。どちらかが欠けると、Release を公開しても何もデプロイされません。どちらもリポジトリの admin が行う設定です。npm 側では、パッケージに、このリポジトリの `release.yml` を trusted publisher として登録し、stage を許可しておきます。npm はすでにあるパッケージにしか trusted publisher を設定できないので、パッケージの最初のバージョンは手作業で出します（手順 7）。macOS のライブラリは組織の Developer ID で署名して公証するので、リポジトリには、姉妹リポジトリが署名に使う組織の secret 6 つ（`APPLE_DEV_ID_APP_CERT`・`APPLE_DEV_ID_APP_CERT_PASS`・`APPLE_DEV_ID_APP_NAME`・`ASC_API_KEY_PASS`・`ASC_API_KEY_ID`・`ASC_API_ISSUER_ID`）が要ります。無ければリリースはビルドの前に止まります。コマンドは bash です。zsh では、`#` のコメントが引数として渡されないよう、先に `setopt interactive_comments` を実行します。Windows では Git Bash を使います。

ここでリリースするのは Player 本体 — `ssplayer-wgpu` クレート、その C フェイス、npm パッケージ `@spritestudio/ssplayer-wgpu` — で、GitHub Release の zip とパッケージの tarball、[プレビューのシェル](./PREVIEW.md)として出し、その tarball を npm にも出します。このリポジトリのパッケージは独立にバージョンを持ち、リリースは設定するバージョンを `<package>@<version>` で指定します。Bevy アダプタ `ssplayer-bevy` は独自のバージョンを持ち、リリースの経路はまだありません。Player のバージョンは `Cargo.toml` の `[workspace.package] version` で、`ssplayer-wgpu` と `ssplayer-wgpu-ffi` がそれを継承し、`packages/ssplayer-wgpu/package.json` が npm に必要な写しを持ちます。`scripts/set-versions.mjs` がそのすべてを書き、`libs/ssplayer-wgpu/CHANGELOG.md` の `## [Unreleased]` のノートをそのバージョンの節に書き換えます。`scripts/release-version.sh` はバージョンを出力します（写しが食い違っている間は出力を拒否します）。各バージョンで何が変わったかは、この changelog にだけ書きます。Release が持つのは zip と tarball、プレビューのシェル、その changelog へのリンクです。コントリビューターのプルリクエストはバージョンも changelog の項目も持ちません。どちらもここで書きます。release ブランチはありません。`main` にあるものがそのまま出るので、`main` には出せるものだけを置きます。

どのブロックも、新しいシェルで書かれたとおりに実行できます。必要なものは git と GitHub から求めます。手で設定する値は 1 つで、埋めるプレースホルダとして書いてあります。手順 4・5 と「リリースの取り消し」にあるその再実行の `<version>` で、手順 1 で決めたバージョン（`7.0.0-beta.2` など）を入れます。run の成果物を組み立てるブロックも、その run を `<run-id>` で受け取り、stage したバージョンに対して操作するブロックは、それを `<stage-id>` で受け取ります。埋めずに実行すると、その行は構文エラーで止まります。

どのブロックもエージェントに実行させられます。手順 4 の検査の一覧は人が仕分けます。2 つの手順は人が判断します。ノートを `main` に入れる手順 2 のマージと、取り消せない公開を行う手順 8 です。2FA のコードを求められる手順 7 も人が行います。

### 1. バージョンを決める

前回のタグから Player が何を得たかを見ます。同梱する SDK は `SpriteStudio-SDK` submodule のコミットです（`ssruntime` と `ssab` はこの submodule へのパス依存）。Player はそのコミットのバージョンで SDK を名乗ります。初回のリリースでは、Player がいま持っているバージョン `7.0.0-beta.1` をそのまま指定します。

```bash
git fetch origin --tags
prev=$(git -c versionsort.suffix=- tag --list 'ssplayer-wgpu-v*' --sort=-v:refname | head -n 1)
echo "last released: ${prev:-never}"
git log --oneline ${prev:+"$prev.."}origin/main
```

手順 4・5 は、決めたバージョンを `<version>` の位置に受け取ります。

### 2. changelog を書く

Player が得たものを `libs/ssplayer-wgpu/CHANGELOG.md` の `## [Unreleased]` の下に、Keep a Changelog の節（`### Added`・`### Changed`・`### Fixed`・`### Removed`）で書き、リリースが同梱する依存に合わせて `THIRD_PARTY_NOTICES.md` を作り直し、両方をプルリクエストで `main` にマージします。リリースは `## [Unreleased]` を `## [<version>] - <date>` に書き換えます。`## [Unreleased]` が空なら、どのプラットフォームもビルドせずに止まります。

同じブランチで、先に依存を最新にして単独のコミットにします。表記はこの依存から作るためです。`cargo update` はライブラリと wasm のビルドに使う `Cargo.lock` を、`pnpm update` は npm パッケージのツールを、範囲が許すいちばん新しい版に上げます。cargo は `ssruntime` と `ssab` を submodule から読むので、ブロックは先に submodule を初期化します。メジャーバージョンを上げるのはコードの変更なので、ここではなく通常のプルリクエストで行います。

```bash
git switch -c release-notes origin/main
git submodule update --init SpriteStudio-SDK &&
  cargo update && pnpm --dir packages/ssplayer-wgpu update
git commit -m "chore(deps): update within their ranges" -- Cargo.lock packages/ssplayer-wgpu/pnpm-lock.yaml packages/ssplayer-wgpu/package.json   # 何も動かなければコミットするものは無い
# libs/ssplayer-wgpu/CHANGELOG.md の ## [Unreleased] の下に書く
./scripts/generate-licenses.sh                    # -> THIRD_PARTY_NOTICES.md
git add libs/ssplayer-wgpu/CHANGELOG.md THIRD_PARTY_NOTICES.md
git commit -m "docs: changelog for the next release"
git push -u origin release-notes
gh pr create -B main --fill
```

**マージは人が行います。** ノートが `main` に入るためです。プルリクエストのページで **Create a merge commit** を選ぶか、次を実行します。

```bash
gh pr merge release-notes --merge --delete-branch
```

### 3. サイトをビルドする

`pages.yml` がサイトをビルドするのは Release を公開したときだけで、そのタグから作ります。再実行しても同じタグをビルドするだけなので、そこで docs・rustdoc・デモの不具合が見つかると、`main` で直してそこからデプロイするしかありません。そこで、リリースを切る `main` から、先にサイトをビルドしておきます。`scripts/build-pages.sh` は、`pages.yml` がデプロイするものを同じ順にビルドします。失敗したら、手順 4 の前にプルリクエストで `main` を直します。

```bash
git switch main && git pull --ff-only && git submodule update --init &&
  scripts/build-pages.sh                            # -> site/。pages.yml がデプロイするサイト
```

### 4. 先に確かめる

dry run は、バージョンを適用して全プラットフォームをビルドし、結果を検査します。コミットもタグも Release も作らず、npm への stage もしません。macOS のライブラリはリリースと同じように署名・公証するので、期限の切れた secret はここで先に失敗します。secret が無ければ、ライブラリを ad-hoc 署名のまま残し、警告でそう伝えます。zip と tarball、プレビューのシェルは run に残ります（`ssplayer-wgpu-release-dist`）。ブロックはそれを `qa/<run ID>` にダウンロードし、run が失敗していればブロックも失敗します。

```bash
VERSION=<version>                                 # 手順 1 で決めたバージョン
RUN=$(gh workflow run release.yml --ref main -f versions="ssplayer-wgpu@${VERSION:?}" -f dry_run=true | sed 's|.*/||')
gh run watch "${RUN:?}" --exit-status
gh run download "${RUN:?}" -n ssplayer-wgpu-release-dist -D "qa/$RUN"
gh run view "${RUN:?}" --exit-status > /dev/null  # run が失敗していればブロックも失敗する
```

`main` で、他者の著作権やライセンスの付いたコードが無いかも検査します。`provenance.yml` は ScanCode で著作権表示とライセンスの記述を見つけ、報告と一覧を run に残します（`provenance`）。ブロックはそれを `qa/<run ID>` にダウンロードし、一覧を表示します。**一覧は人が仕分けます。** 以前の SpriteStudio の Player・SDK の表示が付いたコードは、自社のコードです。このリポジトリが同梱するライブラリの表示は、`THIRD_PARTY_NOTICES.md` に載っていなければなりません。それ以外は書き直します。残せるのは、出所で確かめたライセンスが BSD-3-Clause と両立し、そのライセンスが求める表示をコードに残すときだけです。直すときは、手順 5 の前にプルリクエストで `main` を直し、このブロックをもう一度実行します。

```bash
git switch main && git pull --ff-only &&
  RUN=$(gh workflow run provenance.yml --ref main | sed 's|.*/||') &&
  gh run watch "${RUN:?}" --exit-status &&
  gh run download "${RUN:?}" -n provenance -D "qa/$RUN" &&
  cat "qa/$RUN/summary.md"
```

### 5. リリースを実行する

```bash
VERSION=<version>                                 # 手順 1 で決めたバージョン
RUN=$(gh workflow run release.yml --ref main -f versions="ssplayer-wgpu@${VERSION:?}" | sed 's|.*/||')
gh run watch "${RUN:?}" --exit-status
```

run はまず changelog を検査し、続けて Windows・macOS（ユニバーサル）向けのライブラリと npm パッケージ、プレビューのシェルを、どれもバージョンを適用してビルドし、macOS のライブラリを署名・公証し、zip とパッケージの tarball、シェルを組み立てて検査します。そのあとで初めてバージョンと日付を入れた changelog を `main` へコミットし、`ssplayer-wgpu-v<version>` のタグを打ち、プラットフォームごとの zip と tarball、プレビューのシェル、`SHA256SUMS`、changelog へのリンク付きで**下書き**の Release を作り、その tarball を npm に stage します。stage したものは手順 7 を待ちます。コミットより前に失敗すれば `main` は変わりません。run の途中で `main` が進んでいた場合も、push が拒否されるので同じです。終わったら、できたものを確かめます。

```bash
git fetch origin --tags
git log --oneline -1 origin/main                  # chore(release): ssplayer-wgpu@<バージョン>
gh release list --json tagName,isDraft -q '.[] | select(.isDraft) | .tagName'   # できた下書き
```

npm はまだ無いパッケージを stage できないので、パッケージの最初のバージョンを出す run（初回のリリースがそうです）には `-f publish_npm=false` を足し、手順 7 で手作業で publish します。

### 6. 下書きを確かめる

下書きに書き足すものはありません。印は `scripts/release-channel.sh` が付けてあります。正式版のタグがすでにあれば pre-release、なければ Latest になります。添付を確かめます。`ssplayer-wgpu-windows-x86_64.zip`、`ssplayer-wgpu-macos-universal.zip`、`spritestudio-ssplayer-wgpu-<version>.tgz`、`ssplayer-preview.html`、`SHA256SUMS` です。macOS の zip は静的ライブラリの横に `lib/libssplayer_wgpu.dylib` を持ち、その署名と公証のチケットは run が Apple に確かめ済みです。Mac では、それに対する `spctl --assess --type open --context context:primary-signature --verbose=2` が `source=Notarized Developer ID` と答えます。利用者の Mac で Gatekeeper が下す判断がこれです。npm で承認する前に確かめます。それまでなら、下書きを消して stage したものを取り下げれば取り消せますが（「リリースの取り消し」を参照）、承認した後は新しいバージョンでしか直せません。ブロックは添付を一覧し、続けて tarball の中身（npm に渡すもの）を一覧します。

```bash
tag=$(gh release list --json tagName,isDraft -q '[.[] | select(.isDraft) | .tagName | select(startswith("ssplayer-wgpu-v"))][0] // empty')
gh release view "${tag:?}" --json assets -q '.assets[].name'
dir=$(mktemp -d)
gh release download "${tag:?}" -p '*.tgz' -D "$dir" && tar -tzf "$dir"/*.tgz
```

### 7. npm で承認する

**この手順は人が行います。** 承認も、手作業の publish も、2FA のコードを求められるためです。run が stage したバージョンは、maintainer が承認するまで npm 上で見えないままで、承認すると stage したときの dist-tag が付きます。Release を公開するより先に承認し、Release がバージョンを告知する時点で、`npm install` と README・`examples/cdn` の CDN スニペットがそのバージョンを見つけられるようにします。承認は npmjs.com でも、次のコマンドでもできます。

```bash
npm login                                         # 最初に一度
npm stage list @spritestudio/ssplayer-wgpu        # stage されたバージョンと、その stage id
npm stage approve <stage-id>
```

run が stage しなかったバージョンは、下書きに添付された tarball そのものを手作業で publish します。`publish_npm=false` で実行した run（初回のリリースを含む）と、**Stage on npm** が失敗した後がこれにあたります。dist-tag は `scripts/npm-dist-tag.sh` が決めます。正式版は `latest` です。プレリリースも、パッケージの `latest` がまだ正式版でなければ `latest`、正式版になった後はその語（`beta`・`rc`）です。

```bash
tag=$(gh release list --json tagName,isDraft -q '[.[] | select(.isDraft) | .tagName | select(startswith("ssplayer-wgpu-v"))][0] // empty')
version=${tag#ssplayer-wgpu-v}
dist=$(scripts/npm-dist-tag.sh @spritestudio/ssplayer-wgpu "$version")
dir=$(mktemp -d)
gh release download "${tag:?}" -p '*.tgz' -D "$dir" &&
  npm publish "$dir"/*.tgz --tag "${dist:?}" --access public &&
  npm view @spritestudio/ssplayer-wgpu dist-tags
```

### 8. 公開

続けて Release を公開します。**Release の公開は人が行います。** 公開した Release は取り消せないためです。公開すると `pages.yml` が動き、[ドキュメントサイト](https://cri-middleware.github.io/SSPlayerForWgpu/) を rustdoc と WebGPU デモ付きでデプロイします。pre-release の公開ではサイトはそのままです。

```bash
tag=$(gh release list --json tagName,isDraft -q '[.[] | select(.isDraft) | .tagName | select(startswith("ssplayer-wgpu-v"))][0] // empty')
gh release edit "${tag:?}" --draft=false
```

続けてデプロイを見届けます。ブロックは最後に公開した Release を自分で探し、その run が一覧に出るまで最大 1 分待ちます。run は Release の公開から数秒遅れて出るためです。

```bash
tag=$(gh release list --json tagName,publishedAt,isDraft -q '[.[] | select(.isDraft | not)] | sort_by(.publishedAt) | last | .tagName')
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
  RUN=$(gh run list -w pages.yml -b "${tag:?}" -L 1 --json databaseId -q '.[0].databaseId')
  [ -n "$RUN" ] && break
  sleep 5
done
gh run watch "${RUN:?}" --exit-status
```

終わったら、サイトを開いて確かめます。run が失敗しても Release は公開されたままで、サイトは元のまま残ります。原因がリポジトリの外の設定にあったなら、ブロックが見届けた run を `gh run rerun <run-id>` で再実行します。リポジトリ内の修正は、プルリクエストで `main` に入れてから `gh workflow run pages.yml --ref main` でデプロイします。

## 古い系統へのパッチ

修正は最新のリリースにだけ出します（SECURITY.md）。依頼を受けてパッケージの古い版を直すとき（7.1.0 の後の `ssplayer-wgpu` 7.0.2 など）は、`main` が新しい系統を持っているので、その版のタグから切った `maint/` ブランチから出します。`release.yml` は `maint/` ブランチからも `main` と同じように動き、版のコミットはそのブランチへ push します。`set-versions.mjs` はブランチ自身の版と比べるので、パッチは版の後退になりません。Release がすでにあるパッケージは飛ばすので、タグと下書きができるのは直したパッケージだけです。

1. **ブランチ。** 最初のブロックで、その系統の最新の正式版のタグから `maint/` ブランチを切ります。修正をコミットするか cherry-pick し、パッケージの CHANGELOG.md の `## [Unreleased]` にノートを書きます。
2. **リリース。** 2 つめのブロックで、手順 5 と同じようにブランチからリリースを実行します。npm には `latest-X.Y` で stage されます。npm の `latest` がより新しい正式版なので、`scripts/npm-dist-tag.sh` がそれを選び、素の `npm install` では最新の版が入り続けます。承認は手順 7 のとおりです。
3. **公開。** `scripts/release-channel.sh` がこの Release を `old` と判定し、`pages.yml` は何もデプロイしません。公開は Releases ページではなく最後のブロックで行います。ページの既定のままだと Latest になるためです。
4. **main へは戻さない。** `main` にも要る修正なら cherry-pick します。最後のブロックがブランチを消します。

```bash
git fetch origin --tags
LINE=<x.y>                                        # 系統（major.minor）
BASE=$(git tag --list "ssplayer-wgpu-v${LINE:?}.*" --sort=-v:refname | grep -E -- '-v[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1)
git switch -c "maint/ssplayer-wgpu-v${LINE:?}" "${BASE:?}" && git push -u origin "maint/ssplayer-wgpu-v$LINE"
```

```bash
LINE=<x.y>
VERSION=<version>                                 # パッチの版
RUN=$(gh workflow run release.yml --ref "maint/ssplayer-wgpu-v${LINE:?}" -f versions="ssplayer-wgpu@${VERSION:?}" | sed 's|.*/||')
gh run watch "${RUN:?}" --exit-status
```

```bash
git fetch origin --tags
LINE=<x.y>
TAG=$(git describe --exact-match --tags --match "ssplayer-wgpu-v${LINE:?}.*" "origin/maint/ssplayer-wgpu-v${LINE:?}")
gh release edit "${TAG:?}" --draft=false --latest=false &&
  git push origin --delete "maint/ssplayer-wgpu-v$LINE"
```

## リリースの取り消し

**npm で公開されたバージョンは恒久的です。** 承認したものも手作業で publish したものも、取り下げられず、新しいバージョンで置き換えるしかありません。公開した Release も同じで、ホストはそのタグでアセットを取得しています。どちらかの後に見つかった不具合は、新しいパッチバージョンで直します。stage しただけのバージョンはまだ公開されておらず、`npm stage reject <stage-id>` で取り下げられます（人が 2FA で行います）。それより前なら次のとおりです。

*   コミットが `main` に届く前に失敗した run は、原因を `main` で直してから、同じバージョンで実行し直します。**Re-run** ボタンは古いコミットを再生するので、その push は拒否されます。

    ```bash
    VERSION=<version>                                 # 手順 1 で決めたバージョン
    RUN=$(gh workflow run release.yml --ref main -f versions="ssplayer-wgpu@${VERSION:?}" | sed 's|.*/||')
    gh run watch "${RUN:?}" --exit-status
    ```

*   コミットが `main` に届いた後に失敗した run は、ほかの変更が `main` にマージされる前に、`versions` を空にして実行し直します。`main` が今持っているバージョンを出し、push 済みのタグはそのまま使い、Release の下書きが無ければ作り、npm にそのバージョンが無ければ（stage 済みでも publish 済みでもなければ）tarball を stage します。

    ```bash
    RUN=$(gh workflow run release.yml --ref main | sed 's|.*/||')
    gh run watch "${RUN:?}" --exit-status
    ```

*   下書きに不具合があればタグも取り下げます。下書きとそのタグを消し、run が stage したものを人が取り下げ（やり直しの run は npm が stage 済みのバージョンを飛ばすので、取り下げないと不具合のある tarball が承認待ちのまま残ります）、修正を `main` へマージして（必要ならそのバージョンの changelog の節も直して）、`versions` を空にして実行し直します。直した `main` に同じバージョンのタグが打たれ、もう一度 stage されます。ブロックは下書きの中からタグを探すので、公開済みの Release に触れることはありません。

    ```bash
    tag=$(gh release list --json tagName,isDraft -q '[.[] | select(.isDraft) | .tagName | select(startswith("ssplayer-wgpu-v"))][0] // empty')
    gh release delete "${tag:?}" --cleanup-tag --yes  # タグも手元と GitHub の両方から消える
    ```

    ```bash
    npm stage list @spritestudio/ssplayer-wgpu        # stage されたバージョンと、その stage id
    npm stage reject <stage-id>                       # 人が 2FA で行う
    ```

## リリースのローカルビルド

`scripts/build-release.sh` がリリースのパッケージそのもので、`release.yml` の package ジョブはこのスクリプトを実行しているだけです。何もビルドしません（ネイティブの 2 プラットフォームには、それぞれの OS が要ります）。代わりに run の成果物を組み立てます。ネイティブのプラットフォームごとの zip、npm に渡す tarball にした npm パッケージ（`npm pack` なので `npm` が要ります）、web ジョブがビルドしたままのプレビューのシェル、それらに対する `SHA256SUMS` です。ほかのどこもしない検査を受け持ちます。全プラットフォームが揃っていること、C ヘッダがどれも同じであること、全プラットフォームが同じ SDK のリリース（`local` ではなくリリース）をリンクしていること、npm パッケージがリリースするバージョンを持っていること、macOS の共有ライブラリがユニバーサルで、`@rpath/libssplayer_wgpu.dylib` という名前を持ち、Developer ID で署名されていること、プレビューのシェルのスタンプがそのバージョン、ほかの成果物がリンクする SDK のリリース、チェックアウトが指す SDK のコミットを示し、ライセンスの表記を持ち、パックの穴が空のままであること、各 zip が名前どおりのライブラリを持っていること、tarball がパッケージの告知するものを持っていることです。その署名は Mach-O から読み取ります（Linux では `codesign` が動かないため）。証明できるのは誰が署名したかで、署名が有効であることではありません。署名と公証を検証するのは、macOS ジョブで動く `scripts/release-macos.sh` です。`<tag>` は組み立て直すリリースのタグ（`ssplayer-wgpu-v<version>`）、`<run-id>` はそれをビルドした `release.yml` の run の ID です。

```bash
TAG=<tag>
RUN=<run-id>
git checkout "${TAG:?}" &&
  gh run download "${RUN:?}" -D artifacts &&    # release.yml の run の成果物一式
  scripts/build-release.sh                        # -> release-dist/
```
