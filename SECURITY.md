# Security Policy

## Supported Versions

SSPlayerForWgpu is pre-release: nothing is published yet, and fixes are issued against `main`.
Once there are releases, this becomes "the latest one".

| Version | Supported |
|---|---|
| `main` | ✅ |
| Anything else | ❌ |

## Reporting a Vulnerability

**Please do not open a public GitHub Issue for a security problem.** Report it privately through the
official inquiry form:

👉 [**CRI Middleware Inquiry Form (English)**](https://www.webtech.co.jp/contact/en.html)

The English form has no file upload. If you need to send a reproduction, use the
[Japanese Help Center form](https://www.webtech.co.jp/help/ja/spritestudio7/inquiries/ssplayer_tool/),
which takes a zip of up to 8 MB.

Please include:

- What the impact is — crash, out-of-bounds read, arbitrary code execution, information disclosure.
- Steps to reproduce, ideally with a minimal `.ssab` and the smallest amount of Rust, C or JavaScript
  needed.
- The crate or package version or commit, which face (Rust, Bevy, C, WebGPU), your OS and GPU, and the
  wgpu backend.
- Whether the input that triggers it comes from a trusted source in your project or from somewhere
  untrusted (downloaded content, user-supplied files).

We will confirm receipt, tell you whether we can reproduce it, and let you know when a fix ships.
Please give us a chance to release that fix before disclosing publicly.

### Which repository to report to

Playback is driven by the runtime shared with the other SpriteStudio players, which lives in
SpriteStudio-SDK. If the same problem reproduces on a different SpriteStudio player, it is most likely
in that shared core. Report it here all the same and say so; we will route it.

## Scope Notes

The player links the runtime's Rust API directly and builds its packs on it.

**Neither the player nor the runtime verifies a pack.** `SSPack` hands the bytes to the runtime, which
checks the schema version and parses the rest without validating. A malformed or deliberately crafted
`.ssab` can therefore cause out-of-bounds reads rather than a clean error — in native code, or within the
WebAssembly module's memory on the web.

**Packs can come from files, bytes or the network.** `SSPlayer::load` reads a file, `load_ssab` takes
bytes, and on the web `SsPlayerWeb.load` fetches a URL. Instance parts load further packs by the names
stored inside the pack. Loading `.ssab` from an untrusted source is not a supported configuration: ship
packs with your application, or serve them over a channel you control.

Reports about the known gap above are welcome, but it is already known — a report that demonstrates
impact beyond it is more useful.

---

# セキュリティポリシー

## サポート対象バージョン

SSPlayerForWgpu はリリース前です。まだ何も公開されておらず、修正は `main` に対して提供されます。リリースが始まった時点で「最新リリース」に切り替わります。

| バージョン | サポート |
|---|---|
| `main` | ✅ |
| それ以外 | ❌ |

## 脆弱性の報告

**セキュリティに関する問題は、公開の GitHub Issue に投稿しないでください。** 再現データ（8 MB までの zip ファイル）の添付も可能なヘルプセンターより、非公開でご報告ください。

👉 [**ヘルプセンター（お問い合わせフォーム）**](https://www.webtech.co.jp/help/ja/spritestudio7/inquiries/ssplayer_tool/)

ご報告には以下を含めてください。

- 影響の内容（クラッシュ、境界外読み取り、任意コード実行、情報漏洩など）
- 再現手順。可能であれば最小限の `.ssab` と、必要最小限の Rust / C / JavaScript のコード
- クレートまたはパッケージのバージョンかコミット、利用方法（Rust、Bevy、C、WebGPU）、OS と GPU、wgpu のバックエンド
- 引き金となる入力が、プロジェクト内の信頼できるデータか、信頼できない経路（ダウンロードコンテンツ、ユーザーが用意したファイル）由来か

受領の確認、再現可否、修正版の公開時期についてご連絡します。修正を公開するまでの間、公表をお待ちいただけますようお願いします。

### 報告先リポジトリの判断

再生処理は、他の SpriteStudio Player と共通のランタイムが担っており、SpriteStudio-SDK で開発されています。他の SpriteStudio Player でも同じ問題が再現する場合、共通コア側の問題である可能性が高いため、その旨を添えて本リポジトリへご報告ください。こちらで振り分けます。

## 既知の注意点

本プレイヤーはランタイムの Rust API を直接リンクし、その上にパックを構築します。

**プレイヤーもランタイムもパックを検証しません。** `SSPack` はバイト列をランタイムへ渡し、ランタイムはスキーマのバージョンを確認したうえで、残りを検証せずに解析します。そのため、破損した、あるいは意図的に細工された `.ssab` は、エラーとして弾かれずに境界外読み取り（ネイティブコード内、または Web では WebAssembly モジュールのメモリ内）を引き起こす可能性があります。

**パックはファイル、バイト列、ネットワークのいずれからも読み込めます。** `SSPlayer::load` はファイルを読み、`load_ssab` はバイト列を受け取り、Web の `SsPlayerWeb.load` は URL を取得します。インスタンスパーツは、パック内に記録された名前で別のパックを読み込みます。信頼できない入手元の `.ssab` の読み込みはサポート対象外です。パックはアプリケーションに同梱するか、自社管理の経路で配信してください。

上記の既知の課題に関するご報告も歓迎しますが、すでに把握している内容です。これを超える影響を示すご報告のほうが有用です。
