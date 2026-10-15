# SpriteStudio Player for wgpu — C / C++

Plays SpriteStudio animations from a C or C++ host. The player owns its own graphics device, built on the window you name.

This zip holds the header in `include/`, the static library and its `VERSION` in `lib/` (in the macOS zip, the shared library `libssplayer_wgpu.dylib` as well, signed and notarized), the licences, and the documentation in `docs/`.

`docs/` is the documentation of the release this zip came from, so it describes the API the library in `lib/` actually provides. The online documentation is built from the latest release and can be ahead of this copy.

## Where to look

| To find out | Read |
| --- | --- |
| How to link the library and draw the first frame | [docs/start/cpp.md](docs/start/cpp.md) |
| Which C function does what (the header's comments carry the per-function detail) | [docs/reference/c.md](docs/reference/c.md) |
| What `ss_scene_t` and `ss_player_t` each hold, and how long each lives | [docs/guide/concepts.md](docs/guide/concepts.md) |
| How to produce the `.ssab` files the player loads | [docs/start/assets.md](docs/start/assets.md) |
| How to control playback and react to events | [docs/guide/playback.md](docs/guide/playback.md), [docs/guide/events.md](docs/guide/events.md) |
| What is not supported | [docs/limitations.md](docs/limitations.md) |
| How a program written for SS6Player for CPP maps onto this one | [docs/migration/from-ss6-cpp.md](docs/migration/from-ss6-cpp.md) |

The other pages in `docs/` cover placement, loading, part overrides, texture replacement, motion blending, masks, device loss and performance, and the Rust, Bevy and Web faces of the same player. Links between the pages work offline, except the ones to the Rust API pages and the live demo, which are only online: <https://cri-middleware.github.io/SSPlayerForWgpu/> (Japanese: <https://cri-middleware.github.io/SSPlayerForWgpu/ja/>).

The licence is `LICENSE.md`; `THIRD_PARTY_NOTICES.md` holds the notices of the Rust crates compiled into the library.
