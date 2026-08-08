//! What `SSPlayer::load_with` promises an [`AssetLoader`] (SPEC §8.1, §8.2).
//!
//! The walk is shared by every face; the loader is the only part that differs.
//! So the contract between them is the one thing that, if each face reads it
//! differently, breaks a face nothing tests — and `web.rs` is tested by nothing.
//!
//! It broke exactly once, that way. The web loader was given the `.ssab`'s own
//! URL as its base, so when the walk asked it for `src` — which it does, `src` is
//! just the first file — it resolved `assets/Basic.ssab` against
//! `…/assets/Basic.ssab` and fetched `assets/assets/Basic.ssab`. Every load
//! failed with `NotLoaded`, and no native test could have seen it: the filesystem
//! loader is rooted at `""`, so the same mistake is not expressible there.
//!
//! These tests pin the contract itself rather than either implementation:
//!
//! 1. the first thing asked for is `src`, **verbatim**, while the loader is still
//!    rooted wherever the caller left it;
//! 2. `set_base` is then called once, with `src` verbatim, and everything asked
//!    for after it is a **bare name** — the walk joins nothing onto anything, so
//!    resolving is the loader's whole job and its vocabulary is its own business;
//! 3. `set_base` is derived from the root the loader was *constructed* with, so
//!    it is idempotent and a loader survives being used for a second load;
//! 4. one file is asked for once, however many hashes name it;
//! 5. what comes back is offered as a texture under every hash that named it.
//!
//! Rule 1 before rule 2 is what makes the historical bug unrepresentable rather
//! than merely tested for: `src` is read before any base derived from it exists,
//! so no implementation can resolve it against itself however it is written.
//!
//! A recording loader stands in for a face. It reads real fixture bytes, so the
//! walk does real work — the pack parses, its instance references resolve, both
//! texture tables come out — and every request it made is left behind to assert on.

use ssplayer_wgpu::{AssetLoader, MissingKind, SSPlayer};

fn asset_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")
}

/// Stands in for a face: serves bytes from disk and records what was asked for.
///
/// `root` is where it resolves requests against — and it moves exactly when the
/// walk says so, which is the property under test. It derives the new root the
/// way the filesystem loader does, with `Path::parent`; a web face would use a
/// `URL` base instead, and the walk is meant not to care which.
struct Recorder {
    /// Where the caller rooted it. `set_base` derives from this and never moves
    /// it, which is what the contract asks of an implementation and what lets one
    /// loader serve a second load.
    origin: std::path::PathBuf,
    root: std::path::PathBuf,
    /// One entry per `read` call, so the batching is visible too: the walk is
    /// meant to ask for a whole level at once, which is what lets a face overlap
    /// the waiting (`Promise.allSettled` on the web one).
    reads: Vec<Vec<String>>,
    /// Every `set_base`, verbatim, tagged with how many `read` calls had already
    /// happened — so the ordering rule 1-before-2 rests on is assertable and not
    /// just described.
    rebases: Vec<(usize, String)>,
    /// Hashes offered, in order, and how many bytes each came with.
    supplied: Vec<(u32, usize)>,
}

impl Recorder {
    fn new(root: impl AsRef<std::path::Path>) -> Self {
        Self { origin: root.as_ref().to_path_buf(), root: root.as_ref().to_path_buf(), reads: Vec::new(), rebases: Vec::new(), supplied: Vec::new() }
    }

    /// Every path asked for, flattened, in request order.
    fn requested(&self) -> Vec<&str> {
        self.reads.iter().flatten().map(String::as_str).collect()
    }
}

impl AssetLoader for Recorder {
    fn reset_base(&mut self) {
        self.root.clone_from(&self.origin);
    }

    fn set_base(&mut self, src: &str) {
        self.rebases.push((self.reads.len(), src.to_owned()));
        self.root = self.origin.join(src).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    }

    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        self.reads.push(paths.to_vec());
        paths.iter().map(|path| std::fs::read(self.root.join(path)).ok()).collect()
    }

    async fn supply_texture(&mut self, hash: u32, encoded: &[u8]) {
        self.supplied.push((hash, encoded.len()));
    }

    fn has_texture(&self, _hash: u32) -> bool {
        false
    }

    fn warn(&self, message: &str) {
        // `&self`, so this cannot record; it is a diagnostic channel by design
        // (one missing file must not fail the load). Printed so a broken fixture
        // shows up in the test output rather than as a silent zero.
        eprintln!("[recorder] {message}");
    }
}

