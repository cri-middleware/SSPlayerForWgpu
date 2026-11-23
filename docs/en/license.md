# License

`ssplayer-wgpu`, `ssplayer-bevy` and `@spritestudio/ssplayer-wgpu` are released under the
**BSD 3-Clause License**.

The full text is
[LICENSE.md](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/LICENSE.md) in the
repository (a reference translation is
[LICENSE.ja.md](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/LICENSE.ja.md)), and a copy ships inside every release zip and npm
package.

Copyright © CRI Middleware Co., Ltd.

## Third-party components

The player links the SpriteStudio SDK (`ssruntime`, `ssab`) and a set of Rust crates — wgpu and its
dependencies chief among them — each under its own licence. Their notices are collected in
[THIRD_PARTY_NOTICES.md](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/THIRD_PARTY_NOTICES.md)
in the repository, and a copy ships inside every release zip and npm package. A build of your
application that links the crates itself should collect them the way it collects any other Rust
dependency's notices (`cargo about` and `cargo license` both do this).

[SpriteStudio](https://www.webtech.co.jp/eng/spritestudio/) itself, and the animation data you
author with it, are covered by their own terms — not by this licence.
