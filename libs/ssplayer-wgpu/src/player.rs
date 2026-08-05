//! Layer 3 (operation): SSPlayer — the primary public API.
//!
//! Owns a tree of runtime `Context`s (one per animation instance) and a single
//! shared `SSScene`. Borrow model: it holds no `Device`/`Queue`/
//! `Surface`; the host lends them on every `update`/`draw`.
//!
//! Instance parts reference a sub-animation in another `.ssab` pack. Each
//! instance child gets its **own** `Context` (hence its own SoA buffer), which
//! is how the single-buffer overwrite problem is avoided (see SPEC §7). The
//! host registers every pack via [`SSPlayer::add_ssab`]; external references are
//! resolved by pack name.

use crate::events::{SsEvent, decode_events};
use crate::frame::FrameView;
use crate::mask::{GeomKind, MaskOp, MaskWriter, PartMaskFlags, StencilTest, plan_writers};
use crate::pack::{EmbeddedImage, SSPack, TextureRef};
use crate::renderer::{Retention, SSRenderer};
use crate::scene::{BatchInfo, FrameChunk, NodeSlot};
use ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked};
use ssruntime::error::SSRuntimeError;
use ssruntime::runtime::framedata::ss::runtime::{DrawBatchKind, root_as_frame_data_unchecked};
use ssruntime::{Context, EffectSlot, InstanceSlot};
use std::collections::{HashMap, HashSet};

/// Column-major 4x4 identity.
const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// The three things the loading walk cannot do for itself: reach a file, decode
/// an image, and know what the renderer already holds (SPEC §8.2).
///
/// **This is the whole of the platform-specific part of loading.** Everything
/// else — reading both texture tables, deduplicating by file, walking instance
/// references transitively — is the same everywhere and lives in
/// [`SSPlayer::load_with`], so a face cannot quietly implement half of it. That is the
/// same line `SS6PlayerForCPP` draws between `Common/` and `Platform/`, and the
/// one `ssplayer-core` draws with `IResourceManager`.
///
/// Two implementations ship: the filesystem behind [`SSPlayer::load`], and the
/// web face's (`fetch` + `createImageBitmap`, whose pixels reach the GPU without
/// passing through wasm memory at all, SPEC §10). An engine integration writes a
/// third and gets the walk for free — which it must, because an exported Godot
/// project keeps its resources in a `.pck` and a Unity one in an APK, where no
/// `File::open` will reach them.
///
/// `async` because a browser leaves no choice; a native implementation simply
/// does not await anything, and its caller blocks on the future.
// No `Send` bound: the web implementation holds `Rc`, and nothing here is
// spawned onto another thread.
#[allow(async_fn_in_trait)]
pub trait AssetLoader {
    /// Read every path — **all of them at once**, so an implementation that can
    /// overlap the waiting does (the web one issues one `fetch` per path and
    /// settles them together). Results are positional; `None` is "this one did
    /// not arrive", which is never fatal to the rest.
    ///
    /// Paths arrive relative to whatever the caller passed [`SSPlayer::load_with`]
    /// as `src`, already joined against its directory — resolving that against a
    /// base directory or URL is the implementation's business.
    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>>;

    /// Bytes for an instance-referenced pack, **by the name it is referenced
    /// under** rather than by a path (SPEC §7.1).
    ///
    /// Tried before `<the .ssab's directory>/<name>.ssab`, and returning `None`
    /// — the default — falls back to exactly that. Implement it where a pack is
    /// addressable by name but not by path: an archive keyed by pack name, an
    /// engine's asset server, bytes a drop handler is already holding.
    /// `ssplayer-core` has the same hook for the same reason
    /// (`InstancePackProvider`, tried before its fetch).
    ///
    /// The alternative is to register the pack with [`SSPlayer::add_ssab`] before
    /// loading — the walk skips a pack it already has — but that means knowing
    /// which packs a `.ssab` references before reading it. This is for letting
    /// the walk ask instead.
    ///
    /// Called once per name, not batched: a provider is normally a lookup in
    /// something already in memory, and the batching exists for the reads that
    /// actually wait.
    async fn read_pack(&mut self, _name: &str) -> Option<Vec<u8>> {
        None
    }

    /// Decode `encoded` (a PNG, as the exporter wrote it) and register it under
    /// `hash` — the key a draw batch will ask for (SPEC §8.1).
    async fn supply_texture(&mut self, hash: u32, encoded: &[u8]);

    /// Whether an atlas is already registered for `hash` — a hint, used to skip
    /// reading a file nobody will need.
    ///
    /// Best effort: a loader that cannot see the renderer answers `false` and
    /// lets the flush make the real decision. The texture map is device-scoped
    /// (SPEC §1.2), so the authoritative check is the one that keeps N players of
    /// one pack to one decode and one upload between them.
    fn has_texture(&self, hash: u32) -> bool;

    /// Report a file that could not be loaded. One missing atlas or sub-pack
    /// costs you that one thing, not the character, so this is a diagnostic
    /// rather than an error — the same call `SS6PlayerForGodot` makes with
    /// `ERR_PRINT` and `ssplayer-core` with `console.warn`.
    fn warn(&self, message: &str);
}

/// What kind of thing [`SSPlayer::load`] could not get.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingKind {
    /// An atlas. The parts that use it draw against the 1x1 white fallback, so
    /// they appear as silhouettes rather than not at all (SPEC §8).
    Texture,
    /// An animation pack an instance part plays from. Those parts do not appear.
    Pack,
}

/// Something a load needed and did not get (SPEC §8.1).
///
/// A missing file is deliberately **not** an error: one unreadable atlas must not
/// cost you the character, so [`SSPlayer::load`] reports `Err` only when the
/// `.ssab` itself is unreadable. That leaves the caller unable to tell a complete
/// load from a partial one, which is what this answers — the human-facing half is
/// [`AssetLoader::warn`], and this is the half a program can branch on.
#[derive(Clone, Debug)]
pub struct MissingAsset {
    pub kind: MissingKind,
    /// What was asked for: a path for a texture, a pack name for a pack.
    pub name: String,
    /// For a texture, the key a draw batch will ask for. Zero for a pack, whose
    /// identity is its name (SPEC §7.1).
    pub hash: u32,
}

/// Which way the playhead walks the animation.
///
/// Deliberately a type of this crate rather than a re-export of the runtime's:
/// the runtime discriminates `Forward = 1 / Backward = -1` (it multiplies by
/// them), while every other face of every Player — the C API, the wasm build,
/// and so the sibling Players — uses `0 / 1`. Re-exporting would leak `-1` into
/// the `js` and `ffi` faces and silently disagree with all of them, so the wire
/// value is pinned here and converted at the runtime boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackDirection {
    Forward,
    Backward,
}

impl PlaybackDirection {
    /// The value crossing the `js` / `ffi` faces: `0` forward, `1` backward.
    pub fn as_wire(self) -> i32 {
        match self {
            Self::Forward => 0,
            Self::Backward => 1,
        }
    }

    /// Anything other than `1` reads as forward, matching the C API's own
    /// `direction == 0` test rather than rejecting unknown input.
    pub fn from_wire(value: i32) -> Self {
        if value == 1 { Self::Backward } else { Self::Forward }
    }
}

impl From<PlaybackDirection> for ssruntime::core::playback_controller::PlaybackDirection {
    fn from(value: PlaybackDirection) -> Self {
        match value {
            PlaybackDirection::Forward => Self::Forward,
            PlaybackDirection::Backward => Self::Backward,
        }
    }
}

impl From<ssruntime::core::playback_controller::PlaybackDirection> for PlaybackDirection {
    fn from(value: ssruntime::core::playback_controller::PlaybackDirection) -> Self {
        match value {
            ssruntime::core::playback_controller::PlaybackDirection::Forward => Self::Forward,
            ssruntime::core::playback_controller::PlaybackDirection::Backward => Self::Backward,
        }
    }
}

/// What happens when the playhead reaches the far end of the range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackStyle {
    /// Jump back to the start, or stop, according to the loop count.
    Normal,
    /// Turn around and walk back, so one loop is a full round trip.
    PingPong,
}

impl PlaybackStyle {
    /// See [`PlaybackDirection::as_wire`].
    pub fn as_wire(self) -> i32 {
        match self {
            Self::Normal => 0,
            Self::PingPong => 1,
        }
    }

    /// See [`PlaybackDirection::from_wire`].
    pub fn from_wire(value: i32) -> Self {
        if value == 1 { Self::PingPong } else { Self::Normal }
    }
}

impl From<PlaybackStyle> for ssruntime::core::playback_controller::PlaybackStyle {
    fn from(value: PlaybackStyle) -> Self {
        match value {
            PlaybackStyle::Normal => Self::Normal,
            PlaybackStyle::PingPong => Self::PingPong,
        }
    }
}

impl From<ssruntime::core::playback_controller::PlaybackStyle> for PlaybackStyle {
    fn from(value: ssruntime::core::playback_controller::PlaybackStyle) -> Self {
        match value {
            ssruntime::core::playback_controller::PlaybackStyle::Normal => Self::Normal,
            ssruntime::core::playback_controller::PlaybackStyle::PingPong => Self::PingPong,
        }
    }
}

/// How an override's colour combines with what is underneath — the same four
/// operations the authored PartColor offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlendOp {
    #[default]
    Mix,
    Mul,
    Add,
    Sub,
}

impl BlendOp {
    /// The value crossing the `js` / `ffi` faces, and what the runtime takes.
    pub fn as_wire(self) -> u8 {
        match self {
            Self::Mix => 0,
            Self::Mul => 1,
            Self::Add => 2,
            Self::Sub => 3,
        }
    }

    /// `None` for anything outside `0..=3`, so a bad value from a C or JS caller
    /// becomes a rejected call rather than a silently different blend.
    pub fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Mix),
            1 => Some(Self::Mul),
            2 => Some(Self::Add),
            3 => Some(Self::Sub),
            _ => None,
        }
    }
}