/// True once `scripts/deploy-examples.sh` has been run. The tests skip rather
/// than fail without it, since the fixtures are generated and not committed.
fn fixtures_present() -> bool {
    std::path::Path::new(asset_root()).join("overall/Instance.ssab").exists()
}

/// Load `src` through `recorder`, skipping the test only if the fixture is not on
/// disk.
///
/// **The existence check comes first, and a load that then fails is a failure.**
/// Treating every `Err` as "no fixtures" is how a test that has broken the walk
/// reports itself green, and that is not hypothetical: it swallowed a `set_base`
/// called *before* the pack was read — the precise ordering this file calls
/// load-bearing — because re-rooting early made the pack unreadable, and an
/// unreadable pack looked exactly like an ungenerated one.
fn load(recorder: &mut Recorder, src: &str) -> Option<SSPlayer> {
    // Against `origin`: `src` is named relative to where the caller rooted the
    // loader, which is still true on a second load after the base has moved.
    if !recorder.origin.join(src).exists() {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return None;
    }
    let mut player = SSPlayer::new();
    pollster::block_on(player.load_with(recorder, src)).expect("the fixture is on disk, so a load that fails is the walk's doing, not a missing file");
    Some(player)
}

/// Rule 1 then rule 2: the pack is read exactly as handed in, and only then does
/// the base move — once, to `src` verbatim.
///
/// The order is the whole safety property. While `src` is being read there is no
/// base derived from it yet, so "resolve `src` against `src`" is not a mistake an
/// implementation is in a position to make, however it chooses to derive a
/// directory afterwards.
#[test]
fn the_pack_is_read_before_the_base_moves() {
    let mut recorder = Recorder::new(asset_root());
    let src = "overall/Instance.ssab";
    let Some(_) = load(&mut recorder, src) else { return };

    assert_eq!(recorder.reads.first().map(Vec::as_slice), Some([src.to_string()].as_slice()), "the walk did not ask for `src` verbatim, or asked for something else first");

    assert_eq!(recorder.rebases.len(), 1, "the base moved {} times; a loader cannot track a base that moves under it: {:?}", recorder.rebases.len(), recorder.rebases);
    let (reads_before, given) = &recorder.rebases[0];
    assert_eq!(given, src, "`set_base` was handed something other than `src`; deriving the directory is the loader's job, not the walk's");
    assert_eq!(*reads_before, 1, "the base moved before the pack was read, so `src` can be resolved against itself — the bug this file exists for");
}

/// Rule 2: everything after the pack is a **bare name**, relative to the base.
///
/// The walk joins nothing, so every question about separators belongs to one
/// place: a caller's path vocabulary reaches only the loader that speaks it
/// (SPEC §8.1).
#[test]
fn everything_after_the_pack_is_named_relative_to_the_base() {
    let mut recorder = Recorder::new(asset_root());
    let Some(player) = load(&mut recorder, "overall/Instance.ssab") else { return };

    // What was asked for is exactly what the packs name, with nothing prepended.
    //
    // Stated as membership rather than "contains no separator", because a bare
    // name is not the same thing as a name without directories: an exporter may
    // well write `images/atlas.png`, and passing that through unchanged is the
    // contract being kept, not broken.
    let mut named: std::collections::HashSet<String> = std::collections::HashSet::new();
    for pack in ["Instance", "Instance_SourceAnime"] {
        named.insert(format!("{pack}.ssab"));
        named.extend(player.textures(Some(pack)).unwrap().iter().map(|t| t.path.clone()));
    }
    for path in recorder.requested().iter().skip(1) {
        assert!(named.contains(*path), "{path} is not something the packs name; the walk built a path the loader was meant to resolve");
    }

    // The walk really ran: the sibling pack was asked for by bare name, and the
    // atlases of both packs were offered.
    assert!(recorder.requested().contains(&"Instance_SourceAnime.ssab"), "the referenced instance pack was never requested: {:?}", recorder.requested());
    assert!(player.has_pack("Instance_SourceAnime"));
    assert!(!recorder.supplied.is_empty(), "no atlas was offered");
    assert!(player.missing_assets().is_empty(), "a complete load reported something missing: {:?}", player.missing_assets());
}

