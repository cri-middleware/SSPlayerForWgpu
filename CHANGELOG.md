# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [7.0.0-beta.1] - Unreleased

### Added
- **Initial public release.** A wgpu renderer for SpriteStudio animations (`.ssab`), linking the
  SpriteStudio-SDK runtime (`ssruntime`, `ssab`) as Rust crates. One rendering code path covers
  WebGPU, Vulkan, DirectX 12 and Metal.
- **Four faces over one library**: the Rust crate `ssplayer-wgpu`, the Bevy adapter `ssplayer-bevy`,
  a C / C++ static library with one header, and an npm package that runs the WebGPU renderer as
  WebAssembly, with a `<ss-player-webgpu>` element for the one-character case.
- **Borrow model**: the host owns the wgpu `Device` / `Queue` / `Surface` and lends them per frame, so
  the player draws into the host's own render pass — a `winit` / `egui` / `Slint` renderer, or Bevy's
  2D pass.
- **Playback, events, per-part overrides, motion blending and device-loss recovery**, the same on every
  face that exposes them.
- **Documentation**: bilingual (EN/JA) documentation site with a path per face, and a migration guide
  from SS6Player for CPP.

### Known Limitations
- The API may change without notice while the player is in beta.
- The crates are not on crates.io yet; use them as a git dependency until the SDK crates are published.
- The C face builds and owns its own device; the web face needs WebGPU, with no WebGL fallback. These
  and the other constraints are listed in [Limitations & Scope](./docs/en/limitations.md).
- Constraints inherited from the shared runtime are documented in
  [Shared Limitations](https://cri-middleware.github.io/SpriteStudio-Docs/sdk/limitations/).
