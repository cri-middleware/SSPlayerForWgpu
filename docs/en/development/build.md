# 🛠️ Build Guide

For whoever works on SpriteStudio Player for wgpu, or changes it for a host of their own. Everything
in this repository can be built on one machine, and this page is the whole checklist, in the order a
fresh clone needs it. How the crate is put together, and why, is on
[How the Player Is Put Together](architecture.md).

## Get the source

```bash
git clone --recursive https://github.com/cri-middleware/SSPlayerForWgpu.git
cd SSPlayerForWgpu
git submodule update --init        # if you forgot --recursive
```

`SpriteStudio-SDK` is a **git submodule**, and `ssruntime` / `ssab` are path dependencies into it.
Unlike the other players — which talk to the SDK through the stable C API in a released zip — this
one links those crates directly and reads their FlatBuffers types, so it has to track unreleased
`.ssab` schema changes.

!!! danger "Never commit inside the submodule"
    A change that needs `ssruntime` or `ssab` is a pull request in the **SDK repository**, followed
    by a submodule bump here:

    ```bash
    git -C SpriteStudio-SDK fetch origin && git -C SpriteStudio-SDK checkout origin/develop
    ./scripts/deploy-examples.sh
    cargo test --workspace
    git add SpriteStudio-SDK && git commit -m "chore: bump SDK submodule"
    ```

## Generate the playback data — once, first

```bash
./scripts/deploy-examples.sh
```

It builds `ssconverter-cli` from the submodule, converts the SDK's test projects into `assets/`, and
deploys **both** export modes into `examples/web`. **The tests read `assets/`**, so this is a
prerequisite for `cargo test`, not an optional step.

Run it again after the SDK moves: the `.ssab` format follows the SDK, and data that another SDK
commit converted can make the runtime panic. `run-example.sh` and `build-pages.sh` check for that
themselves (`deploy-examples.sh when=stale`).

## Build every face

| Face | Command |
|---|---|
| Rust library (+ the C face), collected into `bin/` | `./scripts/build.sh` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Format | `cargo fmt --all -- --check` / `cargo fmt --all` |
| Tests | `cargo test --workspace` |
| **wasm / JS face** | `cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web` |
| Lint (wasm / JS face) | `cargo clippy -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web -- -D warnings` |
| **C face** (+ header regeneration) | `cargo build -p ssplayer-wgpu-ffi` |
| C handle lifetimes | `cargo test -p ssplayer-wgpu --features ffi --lib` |
| npm package (wasm + TS layer) | `./scripts/build.sh platform=web` — needs `pnpm` and `wasm-pack` |
| TS layer typecheck | `cd packages/ssplayer-wgpu && pnpm run typecheck` |
| Smallest Rust host | `cargo run -p ssplayer-wgpu-hello` |
| Native viewer | `cargo run -p ssplayer-wgpu-viewer` |
| Bevy viewers | `cargo run -p ssplayer-bevy-overall` / `-p ssplayer-bevy-viewer` |
| Metal C++ viewer (macOS) | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| DX12 C++ viewer (Windows) | `cd examples\viewer-dx12-cpp; cmake -B build; cmake --build build --config Release` |
| Vulkan C++ viewer (Linux) | `cd examples/viewer-vulkan-cpp && cmake -B build && cmake --build build && ./build/ssviewer-vulkan` |
| Web demo | `./scripts/build.sh platform=web && (cd examples/web && python3 -m http.server --bind 127.0.0.1)` |

## Three blind spots to know about

Ordinary `cargo build`, `clippy --all-targets` and `cargo test` compile **none** of the following.
Each rots silently until someone builds it deliberately:

1. **The web face** sits behind the `web` feature and is compiled by no native command. Build and
   lint the wasm target after touching it.
2. **The TypeScript layer** types itself against wasm-pack's generated `.d.ts`, so renaming a
   `#[wasm_bindgen]` method breaks it while cargo stays happy. Run the typecheck after changing the
   web face's public surface.
3. **The C face** sits behind the `ffi` feature, and that build regenerates `ssplayer_wgpu.h`
   through cbindgen. **Commit the regenerated
   header with the change that moved it**, and read the diff: cbindgen sweeps up every module-level
   `pub const` in the crate.

## Build the documentation site

The site is Markdown plus two grafts, and needs no cargo for the prose half:

```bash
./scripts/prepare-docs.sh          # .venv + the pinned zensical
./scripts/build-docs.sh            # both locales, --strict, into site/
./scripts/build-docs.sh serve=yes  # ... and serve it

./scripts/build-pages.sh serve=yes # everything pages.yml publishes: docs + rustdoc + WebGPU demo
```

| Rule | |
|---|---|
| **English is the source, `docs/ja` mirrors it** | Same paths in both trees; only the prose and the nav labels differ |
| **The nav lives in two files** | `mkdocs.yml` and `mkdocs.ja.yml`. Adding, moving or renaming a page means editing both |
| **English builds first** | It clears `site/`, and the Japanese site, the rustdoc and the demo all live inside it |
| **`--strict`, always** | A broken internal link fails the build, here and in CI, rather than passing as a warning |

`build-pages.sh api=no demo=no` is the fast loop while writing prose.

## Where the canon lives