/// Rule 3: one loader, two loads. `set_base` is called once per `load_with`, so
/// an implementation that folds each call into the last instead of deriving from
/// the root it was built with accumulates a base — and the second load goes
/// looking in `overall/overall`.
///
/// This is the shape `load_with` exists for: an engine adapter holds one loader
/// wrapping its `AssetServer` / `ResourceLoader` / `.pck` handle and loads every
/// character through it. Nothing in-repo hits it — `SSPlayer::load` and the web
/// face both build a fresh loader per call — which is exactly why it needs a test.
#[test]
fn one_loader_serves_a_second_load() {
    let mut recorder = Recorder::new(asset_root());
    let Some(_) = load(&mut recorder, "overall/Instance.ssab") else { return };
    let first = recorder.requested().len();

    // The same spelling again: a loader that accumulated would now resolve
    // `overall/overall/TextureChange.ssab` and fail.
    let Some(player) = load(&mut recorder, "overall/TextureChange.ssab") else { return };

    assert!(player.has_pack("TextureChange"), "the second load through a reused loader did not register its pack");
    assert!(player.missing_assets().is_empty(), "the second load could not find what it asked for: {:?}", player.missing_assets());
    assert_eq!(recorder.rebases.len(), 2, "one `set_base` per load: {:?}", recorder.rebases);
    // Same rule as the first load: an atlas the pack names, or a sibling pack by
    // bare name — never anything the walk built.
    let atlases: std::collections::HashSet<&str> = player.textures(Some("TextureChange")).unwrap().iter().map(|t| t.path.as_str()).collect();
    for path in recorder.requested().iter().skip(first + 1) {
        let is_sibling_pack = path.strip_suffix(".ssab").is_some_and(|name| player.has_pack(name));
        assert!(atlases.contains(path) || is_sibling_pack, "{path} is not something the pack names; the second load resolved a path itself");
    }
}

/// And the consequence worth stating on its own: **the walk never looks inside
/// `src`**, so how a host spells it cannot change what gets asked for.
///
/// Two loads of the same pack — one named through a directory, one from a loader
/// already rooted there — must issue an identical sequence of requests after the
/// first. When that holds there is nowhere left for a separator, a drive letter or
/// a URL scheme to be misread, because nothing reads them — a property no walk
/// that takes `src` apart itself can have, since it must pick one answer for
/// filesystems and URLs both.
#[test]
fn what_is_asked_for_does_not_depend_on_how_src_is_spelled() {
    let mut through_a_directory = Recorder::new(asset_root());
    let Some(_) = load(&mut through_a_directory, "overall/Instance.ssab") else { return };

    let mut already_rooted = Recorder::new(format!("{}/overall", asset_root()));
    let Some(_) = load(&mut already_rooted, "Instance.ssab") else { return };

    assert!(through_a_directory.requested().len() > 1, "the walk asked for nothing beyond the pack itself; this test would prove nothing");
    assert_eq!(&through_a_directory.requested()[1..], &already_rooted.requested()[1..], "the walk asked for different things depending on how `src` was spelled, so something is still reading it");
}

/// One file, one request — however many hashes name it. `external_textures()`
/// can name the image a cellmap already named, under a second hash (SPEC §8.1),
/// and on a face where a read is a network round trip that must not double.
#[test]
fn a_file_named_twice_is_read_once() {
    let mut recorder = Recorder::new(asset_root());
    let Some(player) = load(&mut recorder, "overall/TextureChange.ssab") else { return };

    let common = "common.png";
    let requests = recorder.requested().iter().filter(|p| **p == common).count();
    assert_eq!(requests, 1, "common.png was requested {requests} times: {:?}", recorder.requested());

    // But both hashes that name it are still offered a texture — the dedup is on
    // the file, never on the key a draw batch will ask for.
    let named_by: Vec<u32> = player.textures(None).unwrap().iter().filter(|t| t.path == "common.png").map(|t| t.hash).collect();
    assert_eq!(named_by.len(), 2, "the fixture no longer names one file under two hashes; this test would prove nothing");
    for hash in named_by {
        assert!(recorder.supplied.iter().any(|(supplied, _)| *supplied == hash), "{hash:#010x} names common.png but was never offered its bytes");
    }
}