/// How long an override outlives the keyframes it beats.
///
/// Visibility has no priority: a force-hide always wins, and is always cleared
/// when a new animation is set up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverridePriority {
    /// Holds until the animation next drives that attribute, so it shows only
    /// on frames the animation leaves alone.
    OverwriteOnNextKeyframe,
    /// Holds for the current animation, and is cleared when another is set up.
    #[default]
    HoldUntilNextAnimation,
    /// Holds for as long as the same pack is playing, surviving animation
    /// changes.
    Permanent,
}

impl OverridePriority {
    /// See [`BlendOp::as_wire`].
    pub fn as_wire(self) -> u8 {
        match self {
            Self::OverwriteOnNextKeyframe => 0,
            Self::HoldUntilNextAnimation => 1,
            Self::Permanent => 2,
        }
    }

    /// See [`BlendOp::from_wire`].
    pub fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::OverwriteOnNextKeyframe),
            1 => Some(Self::HoldUntilNextAnimation),
            2 => Some(Self::Permanent),
            _ => None,
        }
    }
}

/// Everything in `path` up to the last `/`, with the separator — the directory a
/// pack's siblings are named relative to (SPEC §8.1).
///
/// Deliberately string surgery rather than `Path`: the same joins have to hold
/// for a URL, and `'/'` is the separator in both worlds (Windows accepts it too).
fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(cut) => path[..=cut].to_string(),
        None => String::new(),
    }
}

/// Refill `out` with the timeline events the node's last update crossed.
///
/// Cleared first, so it always describes the tick that just ran and never
/// accumulates. Bails before touching the `.ssab` when the runtime reports
/// nothing, which is the overwhelmingly common case — a frame with no events
/// costs one integer read.
fn collect_events(node: &PlaybackNode, animation: Option<&str>, out: &mut Vec<SsEvent>) {
    out.clear();
    let indices = node.context.get_passed_event_indices();
    if indices.is_empty() {
        return;
    }
    let (Some(animation), Some(ssab_bytes)) = (animation, node.context.get_ssab_data()) else {
        return;
    };
    // SAFETY: the same buffer the node was built from, already validated there.
    let ab = unsafe { root_as_ss_anime_binary_unchecked(ssab_bytes) };
    let Some(anim) = ab.animations().iter().find(|a| a.name() == animation) else {
        return;
    };
    decode_events(&anim, indices, out);
}

/// Guards against cyclic instance references blowing the stack.
const MAX_INSTANCE_DEPTH: u32 = 12;

/// One node in the playback tree: a runtime context plus its instance children
/// and effect slots.
struct PlaybackNode {
    context: Context<'static>,
    /// Instance children keyed by part index (index into parts/draw_order/SoA).
    instances: HashMap<usize, InstanceChild>,
    /// Effect slots keyed by part index. Effects reuse this node's context
    /// (no separate context, unlike instances).
    effects: HashMap<usize, EffectPart>,
    /// Static per-part mask flags, indexed by part index. Read once from ssab
    /// `PartData` at build time (SPEC §6). Empty when the pack has no ssab data.
    mask_flags: Vec<PartMaskFlags>,
}

/// One instance part: its slot, the sub-animation it drives, and what the last
/// [`step_slots`] left behind for the draw walk to use.
struct InstanceChild {
    slot: InstanceSlot,
    node: Box<PlaybackNode>,
    /// Frame the slot last left the child on. Read raw — rounding it and writing
    /// it back would truncate sub-frame progress every tick (instance doc).
    frame_no: f32,
    /// Whether the slot says the child is showing this frame.
    visible: bool,
}

/// One effect part: its slot, and the draw plan the last [`step_slots`] produced.
struct EffectPart {
    slot: EffectSlot<'static>,
    /// This frame's plan, empty when the effect produced nothing.
    ///
    /// Copied rather than borrowed: the buffer behind `step`'s result belongs to
    /// the slot, and the draw walk that consumes it runs after every slot has
    /// stepped. Keeping the raw pointer instead would additionally cost
    /// `SSPlayer: Send` — which `SSScene::update` no longer needs now that its
    /// walk is serial again (SPEC §14.2 item 6), but a host that moves players
    /// between its own threads does. The allocation is reused frame to frame, so
    /// the price is one memcpy per effect.
    plan: Vec<u8>,
}

/// The SpriteStudio player. One instance owns one animation-instance tree.
pub struct SSPlayer {
    /// All registered packs, keyed by pack name (matches `ref_anime_pack`).
    /// Cloning a pack shares its parse and its bytes (SPEC §8.2), so N players of
    /// one character cost one copy of the `.ssab` between them.
    packs: HashMap<String, SSPack>,
    /// Packs that were replaced under a name the live tree may still be playing
    /// from, kept alive until the tree is rebuilt.
    ///
    /// [`build_node`] binds each `Context` to a resource by **raw pointer**, and
    /// the `Arc` keeping that resource alive lives in `packs` — so re-registering
    /// a name (`add_pack` / `add_ssab`) used to free the very resource the tree
    /// was still evaluating. Retiring the old pack here is what keeps
    /// `resource_static_ptr`'s contract ("keep some `Arc` alive for as long as
    /// the pointer is in use"); the tree goes on playing the data it was built
    /// from, and [`SSPlayer::set_animation`] — the one call that rebuilds it —
    /// releases these.
    retired: Vec<SSPack>,
    main_pack: Option<String>,
    root: Option<PlaybackNode>,
    current_animation: Option<String>,
    /// Names of packs whose embedded atlases (SPEC §8.1) have not been offered to
    /// a renderer yet. The offer happens on the next `update`, which is the
    /// earliest point the borrow model lends a `Device`/`Queue`; the decode itself
    /// waits until then too, so a pack whose atlas is already registered is never
    /// decoded at all.
    packs_awaiting_textures: Vec<String>,
    /// Atlases a host-side load read but has not handed to a renderer yet, still
    /// **encoded** (SPEC §8.1).
    ///
    /// The reference-mode twin of `packs_awaiting_textures`, and deliberately the
    /// same mechanism: [`SSPlayer::load`] borrows no `Device`/`Queue` — the borrow
    /// model gives it none until `update` — so what it reads waits here for the
    /// same flush that decodes the embedded ones. Encoded rather than decoded
    /// because a 2048² atlas is a few hundred KB here against 16 MB of pixels,
    /// and because it puts both routes on one timeline.
    pending_images: Vec<(u32, Vec<u8>)>,
    /// What the last load could not get (SPEC §8.1). Reset by each
    /// [`SSPlayer::load_with`], appended to as the walk gives up on a file.
    missing: Vec<MissingAsset>,
    /// Timeline events the last `build_frame` crossed. Cleared and refilled per
    /// update, and reused rather than reallocated — a frame with no events costs
    /// nothing beyond the `passed_event_count` read.
    events: Vec<SsEvent>,
    /// Whether the last update crossed a loop boundary. A pulse the runtime
    /// clears on entry to every update, so it is captured there and mirrored
    /// here for the host to read afterwards.
    looped: bool,
    /// Whether the last update *completed* playback — the rising edge of the
    /// runtime's sticky `is_finished`, not the flag itself.
    finished_edge: bool,
    /// The latch that makes `finished_edge` an edge.
    ///
    /// Held explicitly, as the SDK's Player porting guide asks: the alternative
    /// is to lean on some earlier `is_playing` guard, which works until someone
    /// moves the guard. The guide lists "notifying `is_finished` directly"
    /// (fires every frame afterwards) and "not resetting the latch in `play`"
    /// (the *next* completion goes missing) as defects that really happened in
    /// sibling Players.
    prev_finished: bool,
    /// Where this player sits in the scene — the root transform of its tree
    /// (SPEC §5.4). Composed by [`SSPlayer::set_transform`]; only the matrix is
    /// kept because nothing yet re-composes it. A future `box` (§9.3, the
    /// WebGL player's viewport-tracking rect) moves after `set_transform` runs,
    /// so it will need the authored parameters retained as well.
    transform: [f32; 16],
    /// Draw order within a scene; lower draws first. Nothing in this file reads
    /// it — it exists so the host, and from P7-4 `SSScene`, can sort by it.
    order: i32,
    /// Skip this player entirely when false (see [`SSPlayer::set_visible`]).
    visible: bool,
    /// Player-level fade folded into every vertex (see [`BatchInfo::alpha`]).
    alpha: f32,
}

impl Default for SSPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl SSPlayer {
    /// Create a player. Takes nothing: a player holds no GPU resource at all
    /// (SPEC §1.2), which is what makes putting a hundred of them in a scene
    /// cheap. The device-scoped [`SSRenderer`] and the target-scoped
    /// [`SSScene`] are lent to it per frame by [`SSScene::update`].
    pub fn new() -> Self {
        Self {
            packs: HashMap::new(),
            retired: Vec::new(),
            main_pack: None,
            root: None,
            current_animation: None,
            packs_awaiting_textures: Vec::new(),
            pending_images: Vec::new(),
            missing: Vec::new(),
            events: Vec::new(),
            looped: false,
            finished_edge: false,
            prev_finished: false,
            transform: IDENTITY,
            order: 0,
            visible: true,
            alpha: 1.0,
        }
    }

    /// Re-offer this player's embedded atlases after a device loss (SPEC §14).
    ///
    /// The player itself has nothing to rebuild — it owns no GPU resource. What it
    /// does own is the `.ssab`, which is the only place a [`Retention::Reproducible`]
    /// texture can come back from, so the host calls this once per player after
    /// [`SSRenderer::restore_device`] and [`SSScene::restore_device`].
    ///
    /// Playback state (packs, the instance tree, the current frame) is CPU-side and
    /// was never affected: the animation resumes where it was interrupted.
    pub fn restore_textures(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.packs_awaiting_textures = self.packs.values().filter(|p| p.has_embedded_textures()).map(|p| p.name().to_string()).collect();
        self.flush_textures(renderer, device, queue);
    }