| | |
|---|---|
| [`CONTRIBUTING.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/CONTRIBUTING.md) | The constraints a change must respect, and how to verify one. Read it before a non-trivial change |
| [`ROADMAP.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) | What is planned, what is unverified, and what was deliberately declined |
| [SDK Player Porting Guide](https://github.com/cri-middleware/SpriteStudio-SDK/tree/HEAD/libs/ssruntime/docs) | Playback semantics — the "Brain" — are canonically documented there, not here, and must not be duplicated into this repository |
| These docs | The reader-facing guide. Package READMEs stay the front door on npm and GitHub |

## Verifying a change

A full pass, in the order CI runs it:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                    # needs deploy-examples.sh first
cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web
cargo clippy -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web -- -D warnings
cargo clippy -p ssplayer-wgpu --features ffi --all-targets -- -D warnings
cargo build -p ssplayer-wgpu-ffi                          # also regenerates the C header
cargo test -p ssplayer-wgpu --features ffi --lib
```

Three testing notes:

* **Rendering tests skip themselves when no GPU adapter is present**, so a green run on such a
  machine may have verified nothing. CI installs a software Vulkan device (Mesa lavapipe) and fails
  hard if none is found. When you change rasterisation, dump the readback to a PNG and look at it.
* **Measure a performance change before and after, and drive a real frame loop while doing it.**
  Interleave `update` with the render pass exactly as a frame does: timing them in separate loops
  makes anything that caches across frames look far better than it is. `cargo test --test scale --
  --ignored` is the repository's throughput harness and does both; it is `#[ignore]`d because CI's
  software GPU cannot judge speed. The counters a host sees are in
  [Performance](../guide/performance.md).
* **Take a golden dump before touching geometry.** The golden corpus test (ignored by default)
  renders every fixture/animation/frame/stencil combination and byte-compares it against a
  previous run:

    ```bash
    SS_GOLDEN_DIR=/tmp/before cargo test --test golden -- --ignored
    # ... make the change ...
    SS_GOLDEN_DIR=/tmp/after SS_GOLDEN_COMPARE=/tmp/before cargo test --test golden -- --ignored
    ```

    The reference is GPU-specific, so it is a before/after tool on one machine, not a checked-in set.
    The comparison writes `/tmp/after/summary.md`, and for each image that moved a
    before/after/difference triptych in `/tmp/after/diff/`. CI does the same on every pull request,
    on lavapipe against the base's rendering, and fails on any difference, including one you meant;
    then say in the description why the triptychs it uploads (`golden-diff`) are right.

## Using a changed Player

Each face takes a checkout the way it takes a release:

- **Rust:** a path dependency, `ssplayer-wgpu = { path = "<checkout>/libs/ssplayer-wgpu" }` (and
  `ssplayer-bevy` the same way). `ssruntime` and `ssab` come with it, through the submodule. The
  player borrows the host's device, so the host's `wgpu` must be the version the workspace's
  `Cargo.toml` names.
- **C / C++:** `./scripts/build.sh` collects the library and `ssplayer_wgpu.h` into `bin/`
  (`bin/libs/<platform>/<arch>/`, `bin/include/`); link them as the C++ viewers under `examples/`
  do.
- **npm:** build the package, pack it, and install the tarball in the app:

    ```bash
    ./scripts/build.sh platform=web                  # wasm-pack + tsdown -> packages/ssplayer-wgpu/{wasm,dist}
    (cd packages/ssplayer-wgpu && pnpm pack)         # -> spritestudio-ssplayer-wgpu-<version>.tgz
    ```

    Then `npm install <checkout>/packages/ssplayer-wgpu/spritestudio-ssplayer-wgpu-<version>.tgz` in
    the app. `prepack` refuses a tarball whose advertised
    `dist/` or `wasm/` is missing, so a packed tarball is complete.

## Checking against the real host

What CI and the tests above do not show, and how to look at it:

- **Linux desktops.** CI builds the Vulkan C++ viewer and renders on lavapipe, headless. A Wayland
  compositor, an Xorg session and interactive resizing need a Linux desktop — a VM with one is
  enough. Copy the checkout in with `rsync -a --exclude target`, which also carries the submodule
  without credentials on the VM.
- **An offscreen check never clears to black.** Counting "non-black" pixels after a black clear makes
  black artwork read as nothing drawn, and assets often fade out through a black cell. Clear to
  another colour and count the pixels that differ from it; the render tests' own pixel count clears
  to black and counts non-black, so it is not the tool for such assets. Read the viewport from
  `canvas_size()` after `set_animation()` — before it, it is `None` — and do not treat zero pixels at
  a low resolution as a defect: a 5×4 px cell can vanish there.
- **Profiling a frame.** The workspace's release profile strips symbols, so build what you profile
  with `CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` and a
  `CARGO_TARGET_DIR` of its own, and record with
  [samply](https://github.com/mstange/samply) (`--save-only --unstable-presymbolicate`). LTO inlines
  frames away, so read the self time at the leaves with their nearest caller. `deploy-examples.sh`
  looks for the converter under `SpriteStudio-SDK/target`, so run it with `CARGO_TARGET_DIR` unset.
- **Before-and-after comparisons alternate.** Build both sides, run them in turn three to five
  times, and compare ratios and medians: absolute times move by a quarter with the machine's load,
  and a single GPU timing is bimodal with the GPU's clock state.

## Releasing

`release.yml` builds every platform; `scripts/build-release.sh` assembles the zips, the npm
tarball and the checksums from what the matrix produced. There is no local equivalent of the matrix — Windows and
macOS need an operating system each — so a local run starts from
`gh run download <run-id> -D artifacts`.

`pages.yml` publishes this site, on a release and on demand.