/// Levels arrive whole. The walk is breadth-first so that a face which can
/// overlap the waiting has something to overlap — one `read` per level, not one
/// per file.
#[test]
fn a_level_is_requested_in_one_call() {
    let mut recorder = Recorder::new(asset_root());
    let Some(_) = load(&mut recorder, "overall/TextureChange.ssab") else { return };

    // The pack, then its atlases: two calls, and the second carries all of them.
    assert_eq!(recorder.reads.len(), 2, "expected one call for the pack and one for its atlases, got {:?}", recorder.reads);
    assert!(recorder.reads[1].len() > 1, "the atlases were not batched: {:?}", recorder.reads[1]);
}

/// An embedded pack asks for nothing beyond itself, and still offers every
/// atlas — the bytes were inside the file all along (SPEC §8.1).
///
/// This one found a real bug on its first run. `embedded_images()` required
/// **both** `cellmaps()` and `embedded_assets()` to be present, so a pack with no
/// cellmaps at all — one whose parts only draw TextureChange targets, which
/// `overall_emb/Instance.ssab` is — carried nothing, and its atlas sat unread
/// inside the file while every part drew white.
///
/// `load.rs` could not see it, and the reason is worth keeping: that test asserts
/// on the *end state*, and the end state was fine — the sibling pack
/// `Instance_SourceAnime` embeds the same image under the same hash, so the atlas
/// arrived anyway, from somewhere else. Asserting on what was **asked for** is
/// what made the omission visible.
#[test]
fn an_embedded_pack_asks_for_nothing_else() {
    let mut recorder = Recorder::new(asset_root());
    let Some(player) = load(&mut recorder, "overall_emb/Instance.ssab") else { return };

    let extra: Vec<&str> = recorder.requested().into_iter().filter(|p| p.ends_with(".png")).collect();
    assert!(extra.is_empty(), "an embedded pack asked for images: {extra:?}");

    // The sibling pack is still a file, so that one is still fetched.
    assert!(recorder.requested().contains(&"Instance_SourceAnime.ssab"));
    for name in ["Instance", "Instance_SourceAnime"] {
        for texture in player.textures(Some(name)).unwrap() {
            assert!(recorder.supplied.iter().any(|(hash, _)| *hash == texture.hash), "{name} needs {} ({:#010x}), which was never offered", texture.path, texture.hash);
        }
    }
}

/// A file that does not arrive costs that file and nothing else: the load still
/// succeeds, and everything readable is still offered (SPEC §8.1).
#[test]
fn one_missing_file_does_not_fail_the_load() {
    /// Serves nothing at all.
    struct Empty(Vec<String>);
    impl AssetLoader for Empty {
        // Serves nothing, so it has no base to keep track of.
        fn reset_base(&mut self) {}
        fn set_base(&mut self, _src: &str) {}

        async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
            self.0.extend(paths.iter().cloned());
            paths.iter().map(|_| None).collect()
        }
        async fn supply_texture(&mut self, _hash: u32, _encoded: &[u8]) {}
        fn has_texture(&self, _hash: u32) -> bool {
            false
        }
        fn warn(&self, _message: &str) {}
    }

    // A pack whose atlases are all missing still loads and still plays.
    let mut recorder = Recorder::new(asset_root());
    let Some(mut player) = load(&mut recorder, "overall/Instance.ssab") else { return };
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    assert!(player.set_animation(&anim).is_ok());
    assert!(player.missing_assets().is_empty(), "a complete load reported something missing: {:?}", player.missing_assets());

    // But a pack that is not there at all is an error — that one the caller has
    // to hear about, and it is the only thing `load` fails on.
    let mut empty = Empty(Vec::new());
    let mut orphan = SSPlayer::new();
    assert!(pollster::block_on(orphan.load_with(&mut empty, "overall/Instance.ssab")).is_err(), "a pack that could not be read still reported success");
    assert_eq!(empty.0, ["overall/Instance.ssab"], "the walk kept going after the pack itself failed");
}