    /// Load a `.ssab` **and everything it depends on**, then select it
    /// (SPEC §8.1, §7.1).
    ///
    /// One call is the whole of loading: the pack, both of its texture tables in
    /// either export mode, and — transitively — the sibling packs its instance
    /// parts play from, with their textures. Nothing is left for the caller to
    /// discover and fetch by hand, which is the point: a host that has to walk
    /// `textures()` itself will sooner or later miss `external_textures()` and
    /// ship white silhouettes.
    ///
    /// `src` is where the pack is, in whatever terms `loader` understands — a
    /// path for [`SSPlayer::load`], a URL for the web face. Everything the
    /// pack names is resolved against its **directory**, because that is what the
    /// exporter wrote the paths relative to.
    ///
    /// A pack keeps the name inside the `.ssab`. That is the name instance
    /// references are matched against (`ref_anime_pack`), and the same string is
    /// the sibling file's stem, which is what makes those siblings findable at
    /// all — the rule `SS6PlayerForGodot`'s `_load_external_ssabs` and
    /// `ssplayer-core`'s `resolveInstancePacks` also follow. Use
    /// [`SSPlayer::load_ssab`] to name a pack yourself, and expect external
    /// references to it to stop resolving.
    ///
    /// Only the pack itself is worth an error: everything else is reported
    /// through [`AssetLoader::warn`] and skipped, so one unreadable atlas does
    /// not cost you the character.
    ///
    /// [`SSPlayer::load`] is this call with the filesystem plugged in; reach for
    /// this one when the bytes live somewhere a `File::open` cannot reach — a
    /// `.pck`, an APK, an engine's asset server, a URL.
    ///
    /// [`SSPlayer::load`]: crate::SSPlayer::load
    pub async fn load_with(&mut self, loader: &mut impl AssetLoader, src: &str) -> Result<(), SSRuntimeError> {
        // This load's tally, not the player's history: a caller checks it against
        // the call it just made.
        self.missing.clear();
        let Some(bytes) = loader.read(std::slice::from_ref(&src.to_owned())).await.into_iter().next().flatten() else {
            loader.warn(&format!("could not read {src}"));
            return Err(SSRuntimeError::NotLoaded);
        };
        let pack = SSPack::load(&bytes)?;
        let name = pack.name().to_string();
        self.set_pack(&pack);
        self.load_assets(loader, &name, &parent_of(src)).await;
        Ok(())
    }

    /// Load a `.ssab` off the filesystem **and everything it depends on**, then
    /// select it (SPEC §8.1, §7.1).
    ///
    /// The whole of loading, in one call and with no wgpu state in sight:
    ///
    /// ```no_run
    /// # use ssplayer_wgpu::SSPlayer;
    /// # fn go() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut player = SSPlayer::new();
    /// player.load("assets/overall/Instance.ssab")?;
    /// player.set_animation("anime_1")?;
    /// player.play();
    /// # Ok(()) }
    /// ```
    ///
    /// It borrows no `Device` or `Queue` because the borrow model lends none
    /// until the frame starts: what it reads is decoded and uploaded by the first
    /// `SSScene::update`, on exactly the timeline an embedded atlas already
    /// takes. So nothing is on the GPU when this returns, and nothing needs to be.
    ///
    /// `path` is resolved by the process's working directory; everything the pack
    /// names — its atlases, the sibling packs its instance parts play from — is
    /// resolved against **the `.ssab`'s own directory**, because that is what the
    /// exporter wrote those paths relative to.
    ///
    /// Use [`SSPlayer::load_with`] where a `File::open` cannot reach the bytes: a
    /// Godot `.pck`, an APK, an engine's asset server, a browser (SPEC §8.2).
    #[cfg(all(feature = "embedded", not(target_arch = "wasm32")))]
    pub fn load(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), SSRuntimeError> {
        let path = path.as_ref();
        let mut loader = crate::assets::FsLoader::new("");
        let out = crate::assets::block_on(self.load_with(&mut loader, &path.to_string_lossy()));
        // Queued rather than uploaded: see the note on `pending_images`.
        self.pending_images.append(&mut loader.images);
        out
    }

    /// The half of [`SSPlayer::load`] that runs on an **already registered**
    /// pack: its atlases, and the packs it plays from, transitively.
    ///
    /// For a host that got the bytes some other way — a drop, a bundler, an
    /// archive — and registered them with [`SSPlayer::load_ssab`]. `base` is what
    /// the pack's relative paths resolve against, in the loader's terms; pass
    /// `""` when the loader is already rooted there.
    pub async fn load_assets(&mut self, loader: &mut impl AssetLoader, pack_name: &str, base: &str) {
        let Some(pack) = self.packs.get(pack_name).cloned() else {
            loader.warn(&format!("no pack named \"{pack_name}\" is registered"));
            return;
        };
        self.load_textures(loader, &pack, base).await;

        // Breadth first, one round trip per level: `read` takes the whole level
        // at once so an implementation that can overlap the waiting does. A pack
        // already registered is not fetched again, which is also what stops two
        // packs referencing each other from looping.
        let mut queue = pack.instance_packs();
        while !queue.is_empty() {
            let mut wanted: Vec<String> = queue.drain(..).filter(|name| !self.has_pack(name)).collect();
            wanted.sort();
            wanted.dedup();
            if wanted.is_empty() {
                break;
            }

            // The provider first, for packs that have a name but no path
            // (`read_pack`); whatever it declines is looked for as a sibling file.
            let mut provided: Vec<Option<Vec<u8>>> = Vec::with_capacity(wanted.len());
            for name in &wanted {
                provided.push(loader.read_pack(name).await);
            }
            let paths: Vec<String> = wanted.iter().map(|name| format!("{base}{name}.ssab")).collect();
            let unprovided: Vec<String> = paths.iter().zip(&provided).filter(|(_, bytes)| bytes.is_none()).map(|(path, _)| path.clone()).collect();
            let mut fetched = loader.read(&unprovided).await.into_iter();
            let fetched: Vec<Option<Vec<u8>>> = provided.into_iter().map(|bytes| if bytes.is_some() { bytes } else { fetched.next().flatten() }).collect();

            for ((name, path), bytes) in wanted.iter().zip(&paths).zip(fetched) {
                let Some(bytes) = bytes else {
                    loader.warn(&format!("instance pack \"{name}\" ({path}) did not load; parts that play it will not appear"));
                    self.missing.push(MissingAsset { kind: MissingKind::Pack, name: name.clone(), hash: 0 });
                    continue;
                };
                // Never renamed: `ref_anime_pack` is matched against the name the
                // exporter wrote, so an override here would be a reference that
                // resolves to nothing.
                match SSPack::load(&bytes) {
                    Ok(sub) => {
                        self.add_pack(&sub);
                        self.load_textures(loader, &sub, base).await;
                        queue.extend(sub.instance_packs());
                    }
                    Err(e) => {
                        loader.warn(&format!("instance pack \"{name}\" ({path}) is not a valid .ssab: {e:?}"));
                        self.missing.push(MissingAsset { kind: MissingKind::Pack, name: name.clone(), hash: 0 });
                    }
                }
            }
        }
    }

    /// Supply every atlas `pack` asks for that the renderer does not already
    /// hold — both texture tables, either export mode (SPEC §8.1).
    ///
    /// Embedded images come out of the pack; the rest are read through `loader`,
    /// **once per file**: `external_textures()` can name an image a cellmap
    /// already named, under a different hash, and that must not become two reads.
    async fn load_textures(&mut self, loader: &mut impl AssetLoader, pack: &SSPack, base: &str) {
        let wanted: Vec<TextureRef> = pack.textures().iter().filter(|t| !loader.has_texture(t.hash)).cloned().collect();
        if wanted.is_empty() {
            return;
        }

        let carried: HashMap<u32, Vec<u8>> = pack.embedded_images().into_iter().filter(|i| wanted.iter().any(|t| t.hash == i.hash)).map(|i| (i.hash, i.bytes.to_vec())).collect();
        let mut paths: Vec<String> = wanted.iter().filter(|t| !carried.contains_key(&t.hash)).map(|t| format!("{base}{}", t.path)).collect();
        paths.sort();
        paths.dedup();
        let fetched: HashMap<&String, Option<Vec<u8>>> = paths.iter().zip(loader.read(&paths).await).collect();

        for texture in &wanted {
            let joined = format!("{base}{}", texture.path);
            let bytes = match carried.get(&texture.hash) {
                Some(bytes) => bytes.as_slice(),
                None => match fetched.get(&joined).and_then(|b| b.as_deref()) {
                    Some(bytes) => bytes,
                    None => {
                        loader.warn(&format!("atlas {joined} did not load; parts that use it will draw white"));
                        self.missing.push(MissingAsset { kind: MissingKind::Texture, name: joined, hash: texture.hash });
                        continue;
                    }
                },
            };
            loader.supply_texture(texture.hash, bytes).await;
        }
    }

    /// What the last load could not get, in the order it gave up (SPEC §8.1).
    /// Empty means everything the packs asked for arrived.
    ///
    /// A load reports `Err` only when the `.ssab` itself is unreadable, because
    /// one missing atlas must not cost you the character. This is how a caller
    /// tells "complete" from "played anyway": check it right after `load`, and
    /// decide for itself whether a silhouette is worth failing over.
    ///
    /// ```no_run
    /// # use ssplayer_wgpu::SSPlayer;
    /// # fn go(player: &mut SSPlayer) -> Result<(), Box<dyn std::error::Error>> {
    /// player.load("assets/overall/Instance.ssab")?;   // the pack was readable …
    /// for missing in player.missing_assets() {        // … but maybe not all of it
    ///     eprintln!("{:?} {} is not there", missing.kind, missing.name);
    /// }
    /// # Ok(()) }
    /// ```
    ///
    /// Reset by every [`SSPlayer::load_with`], so it always describes the most
    /// recent call rather than the player's history.
    pub fn missing_assets(&self) -> &[MissingAsset] {
        &self.missing
    }

    /// Whether a pack is registered under `name` — what an instance part's
    /// `ref_anime_pack` will be looked up by (SPEC §7.1).
    ///
    /// For a host resolving those references itself: it answers "have I already
    /// fetched this one", which is what keeps a cycle of packs referencing each
    /// other from being walked forever.
    pub fn has_pack(&self, name: &str) -> bool {
        self.packs.contains_key(name)
    }

