# ssplayer-preview-gen

Fills the [preview shell](../preview/README.md) with a pack and everything that pack
depends on, and writes one openable file.

```sh
./scripts/build-preview.sh                                          # the shell, once
cargo run -p ssplayer-preview-gen -- pack=assets/Ringo/Ringo.ssab open=yes
```

```
pack=<file.ssab>     The pack to preview
shell=<file.html>    Which shell to fill (default: bin/ssplayer-preview.html)
out=<file.html>      Where to write it (default: <pack stem>-preview.html)
animation=<name>     Which animation to open on (default: the pack's first)
open=yes             Hand the result to the desktop afterwards
```

## What it is for

This is the step behind a host's **Preview** button: gather, inline, write, open. A host
in another language ports the walk in `src/lib.rs`; nothing here is Rust-specific and
none of it is in the player, which has no filesystem on every target it builds for.

**The gathering is the part worth having.** Substituting two strings is nothing; knowing
*which* files to substitute means reading the `.ssab`, and getting it wrong is silent:

| Read-out | Answers | Missing it looks like |
|---|---|---|
| `SSPack::textures` | the atlases, by the path the exporter wrote | parts draw as white silhouettes |
| `SSPack::instance_packs` | the sibling packs instance parts play from | those parts draw nothing at all |

Both are walked transitively — a sibling pack has siblings and atlases of its own — and
an embedded-mode pack needs neither, so it inlines as one file.

A file that cannot be found is **reported, not fatal**: the page is still written, and it
says on screen what is absent. A preview of a half-present pack is worth opening; a
silent one gets reported as a renderer bug.