/// A load that succeeds but did not get everything says so — the half of a
/// partial load a program can branch on, where `warn` is the half a human reads
/// (SPEC §8.1).
#[test]
fn a_partial_load_is_reported_without_failing() {
    /// Serves the packs and refuses every image.
    struct NoImages {
        origin: std::path::PathBuf,
        root: std::path::PathBuf,
    }
    impl AssetLoader for NoImages {
        fn reset_base(&mut self) {
            self.root.clone_from(&self.origin);
        }

        fn set_base(&mut self, src: &str) {
            self.root = self.origin.join(src).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
        }

        async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
            paths.iter().map(|path| if path.ends_with(".png") { None } else { std::fs::read(self.root.join(path)).ok() }).collect()
        }
        async fn supply_texture(&mut self, _hash: u32, _encoded: &[u8]) {}
        fn has_texture(&self, _hash: u32) -> bool {
            false
        }
        fn warn(&self, _message: &str) {}
    }

    let mut loader = NoImages { origin: asset_root().into(), root: asset_root().into() };
    let mut player = SSPlayer::new();
    if !fixtures_present() {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    }
    pollster::block_on(player.load_with(&mut loader, "overall/Instance.ssab")).expect("the fixture is on disk, so a load that fails is the walk's doing");

    // It loaded, and it plays.
    let anim = player.animation_names().unwrap().first().cloned().unwrap();
    assert!(player.set_animation(&anim).is_ok());

    // And every image it could not get is named, by the key a draw batch will
    // ask for — so a caller can decide whether a silhouette is worth failing over.
    let missing = player.missing_assets();
    assert!(!missing.is_empty(), "every atlas was refused and nothing was reported missing");
    assert!(missing.iter().all(|m| m.kind == MissingKind::Texture && m.name.ends_with(".png") && m.hash != 0), "{missing:?}");

    // The tally belongs to the last load, not to the player.
    let mut recorder = Recorder::new(asset_root());
    assert!(pollster::block_on(player.load_with(&mut recorder, "overall/Instance.ssab")).is_ok());
    assert!(player.missing_assets().is_empty(), "the previous load's failures survived a successful one: {:?}", player.missing_assets());
}

/// A sub-pack that does not arrive is reported too, and as a pack rather than as
/// the path it was looked for under (SPEC §7.1).
#[test]
fn a_missing_sub_pack_is_reported_by_name() {
    /// Serves the root pack and its images, and no sibling `.ssab`.
    struct NoSiblings {
        origin: std::path::PathBuf,
        root: std::path::PathBuf,
        first: bool,
    }
    impl AssetLoader for NoSiblings {
        fn reset_base(&mut self) {
            self.root.clone_from(&self.origin);
        }

        fn set_base(&mut self, src: &str) {
            self.root = self.origin.join(src).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
        }

        async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
            let root_pack = std::mem::take(&mut self.first);
            paths.iter().map(|path| if path.ends_with(".ssab") && !root_pack { None } else { std::fs::read(self.root.join(path)).ok() }).collect()
        }
        async fn supply_texture(&mut self, _hash: u32, _encoded: &[u8]) {}
        fn has_texture(&self, _hash: u32) -> bool {
            false
        }
        fn warn(&self, _message: &str) {}
    }

    let mut loader = NoSiblings { origin: asset_root().into(), root: asset_root().into(), first: true };
    let mut player = SSPlayer::new();
    if !fixtures_present() {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    }
    pollster::block_on(player.load_with(&mut loader, "overall/Instance.ssab")).expect("the fixture is on disk, so a load that fails is the walk's doing");

    let packs: Vec<&str> = player.missing_assets().iter().filter(|m| m.kind == MissingKind::Pack).map(|m| m.name.as_str()).collect();
    assert_eq!(packs, ["Instance_SourceAnime"], "the missing sub-pack was not reported by the name `ref_anime_pack` resolves against: {:?}", player.missing_assets());
    assert!(!player.has_pack("Instance_SourceAnime"));
}