    /// Register a pack for external instance references (SPEC §7.1) without
    /// changing what is playing. Registration is by [`SSPack::name`].
    ///
    /// Sharing one pack between players is the intended shape (SPEC §8.2): the
    /// clone is an `Arc` bump, and the atlas reaches the GPU once no matter how
    /// many players hold it.
    ///
    /// Re-registering a name a live animation is playing from does **not** change
    /// what is playing: the tree keeps the resource it was built from until the
    /// next [`SSPlayer::set_animation`], because its `Context`s point at that
    /// resource directly (see `retired`).
    pub fn add_pack(&mut self, pack: &SSPack) {
        let name = pack.name().to_string();
        if pack.has_embedded_textures() {
            self.packs_awaiting_textures.push(name.clone());
        }
        // Only while a tree exists is there anything pointing into the pack being
        // replaced; without one the old pack is free to go here and now.
        if let Some(previous) = self.packs.insert(name, pack.clone())
            && self.root.is_some()
        {
            self.retired.push(previous);
        }
    }

    /// Register `pack` and make it the one [`SSPlayer::set_animation`] selects
    /// from. Clears the current animation.
    pub fn set_pack(&mut self, pack: &SSPack) {
        self.add_pack(pack);
        self.main_pack = Some(pack.name().to_string());
        self.root = None;
        self.current_animation = None;
        // Nothing points into a retired pack once the tree is gone.
        self.retired.clear();
    }

    /// Parse `data` and register it under `pack_name` — [`SSPack::load_named`] +
    /// [`SSPlayer::add_pack`] in one call, for hosts holding bytes rather than
    /// packs (the C and js faces, and anything naming packs after files).
    pub fn add_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError> {
        let pack = SSPack::load_named(pack_name, data)?;
        self.add_pack(&pack);
        Ok(())
    }

    /// [`SSPlayer::add_ssab`] + [`SSPlayer::set_pack`]: register `pack_name` and
    /// make it the main pack to play from.
    pub fn load_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError> {
        let pack = SSPack::load_named(pack_name, data)?;
        self.set_pack(&pack);
        Ok(())
    }

    /// Select the animation to play, (re)building the instance tree. The tree is
    /// left at its start frame and **not** playing — call [`SSPlayer::play`].
    ///
    /// This is the allocating half of playback (contexts, slots, mask flags), so
    /// the start/stop calls below can stay allocation-free. Mirrors the runtime's
    /// own `setup_animation` + `play` split.
    pub fn set_animation(&mut self, anim_name: &str) -> Result<(), SSRuntimeError> {
        let main_pack = self.main_pack.clone().ok_or(SSRuntimeError::NotLoaded)?;
        let root = build_node(&self.packs, &main_pack, anim_name, 0).ok_or(SSRuntimeError::NotLoaded)?;
        // Assigning drops the previous tree, and with it the last raw pointer into
        // any pack a re-registration retired — so they can go now. The new tree was
        // built from `packs`, which is where a replacement went.
        self.root = Some(root);
        self.retired.clear();
        self.current_animation = Some(anim_name.to_string());
        // Nothing that happened under the previous animation describes this one.
        self.events.clear();
        self.looped = false;
        self.finished_edge = false;
        self.prev_finished = false;
        Ok(())
    }

    /// Start — or, after [`SSPlayer::pause`], resume — the selected animation.
    /// No-op until [`SSPlayer::set_animation`] has succeeded.
    pub fn play(&mut self) {
        // Re-arm the completion latch. Without this, replaying an animation that
        // already finished would compare against a `true` left over from the
        // previous run and the *next* completion would never be reported — one of
        // the defects the SDK's porting guide records from sibling Players.
        self.prev_finished = false;
        if let Some(root) = &mut self.root {
            root.context.play();
        }
    }

    pub fn pause(&mut self) {
        if let Some(root) = &mut self.root {
            root.context.pause();
        }
    }

    pub fn stop(&mut self) {
        if let Some(root) = &mut self.root {
            root.context.stop();
        }
    }

    pub fn set_speed(&mut self, speed: f32) {
        if let Some(root) = &mut self.root {
            root.context.set_animation_speed(speed);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_playing()).unwrap_or(false)
    }

