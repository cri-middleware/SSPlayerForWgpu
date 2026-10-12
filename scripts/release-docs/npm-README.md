# @spritestudio/ssplayer-wgpu — documentation

This folder is the documentation of the version of `@spritestudio/ssplayer-wgpu` it was installed with, as Markdown, so it describes the API this package's `dist/` and `wasm/` actually provide. The online documentation is built from the latest release and can be ahead of this copy.

## Where to look

| To find out | Read |
| --- | --- |
| How to install the package and draw the first character | [start/web.md](start/web.md) |
| What each function, class and method does (`dist/index.d.ts` carries the types) | [reference/js.md](reference/js.md) |
| The `<ss-player-webgpu>` element's attributes, properties and events | [reference/web-component.md](reference/web-component.md) |
| What `SsContextWeb`, `SsSurfaceWeb` and `SsPlayerWeb` each hold, and how long each lives | [guide/concepts.md](guide/concepts.md) |
| How to produce the `.ssab` files the player loads, and where they have to be | [start/assets.md](start/assets.md), [guide/loading.md](guide/loading.md) |
| How to control playback and react to events | [guide/playback.md](guide/playback.md), [guide/events.md](guide/events.md) |
| What to do when the GPU device is lost | [guide/device-loss.md](guide/device-loss.md) |
| What is not supported | [limitations.md](limitations.md) |

The other pages cover placement, part overrides, texture replacement, motion blending, masks and performance, and the Rust, Bevy and C faces of the same player. Links between the pages work offline, except the ones to the Rust API pages and the live demo, which are only online: <https://cri-middleware.github.io/SSPlayerForWgpu/> (Japanese: <https://cri-middleware.github.io/SSPlayerForWgpu/ja/>).
