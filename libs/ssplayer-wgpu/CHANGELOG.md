# Changelog

All notable changes to `ssplayer-wgpu` are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and the package adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **First release.** A wgpu renderer for SpriteStudio animations (`.ssab`), linking the
  SpriteStudio-SDK runtime (`ssruntime`, `ssab`) as Rust crates. One rendering code path covers
  WebGPU, Vulkan, DirectX 12 and Metal.
- **Three faces over one library**: the Rust crate `ssplayer-wgpu`, a C / C++ static library with
  one header, and an npm package that runs the WebGPU renderer as WebAssembly, with a
  `<ss-player-webgpu>` element for the one-character case.
- **Borrow model**: the host owns the wgpu `Device` / `Queue` / `Surface` and lends them per frame,
  so the player draws into the host's own render pass — a `winit` / `egui` / `Slint` renderer.
- **Playback, events, per-part overrides, motion blending and device-loss recovery**, the same on
  every face that exposes them.
- **Documentation**: bilingual (EN/JA) documentation site with a path per face, and a migration
  guide from SS6Player for CPP.
