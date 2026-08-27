# ライセンス

`ssplayer-wgpu` / `ssplayer-bevy` / `@spritestudio/ssplayer-wgpu` は
**BSD 3-Clause License** で公開されています。

全文はリポジトリの
[LICENSE.md](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/LICENSE.md) にあり（参考訳: [LICENSE.ja.md](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/LICENSE.ja.md)）、
リリース zip と npm パッケージにも同梱されています。

Copyright © CRI Middleware Co., Ltd.

## サードパーティコンポーネント

Player は SpriteStudio SDK（`ssruntime` / `ssab`）と、wgpu とその依存を中心とした Rust クレート群を
リンクしており、それぞれが独自のライセンスを持ちます。適用されるのは各クレートに同梱された条項です。
アプリケーションのビルドでは、他の Rust 依存の告知と同じ方法で収集してください（`cargo about` や
`cargo license` がこれを行います）。

[SpriteStudio](https://www.webtech.co.jp/spritestudio/) 自体と、それでオーサリングした
アニメーションデータには、このライセンスではなくそれぞれの規約が適用されます。