    /// Held in place rather than stopped: [`SSPlayer::is_playing`] stays true.
    pub fn is_paused(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_pausing()).unwrap_or(false)
    }

    /// Every configured loop has played. Sticky until the next play or
    /// [`SSPlayer::set_animation`], and never true under an infinite loop.
    pub fn is_finished(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_finished()).unwrap_or(false)
    }

    /// The last [`SSScene::update`] crossed a loop boundary. An edge, cleared at
    /// the top of every update, so it only reads true inside the tick that
    /// crossed.
    ///
    /// [`SSScene::update`]: crate::SSScene::update
    pub fn is_looped(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_looped()).unwrap_or(false)
    }

    // --- Playhead ----------------------------------------------------------
    //
    // Everything below reads or writes the ROOT context only. Instance children
    // run their own clocks, driven by their slots from the root's frame (SPEC
    // §7.1), so seeking the root carries them with it on the next update.

    /// Jump to `frame`. Fractional values are honoured — the tree is evaluated
    /// at the sub-frame position on the next update.
    pub fn seek(&mut self, frame: f32) {
        if let Some(root) = &mut self.root {
            root.context.set_frame_no(frame);
        }
    }

    /// Current playhead position, fractional. Zero before an animation is set.
    pub fn frame_no(&self) -> f32 {
        self.root.as_ref().map(|r| r.context.get_frame_no()).unwrap_or(0.0)
    }

    /// First and last frame of the range being played, and its length
    /// (`end - start + 1`, both ends inclusive). All zero before an animation is
    /// set. These follow [`SSPlayer::set_animation_section`], so they describe
    /// what the transport actually walks rather than what the pack contains.
    pub fn start_frame(&self) -> i32 {
        self.root.as_ref().map(|r| r.context.get_start_frame()).unwrap_or(0)
    }

    /// See [`SSPlayer::start_frame`].
    pub fn end_frame(&self) -> i32 {
        self.root.as_ref().map(|r| r.context.get_end_frame()).unwrap_or(0)
    }

    /// See [`SSPlayer::start_frame`].
    pub fn total_frames(&self) -> i32 {
        match &self.root {
            Some(root) => root.context.get_end_frame() - root.context.get_start_frame() + 1,
            None => 0,
        }
    }

    // --- Playback shape ----------------------------------------------------
    //
    // NOTE: `set_animation` rebuilds the tree from scratch, so it resets every
    // setting here (and `set_speed`) to the pack's defaults. Re-apply whatever
    // you chose after each call. Caching them on the player, as the Flutter and
    // Unity Players do, is deliberately left out of this layer for now — see the
    // note in SPEC §9.3.

    /// `count` cycles then stop; `-1` (or `0`) loops forever. Under
    /// [`PlaybackStyle::PingPong`] one cycle is a full round trip.
    pub fn set_loop_count(&mut self, count: i32) {
        if let Some(root) = &mut self.root {
            root.context.set_loop(count);
        }
    }

    /// The configured count, and how many cycles are left of it.
    pub fn loop_count(&self) -> i32 {
        self.root.as_ref().map(|r| r.context.get_loops()).unwrap_or(0)
    }

    /// See [`SSPlayer::loop_count`].
    pub fn remaining_loops(&self) -> i32 {
        self.root.as_ref().map(|r| r.context.get_remain_loops()).unwrap_or(0)
    }

    /// Override the advance rate. The pack's authored rate otherwise — prefer
    /// [`SSPlayer::set_speed`], which scales playback without replacing it.
    pub fn set_frame_rate(&mut self, fps: i32) {
        if let Some(root) = &mut self.root {
            root.context.set_frame_rate(fps);
        }
    }

    /// See [`SSPlayer::set_frame_rate`]. Zero before an animation is set.
    pub fn frame_rate(&self) -> i32 {
        self.root.as_ref().map(|r| r.context.get_fps()).unwrap_or(0)
    }

    /// Restrict playback to `start..=end` of the current animation, so looping
    /// and ping-pong turn around inside that range. Reflected by
    /// [`SSPlayer::start_frame`] / [`SSPlayer::end_frame`].
    ///
    /// Call it *after* [`SSPlayer::set_animation`]: frame numbers only mean
    /// something inside the animation they were measured in, and setting an
    /// animation rebuilds the tree and takes the section with it.
    pub fn set_animation_section(&mut self, start: i32, end: i32) {
        if let Some(root) = &mut self.root {
            root.context.set_animation_section(start, end);
        }
    }

    /// Which way the playhead walks, and what it does at the far end. Taken
    /// together because the runtime sets them as a pair — pass the current value
    /// for whichever you are not changing.
    pub fn set_playback_direction(&mut self, direction: PlaybackDirection, style: PlaybackStyle) {
        if let Some(root) = &mut self.root {
            root.context.set_playback_direction(direction.into(), style.into());
        }
    }

    /// See [`SSPlayer::set_playback_direction`].
    pub fn playback_direction(&self) -> PlaybackDirection {
        self.root.as_ref().map(|r| r.context.get_playback_direction().into()).unwrap_or(PlaybackDirection::Forward)
    }

    /// See [`SSPlayer::set_playback_direction`].
    pub fn playback_style(&self) -> PlaybackStyle {
        self.root.as_ref().map(|r| r.context.get_playback_style().into()).unwrap_or(PlaybackStyle::Normal)
    }

    /// How a long tick is spent. On (the default) advances by the elapsed time
    /// and drops the frames in between, keeping playback on the wall clock; off
    /// advances exactly one frame per update, so nothing is skipped but a
    /// stalled host plays in slow motion.
    pub fn set_skip_frames(&mut self, skip: bool) {
        if let Some(root) = &mut self.root {
            root.context.set_skip_frames(skip);
        }
    }

    /// See [`SSPlayer::set_skip_frames`].
    pub fn skip_frames(&self) -> bool {
        self.root.as_ref().map(|r| r.context.get_skip_frames()).unwrap_or(true)
    }

    // --- Per-part overrides (SPEC §9.5) -------------------------------------
    //
    // Say "make this part this colour / this cell / hidden **now**", beating the
    // keyframes underneath. The runtime owns the override table and its whole
    // lifecycle, so these are one-shot calls: never drive them from a per-frame
    // loop, and expect nothing to be re-applied for you.
    //
    // Two things to know before reaching for them:
    //
    // * Part indices address the ROOT animation, and are stable within one pack.
    //   Resolve a name once with [`SSPlayer::find_part_index`] and keep the
    //   index if you override often.
    // * **Overrides do not reach inside an instance part.** Its child animation
    //   runs as its own context (SPEC §7.1), so an override on the part that
    //   *hosts* an instance does not touch what the instance draws.
    //
    // Every call needs an animation: the runtime sizes its override table in
    // `set_animation`, which is also what clears everything but
    // [`OverridePriority::Permanent`]. Overriding before selecting an animation
    // is therefore both rejected and pointless — override after, never before.

    /// Resolve a part name to its index in the current animation, or `None` when
    /// the name is unknown or no animation is set up.
    pub fn find_part_index(&self, name: &str) -> Option<u32> {
        let root = self.root.as_ref()?;
        match root.context.find_part_index(name) {
            index if index >= 0 => Some(index as u32),
            _ => None,
        }
    }

    /// Force-hide a part, or revert it to the animation.
    ///
    /// `cascade` propagates a force-hide to the part's descendants. A keyframe
    /// `hide` never cascades, so this is the only way to hide a whole sub-tree.
    /// Visibility has no priority mode: it always beats the keyframe, and is
    /// always cleared when a new animation is set up.
    pub fn set_part_visibility_override(&mut self, part: u32, hide: bool, cascade: bool) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_part_visibility_override(part, u8::from(hide), cascade)
    }

    /// Revert a part to the animation's own visibility.
    pub fn clear_part_visibility_override(&mut self, part: u32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.clear_part_visibility_override(part)
    }

    /// Override a part's colour with one value across all four corners.
    ///
    /// `rgba` is packed `0xRRGGBBAA` in the same 8-bit sRGB space as the
    /// authored PartColor: the runtime multiplies its alpha with the part's
    /// composed alpha itself, so do not pre-multiply or gamma-convert it here.
    ///
    /// Normal parts only. Others ignore it silently — `Ok` means the override
    /// was recorded, not that anything looks different.
    pub fn set_part_color_override(&mut self, part: u32, blend_op: BlendOp, rgba: u32, priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_part_color_override(part, blend_op.as_wire(), rgba, priority.as_wire())
    }

    /// Override a part's colour with four distinct corners, for a gradient.
    ///
    /// Same contract as [`SSPlayer::set_part_color_override`]. `corners` is in
    /// the runtime's order — **left-top, right-top, left-bottom, right-bottom**,
    /// each `[r, g, b, a]` — and getting that order wrong flips the gradient
    /// silently rather than failing.
    pub fn set_part_color_override_corners(&mut self, part: u32, blend_op: BlendOp, corners: [[u8; 4]; 4], priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_part_color_override_corners(part, blend_op.as_wire(), corners, priority.as_wire())
    }

    /// Revert a part to the animation's own colour. Clears either kind of colour
    /// override — they share one slot per part.
    pub fn clear_part_color_override(&mut self, part: u32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.clear_part_color_override(part)
    }

    /// Make a part draw a different cell.
    ///
    /// `cellmap_name` is written without the `.ssce` extension. Both names are
    /// hashed here, so pass them exactly as they appear in the pack. Normal and
    /// Mask parts only; errors when the cell cannot be resolved against the
    /// bound `.ssab`.
    pub fn set_part_cell_override(&mut self, part: u32, cellmap_name: &str, cell_name: &str, priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        let cellmap_hash = ssruntime::fnv1a_hash_str(cellmap_name);
        let cell_hash = ssruntime::fnv1a_hash_str(cell_name);
        root.context.set_part_cell_override(part, cellmap_hash, cell_hash, priority.as_wire())
    }

    /// Revert a part to the animation's own cell.
    pub fn clear_part_cell_override(&mut self, part: u32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.clear_part_cell_override(part)
    }

    /// Drop every override on the root animation at once.
    pub fn clear_all_part_overrides(&mut self) {
        if let Some(root) = &mut self.root {
            root.context.clear_all_part_overrides();
        }
    }

    // --- Events (SPEC §9.5) -------------------------------------------------
    //
    // Polled, not called back. The borrow model has the host lending the player
    // its device per frame and driving the clock, so "read what happened after
    // update" is the shape that already fits — and it is the only shape that
    // survives being wrapped by both `js` and `ffi` without becoming a different
    // API on each.
    //
    // All three read the tick that `SSScene::update` just ran, and are replaced
    // by the next one.

    /// Whether the last update crossed a loop boundary.
    ///
    /// One report per update however many cycles were crossed, since a long tick
    /// under frame skipping can complete several. Use it for "the walk cycle came
    /// round"; use [`SSPlayer::finished_last_update`] for "the animation is over".
    ///
    /// [`SSScene::update`]: crate::SSScene::update
    pub fn looped_last_update(&self) -> bool {
        self.looped
    }

    /// Whether the last update **completed** playback: the rising edge of the
    /// runtime's sticky finished state, so it reads true for exactly one update.
    ///
    /// Never true under an infinite loop, and **not raised by
    /// [`SSPlayer::stop`]** — it means "every configured cycle has played", not
    /// "playback is no longer running". Counting loops, or calling `stop`
    /// yourself, is how you end an infinite one.
    ///
    /// The related trap is deriving completion from `!is_playing` or from
    /// reaching the last frame; both are wrong in ways the SDK's porting guide
    /// records from sibling Players, which is why neither is offered here.
    pub fn finished_last_update(&self) -> bool {
        self.finished_edge
    }

    /// Timeline events the last update crossed, in the order the runtime
    /// reported the frames carrying them.
    ///
    /// **Frames skipped over are still represented.** The runtime records every
    /// frame it stepped through, so a footstep on a frame that a slow tick
    /// jumped past still appears here — which it would not if the host looked
    /// only at the frame that was landed on.
    ///
    /// Empty on a tick that crossed nothing, and on a player that is not
    /// [`visible`](SSPlayer::set_visible), which does not advance at all.
    pub fn events(&self) -> &[SsEvent] {
        &self.events
    }

    // --- Part read-outs (SPEC §9.5) ----------------------------------------

    /// How many parts the root animation has. Zero before one is set up.
    ///
    /// Derived from the world-matrix buffer, which carries one 4x4 per part, so
    /// it is exactly the bound [`SSPlayer::part_transform`] checks against.
    pub fn part_count(&self) -> usize {
        let Some(root) = &self.root else { return 0 };
        root.context.get_world_matrices_len() / 16
    }

    /// The part's transform on the current frame, as the 2D affine
    /// `[m00, m01, m10, m11, tx, ty]` — the same six numbers, in the same order,
    /// that `ssplayer-webgl`'s `getPartTransform` returns. A point maps as
    /// `x' = m00·x + m01·y + tx`, `y' = m10·x + m11·y + ty`.
    ///
    /// The space is **animation-local and Y-down**: this is the part's pose
    /// inside its own animation, with neither [`SSPlayer::set_transform`] nor
    /// the scene's projection applied. A host pinning something to a part
    /// composes those itself, which is also what lets it pin into whatever
    /// coordinate system it actually uses.
    ///
    /// `None` when no animation is set up or `part` is out of range. Copied out,
    /// so unlike a [`FrameView`] it is safe to keep.
    ///
    /// Read it **after** [`SSScene::update`] — it describes the frame the last
    /// update produced. The matrix is computed for every part whether or not it
    /// is hidden, so a hidden part still reports where it would have been.
    ///
    /// [`SSScene::update`]: crate::SSScene::update
    pub fn part_transform(&self, part: u32) -> Option<[f32; 6]> {
        let root = self.root.as_ref()?;
        let matrices = unsafe { slice_f32(root.context.get_world_matrices_ptr(), root.context.get_world_matrices_len()) };
        let base = (part as usize).checked_mul(16)?;
        // Bounds-checked rather than trusted: these read the runtime's buffer
        // directly, so an out-of-range part would read past it instead of
        // producing the documented None.
        let m = matrices.get(base..base + 16)?;
        Some([m[0], m[4], m[1], m[5], m[12], m[13]])
    }

    /// Place this player: translation, scale, then rotation in radians, composed
    /// into the root transform of its tree.
    ///
    /// This is the per-player half of SPEC §5.4 — the projection belongs to the
    /// render target ([`SSScene::set_viewport`]), the placement to the player.
    /// It costs nothing extra to draw: the tree walk already composes an
    /// arbitrary parent matrix for every instance child, so a non-identity root
    /// simply rides along.
    pub fn set_transform(&mut self, x: f32, y: f32, scale_x: f32, scale_y: f32, rotation: f32) {
        let (sin, cos) = rotation.sin_cos();
        // Column-major (see `mat4_mul` / `apply_transform`): the columns are
        // scale*rotation for x and y, then z, then the translation.
        #[rustfmt::skip]
        let m = [
            scale_x * cos,  scale_x * sin, 0.0, 0.0,
            -scale_y * sin, scale_y * cos, 0.0, 0.0,
            0.0,            0.0,           1.0, 0.0,
            x,              y,             0.0, 1.0,
        ];
        self.transform = m;
    }

    /// Reset the placement to the identity: centred, unscaled, unrotated.
    pub fn reset_transform(&mut self) {
        self.transform = IDENTITY;
    }

    /// Draw order within a scene; lower draws first. Ties keep whatever order the
    /// host hands the players over in.
    pub fn set_order(&mut self, order: i32) {
        self.order = order;
    }

    pub fn order(&self) -> i32 {
        self.order
    }

    /// Skip this player entirely when false: `SSScene::update` advances no
    /// timeline and produces no geometry, so `SSScene::draw` records nothing.
    ///
    /// Distinct from `set_alpha(0.0)`, which is an author-facing fade that keeps
    /// the timeline running. This one is for a host that knows the player cannot
    /// be seen — culled, or scrolled out of the viewport.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Player-level fade, clamped to 0..=1 and multiplied into every part's
    /// colour alpha. Mask coverage is deliberately unaffected: a faded player
    /// clips exactly as it would at full opacity.
    pub fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }

    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Names of all animations in the main pack, if loaded.
    pub fn animation_names(&self) -> Option<Vec<String>> {
        let anim_resource = self.packs.get(self.main_pack.as_ref()?)?.resource().resource()?;
        Some(anim_resource.get_animation_names())
    }

    /// The atlases a registered pack asks for (SPEC §8.1), or `None` if
    /// `pack_name` names no pack this player holds. `None` selects the main pack.
    ///
    /// This is the reference-mode half of texture supply, and it exists because
    /// only the `.ssab` knows where its atlases were exported to: a host calls it
    /// straight after [`SSPlayer::add_ssab`] / [`SSPlayer::load_ssab`], loads
    /// every entry that is not [`TextureRef::embedded`] by resolving
    /// [`TextureRef::path`] against the directory the `.ssab` came from, and hands
    /// the pixels to [`SSRenderer::upload_texture`]. The player never opens a
    /// file itself — it has no filesystem on the web face, and on any face the
    /// host's asset system is the one that should own the bytes (SPEC §8.2).
    pub fn textures(&self, pack_name: Option<&str>) -> Option<&[TextureRef]> {
        let name = pack_name.or(self.main_pack.as_deref())?;
        Some(self.packs.get(name)?.textures())
    }

    /// The encoded atlases a registered pack carries (SPEC §8.1), or `None` if
    /// `pack_name` names no pack this player holds. `None` selects the main pack.
    ///
    /// The embedded-mode counterpart of [`SSPlayer::textures`]: there is nothing
    /// for the host to fetch, but there is something for it to decode. A host
    /// with its own decoder walks this and registers the results, rather than
    /// leaving it to [`SSPlayer::flush_textures`] — which needs the `embedded`
    /// feature and only knows how to decode PNG (SPEC §8.2).
    pub fn embedded_images(&self, pack_name: Option<&str>) -> Option<Vec<EmbeddedImage<'_>>> {
        let name = pack_name.or(self.main_pack.as_deref())?;
        Some(self.packs.get(name)?.embedded_images())
    }

    /// Canvas size of the currently playing animation.
    pub fn canvas_size(&self) -> Option<(f32, f32)> {
        let name = self.current_animation.as_ref()?;
        let anim_resource = self.packs.get(self.main_pack.as_ref()?)?.resource().resource()?;
        let idx = anim_resource.find_animation_index(name)?;
        let anim_meta = &anim_resource.animations()[idx];
        Some((anim_meta.canvas_width, anim_meta.canvas_height))
    }

    /// Hand the renderer any embedded atlas it does not already have (SPEC §8.1).
    /// Called at the start of each `update` — the earliest point the borrow model
    /// lends a `Device`/`Queue` — and by `restore_textures`, which re-queues every
    /// pack against the new device.
    ///
    /// **Whatever is already registered wins**, and that is what makes the shared
    /// texture map pay off: the second player of a pack finds the atlas present,
    /// skips the PNG decode as well as the upload, and a host that supplied its own
    /// image for a hash keeps it. A host that wants the embedded pixels instead
    /// calls [`SSRenderer::forget_texture`] first.
    pub(crate) fn flush_textures(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        let pending = std::mem::take(&mut self.pending_images);
        let mut names = std::mem::take(&mut self.packs_awaiting_textures);
        if pending.is_empty() && names.is_empty() {
            return;
        }
        // Sorted because packs that disagree about a texture hash resolve by who
        // registers first, and that answer must not depend on load order.
        names.sort();
        names.dedup();

        // The batch is assembled before anything is decoded, because the decode is
        // no longer sequential and "already registered" therefore cannot be re-read
        // between images. `claimed` is what used to be implicit in the loop: a hash
        // that an earlier entry took is not decoded a second time.
        //
        // Host-read atlases come first, so a reference-mode image wins over an
        // embedded one for the same hash — the order [`SSPlayer::load`] read them
        // in. Both routes skip a hash the renderer already holds, which is what
        // keeps N players of one pack to one upload between them.
        let mut claimed: HashSet<u32> = HashSet::new();
        let mut jobs: Vec<(u32, &[u8], Retention)> = Vec::new();
        for (hash, encoded) in &pending {
            if renderer.texture_size(*hash).is_none() && claimed.insert(*hash) {
                // Retained: the bytes came from outside, so this player has nothing
                // it could decode again after a device loss.
                jobs.push((*hash, encoded, Retention::Retain));
            }
        }
        for name in &names {
            let Some(pack) = self.packs.get(name) else { continue };
            for image in pack.embedded_images() {
                if renderer.texture_size(image.hash).is_none() && claimed.insert(image.hash) {
                    // Reproducible: this player still holds the `.ssab` and can
                    // decode the atlas again, so the renderer need not retain a
                    // second copy.
                    jobs.push((image.hash, image.bytes, Retention::Reproducible));
                }
            }
        }
        decode_and_register(renderer, device, queue, &jobs);
    }

    /// Advance this player by `delta_secs` and append its geometry to `scene`.
    ///
    /// Called by [`SSScene::update`] for each player in draw order — the scene
    /// owns the frame boundary (`begin_frame` / `end_frame`), because one frame
    /// now spans every player sharing the target.
    pub(crate) fn build_frame(&mut self, scene: &mut FrameChunk, renderer: &SSRenderer, delta_secs: f32) {
        // `visible == false` means "this player cannot be seen": no timeline
        // advance and no geometry. Textures are still flushed above, so it can be
        // made visible again without re-loading anything.
        if !self.visible {
            // What the last visible tick reported is not this tick's news. Leaving
            // it in place made a hidden player answer `events()` with the same
            // footstep on every update — a host that plays a sound per event fires
            // it for as long as the player stays hidden — and kept a loop or finish
            // edge readable long after it was crossed.
            self.events.clear();
            self.looped = false;
            self.finished_edge = false;
            return;
        }

        let Some(root) = &mut self.root else { return };
        // The runtime works in milliseconds for the primary context.
        root.context.update(delta_secs * 1000.0);
        let frame_no = root.context.get_frame_no();
        // The loop edge instance/effect slots re-arm their playback keys on. It
        // is a pulse the controller clears on entry to every `update`, so it has
        // to be read here, right after the one that raised it. Deriving it from
        // "the frame went backwards" instead is wrong under reverse and
        // ping-pong playback, where the frame goes backwards every tick.
        let looped = root.context.is_looped();

        // Collect what the host reacts to before anything that could skip work:
        // events must fire whenever the runtime reports them, whether or not
        // this tick also draws. The Godot Player gated event handling on the
        // integer draw frame changing and lost frame-0 events on the first tick
        // after `play()` as a result.
        let finished = root.context.is_finished();
        collect_events(root, self.current_animation.as_deref(), &mut self.events);
        self.looped = looped;
        self.finished_edge = finished && !self.prev_finished;
        self.prev_finished = finished;

        let root = self.root.as_mut().expect("checked above");

        // Advance every slot first, then draw. The two are separate walks for a
        // reason — see `step_slots`.
        step_slots(root, frame_no, delta_secs, looped);

        let draw = NodeDraw { transform: &self.transform, alpha: self.alpha };
        let mut build = FrameBuild { scene, renderer };
        render_node(root, &mut build, draw, frame_no, StencilTest::None);
    }

    /// What frame each instance child of the root is on, keyed by part index.
    ///
    /// A diagnostic, like [`SSPlayer::instance_node_count`] — and the only way to
    /// see that a child's clock is running while its part is hidden, since a
    /// hidden child draws nothing to look at (instance doc §Supporting
    /// Independent Playback).
    pub fn instance_child_frames(&self) -> Vec<(usize, f32)> {
        let Some(root) = &self.root else { return Vec::new() };
        let mut out: Vec<(usize, f32)> = root.instances.iter().map(|(&part_idx, child)| (part_idx, child.frame_no)).collect();
        out.sort_by_key(|&(part_idx, _)| part_idx);
        out
    }

    /// Total number of instance child nodes in the current tree. A non-zero
    /// count confirms external references were resolved and the tree was built.
    pub fn instance_node_count(&self) -> usize {
        fn count(node: &PlaybackNode) -> usize {
            node.instances.values().map(|c| 1 + count(&c.node)).sum()
        }
        self.root.as_ref().map(count).unwrap_or(0)
    }
}