/// A pack that has a name but no path is resolvable through `read_pack`, and the
/// walk never goes looking for it as a file (SPEC §7.1).
#[test]
fn a_provider_answers_for_packs_with_no_path() {
    /// Serves the root pack and its images by path, and every sibling `.ssab` by
    /// name only — an archive keyed by pack name, in miniature.
    struct ByName {
        origin: std::path::PathBuf,
        root: std::path::PathBuf,
        asked_for: Vec<String>,
        provided: Vec<String>,
    }
    impl AssetLoader for ByName {
        fn reset_base(&mut self) {
            self.root.clone_from(&self.origin);
        }

        fn set_base(&mut self, src: &str) {
            self.root = self.origin.join(src).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
        }

        async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
            self.asked_for.extend(paths.iter().cloned());
            paths.iter().map(|path| std::fs::read(self.root.join(path)).ok()).collect()
        }
        async fn read_pack(&mut self, name: &str) -> Option<Vec<u8>> {
            self.provided.push(name.to_string());
            std::fs::read(self.root.join(format!("{name}.ssab"))).ok()
        }
        async fn supply_texture(&mut self, _hash: u32, _encoded: &[u8]) {}
        fn has_texture(&self, _hash: u32) -> bool {
            false
        }
        fn warn(&self, _message: &str) {}
    }

    let mut loader = ByName { origin: asset_root().into(), root: asset_root().into(), asked_for: Vec::new(), provided: Vec::new() };
    let mut player = SSPlayer::new();
    if !fixtures_present() {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    }
    pollster::block_on(player.load_with(&mut loader, "overall/Instance.ssab")).expect("the fixture is on disk, so a load that fails is the walk's doing");

    assert!(player.has_pack("Instance_SourceAnime"), "the provider's pack was not registered");
    assert_eq!(loader.provided, ["Instance_SourceAnime"], "the provider was asked for the wrong thing: {:?}", loader.provided);
    assert!(player.missing_assets().is_empty(), "{:?}", player.missing_assets());

    // And the file was never looked for: what the provider answers costs no read.
    let looked_for: Vec<&String> = loader.asked_for.iter().filter(|p| p.ends_with("Instance_SourceAnime.ssab")).collect();
    assert!(looked_for.is_empty(), "the provider answered and the walk still went to the filesystem: {looked_for:?}");
}

/// Declining is the default and falls back to the path, so a loader that does not
/// implement `read_pack` behaves exactly as before.
#[test]
fn declining_the_provider_falls_back_to_the_path() {
    let mut recorder = Recorder::new(asset_root());
    let Some(player) = load(&mut recorder, "overall/Instance.ssab") else { return };

    assert!(player.has_pack("Instance_SourceAnime"));
    assert!(recorder.requested().contains(&"Instance_SourceAnime.ssab"), "the sibling was not read by path: {:?}", recorder.requested());
}

/// The other way to get a pack the walk cannot reach: register it first. The walk
/// skips what the player already has, so it never looks for it at all.
#[test]
fn a_pre_registered_pack_is_not_looked_for() {
    let Ok(bytes) = std::fs::read(format!("{}/overall/Instance_SourceAnime.ssab", asset_root())) else {
        eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
        return;
    };
    let mut player = SSPlayer::new();
    player.add_ssab("Instance_SourceAnime", &bytes).unwrap();

    let mut recorder = Recorder::new(asset_root());
    assert!(pollster::block_on(player.load_with(&mut recorder, "overall/Instance.ssab")).is_ok());

    let looked_for: Vec<&str> = recorder.requested().into_iter().filter(|p| p.ends_with("Instance_SourceAnime.ssab")).collect();
    assert!(looked_for.is_empty(), "a pack the player already had was fetched again: {looked_for:?}");
    assert!(player.has_pack("Instance_SourceAnime"), "loading the main pack dropped a pack registered before it");
}
