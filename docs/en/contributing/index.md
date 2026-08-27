# For contributors

Everything in this repository can be built on one machine. This page is the whole checklist, in the
order a fresh clone needs it.

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

## Build every face

| Face | Command |
|---|---|
| Rust library (+ the C face), collected into `bin/` | `./scripts/build.sh` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Format | `cargo fmt --all -- --check` / `cargo fmt --all` |
| Tests | `cargo test --workspace` |
| **wasm / JS face** | `cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web` |
| **C face** (+ header regeneration) | `cargo build -p ssplayer-wgpu-ffi` |
| C handle lifetimes | `cargo test -p ssplayer-wgpu --features ffi --lib` |
| npm package (wasm + TS layer) | `./scripts/build.sh platform=web` — needs `pnpm` and `wasm-pack` |
| TS layer typecheck | `cd packages/ssplayer-wgpu && pnpm run typecheck` |
| Native viewer | `cargo run -p ssplayer-wgpu-viewer` |
| Bevy viewers | `cargo run -p ssplayer-wgpu-overall-bevy` / `-p ssplayer-wgpu-viewer-bevy` |
| Metal C++ viewer (macOS) | `cd examples/viewer-metal-cpp && cmake -B build && cmake --build build && ./build/ssviewer-metal` |
| DX12 C++ viewer (Windows) | `cd examples\viewer-dx12-cpp; cmake -B build; cmake --build build --config Release` |
| Vulkan C++ viewer (Linux) | `cd examples/viewer-vulkan-cpp && cmake -B build && cmake --build build && ./build/ssviewer-vulkan` |
| Web demo | `./scripts/build.sh platform=web && (cd examples/web && python3 -m http.server)` |

## Three blind spots to know about

Ordinary `cargo build`, `clippy --all-targets` and `cargo test` compile **none** of the following.
Each rots silently until someone builds it deliberately:

1. **`web.rs`** sits behind the `web` feature and is compiled by no native command. Build the wasm
   target after touching it.
2. **The TypeScript layer** types itself against wasm-pack's generated `.d.ts`, so renaming a
   `#[wasm_bindgen]` method breaks it while cargo stays happy. Run the typecheck after changing the
   web face's public surface.
3. **`host.rs` / `ffi.rs`** sit behind the `ffi` feature, and that build regenerates
   `libs/ssplayer-wgpu-ffi/include/ssplayer_wgpu.h` through cbindgen. **Commit the regenerated
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
| **`--strict`, always** | Nothing builds the docs on a pull request, so a broken internal link would otherwise surface on the next publish |

`build-pages.sh api=no demo=no` is the fast loop while writing prose.

## Where the canon lives

| | |
|---|---|
| [`AGENTS.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/AGENTS.md) | The invariants a change must respect, and how to verify one. Read it before a non-trivial change |
| [`ROADMAP.md`](https://github.com/cri-middleware/SSPlayerForWgpu/blob/main/ROADMAP.md) | What is planned, what is unverified, and what was deliberately declined |
| [SDK `AGENTS.md`](https://github.com/cri-middleware/SpriteStudio-SDK) | Playback semantics — the "Brain" — are canonically documented there, not here, and must not be duplicated into this repository |
| These docs | The reader-facing guide. Package READMEs stay the front door for crates.io / npm / GitHub |

## Verifying a change

A full pass, in the order CI runs it:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                    # needs deploy-examples.sh first
cargo build -p ssplayer-wgpu --target wasm32-unknown-unknown --no-default-features --features web
cargo build -p ssplayer-wgpu-ffi                          # also regenerates the C header
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
* **Take a golden dump before touching geometry.** `tests/golden.rs::golden_corpus` (ignored by
  default) renders every fixture/animation/frame/stencil combination and byte-compares it against a
  previous run:

    ```bash
    SS_GOLDEN_DIR=/tmp/before cargo test --test golden -- --ignored
    # ... make the change ...
    SS_GOLDEN_COMPARE=/tmp/before cargo test --test golden -- --ignored
    ```

    The reference is GPU-specific, so it is a before/after tool on one machine, not a checked-in set.

## Releasing

`release.yml` builds every platform; `scripts/build-release.sh` assembles the zips and the
checksums from what the matrix produced. There is no local equivalent of the matrix — Linux, Windows
and macOS need three operating systems between them — so a local run starts from
`gh run download <run-id> -D artifacts`.

`pages.yml` publishes this site, on a release and on demand.