/// Recursively build a playback node: create a context bound to `pack`'s
/// resource, set up `anime`, and build an instance child for every instance
/// part found in the pack. Child failures are skipped (not fatal).
fn build_node(packs: &HashMap<String, SSPack>, pack: &str, anime: &str, depth: u32) -> Option<PlaybackNode> {
    if depth > MAX_INSTANCE_DEPTH {
        return None;
    }
    let resource = packs.get(pack)?.resource();
    let res_ptr = resource.resource_static_ptr();
    if res_ptr.is_null() {
        return None;
    }
    // SAFETY: the resource lives in `packs` for the player's lifetime.
    let static_res = unsafe { &*res_ptr };
    let mut context = Context::new();
    context.bind_resource(static_res);
    context.setup_animation(anime).ok()?;

    let mut instances = HashMap::new();
    let mut effects = HashMap::new();
    let mut mask_flags = Vec::new();
    if let Some(ssab_bytes) = context.get_ssab_data() {
        let ab = unsafe { root_as_ss_anime_binary_unchecked(ssab_bytes) };
        // The pack's part list is shared across its animations; part index i
        // matches the FrameData part index used by draw_order/SoA.
        let parts = ab.parts();
        mask_flags = vec![PartMaskFlags::default(); parts.len()];
        for (i, part) in parts.iter().enumerate() {
            let part_type = part.part_type_type();

            // Static mask flags (SPEC §6 / mask doc §1). Geometry kind picks the
            // writer builder; Shape/Text/Nines can't write coverage yet (P6).
            let geom = match part_type {
                PartType::PartTypeNormal | PartType::PartTypeMask => GeomKind::Quad,
                PartType::PartTypeMesh => GeomKind::Mesh,
                PartType::PartTypeShape => GeomKind::Shape,
                _ => GeomKind::Unsupported,
            };
            mask_flags[i] = PartMaskFlags { is_mask_part: part_type == PartType::PartTypeMask, mask_write: part.mask_write(), mask_influence: part.mask_influence(), visible_inside_mask: part.visible_inside_mask(), draw_as_mask: part.draw_as_mask(), geom };

            match part_type {
                PartType::PartTypeInstance => {
                    if let Some(inst) = part.part_type_as_part_type_instance()
                        && let Some(child) = build_node(packs, inst.ref_anime_pack(), inst.ref_anime_name(), depth + 1)
                    {
                        let frame_no = child.context.get_frame_no();
                        instances.insert(i, InstanceChild { slot: InstanceSlot::new(), node: Box::new(child), frame_no, visible: false });
                    }
                }
                PartType::PartTypeEffect => {
                    if let Some(eff) = part.part_type_as_part_type_effect()
                        && let Ok(slot) = EffectSlot::new(static_res, eff.ref_effect_name_hash(), i as u32)
                    {
                        effects.insert(i, EffectPart { slot, plan: Vec::new() });
                    }
                }
                _ => {}
            }
        }
    }

    Some(PlaybackNode { context, instances, effects, mask_flags })
}

