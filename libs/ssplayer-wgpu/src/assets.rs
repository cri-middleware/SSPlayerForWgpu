//! The filesystem behind [`SSPlayer::load`] (SPEC §8.1).
//!
//! One of the two [`AssetLoader`] implementations that ship — the other is the
//! web face's (`fetch` + `createImageBitmap`, SPEC §10). Between them they are
//! the entire platform-specific part of loading; the walk they serve is shared.
//!
//! **Nothing here is privileged.** An engine integration writes its own and gets
//! the same walk: Bevy answers out of its `AssetServer`, a Godot host out of
//! `ResourceLoader` — which it must, because an exported Godot project keeps its
//! resources inside a `.pck` and a Unity one inside an APK, where no `File::open`
//! will reach them (SPEC §8.2).

use crate::player::AssetLoader;

/// Reads with `std::fs` and hands the bytes back **encoded**, for the player to
/// decode and upload on the next `update`.
///
/// It borrows no GPU state, which is the point: [`SSPlayer::load`] must not need
/// a `Device` or a `Queue`, because the borrow model does not lend it any until
/// the frame starts (SPEC §8.1). Uploading is the flush's job, on the same
/// timeline an embedded atlas already takes.
///
/// [`SSPlayer::load`]: crate::SSPlayer::load
pub(crate) struct FsLoader {
    /// Where the caller rooted this loader, which [`AssetLoader::set_base`] never
    /// moves. Every base is derived from *this*, not from the last one, so a
    /// loader can serve more than one load — see the note on `set_base`.
    origin: std::path::PathBuf,
    /// What every path is resolved against. Equal to `origin` until the walk calls
    /// [`AssetLoader::set_base`], which is deliberate: the `.ssab` itself is read
    /// exactly as the caller named it, and only then does its directory become
    /// the base for everything the pack points at.
    root: std::path::PathBuf,
    /// What was read, in the order the walk asked for it; the player takes it.
    pub(crate) images: Vec<(u32, Vec<u8>)>,
    /// Hashes already read during *this* load, so the walk does not re-read an
    /// atlas two sibling packs both name (see [`FsLoader::has_texture`]).
    supplied: std::collections::HashSet<u32>,
}

impl FsLoader {
    pub(crate) fn new() -> Self {
        Self { origin: std::path::PathBuf::new(), root: std::path::PathBuf::new(), images: Vec::new(), supplied: std::collections::HashSet::new() }
    }
}

impl AssetLoader for FsLoader {
    fn reset_base(&mut self) {
        self.root.clone_from(&self.origin);
    }

    /// `Path::parent` **is** the platform's answer to "which directory is this
    /// in", and that is the whole reason this method exists.
    ///
    /// **Do not cut `src` by hand here.** Separators are `Path`'s business: it
    /// knows which characters divide a path on the platform it was compiled for,
    /// and we do not have to. Getting that wrong does not fail loudly — the base
    /// lands one directory off, the atlases fall back to the 1x1 white texture,
    /// and the parts draw as silhouettes behind a single `warn` (SPEC §8.1).
    ///
    /// Derived from `origin` — where the caller rooted this loader — rather than
    /// from the base a previous load left behind. A relative `src` therefore stays
    /// inside the subtree the loader was handed, and, because `origin` never
    /// moves, one loader serves any number of loads: the second `load_with` sees
    /// the same base the first one did, not `overall/overall`.
    ///
    /// (An absolute `src` replaces `origin` entirely, which is `Path::join`'s
    /// meaning of absolute and the right answer here — a caller naming a full path
    /// is not asking to stay anywhere.)
    fn set_base(&mut self, src: &str) {
        self.root = self.origin.join(src).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    }

    /// Sequential: overlapping filesystem reads is not where the time goes, and a
    /// thread pool here would buy a dependency rather than latency. The batch
    /// shape exists for the face where it does matter (SPEC §10).
    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        paths.iter().map(|path| std::fs::read(self.root.join(path)).ok()).collect()
    }

    async fn supply_texture(&mut self, hash: u32, encoded: &[u8]) {
        self.supplied.insert(hash);
        self.images.push((hash, encoded.to_vec()));
    }

    /// Only what this load has already read.
    ///
    /// That much is safe to answer because a `FsLoader` lives for exactly one
    /// [`SSPlayer::load`], so it cannot go stale against a renderer it never saw —
    /// and it is worth answering, because the instance-pack walk asks once per pack
    /// and sibling packs routinely share an atlas (SPEC §7.1, §8.1).
    ///
    /// It stays `false` for anything the *renderer* already holds: the authoritative
    /// "already registered" check happens at the flush instead. The cost is that a
    /// second player of a reference-mode pack re-reads its atlas files before the
    /// flush discards them — file reads, not uploads or decodes.
    ///
    /// [`SSPlayer::load`]: crate::SSPlayer::load
    fn has_texture(&self, hash: u32) -> bool {
        self.supplied.contains(&hash)
    }

    fn warn(&self, message: &str) {
        eprintln!("[ssplayer-wgpu] {message}");
    }
}

/// Run a future that never actually waits.
///
/// [`AssetLoader`] is `async` because a browser leaves no choice; [`FsLoader`]
/// awaits nothing, so the first poll always completes and no executor is
/// warranted. The `unreachable!` is the honest version of that claim — it fires
/// only if this file grows a real await, which is the moment to reach for a
/// proper executor rather than to loop here.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RawWaker::new(core::ptr::null(), &VTABLE), |_| {}, |_| {}, |_| {});
    // SAFETY: every vtable entry is a no-op that touches none of the data
    // pointer, so the null pointer is never dereferenced.
    let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) };

    let mut future = core::pin::pin!(future);
    match future.as_mut().poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(out) => out,
        Poll::Pending => unreachable!("the filesystem loader awaited something; it needs a real executor now"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rooted_at(origin: &str) -> FsLoader {
        FsLoader { origin: origin.into(), ..FsLoader::new() }
    }

    /// Where a pack's siblings are looked for.
    #[test]
    fn the_base_becomes_the_packs_directory() {
        let mut loader = FsLoader::new();
        loader.set_base("overall/Instance.ssab");
        assert_eq!(loader.root, std::path::Path::new("overall"));

        // A bare name leaves it where it was: there is no directory to move into,
        // and the siblings will be bare too.
        let mut loader = FsLoader::new();
        loader.set_base("Instance.ssab");
        assert_eq!(loader.root, std::path::Path::new(""));
    }

    /// Derived from the root the loader was handed, so one rooted at a subtree —
    /// an engine adapter behind a `res://` prefix or an archive — stays inside it.
    #[test]
    fn the_base_stays_inside_the_root_it_started_from() {
        let mut loader = rooted_at("chars");
        loader.set_base("hero/hero.ssab");
        assert_eq!(loader.root, std::path::Path::new("chars/hero"));
    }

    /// And derived from that root *every time*, not folded into the last answer,
    /// so a loader survives being used for a second load.
    ///
    /// Getting this wrong is not theoretical: it accumulated, and the second
    /// `load_with` on a reused loader went looking in `chars/hero/hero` and failed.
    #[test]
    fn the_base_does_not_accumulate_across_loads() {
        let mut loader = rooted_at("chars");
        loader.set_base("hero/hero.ssab");
        loader.set_base("hero/hero.ssab");
        assert_eq!(loader.root, std::path::Path::new("chars/hero"), "set_base is not idempotent");

        loader.set_base("villain/villain.ssab");
        assert_eq!(loader.root, std::path::Path::new("chars/villain"), "the previous load's base was still in force");
    }
}
