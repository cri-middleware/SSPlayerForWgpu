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
    root: std::path::PathBuf,
    /// What was read, in the order the walk asked for it; the player takes it.
    pub(crate) images: Vec<(u32, Vec<u8>)>,
}

impl FsLoader {
    pub(crate) fn new(root: impl AsRef<std::path::Path>) -> Self {
        Self { root: root.as_ref().to_path_buf(), images: Vec::new() }
    }
}

impl AssetLoader for FsLoader {
    /// Sequential: overlapping filesystem reads is not where the time goes, and a
    /// thread pool here would buy a dependency rather than latency. The batch
    /// shape exists for the face where it does matter (SPEC §10).
    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        paths.iter().map(|path| std::fs::read(self.root.join(path)).ok()).collect()
    }

    async fn supply_texture(&mut self, hash: u32, encoded: &[u8]) {
        self.images.push((hash, encoded.to_vec()));
    }

    /// Always false: this loader cannot see the renderer, so the authoritative
    /// "already registered" check happens at the flush instead. The cost is that
    /// a second player of a reference-mode pack re-reads its atlas files before
    /// the flush discards them — file reads, not uploads or decodes.
    fn has_texture(&self, _hash: u32) -> bool {
        false
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