/// Advance every instance and effect slot this node owns, and recurse through
/// the instance children — **over the parts, not over what is drawn**.
///
/// This is deliberately a separate walk from [`render_node`], because the two
/// answer different questions: *whose clock moves* is every slot in the pack,
/// while *what is drawn and in what order* is `draw_order`. The runtime drops
/// `Hide`-ed parts from `draw_order` / `draw_batches` entirely (`framedata.rs`,
/// `drawn_as_self`), so stepping from the draw walk stops a slot the moment its
/// part is hidden. For an independently-played effect or instance — one running
/// on its own elapsed time rather than the parent's frame — that freezes the
/// clock and makes it resume late by exactly the time it spent hidden, which is
/// not what SS7 means by independent playback (instance doc §Supporting
/// Independent Playback, effect doc). The slots are built from the pack's part
/// list in [`build_node`], so they are all here to walk.
///
/// What the draw walk needs afterwards is left on each slot: the child's frame
/// and visibility, and the effect's draw plan.
fn step_slots(node: &mut PlaybackNode, frame_no: f32, delta_secs: f32, node_looped: bool) {
    let fps = node.context.get_fps() as f32;
    for (&part_idx, effect) in node.effects.iter_mut() {
        let ev = node.context.get_active_effect_event(part_idx as u32);
        // vert_stride=2 (packed XY), y_flip=false (Y handled by the world matrix).
        let r = effect.slot.step(ev, frame_no, delta_secs, fps, node_looped, false, 2);
        effect.plan.clear();
        if r.visible && !r.draw_plan_buf.is_null() && r.draw_plan_len > 0 {
            // SAFETY: the slot just built this buffer and owns it until its next
            // step; nothing steps it between here and the copy.
            effect.plan.extend_from_slice(unsafe { std::slice::from_raw_parts(r.draw_plan_buf, r.draw_plan_len) });
        }
    }

    for (&part_idx, child) in node.instances.iter_mut() {
        let ev = node.context.get_active_event_instance(part_idx as u32);
        // An independent child runs its own clock at the key's speed, so its own
        // sub-tree has to be stepped with the same scaling — the parent's raw
        // delta would run the grandchildren at the wrong rate.
        let child_delta = delta_secs * ev.speed.abs();
        let r = child.slot.step(ev, &mut child.node.context, frame_no, delta_secs, node_looped);

        // On a transition edge the slot configures the child's section but leaves
        // the frame snap to the host: play() moves the child to its start frame
        // so independent playback actually advances.
        if r.transitioned {
            child.node.context.play();
        }
        // Raw, never rounded: rounding here would truncate sub-frame progress on
        // every tick and freeze the child when a tick is worth less than a frame.
        child.frame_no = child.node.context.get_frame_no();
        child.visible = r.visible;

        // Recurse regardless of visibility, and re-arm the grandchildren on the
        // *child's* loop rather than the parent animation's: an independent child
        // runs on its own clock, so that is the edge they must re-fire on.
        step_slots(&mut child.node, child.frame_no, child_delta, r.child_looped);
    }
}

/// `DrawBatch.flags` bit set when a content batch is a masked target
/// (mirrors `FLAG_MASK_INFLUENCE` in the runtime).
const FLAG_MASK_INFLUENCE: u8 = 0x01;

/// Advance the mask cursor to `up_to_rank`, appending stencil writes for every
/// planned writer whose rank is below it (mask doc §2-1, WebGL `syncMasksTo`):
/// a pure mask's backward scope has ended → remove it (Decr/Invert); a clipping
/// writer's forward scope has begun → add it (Incr/Invert). Monotonic in
/// `cursor`; ops are baked per pipeline, so ref is always 0.
fn sync_masks_to(build: &mut FrameBuild<'_>, frame: &FrameView, node: &NodeSlot, writers: &[MaskWriter], cursor: &mut usize, up_to_rank: usize) {
    while *cursor < writers.len() && writers[*cursor].rank < up_to_rank {
        let w = &writers[*cursor];
        let op = if w.is_clipping {
            if w.op_invert { MaskOp::Invert } else { MaskOp::Incr }
        } else if w.op_invert {
            MaskOp::Invert
        } else {
            MaskOp::Decr
        };
        build.scene.append_mask_writer(frame, node, w, op);
        *cursor += 1;
    }
}

/// The two halves of layer 2, bundled for the tree walk: the scene being
/// filled, and the device-scoped renderer it reads texture
/// sizes from. Only the scene is mutated — pipelines are compiled later, in
/// `end_frame`, which is where the renderer is borrowed mutably.
struct FrameBuild<'a> {
    scene: &'a mut FrameChunk,
    renderer: &'a SSRenderer,
}

/// How one node of the tree is drawn: where it sits and how opaque it is.
///
/// `transform` changes per recursion level (each instance child composes its
/// part's world matrix onto its parent's), `alpha` is the owning player's and is
/// constant for the whole walk. They travel together so `render_node` keeps a
/// readable signature as more player-level properties land — tint, clip and
/// backdrop are all queued behind SPEC §5.5.
#[derive(Clone, Copy)]
struct NodeDraw<'a> {
    transform: &'a [f32; 16],
    /// Player-level fade; see [`BatchInfo::alpha`]. Never applied to mask
    /// coverage — a faded player clips exactly as it would at full opacity.
    alpha: f32,
}

