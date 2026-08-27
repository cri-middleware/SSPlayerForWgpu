# License

`ssplayer-wgpu`, `ssplayer-bevy` and `@spritestudio/ssplayer-wgpu` are released under the
**BSD 3-Clause License**.

The full text is
[LICENSE.md](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/LICENSE.md) in the
repository, and a copy ships inside every release zip and npm package.

Copyright © CRI Middleware Co., Ltd.

## Third-party components

The player links the SpriteStudio SDK (`ssruntime`, `ssab`) and a set of Rust crates — wgpu and its
dependencies chief among them — each under its own licence. Their terms are the ones that ship with
those crates; a build of your application should collect them the way it collects any other Rust
dependency's notices (`cargo about` and `cargo license` both do this).

[OPTPiX SpriteStudio](https://www.webtech.co.jp/spritestudio/) itself, and the animation data you
author with it, are covered by their own terms — not by this licence.
