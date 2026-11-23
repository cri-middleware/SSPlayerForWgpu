[**English**](./README.md) | [**日本語**](./README.ja.md)

# SpriteStudio Player for wgpu

**Professional 2D animation for Rust — one wgpu renderer, from a native window to the browser to a C++ host.**

▶️ **[Try the live WebGPU demo in your browser](https://cri-middleware.github.io/SSPlayerForWgpu/demo/)** — SpriteStudio animations rendered with WebGPU, with nothing to install.

A high-performance player for **[SpriteStudio](https://www.webtech.co.jp/spritestudio/)**
animations (`.ssab`), built on [wgpu](https://wgpu.rs/). Animate the sprite art you already have
(PNG / WebP) in a dedicated editor, then play the finished animation in your application — no video
files, no re-authoring, no engine lock-in.

## ✨ Why use SpriteStudio with wgpu?

- **Bring your characters — and whole scenes — to life, with no animation code.** Breathing and blinking, swaying hair and clothes, flashy slash and magic effects, even cutscenes and UI: bone rigs, mesh deformation and particle effects are authored visually in SpriteStudio and exported as one `.ssab`. The player plays it, and timeline events and per-part overrides connect it to your game logic.
- **Blend between motions.** Layer animations on tracks and crossfade between them, so a walk eases into a run instead of snapping — see [Motion blending](./docs/en/guide/motion-blending.md).
- **It borrows your GPU; it never owns it.** A Rust host owns the wgpu `Device` / `Queue` / `Surface` and lends them per frame (the C face builds its own from the window you hand it), so the player rides inside any wgpu application — your own, egui, Bevy — and a scene of many characters shares one upload of each atlas.
- **Use it from Rust, Bevy, C/C++ or the browser.** In **Bevy**, `ssplayer-bevy` adds three components that draw a character as part of Bevy's own 2D rendering. From C, C++ or Objective-C it is a static library and one header: hand it your `HWND`, `NSView`, `CAMetalLayer`, X11 `Window` or `wl_surface` and call two functions per frame. In the browser it is a WebAssembly build. Wherever it runs, one wgpu code path draws through the platform's native graphics API — DirectX 12 on Windows, Metal on macOS, Vulkan on Linux, WebGPU in the browser.
- **Zero-parse loading, SIMD playback.** `.ssab` is a FlatBuffers binary read in place, and poses come from the SDK's `#![no_std]` Rust core with SIMD — the same core every official SpriteStudio player uses.

## 📚 Documentation

- [**Documentation site**](https://cri-middleware.github.io/SSPlayerForWgpu/) — start here: one path per
  host (Rust / Bevy / C・C++ / WebGPU), a shared guide and the API reference for every face. Its source is [`docs/en/`](./docs/en/index.md) in the repository
- [**日本語ドキュメント**](https://cri-middleware.github.io/SSPlayerForWgpu/ja/) — the same site
  ([`docs/ja/`](./docs/ja/index.md) in the repository)
- [**Limitations & Scope**](./docs/en/limitations.md) — what this player draws today, and what it leaves to the host
- [**Migrating from SS6Player for CPP**](./docs/en/migration/from-ss6-cpp.md)
- [**ROADMAP**](./ROADMAP.md) — what is planned, and what was deliberately declined
- [**For AI assistants**](https://cri-middleware.github.io/SSPlayerForWgpu/llms.txt) — these docs as Markdown: `llms.txt` indexes the pages and `llms-full.txt` holds all of them in one file; the npm package carries the same pages in its `docs/` folder and as an Agent Skill in `skills/` ([installing the skill](https://cri-middleware.github.io/SpriteStudio-Docs/ai/docs/))
- [**SpriteStudio Docs (portal)**](https://cri-middleware.github.io/SpriteStudio-Docs/) — the SDK and every
  official player in one place, with a [plugin](https://cri-middleware.github.io/SpriteStudio-Docs/ai/) that tells your coding assistant about SpriteStudio

## 📦 Installation

**Using it in your application?** Read on — pick your host, then [Usage](#-usage).
Everything after the first character (events, per-part overrides, motion blending,
device-loss recovery) is the same path further in, in the [documentation](./docs/en/index.md).
**Working on the player itself?** [CONTRIBUTING.md](./CONTRIBUTING.md), the [Build Guide](./docs/en/development/build.md) — which also covers using a changed player in a host and checking it on real hosts — and [How the Player Is Put Together](./docs/en/development/architecture.md) for the layers and the reasons behind them.

### Rust

Add the crate from crates.io:

```bash
cargo add ssplayer-wgpu
```

To use changes that are not released yet, make it a git dependency on this repository instead. Cargo
checks out the submodule for you, and no other tooling is needed:

```toml
[dependencies]
ssplayer-wgpu = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
```

Inside a **Bevy** app, depend on the adapter instead — it pulls the player in and adds the three
components (`SsPlayer` / `SsTransform` / `SsPlayback`) that draw a character as part of Bevy's own 2D rendering:

```bash
cargo add ssplayer-bevy
```

or, for changes that are not released yet, as a git dependency:

```toml
[dependencies]
ssplayer-bevy = { git = "https://github.com/cri-middleware/SSPlayerForWgpu" }
```

It tracks one Bevy line at a time — named in [its README](./libs/ssplayer-bevy) — because Bevy
and the player have to resolve to a single `wgpu`. A mismatch is a compile error, not a runtime
surprise.

### C and C++

The C face is a cdylib / staticlib plus a generated header. Each
[`ssplayer-wgpu` Release](https://github.com/cri-middleware/SSPlayerForWgpu/releases?q=ssplayer-wgpu&expanded=true)
carries the static library as one zip per platform under **Assets** —
`ssplayer-wgpu-windows-x86_64.zip` (MSVC, `/MD` runtime) and `ssplayer-wgpu-macos-universal.zip`
(arm64 + x86_64, with the shared library beside the static one, signed and notarized) — each
holding `include/`, `lib/`, `LICENSE.md`, `LICENSE.ja.md`, `THIRD_PARTY_NOTICES.md`, and the
documentation as Markdown (`README.md`, `docs/`), with `SHA256SUMS` beside them.
[Get started with C / C++](./docs/en/start/cpp.md) covers linking it, building for the static
runtime (`/MT`), and building from a checkout — on Linux, or for the Windows DLL — and the C++
viewers under [`examples/`](#-samples) are the shortest working reference to copy from.

### Browser (WebGPU)

The browser face is an npm package, [`@spritestudio/ssplayer-wgpu`](./packages/ssplayer-wgpu) — the
wasm plus a small TypeScript layer over it. Install it from npm:

```bash
npm i @spritestudio/ssplayer-wgpu
```

…and import what you need:

```js
import { sharedContext } from '@spritestudio/ssplayer-wgpu';           // the page's device
import { defineSsPlayerWebGPUElement } from '@spritestudio/ssplayer-wgpu/element';  // <ss-player-webgpu>
```

`SsContextWeb` / `SsSurfaceWeb` / `SsPlayerWeb` are the full API — one device per page, one surface
per canvas, any number of characters in one pass — and the custom element is the one-character
convenience wrapper over them, in its own entry point so a bundle that does not use it does not carry
it. [Get started on the web](./docs/en/start/web.md) is the JavaScript-facing guide.

Building it from this repository is covered in the [Build Guide](./docs/en/development/build.md).

### Where that `.ssab` comes from

Animations are authored in SpriteStudio — **7.6 or later** — and converted ahead of time; this player
converts nothing at runtime and bundles no converter. Use the desktop
[SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI), or `ssconverter-cli` — the zip for
your OS from a [SpriteStudio-SDK Release](https://github.com/cri-middleware/SpriteStudio-SDK/releases)'s
**Assets** — which converts a project into a folder of `.ssab` files in one command:

```bash
ssconverter-cli overall.sspj -o assets/overall
```

[Converting with a standalone tool](https://cri-middleware.github.io/SpriteStudio-Docs/overview/convert-with-a-standalone-tool/)
covers picking one and putting it in a build pipeline. No `.sspj` of your own yet? SpriteStudio's
[official sample data](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/) works as-is (the page is
Japanese only; the downloads are language-neutral) — [Ringo](https://www.webtech.co.jp/help/ja/spritestudio7/download/sample/#ringo)
is a good character — and feeds either tool above. The samples below, which need a checkout, run on
the SDK's own test animations.

## ⚡ Usage

In Rust: one `SSRenderer` per device, one `SSScene` per render target, and any number of `SSPlayer`s
drawn into it in one pass:

```rust
let mut renderer = SSRenderer::new(&device);
let mut scene = SSScene::new(&device, &SceneConfig { color_format, stencil_format, mask: MaskBackend::Stencil });
scene.set_viewport(&queue, width as f32, height as f32, /* y_down */ false);

let mut player = SSPlayer::new();
player.load("assets/overall/Basic.ssab")?;   // any converted .ssab: pack, atlases and sibling packs, in one call
player.set_animation("anime_1")?;
player.fit_into(width as f32, height as f32, &Placement::default());   // the whole canvas in view
player.play();

scene.update(&mut renderer, &device, &queue, delta_secs, &mut [&mut player]);
scene.draw(&renderer, &mut rpass);           // into a pass the host opened
```

The full API — the three scopes, loading, transport, per-part overrides, events, motion blending,
device-loss recovery — is in the [Rust API reference](./docs/en/reference/rust.md) and the guides it
links to. The C and JavaScript faces are
the same shape under different names, and each sample below documents its own.

**Read next:** [Get started](./docs/en/start/rust.md) — or [Bevy](./docs/en/start/bevy.md), [C / C++](./docs/en/start/cpp.md), [the browser](./docs/en/start/web.md) · [Playback](./docs/en/guide/playback.md) · [Limitations & Scope](./docs/en/limitations.md)

## 🎬 Samples

One host per environment — two for Rust, the smallest one beside the development viewer, and two for Bevy, where the engine owns the window — each self-contained. Every
one has a README that doubles as the usage guide for that environment: how the window is created, how
the device is lent, and how assets get in.

| Sample | Environment | Run it |
|---|---|---|
| [`examples/hello`](./examples/hello) | Rust, winit — the smallest host, one character | `cargo run -p ssplayer-wgpu-hello` |
| [`examples/viewer`](./examples/viewer) | Rust, winit — the development viewer, a live cast | `cargo run -p ssplayer-wgpu-viewer` |
| [`examples/web`](./examples/web) | WebGPU in a browser | `./scripts/run-web.sh` |
| [`examples/cdn`](./examples/cdn) | WebGPU in a browser, the npm package straight from jsDelivr — no build step | `cp assets/Ringo_emb/Ringo.ssab examples/cdn/ && (cd examples/cdn && python3 -m http.server --bind 127.0.0.1)` |
| [`examples/viewer-metal-cpp`](./examples/viewer-metal-cpp) | C++ on Metal, macOS | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| [`examples/viewer-dx12-cpp`](./examples/viewer-dx12-cpp) | C++ on DX12, Windows | `cd examples\viewer-dx12-cpp; cmake -B build; cmake --build build --config Release; .\build\Release\ssviewer-dx12.exe` |
| [`examples/viewer-vulkan-cpp`](./examples/viewer-vulkan-cpp) | C++ on Vulkan, Linux — X11 and Wayland in one binary | `cd examples/viewer-vulkan-cpp && cmake -B build && cmake --build build && ./build/ssviewer-vulkan` |
| [`examples/viewer-bevy`](./examples/viewer-bevy) | Bevy — the adapter's development viewer, a live cast | `cargo run -p ssplayer-bevy-viewer` |
| [`examples/overall-bevy`](./examples/overall-bevy) | Bevy — the scripted feature tour | `cargo run -p ssplayer-bevy-overall` |

From a fresh checkout, one script converts the data on its first run and then starts a sample —
`hello` unless you name another (`.ps1` on Windows):

```bash
./scripts/run-example.sh example=viewer   # or viewer-bevy / overall-bevy / web
./scripts/run-web.sh                      # the browser demo: builds the package if missing, serves and opens it
```

Running one by hand, as in the table, needs the generated animation data first:

```bash
./scripts/deploy-examples.sh    # builds ssconverter-cli from the SDK, converts the test projects → assets/
```

The C++ viewers drive the library through CMake, which runs cargo itself, so they need no separate build
step.

## 🔗 Related Repositories

- [SpriteStudio Docs](https://cri-middleware.github.io/SpriteStudio-Docs/) — the portal for the SDK and every official player
- [SpriteStudio-SDK](https://github.com/cri-middleware/SpriteStudio-SDK) — the core SDK providing `ssruntime` and `ssconverter`
- [SSPlayerForWeb](https://github.com/cri-middleware/SSPlayerForWeb) — the Web players, including a WebGL renderer for where WebGPU is missing
- [SSConverterGUI](https://github.com/cri-middleware/SSConverterGUI) — desktop GUI tool for converting `.sspj` to `.ssab` without a player

## 📄 License

BSD-3-Clause. See [LICENSE.md](./LICENSE.md).

The library also links Rust crates of its own — wgpu and its backends, FlatBuffers, the `image`
decoder — whose licences carry attribution obligations onto whatever you ship. What to include is in
[THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md), which `scripts/generate-licenses.sh` regenerates
from the dependency tree.