/// Recursively append a node's geometry to the renderer, composing transforms.
///
/// Draw only: every slot was advanced by [`step_slots`] before this walk began,
/// so this reads what the step left behind rather than driving it. The node's
/// context is already at `frame_no` (root by `update`, children by their
/// parent's `InstanceSlot::step`).
///
/// Masking (SPEC §6): when the renderer has a stencil buffer and this is the
/// root scope (`inherited_stencil == None`), pure masks are pre-written and the
/// coverage is scoped-replayed as content is drawn in rank order; masked content
/// is tested per part by its polarity. When `inherited_stencil` is set, this
/// node is a sub-animation clipped by a parent mask (screen-space, single level,
/// mask doc §3-2-2): it runs no masks of its own and every content batch is
/// tested with the inherited polarity.
///
/// `draw` carries the owning player's placement and fade down the tree; whatever
/// coverage this scope writes, it takes back before returning (see
/// [`unwind_masks`]).
fn render_node(node: &mut PlaybackNode, build: &mut FrameBuild<'_>, draw: NodeDraw, frame_no: f32, inherited_stencil: StencilTest) {
    let NodeDraw { transform, alpha } = draw;
    let frame_data = match node.context.get_frame_data(frame_no) {
        Ok(bytes) => unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) },
        Err(_) => return,
    };
    let frame = frame_view(&mut node.context, frame_data);
    // Hand this node's runtime buffers to the scene once, up front: everything
    // below refers to them by arena offset rather than expanding them (SPEC §5.5).
    let node_slot = build.scene.begin_node(&frame, transform);

    // Own masking runs only at the root scope; a masked sub-animation inherits.
    let masking = build.scene.stencil_enabled() && inherited_stencil == StencilTest::None;
    let writers = if masking { plan_writers(&frame, &node.mask_flags) } else { Vec::new() };

    // Pre-write pure masks so they are active for lower-rank content (§2-1).
    for w in &writers {
        if !w.is_clipping {
            let op = if w.op_invert { MaskOp::Invert } else { MaskOp::Incr };
            build.scene.append_mask_writer(&frame, &node_slot, w, op);
        }
    }
    let mut cursor = 0usize;

    let fd = unsafe { root_as_frame_data_unchecked(frame.frame_data) };
    let draw_order = fd.draw_order();
    for batch in fd.draw_batches() {
        let start_rank = batch.start_rank() as usize;
        let count = batch.count() as usize;
        let kind = batch.kind();
        // A masked target only clips when writers are actually active this frame
        // (WebGL gates the masked path on maskWriterCount > 0). With no writers the
        // mask_influence opt-in is inert, so the content draws normally — crucially
        // via the stencil=None path, which is where shader dispatch happens.
        let masked_target = masking && !writers.is_empty() && (batch.flags() & FLAG_MASK_INFLUENCE) != 0;
        match kind {
            DrawBatchKind::Normal | DrawBatchKind::Mesh | DrawBatchKind::Shape => {
                // Pure masks (PartTypeMask / draw_as_mask) contribute coverage but
                // never draw their own colour (mask doc §3-3). The runtime still
                // emits a content batch for them, so skip it here.
                let is_pure = |pi: usize| node.mask_flags.get(pi).map(|f| f.pure_mask()).unwrap_or(false);
                let has_pure = (start_rank..start_rank + count).any(|r| is_pure(draw_order.get(r) as usize));

                if masked_target {
                    // Polarity and active coverage vary per part → draw one at a time.
                    for rank in start_rank..start_rank + count {
                        let part_idx = draw_order.get(rank) as usize;
                        sync_masks_to(build, &frame, &node_slot, &writers, &mut cursor, rank);
                        if is_pure(part_idx) {
                            continue;
                        }
                        let stencil = node.mask_flags.get(part_idx).map(|f| f.polarity()).unwrap_or(StencilTest::None);
                        let info = BatchInfo { start_rank: rank, count: 1, texture_hash: batch.texture_hash(), blend_type: batch.blend_type(), kind, stencil, alpha };
                        build.scene.append_batch(build.renderer, &frame, &node_slot, &info);
                    }
                } else if has_pure {
                    // Non-masked batch that still contains a pure mask: draw the
                    // non-mask parts individually, skipping the mask's own colour.
                    for rank in start_rank..start_rank + count {
                        let part_idx = draw_order.get(rank) as usize;
                        if is_pure(part_idx) {
                            continue;
                        }
                        let info = BatchInfo { start_rank: rank, count: 1, texture_hash: batch.texture_hash(), blend_type: batch.blend_type(), kind, stencil: inherited_stencil, alpha };
                        build.scene.append_batch(build.renderer, &frame, &node_slot, &info);
                    }
                } else {
                    let info = BatchInfo { start_rank, count, texture_hash: batch.texture_hash(), blend_type: batch.blend_type(), kind, stencil: inherited_stencil, alpha };
                    build.scene.append_batch(build.renderer, &frame, &node_slot, &info);
                }
            }
            DrawBatchKind::Instance => {
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;

                    // Decide the stencil this child sub-animation inherits: our own
                    // mask (if the instance part is a masked target), else propagate
                    // whatever we inherited (screen-space clip continues down).
                    let child_inherited = if masking && !writers.is_empty() {
                        let f = node.mask_flags.get(part_idx).copied().unwrap_or_default();
                        if f.mask_influence {
                            sync_masks_to(build, &frame, &node_slot, &writers, &mut cursor, rank);
                            f.polarity()
                        } else {
                            StencilTest::None
                        }
                    } else {
                        inherited_stencil
                    };

                    let Some(child) = node.instances.get_mut(&part_idx) else { continue };
                    if !child.visible {
                        continue;
                    }
                    // Compose the instance part's world matrix onto the parent transform.
                    let child_frame_no = child.frame_no;
                    let m: [f32; 16] = frame.world_matrices[part_idx * 16..part_idx * 16 + 16].try_into().unwrap();
                    let child_transform = mat4_mul(transform, &m);
                    let child_draw = NodeDraw { transform: &child_transform, alpha };
                    render_node(&mut child.node, build, child_draw, child_frame_no, child_inherited);
                }
            }
            DrawBatchKind::Effect => {
                // A masked effect (or one inside an inherited-masked subtree) tests
                // against the screen-space stencil; sync coverage once at its rank.
                let eff_stencil = if masked_target {
                    let part_idx = draw_order.get(start_rank) as usize;
                    sync_masks_to(build, &frame, &node_slot, &writers, &mut cursor, start_rank);
                    node.mask_flags.get(part_idx).map(|f| f.polarity()).unwrap_or(StencilTest::None)
                } else {
                    inherited_stencil
                };
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    // Whatever `step_slots` produced for this part earlier in the
                    // frame; empty when the effect had nothing to draw.
                    let Some(plan) = node.effects.get(&part_idx).map(|e| e.plan.as_slice()).filter(|p| !p.is_empty()) else { continue };
                    let m: [f32; 16] = frame.world_matrices[part_idx * 16..part_idx * 16 + 16].try_into().unwrap();
                    let eff_transform = mat4_mul(transform, &m);
                    // The effect draws from its own plan, so it needs its own
                    // arena slot rather than this node's.
                    let eff_slot = build.scene.push_transform(&eff_transform);
                    build.scene.append_effect(plan, &eff_slot, eff_stencil, alpha);
                }
            }
            // Mask batches emit no colour (coverage is handled by the writer
            // pre-write / scoped replay above). Text / Nines: not yet handled.
            _ => {}
        }
    }

    if masking {
        unwind_masks(build, &frame, &node_slot, &writers, cursor);
    }
}

/// Take back every bit of stencil coverage this scope wrote (SPEC §6).
///
/// The batch loop leaves the buffer unbalanced in two ways, because the scoped
/// replay only runs as far as the last masked target it met:
///
/// * a pure mask whose rank the cursor never reached is still pre-written, and
/// * a clipping writer the cursor *did* reach is still applied.
///
/// Both are undone here — increment pairs with decrement, invert is its own
/// inverse — so the scope returns the buffer in the state it found it. Without
/// this the invariant only holds by accident: one player per render pass, with
/// `LoadOp::Clear(0)` wiping the residue between frames. Stack a second player
/// into the same pass and the first one's mask clips it.
///
/// The extra draw calls are bounded by the writer count (at most
/// [`crate::mask::MAX_MASK_WRITERS`]) and write no colour.
fn unwind_masks(build: &mut FrameBuild<'_>, frame: &FrameView, node: &NodeSlot, writers: &[MaskWriter], cursor: usize) {
    let remove = |w: &MaskWriter| if w.op_invert { MaskOp::Invert } else { MaskOp::Decr };
    // Consumed clipping writers: `sync_masks_to` added their coverage.
    for w in writers[..cursor].iter().filter(|w| w.is_clipping) {
        build.scene.append_mask_writer(frame, node, w, remove(w));
    }
    // Unconsumed pure masks: pre-written before the batch loop, never removed.
    for w in writers[cursor..].iter().filter(|w| !w.is_clipping) {
        build.scene.append_mask_writer(frame, node, w, remove(w));
    }
}

/// Build a [`FrameView`] from the context's current (already-evaluated) SoA.
fn frame_view<'a>(ctx: &mut Context<'static>, frame_data: &'a [u8]) -> FrameView<'a> {
    FrameView {
        frame_data,
        world_matrices: unsafe { slice_f32(ctx.get_world_matrices_ptr(), ctx.get_world_matrices_len()) },
        local_vertices: unsafe { slice_f32(ctx.get_local_vertices_ptr(), ctx.get_local_vertices_len()) },
        local_uvs: unsafe { slice_f32(ctx.get_local_uvs_ptr(), ctx.get_local_uvs_len()) },
        mesh_vertices_x: unsafe { slice_f32(ctx.get_mesh_vertices_x_ptr(), ctx.get_mesh_vertices_x_len()) },
        mesh_vertices_y: unsafe { slice_f32(ctx.get_mesh_vertices_y_ptr(), ctx.get_mesh_vertices_y_len()) },
        mesh_vertex_offsets: unsafe { slice_u32(ctx.get_mesh_vertex_offsets_ptr(), ctx.get_mesh_vertex_offsets_len()) },
        mesh_uvs: unsafe { slice_f32(ctx.get_mesh_uvs_ptr(), ctx.get_mesh_uvs_len()) },
        mesh_indices: unsafe { slice_i32(ctx.get_mesh_indices_ptr(), ctx.get_mesh_indices_len()) },
        mesh_index_offsets: unsafe { slice_u32(ctx.get_mesh_index_offsets_ptr(), ctx.get_mesh_index_offsets_len()) },
        shape_vertices: unsafe { slice_f32(ctx.get_shape_vertices_ptr(), ctx.get_shape_vertices_len()) },
        shape_vertex_box_coords: unsafe { slice_f32(ctx.get_shape_vertex_box_coords_ptr(), ctx.get_shape_vertex_box_coords_len()) },
        shape_vertex_counts: unsafe { slice_i32(ctx.get_shape_vertex_counts_ptr(), ctx.get_shape_vertex_counts_len()) },
    }
}

/// Multiply two column-major 4x4 matrices: `a * b`.
fn mat4_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[col * 4 + k];
            }
            r[col * 4 + row] = s;
        }
    }
    r
}

/// Build a slice from a runtime pointer/length, or empty if null/zero.
///
/// # Safety
/// `ptr` must be valid for `len` `f32`s and stay alive for `'a` (guaranteed
/// until the next `get_frame_data` call on the same context).
unsafe fn slice_f32<'a>(ptr: *const f32, len: usize) -> &'a [f32] {
    if ptr.is_null() || len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len) } }
}

/// See [`slice_f32`]. `ptr` must be valid for `len` `u32`s.
unsafe fn slice_u32<'a>(ptr: *const u32, len: usize) -> &'a [u32] {
    if ptr.is_null() || len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len) } }
}

/// See [`slice_f32`]. `ptr` must be valid for `len` `i32`s.
unsafe fn slice_i32<'a>(ptr: *const i32, len: usize) -> &'a [i32] {
    if ptr.is_null() || len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len) } }
}

/// Decode a flush's whole batch of atlases and register them **in `jobs` order**
/// (SPEC §8.1).
///
/// One call for the batch rather than one per image so the decodes can overlap: a
/// character with three 4096² atlases otherwise pays for them end to end on the
/// host's thread, which is the load stall this exists to avoid. The registration
/// order is still the caller's, not the order the decodes happened to finish in.
#[cfg(feature = "embedded")]
fn decode_and_register(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, jobs: &[(u32, &[u8], Retention)]) {
    let encoded: Vec<&[u8]> = jobs.iter().map(|(_, bytes, _)| *bytes).collect();
    // Premultiplied on the worker (SPEC §5.2), so all that is left for this thread
    // is creating the texture and writing it.
    for (decoded, (hash, _, retention)) in crate::decode::decode_all(&encoded, true).into_iter().zip(jobs) {
        let Some(decoded) = decoded else {
            eprintln!("[ssplayer-wgpu] an image for {hash:#010x} could not be decoded");
            continue;
        };
        renderer.register_premultiplied(device, queue, *hash, decoded.rgba, (decoded.width, decoded.height), *retention);
    }
}

/// Without a decoder there is nothing to do here: on that build the host supplies
/// textures itself (the web face decodes with `createImageBitmap`, SPEC §10).
#[cfg(not(feature = "embedded"))]
fn decode_and_register(_: &mut SSRenderer, _: &wgpu::Device, _: &wgpu::Queue, _: &[(u32, &[u8], Retention)]) {}
