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
    /// What every path is resolved against. Empty until the walk calls
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
        Self { root: std::path::PathBuf::new(), images: Vec::new(), supplied: std::collections::HashSet::new() }
    }
}

impl AssetLoader for FsLoader {
    /// `Path::parent` **is** the platform's answer to "which directory is this
    /// in", and that is the whole reason this method exists (SPEC §8.1).
    ///
    /// It cuts a native `\`, a `/`, the mix of the two that `read_dir` produces on
    /// Windows, and a `\\?\` verbatim prefix — each correctly, on the platform
    /// where that shape is meaningful, and none of them on the platforms where a
    /// backslash is simply a character a file is allowed to be named with. The
    /// walk used to do this itself by scanning for both separators, which got the
    /// Windows cases right by taking the POSIX one wrong.
    ///
    /// Joined onto the existing root rather than replacing it, so a loader handed
    /// a subtree stays inside it.
    fn set_base(&mut self, src: &str) {
        self.root = self.root.join(src).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
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

    /// Where a pack's siblings are looked for, and who decides.
    ///
    /// This replaces a test of the walk's own `parent_of`, which scanned `src` for
    /// `'/'` *and* `'\\'` because it had to serve every host with one answer. It
    /// does not any more: `Path::parent` is the answer on this face, `URL` is the
    /// answer on the web one, and each is right about its own vocabulary (SPEC
    /// §8.1). So the shapes worth asserting are the ones meaningful here.
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

    /// Joined onto the root already in force rather than replacing it, so a loader
    /// handed a subtree stays inside it — the shape an engine adapter rooted at a
    /// `res://` prefix or an archive needs.
    #[test]
    fn the_base_stays_inside_the_root_it_started_from() {
        let mut loader = FsLoader::new();
        loader.root = std::path::PathBuf::from("chars");
        loader.set_base("hero/hero.ssab");
        assert_eq!(loader.root, std::path::Path::new("chars/hero"));
    }

    /// Off Windows a backslash is a character a file may be named with, not a
    /// separator — so the base is the directory the file is really in.
    ///
    /// This is the case the string cut got wrong, silently: it took `overall/weird\`
    /// for a directory, every atlas resolved one level too deep, and the parts drew
    /// as white silhouettes with no error and no counter moving. Nothing about it
    /// is Windows-specific to get right; it is simply what `Path` already knows.
    #[cfg(not(windows))]
    #[test]
    fn a_backslash_is_part_of_the_name_here() {
        let mut loader = FsLoader::new();
        loader.set_base("overall/weird\\name.ssab");
        assert_eq!(loader.root, std::path::Path::new("overall"));
    }

    /// And on Windows the same call cuts every native shape, including the two the
    /// walk had to be taught by hand: `read_dir` joining a backslash onto a
    /// forward-slash root, and a `\\?\` verbatim path that carries no `'/'` at all.
    ///
    /// It compiles everywhere and runs only where those shapes mean anything, which
    /// is the trade the string version could not make. The Windows CI job builds
    /// without testing, so treat this as documentation that `Path` is load-bearing
    /// here rather than as a check that runs.
    #[cfg(windows)]
    #[test]
    fn the_base_cuts_every_native_shape_on_windows() {
        for (src, want) in [(r"overall\Instance.ssab", r"overall"), (r"U:/assets/overall\Basic.ssab", r"U:/assets/overall"), (r"\\?\U:\assets\overall\Basic.ssab", r"\\?\U:\assets\overall")] {
            let mut loader = FsLoader::new();
            loader.set_base(src);
            assert_eq!(loader.root, std::path::Path::new(want), "{src}");
        }
    }
}
