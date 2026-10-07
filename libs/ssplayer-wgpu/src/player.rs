//! [`SSPlayer`] — the primary public API.
//!
//! Owns a tree of runtime `Context`s, one per animation instance, and holds no
//! GPU resource: the host lends `Device` and `Queue` on every `update`.
//!
//! Instance parts play a sub-animation from another `.ssab` pack. Each instance
//! child gets its **own** `Context`, and so its own SoA buffer, which is what
//! keeps a child's frame from overwriting its parent's. The host registers every
//! pack via [`SSPlayer::add_ssab`]; external references resolve by pack name.

use crate::events::{SsEvent, decode_events};
use crate::frame::FrameView;
use crate::mask::{GeomKind, MaskOp, MaskWriter, MaskWriters, PartMaskFlags, ScopedWriter, StencilTest, plan_writers};
use crate::pack::{EmbeddedImage, SSPack, TextureRef};
use crate::renderer::{Retention, SSRenderer};
use crate::scene::{BatchInfo, FrameChunk, NodeSlot, pack_tint};
use ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked};
use ssruntime::error::SSRuntimeError;
use ssruntime::runtime::framedata::ss::runtime::{DrawBatchKind, root_as_frame_data_unchecked};
use ssruntime::{Context, EffectSlot, InstanceSlot, TrackBlend, TrackSync};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The tint that changes nothing — white, in the `0xRRGGBB` order
/// [`SSPlayer::set_tint`] takes. Deliberately not `pub`: cbindgen sweeps every
/// module-level `pub const` into the C header.
pub(crate) const WHITE_TINT: u32 = 0x00FF_FFFF;

/// Column-major 4x4 identity.
const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// The three things the loading walk cannot do for itself: reach a file, decode
/// an image, and know what the renderer already holds.
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
/// passing through wasm memory at all). An engine integration writes a
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
    /// Go back to the root you were built with, discarding whatever base a
    /// previous load left behind.
    ///
    /// [`SSPlayer::load_with`] calls this first, before it reads anything. It is
    /// what lets one loader serve more than one load: without it the base set by
    /// the last load is still in force when the next `src` is read, and a second
    /// `load_with(loader, "overall/Other.ssab")` goes looking in
    /// `overall/overall`. An engine adapter holding a single loader over its
    /// `AssetServer` / `ResourceLoader` / `.pck` handle is the shape that needs
    /// this; nothing else about the walk changes.
    ///
    /// Keep the root the caller gave you, and restore it here — do not try to undo
    /// [`AssetLoader::set_base`] by walking back up, which cannot be right for a
    /// URL and is not always right for a path.
    fn reset_base(&mut self);

    /// Re-root yourself at the directory `src` turned out to live in: every path
    /// the walk asks for after this is named relative to **that**.
    ///
    /// `src` is the pack's own path — *not* its directory. Resolving it, and
    /// deciding what "its directory" means, is the whole reason this method
    /// exists: only the implementation knows whether it is holding a filesystem
    /// path, a URL, or an archive key, and each of those cuts a directory
    /// differently. `Path::parent` and a `URL` base already know their own
    /// vocabulary; the walk, serving all three at once, did not.
    ///
    /// Resolve `src` **against the base you were constructed with**, then take its
    /// directory, so a loader that starts out rooted somewhere (a Godot `res://`
    /// prefix, an archive subtree) stays inside it.
    ///
    /// **Derived from the root you were built with, not folded into the base a
    /// previous call left behind**, so that this is idempotent and pairs with
    /// [`AssetLoader::reset_base`]: two loads through one loader land on the same
    /// base the two would have reached separately.
    ///
    /// [`SSPlayer::load_with`] calls this exactly once per load, **after** reading
    /// the pack and before asking for anything else. That order is what makes the
    /// bug this contract exists for unrepresentable: `src` is read while the base
    /// is still whatever the caller set up, so it can never be resolved against
    /// itself. [`SSPlayer::load_assets`] does not call it at all — a caller
    /// reaching for that one is expected to have rooted the loader itself.
    fn set_base(&mut self, src: &str);

    /// Read every path — **all of them at once**, so an implementation that can
    /// overlap the waiting does (the web one issues one `fetch` per path and
    /// settles them together). Results are positional; `None` is "this one did
    /// not arrive", which is never fatal to the rest.
    ///
    /// Every path is relative to the base — bare, never joined onto anything by
    /// the caller. The one exception is the first call of a
    /// [`SSPlayer::load_with`], which is `src` itself, verbatim, before
    /// [`AssetLoader::set_base`] has been called.
    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>>;

    /// Bytes for an instance-referenced pack, **by the name it is referenced
    /// under** rather than by a path.
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

    /// Which renderer key `path`'s image belongs under.
    ///
    /// **A cellmap hash names a slot inside one `.ssab`, not an image.** Adobe
    /// Animate writes `spritemap1.png` for every project it exports, so two packs
    /// from two projects ask for one hash and mean two different atlases; keyed by
    /// the hash alone, the second registration is skipped as "already held" and
    /// that pack silently draws the first one's atlas. Resolving `path` the way
    /// [`AssetLoader::read`] would and hashing **that** is what tells them apart,
    /// and it is the implementation that knows how — a path, a URL and an archive
    /// key each resolve differently, which is the same reason
    /// [`AssetLoader::set_base`] exists.
    ///
    /// Equal keys must mean the same pixels: that is what keeps the sibling packs
    /// of one project — all naming the one atlas beside them — to a single upload.
    /// Answering differently for two names of one file costs an upload, which is
    /// the harmless direction; answering alike for two files draws the wrong
    /// picture.
    ///
    /// The default answers `cellmap_hash`, which is what a loader rooted in one
    /// place forever has always done.
    fn texture_key(&self, _path: &str, cellmap_hash: u32) -> u32 {
        cellmap_hash
    }

    /// Decode `encoded` (a PNG, as the exporter wrote it) and register it under
    /// `key` — what [`AssetLoader::texture_key`] answered for it, and what the
    /// player will translate a draw batch's cellmap hash into.
    async fn supply_texture(&mut self, key: u32, encoded: &[u8]);

    /// Whether an atlas is already registered for `key` — a hint, used to skip
    /// reading a file nobody will need. `key` is
    /// [`AssetLoader::texture_key`]'s answer, not a cellmap hash.
    ///
    /// Best effort: a loader that cannot see the renderer answers `false` and
    /// lets the flush make the real decision. The texture map is device-scoped, so the authoritative check is the one that keeps N players of
    /// one pack to one decode and one upload between them.
    fn has_texture(&self, key: u32) -> bool;

    /// Report a file that could not be loaded. One missing atlas or sub-pack
    /// costs you that one thing, not the character, so this is a diagnostic
    /// rather than an error — the same call `SS6PlayerForGodot` makes with
    /// `ERR_PRINT` and `ssplayer-core` with `console.warn`.
    fn warn(&self, message: &str);
}

/// Point every planned writer's cutout at the renderer key its node's pack
/// resolved to. A writer's `texture_hash` comes from the batch it belongs to, so
/// it arrives in cellmap-hash terms like every other one.
fn retarget(writers: &mut MaskWriters, keys: &AtlasKeys) {
    for writer in writers.iter_mut() {
        writer.texture_hash = keys.resolve(writer.texture_hash);
    }
}

/// Re-read every node's table from the player's, for the whole tree.
///
/// Recursive because an instance child is a node on its own pack: one `.ssab`
/// learning where its atlas went says nothing about the sibling packs its
/// instance parts play from, and those learn theirs on their own pass.
fn refresh_atlas_keys(node: &mut PlaybackNode, atlas_keys: &HashMap<String, Arc<AtlasKeys>>) {
    node.atlas_keys = node_atlas_keys(atlas_keys, &node.pack);
    for child in node.instances.values_mut() {
        refresh_atlas_keys(&mut child.node, atlas_keys);
    }
}

/// The table for one pack, or an empty one when the player has never seen its
/// atlases — in which case every slot resolves to itself and the renderer is
/// keyed exactly as it was before there was a table.
fn node_atlas_keys(atlas_keys: &HashMap<String, Arc<AtlasKeys>>, pack: &str) -> Arc<AtlasKeys> {
    atlas_keys.get(pack).cloned().unwrap_or_default()
}

/// fnv1a over bytes — the construction `ssruntime::fnv1a_hash_str` applies to a
/// `str`, which is the only form the runtime ever needs.
fn fnv1a_bytes(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 2166136261;
    for b in bytes {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

/// The renderer key for one atlas slot.
///
/// **Embedded atlases are keyed by their own bytes.** They have no location to
/// resolve — they are inside the `.ssab` — and hashing the pixels is what keeps
/// two players that each read the same embedded pack from the disk to one
/// upload between them, which keying by pack identity would lose.
///
/// A reference-mode atlas is keyed by where it resolves to, which is
/// [`AssetLoader::texture_key`]'s answer: the bytes are not in hand yet, and the
/// decision has to be made before [`AssetLoader::has_texture`] is asked.
fn atlas_key(loader: &impl AssetLoader, texture: &TextureRef, embedded: Option<&[u8]>) -> u32 {
    match embedded {
        Some(bytes) => fnv1a_bytes(bytes),
        None => loader.texture_key(&texture.path, texture.slot),
    }
}

/// Which renderer key each of one pack's cellmaps resolves to.
///
/// The runtime speaks in cellmap hashes — `DrawBatch.texture_hash`, an effect
/// command's `cellmap_hash`, a mask writer's cutout — and a cellmap hash is
/// unique only *inside* its own `.ssab`. [`SSRenderer`]'s texture map is scoped
/// to the device instead, because that is what lets N players of one pack share
/// one upload. This is the join between the two: the player assigns a key that
/// identifies the image (see [`AssetLoader::texture_key`]) and translates every
/// hash through this table on the way to a draw call.
///
/// A slot with no entry resolves to itself, so a pack whose atlases the player
/// never saw — the host registered them against [`SSRenderer`] directly — keeps
/// behaving exactly as it did before there was a table.
///
/// A `Vec` rather than a map: packs have a handful of cellmaps (usually one), and
/// this is read once per draw batch.
#[derive(Clone, Debug, Default)]
pub struct AtlasKeys(Vec<(u32, u32)>);

impl AtlasKeys {
    /// The renderer key for `cellmap_hash`, or `cellmap_hash` itself when this
    /// table says nothing about it.
    pub fn resolve(&self, cellmap_hash: u32) -> u32 {
        self.0.iter().find(|(hash, _)| *hash == cellmap_hash).map(|(_, key)| *key).unwrap_or(cellmap_hash)
    }

    /// Record — or correct — where `cellmap_hash` points.
    ///
    /// Last writer wins, which is the same rule the load walk follows for the
    /// bytes themselves: a reload that finds a different file behind one name is
    /// re-registering it, not adding a second.
    fn set(&mut self, cellmap_hash: u32, key: u32) {
        match self.0.iter_mut().find(|(hash, _)| *hash == cellmap_hash) {
            Some(entry) => entry.1 = key,
            None => self.0.push((cellmap_hash, key)),
        }
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What kind of thing [`SSPlayer::load`] could not get.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingKind {
    /// An atlas. The parts that use it draw against the 1x1 white fallback, so
    /// they appear as silhouettes rather than not at all.
    Texture,
    /// An animation pack an instance part plays from. Those parts do not appear.
    Pack,
}

/// Something a load needed and did not get.
///
/// A missing file is deliberately **not** an error: one unreadable atlas must not
/// cost you the character, so [`SSPlayer::load`] reports `Err` only when the
/// `.ssab` itself is unreadable. That leaves the caller unable to tell a complete
/// load from a partial one, which is what this answers — the human-facing half is
/// [`AssetLoader::warn`], and this is the half a program can branch on.
#[derive(Clone, Debug)]
pub struct MissingAsset {
    pub kind: MissingKind,
    /// What was asked for: for a texture, the path the exporter wrote —
    /// relative to its pack's directory, which is what the loader resolved it
    /// against and the only form the walk ever sees. For a pack, its name.
    pub name: String,
    /// For a texture, the slot a draw batch will ask for — a cellmap's
    /// `name_hash`, which [`SSPlayer::texture_key`] turns into the renderer's
    /// key. Zero for a pack, whose identity is its name.
    pub slot: u32,
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
    // SAFETY: the buffer the node was built from, which the host guaranteed is a valid
    // `.ssab` (`SSPack::build`).
    let ab = unsafe { root_as_ss_anime_binary_unchecked(ssab_bytes) };
    let Some(anim) = ab.animations().iter().find(|a| a.name() == animation) else {
        return;
    };
    decode_events(&ab, &anim, indices, out);
}

/// One node in the playback tree: a runtime context plus its instance children
/// and effect slots.
struct PlaybackNode {
    context: Context<'static>,
    /// The resource this node's context is bound to, for **identity** only.
    /// `set_animation` compares it with the main pack's current resource to
    /// decide whether the context can be handed a new clip (the cheap path, and
    /// the one that keeps the runtime's transport settings) or has to be rebuilt
    /// against a resource that has since been replaced.
    ///
    /// A shared reference rather than the `*const` the pointer comparison wants:
    /// a raw pointer is neither `Send` nor `Sync`, and storing one here would
    /// take both off `SSPlayer` — which the Bevy adapter needs to hold it in the
    /// ECS at all. The context already holds the same reference.
    res: &'static ssruntime::AnimationResource<'static>,
    /// Instance children keyed by part index (index into parts/draw_order/SoA).
    instances: HashMap<usize, InstanceChild>,
    /// Effect slots keyed by part index. Effects reuse this node's context
    /// (no separate context, unlike instances).
    effects: HashMap<usize, EffectPart>,
    /// Static per-part mask flags, indexed by part index. Read once from ssab
    /// `PartData` at build time. Empty when the pack has no ssab data.
    mask_flags: Vec<PartMaskFlags>,
    /// Which pack this node plays from — the name `atlas_keys` is filed under.
    /// `res` identifies the same thing but only by pointer, and a refresh needs
    /// to look the table up by name.
    pack: String,
    /// Where this node's pack's cellmap hashes point in the renderer's texture
    /// map. Read once at build time, for the same reason `mask_flags` is: the
    /// draw walk needs it per batch and it cannot change under a live tree.
    ///
    /// Per node rather than per player because a node plays from *one* pack
    /// while the tree spans several — an instance part's sub-animation is a
    /// child node on its own pack, and that is exactly the case two projects'
    /// `spritemap1` would be confused across.
    atlas_keys: Arc<AtlasKeys>,
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
    /// `SSPlayer: Send`, which `SSScene::update` does not require now that its
    /// walk is serial again, but a host that moves players
    /// between its own threads does. The allocation is reused frame to frame, so
    /// the price is one memcpy per effect.
    plan: Vec<u8>,
}

/// The SpriteStudio player. One instance owns one animation-instance tree.
pub struct SSPlayer {
    /// All registered packs, keyed by pack name (matches `ref_anime_pack`).
    /// Cloning a pack shares its parse and its bytes, so N players of
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
    /// Names of packs whose embedded atlases have not been offered to
    /// a renderer yet. The offer happens on the next `update`, which is the
    /// earliest point the borrow model lends a `Device`/`Queue`; the decode itself
    /// waits until then too, so a pack whose atlas is already registered is never
    /// decoded at all.
    packs_awaiting_textures: Vec<String>,
    /// Atlases a host-side load read but has not handed to a renderer yet, still
    /// **encoded**.
    ///
    /// The reference-mode twin of `packs_awaiting_textures`, and deliberately the
    /// same mechanism: [`SSPlayer::load`] borrows no `Device`/`Queue` — the borrow
    /// model gives it none until `update` — so what it reads waits here for the
    /// same flush that decodes the embedded ones. Encoded rather than decoded
    /// because a 2048² atlas is a few hundred KB here against 16 MB of pixels,
    /// and because it puts both routes on one timeline.
    pending_images: Vec<(u32, Vec<u8>)>,
    /// Per pack name, which renderer key each of its cellmaps resolves to.
    ///
    /// Filled as the atlases arrive — by the load walk for the ones it read, and
    /// by the flush for embedded ones — and read by [`build_node`], which hands
    /// each node the table for the pack it plays from. Per *pack* and not per
    /// player because one player holds the sibling packs its instance parts
    /// reference, and a host is free to [`SSPlayer::add_ssab`] packs from two
    /// projects into one player, which is exactly where two `spritemap1`s meet.
    ///
    /// `Arc` because every node clones it, and because replacing a pack must not
    /// disturb the tree still playing the old one.
    atlas_keys: HashMap<String, Arc<AtlasKeys>>,
    /// What the last load could not get. Reset by each
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
    /// Held explicitly: the alternative
    /// is to lean on some earlier `is_playing` guard, which works until someone
    /// moves the guard. Notifying `is_finished` directly
    /// (fires every frame afterwards) and not resetting the latch in `play`
    /// (the *next* completion goes missing) are defects that really happened in
    /// sibling Players.
    prev_finished: bool,
    /// Whether the last update was the first to run after a `play`.
    started_edge: bool,
    /// A `play` that no tick has run since. The tick reports it, not the call,
    /// so a culled player's skipped tick leaves it for the first one that runs.
    start_pending: bool,
    /// Where this player sits in the scene — the root transform of its tree. Composed by [`SSPlayer::set_transform`]; only the matrix is
    /// kept because nothing yet re-composes it. A future `box` (the
    /// WebGL player's viewport-tracking rect) moves after `set_transform` runs,
    /// so it will need the authored parameters retained as well.
    transform: [f32; 16],
    /// Draw order within a scene; lower draws first. Nothing in this file reads
    /// it — it exists so the host and [`crate::SSScene`] can sort by it.
    order: i32,
    /// Draw nothing when false, while the timeline keeps running (see
    /// [`SSPlayer::set_visible`]).
    visible: bool,
    /// Skip this player entirely when true (see [`SSPlayer::set_culled`]).
    culled: bool,
    /// Player-level fade folded into every vertex (see [`BatchInfo::alpha`]).
    alpha: f32,
    /// Player-level tint as the API gives it, `0xRRGGBB` (see
    /// [`SSPlayer::set_tint`]). Kept in that order rather than the shader's so a
    /// reader of this field sees what the caller passed; `pack_tint` converts it
    /// once per frame.
    tint: u32,
    /// The loop count the host chose, re-applied after every `setup_animation`.
    ///
    /// Held for the reason every sibling Player holds it: the runtime resets the
    /// count on every transition, deliberately, because a transition is what
    /// states it.
    ///
    /// `None` until the host sets one, so a count it never chose is never
    /// written over the animation's own default.
    loop_count: Option<i32>,
    /// The rest of the transport the host chose — speed, the frame-rate
    /// override, frame skip, the direction and style — handed to every root
    /// context this player builds.
    ///
    /// The runtime carries these across a `setup_animation` itself, but they
    /// live on its context, and this player builds a new one for its first
    /// animation and whenever the pack behind it changes. The sibling Players
    /// keep one context and rebind it, so there the settings hold from before
    /// the first animation and across a pack change; holding them here is what
    /// makes that true of this one. Only the setters below write them, so the
    /// copy cannot drift from the context it was handed to.
    transport: HostTransport,
    /// Whether the *display* frame keeps its fraction (see
    /// [`SSPlayer::set_sub_frame_enabled`]). The playhead is an `f32` either way.
    sub_frame: bool,
    /// This player's identity, drawn once from [`NEXT_PLAYER_ID`]. It exists for
    /// one reason: a runtime texture override ([`SSPlayer::set_cellmap_texture`])
    /// registers its replacement in the device-scoped renderer under a *private*
    /// key, and two players overriding the same cellmap must not collide on it.
    /// Mixing this id into that key is what keeps the override per player, the way
    /// each sibling Player's own texture map is.
    id: u32,
    /// Active runtime texture overrides, per pack name. Each entry redirects one
    /// cellmap's draws to a private renderer key; the map here is the source of
    /// truth a reload re-applies against (`record_atlas_keys`), so the redirect
    /// survives the atlas being re-registered under a new base key.
    overrides: HashMap<String, Vec<CellmapOverride>>,
    /// Override pixels a `set_cellmap_texture` accepted but has not handed to a
    /// renderer yet — the same borrow-model deferral as [`pending_images`](Self::pending_images):
    /// the player is lent no `Device`/`Queue` until `update`, so the upload waits
    /// for the same flush. `(private key, straight-alpha RGBA8, width, height)`.
    pending_overrides: Vec<(u32, Vec<u8>, u32, u32)>,
    /// Private override keys a restore has retired, dropped from the renderer on
    /// the next flush. Deferred for the same reason as the uploads.
    pending_forgets: Vec<u32>,
}

/// The transport settings of [`SSPlayer::transport`](SSPlayer), at the
/// runtime's own defaults until the host changes one.
#[derive(Clone, Copy, Debug)]
struct HostTransport {
    speed_scale: f32,
    /// The frame-rate override; `0` leaves the authored rate.
    frame_rate: i32,
    frame_skip: bool,
    direction: PlaybackDirection,
    style: PlaybackStyle,
}

impl Default for HostTransport {
    fn default() -> Self {
        Self { speed_scale: 1.0, frame_rate: 0, frame_skip: true, direction: PlaybackDirection::Forward, style: PlaybackStyle::Normal }
    }
}

/// One active runtime texture override: a cellmap redirected to a private
/// renderer key. The `original` key is what a restore points back at — captured
/// when the override is first set, and kept current across reloads by
/// [`SSPlayer::record_atlas_keys`], since the atlas may re-register under a new
/// base key while the override is live.
#[derive(Clone, Copy, Debug)]
struct CellmapOverride {
    /// The cellmap's `name_hash` — what a draw batch and the runtime speak in.
    cellmap_hash: u32,
    /// The private renderer key the override texture is registered under, unique
    /// to this player and this cellmap.
    key: u32,
    /// The base key a restore redirects back to (the shared atlas, untouched).
    original: u32,
}

/// Hands each [`SSPlayer`] a distinct [`SSPlayer::id`]. Starts at 1 so 0 stays
/// free as "no player".
static NEXT_PLAYER_ID: AtomicU32 = AtomicU32::new(1);

/// The private renderer key a runtime override registers its texture under.
///
/// Derived rather than allocated so re-overriding one cellmap reuses the key
/// (overwriting the texture, not leaking a new entry), and mixed with the
/// player's id so two players overriding the same cellmap stay independent. It is
/// an fnv1a like every other renderer key, so it shares that keyspace's
/// already-trusted assumption that distinct inputs do not collide.
fn override_key(id: u32, cellmap_hash: u32) -> u32 {
    let mut buf = [0u8; 20];
    buf[..12].copy_from_slice(b"\0ssoverride\0");
    buf[12..16].copy_from_slice(&id.to_le_bytes());
    buf[16..20].copy_from_slice(&cellmap_hash.to_le_bytes());
    fnv1a_bytes(&buf)
}

impl Default for SSPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl SSPlayer {
    /// Create a player. Takes nothing: a player holds no GPU resource at all, which is what makes putting a hundred of them in a scene
    /// cheap. The device-scoped [`SSRenderer`] and the target-scoped
    /// [`SSScene`](crate::SSScene) are lent to it per frame by
    /// [`SSScene::update`](crate::SSScene::update).
    pub fn new() -> Self {
        Self {
            packs: HashMap::new(),
            retired: Vec::new(),
            main_pack: None,
            root: None,
            current_animation: None,
            packs_awaiting_textures: Vec::new(),
            pending_images: Vec::new(),
            atlas_keys: HashMap::new(),
            missing: Vec::new(),
            events: Vec::new(),
            looped: false,
            finished_edge: false,
            prev_finished: false,
            started_edge: false,
            start_pending: false,
            transform: IDENTITY,
            order: 0,
            visible: true,
            culled: false,
            alpha: 1.0,
            tint: WHITE_TINT,
            loop_count: None,
            transport: HostTransport::default(),
            // On, matching the Web and Flutter players: this one draws for
            // applications and for hosts with a display refresh to keep up with,
            // where interpolating between authored frames is the point. The game
            // engines default it off.
            sub_frame: true,
            id: NEXT_PLAYER_ID.fetch_add(1, Ordering::Relaxed),
            overrides: HashMap::new(),
            pending_overrides: Vec::new(),
            pending_forgets: Vec::new(),
        }
    }

    /// Re-offer this player's embedded atlases after a device loss.
    ///
    /// The player itself has nothing to rebuild — it owns no GPU resource. What it
    /// does own is the `.ssab`, which is the only place a [`Retention::Reproducible`]
    /// texture can come back from, so the host calls this once per player after
    /// [`SSRenderer::restore_device`] and
    /// [`SSScene::restore_device`](crate::SSScene::restore_device).
    ///
    /// Playback state (packs, the instance tree, the current frame) is CPU-side and
    /// was never affected: the animation resumes where it was interrupted.
    pub fn restore_textures(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.packs_awaiting_textures = self.packs.values().filter(|p| p.has_embedded_textures()).map(|p| p.name().to_string()).collect();
        self.flush_textures(renderer, device, queue);
    }

    /// Load a `.ssab` **and everything it depends on**, then select it.
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
    /// exporter wrote the paths relative to; the walk never joins those paths
    /// itself, it hands `src` to [`AssetLoader::set_base`] once and asks for bare
    /// names after that.
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
        // Back to where the caller rooted it, discarding any base a previous load
        // left behind — that is what makes `src` below mean the same thing on the
        // second call as on the first.
        loader.reset_base();
        let Some(bytes) = loader.read(std::slice::from_ref(&src.to_owned())).await.into_iter().next().flatten() else {
            loader.warn(&format!("could not read {src}"));
            return Err(SSRuntimeError::NotLoaded);
        };
        let pack = SSPack::load(&bytes)?;
        let name = pack.name().to_string();
        self.set_pack(&pack);
        // Read first, re-root second. Everything from here is named relative to
        // wherever `src` turned out to be, and working that out belongs to the
        // loader — it is the only party that knows which vocabulary it is in.
        loader.set_base(src);
        self.load_assets(loader, &name).await;
        Ok(())
    }

    /// Load a `.ssab` off the filesystem **and everything it depends on**, then
    /// select it.
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
    /// exporter wrote those paths relative to. That directory is `Path::parent`'s
    /// answer, so the platform decides it rather than us.
    ///
    /// Use [`SSPlayer::load_with`] where a `File::open` cannot reach the bytes: a
    /// Godot `.pck`, an APK, an engine's asset server, a browser.
    #[cfg(all(feature = "embedded", not(target_arch = "wasm32")))]
    pub fn load(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), SSRuntimeError> {
        let path = path.as_ref();
        // Rooted nowhere: `path` is read as given, and the walk then re-roots the
        // loader at its directory through `set_base`.
        let mut loader = crate::assets::FsLoader::new();
        let out = crate::assets::block_on(self.load_with(&mut loader, &path.to_string_lossy()));
        // Queued rather than uploaded: see the note on `pending_images`.
        self.pending_images.append(&mut loader.images);
        out
    }

    /// The half of [`SSPlayer::load`] that runs on an **already registered**
    /// pack: its atlases, and the packs it plays from, transitively.
    ///
    /// For a host that got the bytes some other way — a drop, a bundler, an
    /// archive — and registered them with [`SSPlayer::load_ssab`].
    ///
    /// **`loader` must already be rooted where the pack's relative paths resolve**,
    /// because this entry point has no `src` to hand [`AssetLoader::set_base`]:
    /// the bytes came from somewhere it never saw. Everything asked for here is a
    /// bare name — an atlas path as the exporter wrote it, or `<pack>.ssab`.
    pub async fn load_assets(&mut self, loader: &mut impl AssetLoader, pack_name: &str) {
        let Some(pack) = self.packs.get(pack_name).cloned() else {
            loader.warn(&format!("no pack named \"{pack_name}\" is registered"));
            return;
        };
        self.load_textures(loader, &pack).await;

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
            // A bare sibling name: the loader is rooted at the pack's own
            // directory, which is what the exporter wrote these relative to.
            let paths: Vec<String> = wanted.iter().map(|name| format!("{name}.ssab")).collect();
            let unprovided: Vec<String> = paths.iter().zip(&provided).filter(|(_, bytes)| bytes.is_none()).map(|(path, _)| path.clone()).collect();
            let mut fetched = loader.read(&unprovided).await.into_iter();
            let fetched: Vec<Option<Vec<u8>>> = provided.into_iter().map(|bytes| if bytes.is_some() { bytes } else { fetched.next().flatten() }).collect();

            for (name, bytes) in wanted.iter().zip(fetched) {
                let Some(bytes) = bytes else {
                    loader.warn(&format!("instance pack \"{name}\" did not load; parts that play it will not appear"));
                    self.missing.push(MissingAsset { kind: MissingKind::Pack, name: name.clone(), slot: 0 });
                    continue;
                };
                // Never renamed: `ref_anime_pack` is matched against the name the
                // exporter wrote, so an override here would be a reference that
                // resolves to nothing.
                match SSPack::load(&bytes) {
                    Ok(sub) => {
                        self.add_pack(&sub);
                        self.load_textures(loader, &sub).await;
                        queue.extend(sub.instance_packs());
                    }
                    Err(e) => {
                        loader.warn(&format!("instance pack \"{name}\" is not a valid .ssab: {e:?}"));
                        self.missing.push(MissingAsset { kind: MissingKind::Pack, name: name.clone(), slot: 0 });
                    }
                }
            }
        }
    }

    /// Supply every atlas `pack` asks for that the renderer does not already
    /// hold — both texture tables, either export mode.
    ///
    /// Embedded images come out of the pack; the rest are read through `loader`,
    /// **once per file**: `external_textures()` can name an image a cellmap
    /// already named, under a different hash, and that must not become two reads.
    ///
    /// The paths asked for are `TextureRef::path` exactly as the exporter wrote
    /// it, resolved by the loader against its base — so the name that reaches
    /// [`SSPlayer::missing_assets`] is that same relative path, and the warning
    /// names the pack to say which directory it was relative to.
    async fn load_textures(&mut self, loader: &mut impl AssetLoader, pack: &SSPack) {
        let refs = pack.textures();
        if refs.is_empty() {
            return;
        }
        let carried = pack.embedded_images();
        let embedded = |hash: u32| carried.iter().find(|i| i.slot == hash).map(|i| i.bytes);

        // Before anything is read, because `has_texture` is asked about the key
        // and the table has to describe the atlases already held just as much as
        // the ones about to arrive.
        let keys: Vec<u32> = refs.iter().map(|t| atlas_key(loader, t, embedded(t.slot))).collect();
        self.record_atlas_keys(pack.name(), refs.iter().map(|t| t.slot).zip(keys.iter().copied()));

        let wanted: Vec<(&TextureRef, u32)> = refs.iter().zip(keys.iter().copied()).filter(|(_, key)| !loader.has_texture(*key)).collect();
        if wanted.is_empty() {
            return;
        }

        let mut paths: Vec<String> = wanted.iter().filter(|(t, _)| embedded(t.slot).is_none()).map(|(t, _)| t.path.clone()).collect();
        paths.sort();
        paths.dedup();
        let fetched: HashMap<&String, Option<Vec<u8>>> = paths.iter().zip(loader.read(&paths).await).collect();

        for (texture, key) in &wanted {
            let bytes = match embedded(texture.slot) {
                Some(bytes) => bytes,
                None => match fetched.get(&texture.path).and_then(|b| b.as_deref()) {
                    Some(bytes) => bytes,
                    None => {
                        loader.warn(&format!("atlas {} for pack \"{}\" did not load; parts that use it will draw white", texture.path, pack.name()));
                        self.missing.push(MissingAsset { kind: MissingKind::Texture, name: texture.path.clone(), slot: texture.slot });
                        continue;
                    }
                },
            };
            loader.supply_texture(*key, bytes).await;
        }
    }

    /// Fold what a load or a flush learned into the pack's table, leaving the
    /// slots it says nothing about alone.
    ///
    /// A base key an atlas registers under is also the key a *restore* returns to,
    /// so a cellmap that is actively overridden takes the newly-learned key as its
    /// new `original` — and the override's redirect is re-applied on top, so a
    /// reload never quietly cancels a live override (the same
    /// name-over-hash follow-through the reference Godot Player does on a resource
    /// swap).
    fn record_atlas_keys(&mut self, pack: &str, entries: impl Iterator<Item = (u32, u32)>) {
        let mut entries: Vec<(u32, u32)> = entries.collect();
        if let Some(overrides) = self.overrides.get_mut(pack) {
            for over in overrides.iter_mut() {
                if let Some((_, key)) = entries.iter().find(|(hash, _)| *hash == over.cellmap_hash) {
                    over.original = *key;
                }
            }
            // The overrides win: their redirects go on last.
            entries.extend(overrides.iter().map(|over| (over.cellmap_hash, over.key)));
        }
        self.apply_atlas_entries(pack, entries);
    }

    /// Set exactly these `cellmap_hash -> renderer key` entries on a pack's table
    /// and push the result to the live tree. The mechanical core shared by
    /// [`SSPlayer::record_atlas_keys`] and the override redirects, which speak the
    /// same table but decide its contents differently.
    fn apply_atlas_entries(&mut self, pack: &str, entries: Vec<(u32, u32)>) {
        let mut table = self.atlas_keys.get(pack).map(|keys| (**keys).clone()).unwrap_or_default();
        for (cellmap_hash, key) in entries {
            table.set(cellmap_hash, key);
        }
        if table.is_empty() {
            return;
        }
        self.atlas_keys.insert(pack.to_string(), Arc::new(table));
        // The tree may predate this. A host is free to register bytes, select an
        // animation, and only then hand the walk a loader (`load_assets`), and
        // selecting again cannot repair it — `set_animation` returns early for the
        // name already selected. So the keys are pushed to the nodes here, where
        // they are learned, rather than waited for at a selection that may never
        // come.
        if let Some(root) = &mut self.root {
            refresh_atlas_keys(root, &self.atlas_keys);
        }
    }

    /// What the last load could not get, in the order it gave up.
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
    /// player.load("assets/overall/Instance.ssab")?; // the pack was readable …
    /// for missing in player.missing_assets() { // … but maybe not all of it
    /// eprintln!("{:?} {} is not there", missing.kind, missing.name);
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
    /// `ref_anime_pack` will be looked up by.
    ///
    /// For a host resolving those references itself: it answers "have I already
    /// fetched this one", which is what keeps a cycle of packs referencing each
    /// other from being walked forever.
    pub fn has_pack(&self, name: &str) -> bool {
        self.packs.contains_key(name)
    }

    /// Register a pack for external instance references without
    /// changing what is playing. Registration is by [`SSPack::name`].
    ///
    /// Sharing one pack between players is the intended shape: the
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
        // Here rather than at the flush, so [`SSPlayer::texture_key`] can answer
        // the moment a pack is registered: a host supplying its own pixels
        // (Bevy lending a `GpuImage`'s view) asks between this call and the
        // first update, and an answer that arrived later would be an answer it
        // had already registered under something else.
        let embedded: Vec<(u32, u32)> = pack.embedded_images().iter().map(|image| (image.slot, fnv1a_bytes(image.bytes))).collect();
        self.record_atlas_keys(&name, embedded.into_iter());
        // Only while a tree exists is there anything pointing into the pack being
        // replaced; without one the old pack is free to go here and now.
        if let Some(previous) = self.packs.insert(name, pack.clone())
            && self.root.is_some()
        {
            self.retired.push(previous);
            // The live tree was built from the pack just replaced, so it is no
            // longer what this name means. Forgetting the selection is what lets
            // `set_animation` -- a no-op for the name already set up -- still
            // rebuild from the replacement, which is the contract this method
            // documents ("the replacement is what the next tree is built from").
            self.current_animation = None;
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
        // Already set up: a switch with nothing to switch to, so nothing moves.
        // `current_animation` is cleared by `set_pack` and by an `add_pack` that
        // replaced a pack, so this can never mistake "the name the *previous*
        // pack had selected" for a tree that is still current.
        if self.current_animation.as_deref() == Some(anim_name) {
            return Ok(());
        }
        let main_pack = self.main_pack.clone().ok_or(SSRuntimeError::NotLoaded)?;
        let res_ptr = self.packs.get(&main_pack).ok_or(SSRuntimeError::NotLoaded)?.resource().resource_static_ptr();
        if res_ptr.is_null() {
            return Err(SSRuntimeError::NotLoaded);
        }
        // SAFETY: the resource lives in `packs` for the player's lifetime, and
        // the pointer just came from the entry still held there.
        let static_res: &'static ssruntime::AnimationResource<'static> = unsafe { &*res_ptr };

        // Hand the existing root context the new clip whenever it is still bound
        // to the pack we are selecting from, as every sibling Player does: the
        // runtime carries the host's transport across a setup on one context. A
        // context built new — the first animation, or a replaced pack — is
        // handed it from `transport` below instead.
        //
        // Only the per-animation rig is rebuilt: which instance children are
        // mounted and which effect slots exist is the animation's business.
        let reused = match &mut self.root {
            Some(root) if std::ptr::eq(root.res, static_res) => {
                root.context.setup_animation(anim_name)?;
                let (instances, effects, mask_flags) = build_rig(&root.context, static_res, &self.packs, &self.atlas_keys);
                root.instances = instances;
                root.effects = effects;
                root.mask_flags = mask_flags;
                true
            }
            _ => false,
        };
        if !reused {
            let mut root = build_node(&self.packs, &self.atlas_keys, &main_pack, anim_name).ok_or(SSRuntimeError::NotLoaded)?;
            // A new context starts at the runtime's defaults; the host's settings
            // are this player's, not the tree's.
            let t = self.transport;
            root.context.set_speed_scale(t.speed_scale);
            root.context.set_frame_rate(t.frame_rate);
            root.context.set_skip_frames(t.frame_skip);
            root.context.set_playback_direction(t.direction.into(), t.style.into());
            // Assigning drops the previous tree, and with it the last raw pointer
            // into any pack a re-registration retired.
            self.root = Some(root);
        }
        // Either way nothing points into a retired pack any more: a rebuilt tree
        // replaced every context, and a reused one kept only the root's — which,
        // by the check above, is bound to a pack that was never retired. Its rig
        // was rebuilt from `packs`, which is where a replacement went.
        self.retired.clear();
        self.current_animation = Some(anim_name.to_string());
        // Nothing that happened under the previous animation describes this one.
        self.events.clear();
        self.looped = false;
        self.finished_edge = false;
        self.prev_finished = false;
        // The setup leaves the new animation stopped, so a `play` made before it
        // started nothing that a tick could report.
        self.started_edge = false;
        self.start_pending = false;
        // The one thing the runtime does NOT carry across a setup, deliberately,
        // because the transition is what states it.
        self.reapply_loop_count();
        Ok(())
    }

    /// Put the host's loop count back after a `setup_animation` dropped it. See
    /// [`SSPlayer::loop_count`](struct.SSPlayer.html) — it is the one setting a
    /// setup resets, and this is the only place it is replayed.
    fn reapply_loop_count(&mut self) {
        let Some(count) = self.loop_count else { return };
        if let Some(root) = &mut self.root {
            root.context.set_loop_count(count);
        }
    }

    /// [`SSPlayer::play`] from `start_frame` rather than from the head of the
    /// section. The family spells this `play(startFrame)`; Rust has no
    /// overloads, so it is a second name here.
    ///
    /// Not a resume either — it re-arms the loop count exactly as `play` does,
    /// and the frame it starts from becomes the reference the loop counts from.
    pub fn play_from(&mut self, start_frame: f32) {
        self.prev_finished = false;
        if let Some(root) = &mut self.root {
            root.context.play_with_start_frame(start_frame);
            self.start_pending = true;
        }
    }

    /// Start the selected animation from the head of its section, re-arming the
    /// loop count. **Not a resume**: it rewinds, so lifting a
    /// [`SSPlayer::pause`] is [`SSPlayer::resume`]'s job.
    /// No-op until [`SSPlayer::set_animation`] has succeeded.
    pub fn play(&mut self) {
        // Re-arm the completion latch. Without this, replaying an animation that
        // already finished would compare against a `true` left over from the
        // previous run and the *next* completion would never be reported — one of
        // the defects found in sibling Players.
        self.prev_finished = false;
        if let Some(root) = &mut self.root {
            root.context.play();
            self.start_pending = true;
        }
    }

    /// Hold playback on the current frame. Idempotent, and ignored while
    /// nothing is playing. [`SSPlayer::is_playing`] stays true — a hold is not
    /// a stop.
    pub fn pause(&mut self) {
        if let Some(root) = &mut self.root {
            root.context.pause();
        }
    }

    /// Lift a [`SSPlayer::pause`], carrying on from the same frame. Idempotent,
    /// and a no-op on a stopped animation — starting one is [`SSPlayer::play`]'s
    /// job, and it rewinds.
    pub fn resume(&mut self) {
        if let Some(root) = &mut self.root {
            root.context.resume();
        }
    }

    /// End playback. The playhead stays where it stands; nothing rewinds, so the
    /// host keeps drawing the frame it stopped on.
    pub fn stop(&mut self) {
        // A `play` stopped before any tick ran never started.
        self.start_pending = false;
        if let Some(root) = &mut self.root {
            root.context.stop();
        }
    }

    /// Scale playback by `speed`, a multiple of the frame rate. `0.0` and
    /// negatives are a **stop**, not reverse playback -- that is
    /// [`SSPlayer::set_playback_direction`] -- and read back from
    /// [`SSPlayer::speed_scale`] as the `0.0` they became.
    ///
    /// Survives [`SSPlayer::set_animation`] and a pack change, and may be set
    /// before the first animation.
    pub fn set_speed_scale(&mut self, speed: f32) {
        self.transport.speed_scale = speed;
        if let Some(root) = &mut self.root {
            root.context.set_speed_scale(speed);
        }
    }

    /// The speed in effect, as a multiple of the frame rate. Read from the
    /// runtime, so a rate that clamped to a stop reads back as `0.0` rather than
    /// as the negative that was asked for.
    pub fn speed_scale(&self) -> f32 {
        // The runtime stores anything not above zero as the stop it is.
        self.root.as_ref().map(|r| r.context.get_speed_scale()).unwrap_or(self.transport.speed_scale.max(0.0))
    }

    pub fn is_playing(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_playing()).unwrap_or(false)
    }

    /// Held in place rather than stopped: [`SSPlayer::is_playing`] stays true.
    /// False before the first [`SSPlayer::play`] and after [`SSPlayer::stop`] —
    /// the runtime keeps "paused" implying "playing".
    pub fn is_pausing(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_pausing()).unwrap_or(false)
    }

    /// Which way the playhead is actually travelling, which is **not** the
    /// heading [`SSPlayer::set_playback_direction`] configured: under ping-pong
    /// the return leg runs backward while [`SSPlayer::playback_direction`] still
    /// reads forward. A negative speed does not flip it either — that is a stop.
    ///
    /// This is the gate to put in front of audio. SpriteStudio has no reverse
    /// audio, so an [`crate::SsEvent::Audio`] crossed on a backward leg is not
    /// meant to sound; the sibling players that own an audio stack apply exactly
    /// this test before playing one.
    ///
    /// True with no animation set up: forward is the resting state, and
    /// reporting backward would gate a host's audio off on its first frame.
    pub fn is_playing_forward(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_playing_forward()).unwrap_or(true)
    }

    /// Every configured loop has played. Sticky until the next play or
    /// [`SSPlayer::set_animation`], and never true under an infinite loop.
    pub fn is_finished(&self) -> bool {
        self.root.as_ref().map(|r| r.context.is_finished()).unwrap_or(false)
    }

    // --- Playhead ----------------------------------------------------------
    //
    // Everything below reads or writes the ROOT context only. Instance children
    // run their own clocks, driven by their slots from the root's frame, so
    // seeking the root carries them with it on the next update.

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
    // NOTE: everything here holds across `set_animation` and a pack change, and
    // may be set before the first animation, as on every other Player. The
    // runtime keeps the speed, the frame skip, the frame-rate override and the
    // direction across a setup on one context; this Player also builds a new
    // context for its first animation and for a replaced pack, so it holds
    // those in `transport` and hands them to each root it builds. The loop
    // count is the exception the runtime makes on purpose — the transition is
    // what states a count — so it is held too and put back after every setup,
    // exactly as the others do.
    //
    // What does NOT survive is the playback SECTION: frame numbers mean nothing
    // against a different clip.

    /// `count` cycles then stop; `-1` (or `0`) loops forever. Under
    /// [`PlaybackStyle::PingPong`] one cycle is a full round trip.
    pub fn set_loop_count(&mut self, count: i32) {
        self.loop_count = Some(count);
        if let Some(root) = &mut self.root {
            root.context.set_loop_count(count);
        }
    }

    /// The configured count, and how many cycles are left of it.
    pub fn loop_count(&self) -> i32 {
        self.root.as_ref().map(|r| r.context.get_loop_count()).unwrap_or(0)
    }

    /// See [`SSPlayer::loop_count`].
    pub fn remaining_loops(&self) -> i32 {
        self.root.as_ref().map(|r| r.context.get_remaining_loops()).unwrap_or(0)
    }

    /// Override the advance rate. The pack's authored rate otherwise, and `0`
    /// or below goes back to it — prefer [`SSPlayer::set_speed_scale`], which
    /// scales playback without replacing it.
    ///
    /// Survives [`SSPlayer::set_animation`] and a pack change, and may be set
    /// before the first animation.
    pub fn set_frame_rate(&mut self, fps: i32) {
        self.transport.frame_rate = fps;
        if let Some(root) = &mut self.root {
            root.context.set_frame_rate(fps);
        }
    }

    /// See [`SSPlayer::set_frame_rate`]. Before an animation is set, the
    /// override, or zero without one.
    pub fn frame_rate(&self) -> i32 {
        self.root.as_ref().map(|r| r.context.get_frame_rate()).unwrap_or(self.transport.frame_rate.max(0))
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

    // --- Tracks ------------------------------------------------------------
    //
    // Pass-throughs to the runtime's track stack, on the ROOT context. Tracks
    // fold onto the setup pose in ascending order; track 0 is what
    // [`SSPlayer::set_animation`] establishes and carries the structure. A
    // track's `alpha` is its strength; a source's `weight` only sets
    // proportions among several animations sharing one track.
    //
    // Instance children are not layered: they run their own contexts, driven by
    // their slots from the root's frame.

    /// Put `anim_name` on `track`, replacing what it was playing. Track 0 is
    /// better set with [`SSPlayer::set_animation`], which rebuilds the instance
    /// tree; this is for the overlay tracks above it.
    pub fn set_track_animation(&mut self, track: u32, anim_name: &str) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_track_animation(track, anim_name)
    }

    /// How much of `track` survives onto the pose the tracks below it produced.
    pub fn set_track_alpha(&mut self, track: u32, alpha: f32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_track_alpha(track, alpha)
    }

    pub fn track_alpha(&self, track: u32) -> Option<f32> {
        self.root.as_ref()?.context.get_track_alpha(track)
    }

    /// [`TrackBlend::Mix`] lerps toward the track's pose; [`TrackBlend::Additive`]
    /// adds its offset from the setup pose, leaving what is below showing through.
    pub fn set_track_blend(&mut self, track: u32, blend: TrackBlend) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_track_blend(track, blend)
    }

    pub fn track_blend(&self, track: u32) -> Option<TrackBlend> {
        self.root.as_ref()?.context.get_track_blend(track)
    }

    /// Where the animations laid on `track` start: [`TrackSync::Free`] from the
    /// head of their section, [`TrackSync::Start`] at the phase track 0 stands
    /// at, so a run laid over a walk keeps its stride. Set it before laying;
    /// [`SSPlayer::set_animation`] resets every track to `Free`.
    pub fn set_track_sync(&mut self, track: u32, sync: TrackSync) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_track_sync(track, sync)
    }

    pub fn track_sync(&self, track: u32) -> Option<TrackSync> {
        self.root.as_ref()?.context.get_track_sync(track)
    }

    pub fn track_count(&self) -> u32 {
        self.root.as_ref().map(|r| r.context.get_track_count()).unwrap_or(0)
    }

    /// Silence a track. Track 0 is refused — fade it to alpha 0 instead.
    pub fn clear_track(&mut self, track: u32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.clear_track(track)
    }

    /// Fade `track` to `anim_name` over `duration_ms`, starting from whatever it
    /// is playing. Advanced by [`SSScene::update`].
    ///
    /// On track 0 this does **not** rebuild the instance tree the way
    /// [`SSPlayer::set_animation`] does, so cross-fading to an animation whose
    /// instance parts differ is out of scope — the tree stays as the current
    /// animation built it.
    ///
    /// [`SSScene::update`]: crate::SSScene::update
    pub fn crossfade(&mut self, track: u32, anim_name: &str, duration_ms: f32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.crossfade(track, anim_name, duration_ms)
    }

    /// Drive `track`'s alpha to `target` over `duration_ms`. Fading track 0 to 0
    /// settles the pose onto the setup pose.
    pub fn fade_track(&mut self, track: u32, target: f32, duration_ms: f32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.fade_track(track, target, duration_ms)
    }

    pub fn track_fade_progress(&self, track: u32) -> Option<f32> {
        self.root.as_ref()?.context.get_track_fade_progress(track)
    }

    pub fn cancel_track_fade(&mut self, track: u32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.cancel_track_fade(track)
    }

    /// Seek a track's animation — how a continuous-control axis is driven:
    /// author it as an animation and scrub it with the tracked value.
    pub fn set_track_frame(&mut self, track: u32, frame: f32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_track_frame(track, frame)
    }

    pub fn track_frame(&self, track: u32) -> Option<f32> {
        self.root.as_ref()?.context.get_track_frame(track)
    }

    pub fn set_track_speed(&mut self, track: u32, speed: f32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_track_speed(track, speed)
    }

    // --- Sources: several animations on one track ------------------------------
    //
    // A track can carry more than one animation, mixed by `weight` — a walk and a
    // run on track 0 blended by the character's speed. Each one is a *source*,
    // named by the handle these return; handles stay valid until the source is
    // released (a crossfade completing, `clear_track`, `set_animation`).

    /// The source carrying `track`'s animation — the one `set_track_animation`,
    /// `crossfade` or `set_animation` put there.
    pub fn track_main_source(&self, track: u32) -> Option<u32> {
        self.root.as_ref()?.context.get_track_main_source(track)
    }

    /// Lay `anim_name` on `track` beside what it already carries, at `weight`,
    /// and return its handle. On an empty track it becomes the main source.
    /// It starts per the track's [`TrackSync`].
    pub fn add_track_source(&mut self, track: u32, anim_name: &str, weight: f32) -> Result<u32, SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.add_track_source(track, anim_name, weight)
    }

    /// A source's share **within its track**: weights are ratios, so `(0.7, 0.3)`
    /// and `(7, 3)` mix alike. The track's strength is [`SSPlayer::set_track_alpha`].
    pub fn set_source_weight(&mut self, source: u32, weight: f32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_source_weight(source, weight)
    }

    pub fn source_weight(&self, source: u32) -> Option<f32> {
        self.root.as_ref()?.context.get_source_weight(source)
    }

    /// Seek one source. Sources of different lengths drift apart; this is how a
    /// host keeps them in step.
    pub fn set_source_frame(&mut self, source: u32, frame: f32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_source_frame(source, frame)
    }

    pub fn source_frame(&self, source: u32) -> Option<f32> {
        self.root.as_ref()?.context.get_source_frame(source)
    }

    /// The weight at or above which `source` fires its events. Only track 0's
    /// main source fires by default (the default threshold is `+∞`); lower this
    /// to the source's weight or below to have it fire too.
    pub fn set_source_event_threshold(&mut self, source: u32, threshold: f32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_source_event_threshold(source, threshold)
    }

    pub fn source_event_threshold(&self, source: u32) -> Option<f32> {
        self.root.as_ref()?.context.get_source_event_threshold(source)
    }

    /// Which way the playhead walks, and what it does at the far end. Taken
    /// together because the runtime sets them as a pair — pass the current value
    /// for whichever you are not changing.
    ///
    /// Survives [`SSPlayer::set_animation`] and a pack change, and may be set
    /// before the first animation.
    pub fn set_playback_direction(&mut self, direction: PlaybackDirection, style: PlaybackStyle) {
        self.transport.direction = direction;
        self.transport.style = style;
        if let Some(root) = &mut self.root {
            root.context.set_playback_direction(direction.into(), style.into());
        }
    }

    /// See [`SSPlayer::set_playback_direction`].
    pub fn playback_direction(&self) -> PlaybackDirection {
        self.root.as_ref().map(|r| r.context.get_playback_direction().into()).unwrap_or(self.transport.direction)
    }

    /// See [`SSPlayer::set_playback_direction`].
    pub fn playback_style(&self) -> PlaybackStyle {
        self.root.as_ref().map(|r| r.context.get_playback_style().into()).unwrap_or(self.transport.style)
    }

    /// Whether the frame handed to the runtime **for display** keeps its
    /// fraction. On by default here.
    ///
    /// The playhead is an `f32` either way — [`SSPlayer::frame_no`] reports it
    /// unrounded, and the instance / effect slots are always stepped from the
    /// unrounded value, because truncating there would freeze a child whenever a
    /// tick is worth less than a frame. Turning this off only floors what is
    /// drawn, which is what pins playback to whole authored frames.
    ///
    /// Distinct from [`SSPlayer::set_frame_skip_enabled`], which is about *time*: skip
    /// frames decides whether a long tick catches up or advances one frame; this
    /// decides whether what is drawn may sit between two of them.
    pub fn set_sub_frame_enabled(&mut self, enabled: bool) {
        self.sub_frame = enabled;
    }

    /// See [`SSPlayer::set_sub_frame_enabled`].
    pub fn sub_frame_enabled(&self) -> bool {
        self.sub_frame
    }

    /// How a long tick is spent. On (the default) advances by the elapsed time
    /// and drops the frames in between, keeping playback on the wall clock; off
    /// advances exactly one frame per update, so nothing is skipped but a
    /// stalled host plays in slow motion.
    ///
    /// Survives [`SSPlayer::set_animation`] and a pack change, and may be set
    /// before the first animation.
    pub fn set_frame_skip_enabled(&mut self, skip: bool) {
        self.transport.frame_skip = skip;
        if let Some(root) = &mut self.root {
            root.context.set_skip_frames(skip);
        }
    }

    /// See [`SSPlayer::set_frame_skip_enabled`].
    pub fn frame_skip_enabled(&self) -> bool {
        self.root.as_ref().map(|r| r.context.get_skip_frames()).unwrap_or(self.transport.frame_skip)
    }

    // --- Per-part overrides -------------------------------------
    //
    // Say "make this part this colour / this cell / hidden **now**", beating the
    // keyframes underneath. The runtime owns the override table and its whole
    // lifecycle, so these are one-shot calls: never drive them from a per-frame
    // loop, and expect nothing to be re-applied for you.
    //
    // Two things to know before reaching for them:
    //
    // * Part indices address the ROOT animation, and are stable within one pack.
    // Resolve a name once with [`SSPlayer::find_part_index`] and keep the
    // index if you override often.
    // * **Overrides do not reach inside an instance part.** Its child animation
    // runs as its own context, so an override on the part that
    // *hosts* an instance does not touch what the instance draws.
    //
    // Every call needs an animation: the runtime sizes its override table in
    // `set_animation`, which is also what clears everything but
    // [`OverridePriority::Permanent`]. Overriding before selecting an animation
    // is therefore both rejected and pointless — override after, never before.

    /// Resolve a part name to its index in the current animation, or `None` when
    /// the name is unknown or no animation is set up.
    ///
    /// Only worth calling to cache the index across many frames — every override
    /// below takes the name directly and resolves it for you.
    pub fn find_part_index(&self, name: &str) -> Option<u32> {
        let root = self.root.as_ref()?;
        match root.context.find_part_index(name) {
            index if index >= 0 => Some(index as u32),
            _ => None,
        }
    }

    /// `find_part_index` with the failure turned into the error the override
    /// methods return, so each of them stays one line.
    fn resolve_part(&self, name: &str) -> Result<u32, SSRuntimeError> {
        self.find_part_index(name).ok_or(SSRuntimeError::InvalidParameter)
    }

    /// Force-hide a part, or revert it to the animation.
    ///
    /// `cascade` propagates a force-hide to the part's descendants. A keyframe
    /// `hide` never cascades, so this is the only way to hide a whole sub-tree.
    /// Visibility has no priority mode: it always beats the keyframe, and is
    /// always cleared when a new animation is set up.
    ///
    /// Errors with `InvalidParameter` when the name is not in the current
    /// animation. Take [`SSPlayer::set_part_visibility_override_by_index`] when
    /// you already hold the index.
    pub fn set_part_visibility_override(&mut self, part: &str, hide: bool, cascade: bool) -> Result<(), SSRuntimeError> {
        let index = self.resolve_part(part)?;
        self.set_part_visibility_override_by_index(index, hide, cascade)
    }

    /// Index form of [`SSPlayer::set_part_visibility_override`].
    pub fn set_part_visibility_override_by_index(&mut self, part: u32, hide: bool, cascade: bool) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_part_visibility_override(part, u8::from(hide), cascade)
    }

    /// Revert a part to the animation's own visibility.
    pub fn clear_part_visibility_override(&mut self, part: &str) -> Result<(), SSRuntimeError> {
        let index = self.resolve_part(part)?;
        self.clear_part_visibility_override_by_index(index)
    }

    /// Index form of [`SSPlayer::clear_part_visibility_override`].
    pub fn clear_part_visibility_override_by_index(&mut self, part: u32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.clear_part_visibility_override(part)
    }

    /// Override a part's colour with one value across all four corners.
    ///
    /// `rgba` is packed `0xRRGGBBAA` in the same 8-bit sRGB space as the
    /// authored PartColor: the runtime multiplies its alpha with the part's
    /// composed alpha itself, so do not pre-multiply or gamma-convert it here.
    /// That alpha is also the strength, which is why `BlendOp::Mix` is the plain
    /// case and the value to pass when you have no reason to pass another.
    ///
    /// Normal parts only. Others ignore it silently — `Ok` means the override
    /// was recorded, not that anything looks different.
    ///
    /// The C ABI takes `blend_op` before the colour; this face deliberately does
    /// not (see the SDK's Player API conventions).
    pub fn set_part_color_override(&mut self, part: &str, rgba: u32, blend_op: BlendOp, priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let index = self.resolve_part(part)?;
        self.set_part_color_override_by_index(index, rgba, blend_op, priority)
    }

    /// Index form of [`SSPlayer::set_part_color_override`].
    pub fn set_part_color_override_by_index(&mut self, part: u32, rgba: u32, blend_op: BlendOp, priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_part_color_override(part, blend_op.as_wire(), rgba, priority.as_wire())
    }

    /// Override a part's colour with four distinct corners, for a gradient.
    ///
    /// Same contract as [`SSPlayer::set_part_color_override`]. `corners` is in
    /// the runtime's order — **left-top, right-top, left-bottom, right-bottom**,
    /// each `[r, g, b, a]` — and getting that order wrong flips the gradient
    /// silently rather than failing. One argument rather than four, so the pair
    /// of colour methods reads the same.
    pub fn set_part_color_override_corners(&mut self, part: &str, corners: [[u8; 4]; 4], blend_op: BlendOp, priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let index = self.resolve_part(part)?;
        self.set_part_color_override_corners_by_index(index, corners, blend_op, priority)
    }

    /// Index form of [`SSPlayer::set_part_color_override_corners`].
    pub fn set_part_color_override_corners_by_index(&mut self, part: u32, corners: [[u8; 4]; 4], blend_op: BlendOp, priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.set_part_color_override_corners(part, blend_op.as_wire(), corners, priority.as_wire())
    }

    /// Revert a part to the animation's own colour. Clears either kind of colour
    /// override — they share one slot per part.
    pub fn clear_part_color_override(&mut self, part: &str) -> Result<(), SSRuntimeError> {
        let index = self.resolve_part(part)?;
        self.clear_part_color_override_by_index(index)
    }

    /// Index form of [`SSPlayer::clear_part_color_override`].
    pub fn clear_part_color_override_by_index(&mut self, part: u32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.clear_part_color_override(part)
    }

    /// Make a part draw a different cell.
    ///
    /// `cellmap_name` is written without the `.ssce` extension. Both names are
    /// hashed here, so pass them exactly as they appear in the pack — see
    /// [`SSPlayer::cellmap_names`] and [`SSPlayer::cell_names`] for what is
    /// available. Normal and Mask parts only; errors when the cell cannot be
    /// resolved against the bound `.ssab`.
    pub fn set_part_cell_override(&mut self, part: &str, cellmap_name: &str, cell_name: &str, priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let index = self.resolve_part(part)?;
        self.set_part_cell_override_by_index(index, cellmap_name, cell_name, priority)
    }

    /// Index form of [`SSPlayer::set_part_cell_override`].
    pub fn set_part_cell_override_by_index(&mut self, part: u32, cellmap_name: &str, cell_name: &str, priority: OverridePriority) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        let cellmap_hash = ssruntime::fnv1a_hash_str(cellmap_name);
        let cell_hash = ssruntime::fnv1a_hash_str(cell_name);
        root.context.set_part_cell_override(part, cellmap_hash, cell_hash, priority.as_wire())
    }

    /// Revert a part to the animation's own cell.
    pub fn clear_part_cell_override(&mut self, part: &str) -> Result<(), SSRuntimeError> {
        let index = self.resolve_part(part)?;
        self.clear_part_cell_override_by_index(index)
    }

    /// Index form of [`SSPlayer::clear_part_cell_override`].
    pub fn clear_part_cell_override_by_index(&mut self, part: u32) -> Result<(), SSRuntimeError> {
        let root = self.root.as_mut().ok_or(SSRuntimeError::NotLoaded)?;
        root.context.clear_part_cell_override(part)
    }

    /// Drop every override on the root animation at once.
    pub fn clear_all_part_overrides(&mut self) {
        if let Some(root) = &mut self.root {
            root.context.clear_all_part_overrides();
        }
    }

    // --- Events -------------------------------------------------
    //
    // Polled, not called back. The borrow model has the host lending the player
    // its device per frame and driving the clock, so "read what happened after
    // update" is the shape that already fits — and it is the only shape that
    // survives being wrapped by both `js` and `ffi` without becoming a different
    // API on each.
    //
    // All four read the tick that `SSScene::update` just ran, and are replaced
    // by the next one.

    /// Whether the last update was the first to run after a [`SSPlayer::play`]
    /// or [`SSPlayer::play_from`] — the start edge, true for exactly one update.
    ///
    /// **Not raised by [`SSPlayer::resume`]**: lifting a pause carries on, it
    /// does not start. The tick reports it rather than the call, so a `play` on
    /// a [culled](SSPlayer::set_culled) player is reported by the first tick that
    /// runs once it is unculled, and a `play` that [`SSPlayer::stop`] or
    /// [`SSPlayer::set_animation`] overtook before any tick ran is not reported.
    ///
    /// After a `play` of your own the call site already knows. This is for a
    /// start something else made — a declarative binding that plays whatever
    /// animation it is handed, as the Bevy adapter does.
    pub fn just_started(&self) -> bool {
        self.started_edge
    }

    /// Whether the last update crossed a loop boundary.
    ///
    /// One report per update however many cycles were crossed, since a long tick
    /// under frame skipping can complete several. Use it for "the walk cycle came
    /// round"; use [`SSPlayer::just_finished`] for "the animation is over".
    ///
    /// [`SSScene::update`]: crate::SSScene::update
    pub fn just_looped(&self) -> bool {
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
    /// reaching the last frame; both are wrong in ways sibling Players
    /// have run into, which is why neither is offered here.
    pub fn just_finished(&self) -> bool {
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
    /// Empty on a tick that crossed nothing, and on a
    /// [culled](SSPlayer::set_culled) player, which does not advance at all. A
    /// player that is merely not [`visible`](SSPlayer::set_visible) still advances,
    /// so it reports its events as usual.
    pub fn events(&self) -> &[SsEvent] {
        &self.events
    }

    // --- Part read-outs ----------------------------------------

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
    pub fn part_transform(&self, part: &str) -> Option<[f32; 6]> {
        self.part_transform_by_index(self.find_part_index(part)?)
    }

    /// Index form of [`SSPlayer::part_transform`], for a caller that already
    /// resolved the name and reads the pose every frame.
    pub fn part_transform_by_index(&self, part: u32) -> Option<[f32; 6]> {
        let root = self.root.as_ref()?;
        let matrices = unsafe { slice_f32(root.context.get_world_matrices_ptr(), root.context.get_world_matrices_len()) };
        let base = (part as usize).checked_mul(16)?;
        // Bounds-checked rather than trusted: these read the runtime's buffer
        // directly, so an out-of-range part would read past it instead of
        // producing the documented None.
        let m = matrices.get(base..base + 16)?;
        Some([m[0], m[4], m[1], m[5], m[12], m[13]])
    }

    /// Whether `part` is hidden on the frame that is currently displayed, or
    /// `None` when the pack has no such part.
    ///
    /// The companion to [`SSPlayer::part_transform`], which reports where a part
    /// *would* be whether or not it is drawn. A host following a part with its
    /// own scene graph needs both: the pose to place its follower, and this to
    /// decide whether to show it — the `on_hide` option the sibling Players
    /// offer on their built-in attachments.
    ///
    /// **Takes `&mut self`, unlike the other read-outs.** The hide flag lives in
    /// the frame's `FrameData`, which the runtime rebuilds into its own buffer on
    /// demand (`Context::get_frame_data`), where the world matrices this reads
    /// alongside are already sitting in memory. A caller reading it every frame
    /// is doing what the renderer does anyway.
    pub fn is_part_hidden(&mut self, part: &str) -> Option<bool> {
        self.is_part_hidden_by_index(self.find_part_index(part)?)
    }

    /// Index form of [`SSPlayer::is_part_hidden`], for a caller that already
    /// resolved the name.
    pub fn is_part_hidden_by_index(&mut self, part: u32) -> Option<bool> {
        // The DISPLAY frame, floored when sub-frame is off -- the same frame the
        // renderer asks for, so this cannot disagree with what is on screen.
        let frame_no = if self.sub_frame { self.frame_no() } else { self.frame_no().floor() };
        let root = self.root.as_mut()?;
        let bytes = root.context.get_frame_data(frame_no).ok()?;
        // SAFETY: the buffer is the runtime's own, built by the call above.
        let frame_data = unsafe { root_as_frame_data_unchecked(bytes) };
        let parts = frame_data.parts();
        if part as usize >= parts.len() {
            return None;
        }
        Some(parts.get(part as usize).hide())
    }

    /// Whether `part` is a **skinned** mesh — a mesh part whose vertices are
    /// drawn by bone skinning — or `None` when the pack has no such part.
    ///
    /// **Why a host wants it.** [`SSPlayer::part_transform`] reports a part's own
    /// node transform, which for a skinned mesh stays at its setup position
    /// (usually near the character root) while the visible vertices are pulled
    /// around by their bones. So pinning a follower to a skinned mesh snaps it to
    /// the root rather than onto the art. A host offering a "follow this part"
    /// picker can use this to warn, or to steer the choice to a null / bone part,
    /// which *does* carry its transform. It does not change what plays — the
    /// transform is faithful to the node, this only names when that is not what a
    /// follower wants.
    ///
    /// A **rigid** (deform-only) mesh is not skinned and follows correctly, so it
    /// reports `false`, as does every non-mesh part. The test is one bone binding
    /// or more, matching the sibling Players' `is_part_skinned_mesh`.
    ///
    /// Unlike [`SSPlayer::is_part_hidden`], this is a static property of the pack,
    /// not a per-frame state, so it takes `&self` and reads the `.ssab` directly.
    pub fn is_part_skinned_mesh(&self, part: &str) -> Option<bool> {
        self.is_part_skinned_mesh_by_index(self.find_part_index(part)?)
    }

    /// Index form of [`SSPlayer::is_part_skinned_mesh`], for a caller that already
    /// resolved the name.
    pub fn is_part_skinned_mesh_by_index(&self, part: u32) -> Option<bool> {
        let pack = self.packs.get(self.main_pack.as_ref()?)?;
        let bytes = pack.resource().data();
        // SAFETY: the pack's bytes, which the host guaranteed are a valid `.ssab` when it
        // loaded them (`SSPack::build`).
        let ab = unsafe { root_as_ss_anime_binary_unchecked(bytes.as_slice()) };
        let parts = ab.parts();
        if part as usize >= parts.len() {
            return None;
        }
        // Skinned == bound to at least one bone. A rigid (deform-only) mesh has a
        // binding with no influence bones, and a non-mesh part has none at all;
        // both are followable, so both report false.
        let skinned = parts.get(part as usize).mesh_binding().and_then(|b| b.influence_bone()).is_some_and(|bones| !bones.is_empty());
        Some(skinned)
    }

    /// Place this player: translation, scale, then rotation in radians, composed
    /// into the root transform of its tree.
    ///
    /// `y` is measured **downward** from the centre, matching the Y-down space
    /// the players and their [`SSPlayer::part_transform`] output live in (and
    /// ForWeb's `offsetY`). Before v7.0.0-beta.2 this axis pointed up.
    ///
    /// The per-player half of placement: the projection belongs to the render
    /// target ([`SSScene::set_viewport`](crate::SSScene::set_viewport)), the
    /// placement to the player.
    /// It costs nothing extra to draw: the tree walk already composes an
    /// arbitrary parent matrix for every instance child, so a non-identity root
    /// simply rides along.
    pub fn set_transform(&mut self, x: f32, y: f32, scale_x: f32, scale_y: f32, rotation: f32) {
        let (sin, cos) = rotation.sin_cos();
        // Column-major (see `mat4_mul` / `apply_transform`): the columns are
        // scale*rotation for x and y, then z, then the translation.
        #[rustfmt::skip]
        let m = [
            scale_x * cos, scale_x * sin, 0.0, 0.0,
            -scale_y * sin, scale_y * cos, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            x, y, 0.0, 1.0,
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

    /// Draw nothing when false, **while the timeline keeps running**: the
    /// playhead advances, events, loops and completions are reported, and the
    /// part read-outs ([`SSPlayer::part_transform`], [`SSPlayer::is_part_hidden`])
    /// follow the animation. Only the geometry is skipped.
    ///
    /// This is what `visible` means on the other Players, so hiding a character
    /// for a moment — a blink, a flicker — does not slow its animation down.
    /// To stop paying for a character that cannot be seen at all, use
    /// [`SSPlayer::set_culled`] instead.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Skip this player entirely when true: `SSScene::update` advances no
    /// timeline and produces no geometry, so `SSScene::draw` records nothing and
    /// the player costs nothing. The time spent culled is dropped, not caught up:
    /// uncull it and it carries on from where it stopped.
    ///
    /// For a host that knows the player cannot be seen — outside the viewport, or
    /// scrolled away. Because the timeline stops, it is the wrong tool for an
    /// effect: a character blinked with it plays in slow motion. Use
    /// [`SSPlayer::set_visible`] for that, or [`SSPlayer::set_alpha`] for a fade.
    pub fn set_culled(&mut self, culled: bool) {
        self.culled = culled;
    }

    pub fn is_culled(&self) -> bool {
        self.culled
    }

    /// Whether `SSScene::update` builds geometry for this player.
    pub(crate) fn draws(&self) -> bool {
        self.visible && !self.culled
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

    /// Player-level tint: `0xRRGGBB`, multiplied into every part the player
    /// draws — its instance children included, since the whole tree is one
    /// player. White (`0xFFFFFF`, the default) leaves colours untouched.
    ///
    /// It lands *after* the PartsColor blend, which is what makes it a tint on
    /// the finished part rather than another term in that formula. Mask coverage
    /// is unaffected, exactly as with [`SSPlayer::set_alpha`]: a tinted player
    /// clips the shape it always did.
    pub fn set_tint(&mut self, rgb: u32) {
        self.tint = rgb & 0x00FF_FFFF;
    }

    pub fn tint(&self) -> u32 {
        self.tint
    }

    /// Names of all animations in the main pack, if loaded.
    /// The animation currently set up, or `None` before the first
    /// [`SSPlayer::set_animation`] — and again after a pack registration
    /// replaced what the tree was built from, which is what makes the next
    /// `set_animation` rebuild rather than recognise the name as already set up.
    pub fn current_animation(&self) -> Option<&str> {
        self.current_animation.as_deref()
    }

    pub fn animation_names(&self) -> Option<Vec<String>> {
        let anim_resource = self.packs.get(self.main_pack.as_ref()?)?.resource().resource()?;
        Some(anim_resource.get_animation_names())
    }

    /// Every part name in the main pack, in part-index order.
    ///
    /// `None` until a pack is bound. This is what turns the `part_index` an
    /// [`SsEvent`](crate::SsEvent) carries back into something a human reads, and
    /// what fills a part picker; `len()` is the part count.
    ///
    /// Reads the `.ssab` parts table, which is static for the resource, rather
    /// than [`SSPlayer::part_count`] — that one is derived from the world-matrix
    /// buffer and is zero until an animation has been set up and stepped.
    ///
    /// A part authored without a name holds its slot as an empty string, so the
    /// vector stays index-aligned with `part_index`.
    pub fn part_names(&self) -> Option<Vec<String>> {
        let pack = self.packs.get(self.main_pack.as_ref()?)?;
        let bytes = pack.resource().data();
        // SAFETY: the pack's bytes, which the host guaranteed are a valid `.ssab` when it
        // loaded them (`SSPack::build`).
        let ab = unsafe { root_as_ss_anime_binary_unchecked(bytes.as_slice()) };
        let parts = ab.parts();
        Some((0..parts.len()).map(|i| parts.get(i).name().to_string()).collect())
    }

    /// Every cellmap name in the main pack, without the `.ssce` extension — the
    /// namespace [`SSPlayer::set_part_cell_override`] takes.
    ///
    /// `None` until a pack is bound. Reads the `.ssab` directly rather than the
    /// texture list, because a cellmap whose atlas is embedded still has a name.
    pub fn cellmap_names(&self) -> Option<Vec<String>> {
        let pack = self.packs.get(self.main_pack.as_ref()?)?;
        let bytes = pack.resource().data();
        // SAFETY: the pack's bytes, which the host guaranteed are a valid `.ssab` when it
        // loaded them (`SSPack::build`).
        let ab = unsafe { root_as_ss_anime_binary_unchecked(bytes.as_slice()) };
        Some(ab.cellmaps()?.iter().map(|c| c.name().to_string()).collect())
    }

    /// The cell names inside one cellmap, or `None` when no pack is bound or the
    /// cellmap is not in it. The other half of what
    /// [`SSPlayer::set_part_cell_override`] takes.
    pub fn cell_names(&self, cellmap_name: &str) -> Option<Vec<String>> {
        let pack = self.packs.get(self.main_pack.as_ref()?)?;
        let bytes = pack.resource().data();
        // SAFETY: as above.
        let ab = unsafe { root_as_ss_anime_binary_unchecked(bytes.as_slice()) };
        let cellmap = ab.cellmaps()?.iter().find(|c| c.name() == cellmap_name)?;
        Some(cellmap.cells()?.iter().map(|c| c.name().to_string()).collect())
    }

    /// The atlases a registered pack asks for, or `None` if
    /// `pack_name` names no pack this player holds. `None` selects the main pack.
    ///
    /// This is the reference-mode half of texture supply, and it exists because
    /// only the `.ssab` knows where its atlases were exported to: a host calls it
    /// straight after [`SSPlayer::add_ssab`] / [`SSPlayer::load_ssab`], loads
    /// every entry that is not [`TextureRef::embedded`] by resolving
    /// [`TextureRef::path`] against the directory the `.ssab` came from, and hands
    /// the pixels to [`SSRenderer::upload_texture`] **under
    /// [`SSPlayer::texture_key`]'s answer for that entry**, not under
    /// [`TextureRef::slot`]. The player never opens a file itself — it has no
    /// filesystem on the web face, and on any face the host's asset system is the
    /// one that should own the bytes.
    /// Where a pack's cellmap registers in the renderer's texture map.
    ///
    /// [`TextureRef::slot`] names a *slot inside one `.ssab`*, and two packs from
    /// two Adobe Animate projects both call theirs `spritemap1`. The renderer's
    /// map is device-scoped — that is what keeps N players of one pack to one
    /// upload — so the player assigns each atlas a key that identifies the image
    /// and translates every draw batch through it. This is that translation, and
    /// it is what a host supplying its own pixels has to register under:
    ///
    /// ```no_run
    /// # use ssplayer_wgpu::{Retention, SSPlayer, SSRenderer};
    /// # fn go(player: &SSPlayer, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, rgba: &[u8]) {
    /// for texture in player.textures(None).unwrap_or(&[]) {
    ///     let key = player.texture_key(None, texture.slot);
    ///     renderer.upload_texture(device, queue, key, rgba, (1024, 1024), Retention::Retain);
    /// }
    /// # }
    /// ```
    ///
    /// Answers `cellmap_hash` itself for a pack whose atlases the player has
    /// never seen, so a host that registered everything against the renderer
    /// directly before this existed keeps working unchanged — and pays for it
    /// with the collision this exists to remove.
    pub fn texture_key(&self, pack_name: Option<&str>, slot: u32) -> u32 {
        let Some(name) = pack_name.or(self.main_pack.as_deref()) else { return slot };
        self.atlas_keys.get(name).map(|keys| keys.resolve(slot)).unwrap_or(slot)
    }

    pub fn textures(&self, pack_name: Option<&str>) -> Option<&[TextureRef]> {
        let name = pack_name.or(self.main_pack.as_deref())?;
        Some(self.packs.get(name)?.textures())
    }

    /// The encoded atlases a registered pack carries, or `None` if
    /// `pack_name` names no pack this player holds. `None` selects the main pack.
    ///
    /// The embedded-mode counterpart of [`SSPlayer::textures`]: there is nothing
    /// for the host to fetch, but there is something for it to decode. A host
    /// with its own decoder walks this and registers the results, rather than
    /// leaving it to the decode the next [`SSScene::update`](crate::SSScene::update)
    /// runs — which needs the `embedded` feature and only knows how to decode PNG.
    pub fn embedded_images(&self, pack_name: Option<&str>) -> Option<Vec<EmbeddedImage<'_>>> {
        let name = pack_name.or(self.main_pack.as_deref())?;
        Some(self.packs.get(name)?.embedded_images())
    }

    // --- Runtime texture replacement ---------------------------
    //
    // Swapping the texture a cellmap binds *from your program* — costume changes,
    // dynamic expressions — rather than from the authoring data. The SDK defines
    // it as a Player-side responsibility, distinct from the Override Layer API.
    //
    // Where the sibling Players keep a per-player texture map and overwrite an
    // entry in it, this player's texture map is the device-scoped [`SSRenderer`]'s,
    // shared so N players of one character upload it once. So an override here is
    // a *redirect*: the replacement is registered under a private key (unique to
    // this player and this cellmap), and the player's own atlas-key table — which
    // every draw already resolves through — is pointed at it. That keeps the
    // override per player without giving `SSPlayer` a wgpu-typed field, and makes
    // a restore a redirect back to the shared atlas rather than a reload of it.
    //
    // An instance child plays from its own pack and so its own table, so a parent
    // override does not reach it — the same "override is per player instance" the
    // reference Godot Player has, for the same structural reason.

    /// Every cellmap name and external-texture name in a pack, the namespace
    /// [`SSPlayer::set_cellmap_texture`] takes. `None` selects the main pack;
    /// `None` return means no such pack is bound.
    ///
    /// Both tables, deliberately: a `DrawBatch.texture_hash` is a cellmap's
    /// `name_hash` for an ordinary part but a texture *file name*'s hash for one
    /// under an authored TextureChange, so the file names a `TCHG` can select
    /// (`common.png` and the like, including a cellmap's own image) are listed
    /// too — all of them legitimately replaceable (SDK §"Texture resolution").
    pub fn replaceable_cellmap_names(&self, pack_name: Option<&str>) -> Option<Vec<String>> {
        let name = pack_name.or(self.main_pack.as_deref())?;
        let pack = self.packs.get(name)?;
        let bytes = pack.resource().data();
        // SAFETY: the pack's bytes, which the host guaranteed are a valid `.ssab` when it
        // loaded them (`SSPack::build`).
        let ab = unsafe { root_as_ss_anime_binary_unchecked(bytes.as_slice()) };
        let mut names: Vec<String> = Vec::new();
        if let Some(cellmaps) = ab.cellmaps() {
            names.extend(cellmaps.iter().map(|c| c.name().to_string()));
        }
        if let Some(externals) = ab.external_textures() {
            for et in externals {
                if let Some(tex) = et.name()
                    && !names.iter().any(|held| held == tex)
                {
                    names.push(tex.to_string());
                }
            }
        }
        Some(names)
    }

    /// Replace the texture a cellmap binds, from straight-alpha RGBA8 pixels.
    ///
    /// `name` is a cellmap name (no `.ssce`) or a texture file name — one of the
    /// strings [`SSPlayer::replaceable_cellmap_names`] lists; it is hashed here.
    /// `None` selects the main pack. `rgba` is `width * height * 4` bytes and is
    /// premultiplied on the way to the GPU, exactly like a loaded atlas.
    ///
    /// The replacement should share the original atlas's size and cell layout: the
    /// runtime's UVs stay in the *original* cellmap's pixel space, so a different
    /// resolution samples a different region (SDK §"The size of the replacement
    /// texture"). Pass a part under a TextureChange its authored file name to
    /// affect the frames the change is active on — the cellmap name will not,
    /// because those frames bind through a different hash.
    ///
    /// The pixels reach the GPU on the next `update` (the borrow model lends no
    /// device before then), so the swap is visible from the following frame.
    pub fn set_cellmap_texture(&mut self, pack_name: Option<&str>, name: &str, rgba: &[u8], width: u32, height: u32) -> Result<(), SSRuntimeError> {
        if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 {
            return Err(SSRuntimeError::InvalidParameter);
        }
        let key = self.override_cellmap_texture_key(pack_name, name).ok_or(SSRuntimeError::NotLoaded)?;
        // Last write wins, and never leaves a stale copy queued for one key.
        self.pending_overrides.retain(|(queued, ..)| *queued != key);
        self.pending_overrides.push((key, rgba.to_vec(), width, height));
        Ok(())
    }

    /// Redirect a cellmap to its private override key and return that key, without
    /// supplying any pixels — for a host that owns the GPU texture itself (an
    /// engine adapter, or the web face registering an `ImageBitmap`). The caller
    /// registers a texture under the returned key against the [`SSRenderer`]; every
    /// draw of this player's cellmap then resolves to it.
    ///
    /// `None` on an unbound player (no pack to attach the override to). The lower
    /// half of [`SSPlayer::set_cellmap_texture`], and the seam
    /// `SSPlayer::restore_cellmap_texture` reverses.
    pub fn override_cellmap_texture_key(&mut self, pack_name: Option<&str>, name: &str) -> Option<u32> {
        let pack = self.resolve_pack_name(pack_name)?;
        let cellmap_hash = ssruntime::fnv1a_hash_str(name);
        let key = override_key(self.id, cellmap_hash);
        // The key a restore returns to is whatever the cellmap resolves to *now*,
        // before this redirect — captured only when the override is first set, so
        // re-overriding does not overwrite it with the override's own key.
        let original = self.atlas_keys.get(&pack).map(|keys| keys.resolve(cellmap_hash)).unwrap_or(cellmap_hash);
        let entry = self.overrides.entry(pack.clone()).or_default();
        if !entry.iter().any(|over| over.cellmap_hash == cellmap_hash) {
            entry.push(CellmapOverride { cellmap_hash, key, original });
        }
        // A pending drop of this key is now stale — the override is live again.
        self.pending_forgets.retain(|queued| *queued != key);
        self.apply_atlas_entries(&pack, vec![(cellmap_hash, key)]);
        Some(key)
    }

    /// Undo a runtime override: point the cellmap back at the shared atlas and
    /// retire the private key. Returns the retired key (so a host that registered
    /// its own texture under it can drop that too), or `None` if the cellmap was
    /// not overridden. `None` pack selects the main pack.
    ///
    /// The original atlas was never touched, so there is nothing to reload — the
    /// redirect back is the whole of it. The private key's texture is dropped from
    /// the renderer on the next `update`.
    pub fn restore_cellmap_texture(&mut self, pack_name: Option<&str>, name: &str) -> Option<u32> {
        let pack = self.resolve_pack_name(pack_name)?;
        let cellmap_hash = ssruntime::fnv1a_hash_str(name);
        let (over, now_empty) = {
            let overrides = self.overrides.get_mut(&pack)?;
            let pos = overrides.iter().position(|over| over.cellmap_hash == cellmap_hash)?;
            (overrides.remove(pos), overrides.is_empty())
        };
        if now_empty {
            self.overrides.remove(&pack);
        }
        // Drop any queued upload for the key; schedule the renderer entry's drop.
        self.pending_overrides.retain(|(queued, ..)| *queued != over.key);
        self.pending_forgets.push(over.key);
        self.apply_atlas_entries(&pack, vec![(cellmap_hash, over.original)]);
        Some(over.key)
    }

    /// Whether a cellmap currently carries a runtime override. `None` pack selects
    /// the main pack.
    pub fn is_cellmap_overridden(&self, pack_name: Option<&str>, name: &str) -> bool {
        let Some(pack) = self.resolve_pack_name(pack_name) else { return false };
        let cellmap_hash = ssruntime::fnv1a_hash_str(name);
        self.overrides.get(&pack).is_some_and(|overrides| overrides.iter().any(|over| over.cellmap_hash == cellmap_hash))
    }

    /// The pack a texture-override call names, or the main pack for `None`.
    fn resolve_pack_name(&self, pack_name: Option<&str>) -> Option<String> {
        pack_name.map(str::to_string).or_else(|| self.main_pack.clone())
    }

    /// The authored canvas of the currently playing animation, as a box in
    /// player-local units — what [`SSPlayer::fit_into`] fits when the caller
    /// supplies no frame of its own.
    ///
    /// This is the frame the artist composed in, not a measured bounding box, and
    /// the difference is the point: it costs nothing to read, it holds still while
    /// the animation plays, and it is where the author decided the empty space
    /// belongs. A tighter box is the host's to supply through
    /// [`Placement::frame`](crate::fit::Placement::frame) — measuring one here
    /// would mean baking every frame of the animation at load.
    ///
    /// It is the *current* animation's canvas, and one pack's animations may
    /// differ (Ringo's `dead` is 1300 wide, its others 800), so a host that wants
    /// one scale across a switch reads this once, after its first
    /// [`SSPlayer::set_animation`], and passes it as
    /// [`Placement::frame`](crate::fit::Placement::frame) from then on.
    ///
    /// The canvas is placed against the origin by the animation's **pivot**,
    /// which the runtime does not apply (it carries no pivot in its metadata) and
    /// which is normalised from the canvas centre: `(0, -0.5)` is the bottom edge,
    /// so a character authored standing on the ground reports a box entirely above
    /// the origin — negative `y`, because player-local `y` runs downward.
    pub fn canvas_rect(&self) -> Option<crate::fit::Rect> {
        let name = self.current_animation.as_ref()?;
        let pack = self.packs.get(self.main_pack.as_ref()?)?;
        let index = pack.resource().resource()?.find_animation_index(name)?;
        let bytes = pack.resource().data();
        // SAFETY: the pack's bytes, which the host guaranteed are a valid `.ssab` when it
        // loaded them (`SSPack::build`).
        let ab = unsafe { root_as_ss_anime_binary_unchecked(bytes.as_slice()) };
        let animations = ab.animations();
        // The runtime built its metadata by walking this same vector in order, so
        // the index is in range — checked rather than trusted, as the other reads
        // straight out of the buffer are.
        if index >= animations.len() {
            return None;
        }
        let animation = animations.get(index);
        let (width, height) = (animation.canvas_size().v1(), animation.canvas_size().v2());
        let (pivot_x, pivot_y) = (animation.pivot().v1(), animation.pivot().v2());
        // The pivot is where the origin sits inside the canvas, in editor space
        // (Y-up); the box is reported in player-local space (Y-down), which is
        // the one flip in this arithmetic.
        Some(crate::fit::Rect { x: -pivot_x * width - width / 2.0, y: pivot_y * height - height / 2.0, width, height })
    }

    /// Fit the animation into a `width` x `height` box and place it there,
    /// writing the result through [`SSPlayer::set_transform`].
    ///
    /// The box the host lays out is in **pixels of the render target**, so call
    /// this again when that box changes — on a resize, and after
    /// [`SSPlayer::set_animation`] if the packs differ in canvas. It is pure
    /// arithmetic over [`Placement::frame`](crate::fit::Placement::frame) or
    /// [`SSPlayer::canvas_rect`]; nothing is measured and no GPU state is touched.
    ///
    /// The returned [`Placed`](crate::fit::Placed) is what was written, so a host
    /// can read back the scale it got rather than recomputing it — and can tell
    /// the fit from the no-fit answer, which a collapsed box or an animation that
    /// is not set up both produce (see [`solve`](crate::fit::solve)).
    pub fn fit_into(&mut self, width: f32, height: f32, placement: &crate::fit::Placement) -> crate::fit::Placed {
        let content = placement.frame.or_else(|| self.canvas_rect());
        let placed = crate::fit::solve(width, height, content, placement);
        self.set_transform(placed.x, placed.y, placed.scale_x, placed.scale_y, placement.rotation);
        placed
    }

    /// Canvas size of the currently playing animation.
    pub fn canvas_size(&self) -> Option<(f32, f32)> {
        let name = self.current_animation.as_ref()?;
        let anim_resource = self.packs.get(self.main_pack.as_ref()?)?.resource().resource()?;
        let idx = anim_resource.find_animation_index(name)?;
        let anim_meta = &anim_resource.animations()[idx];
        Some((anim_meta.canvas_width, anim_meta.canvas_height))
    }

    /// Hand the renderer any embedded atlas it does not already have.
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
        // Runtime overrides first, and unconditionally: they are host-supplied
        // pixels the renderer must retain (nothing can reproduce them after a
        // device loss), and a restore's drop rides the same flush. A key is in at
        // most one of the two lists — an override clears its pending drop, a
        // restore clears its pending upload.
        for (key, rgba, width, height) in std::mem::take(&mut self.pending_overrides) {
            renderer.upload_texture_owned(device, queue, key, rgba, (width, height), Retention::Retain);
        }
        for key in std::mem::take(&mut self.pending_forgets) {
            renderer.forget_texture(key);
        }

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
        for (key, encoded) in &pending {
            if wanted_now(renderer, *key, encoded) && claimed.insert(*key) {
                // Retained: the bytes came from outside, so this player has nothing
                // it could decode again after a device loss.
                jobs.push((*key, encoded, Retention::Retain));
            }
        }
        for name in &names {
            let Some(pack) = self.packs.get(name) else { continue };
            // Registration decided these (see `add_pack`); reading them back is
            // what keeps the tree's lookups and this upload on one key.
            let keys = self.atlas_keys.get(name);
            for image in pack.embedded_images() {
                let key = keys.map(|keys| keys.resolve(image.slot)).unwrap_or(image.slot);
                if wanted_now(renderer, key, image.bytes) && claimed.insert(key) {
                    // Reproducible: this player still holds the `.ssab` and can
                    // decode the atlas again, so the renderer need not retain a
                    // second copy.
                    jobs.push((key, image.bytes, Retention::Reproducible));
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
        // `culled` means "this player cannot be seen": no timeline advance and no
        // geometry. Textures are still flushed above, so it can be unculled
        // without re-loading anything.
        if self.culled {
            // What the last tick that ran reported is not this tick's news. Leaving
            // it in place made a culled player answer `events()` with the same
            // footstep on every update — a host that plays a sound per event fires
            // it for as long as the player stays culled — and kept a loop or finish
            // edge readable long after it was crossed.
            self.events.clear();
            self.looped = false;
            self.finished_edge = false;
            self.started_edge = false;
            return;
        }

        let Some(root) = &mut self.root else { return };
        // The runtime works in milliseconds for the primary context.
        root.context.update(delta_secs * 1000.0);
        self.started_edge = std::mem::take(&mut self.start_pending);
        let frame_no = root.context.get_frame_no();
        // The loop edge instance/effect slots re-arm their playback keys on. It
        // is a pulse the controller clears on entry to every `update`, so it has
        // to be read here, right after the one that raised it. Deriving it from
        // "the frame went backwards" instead is wrong under reverse and
        // ping-pong playback, where the frame goes backwards every tick.
        let looped = root.context.just_looped();

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

        if !self.visible {
            // Hidden, not culled: the clocks above have moved, but nothing is drawn.
            // The root's frame is still built, because the part read-outs read the
            // world matrices it leaves behind — without it a follower pinned to a
            // hidden character would freeze where the character was last drawn.
            // The display frame, as `prepare_node` asks for it.
            let frame_no = if self.sub_frame { frame_no } else { frame_no.floor() };
            let _ = root.context.get_frame_data(frame_no);
            return;
        }

        let draw = NodeDraw { alpha: self.alpha, tint: pack_tint(self.tint) };
        let mut build = FrameBuild { scene, renderer, sub_frame: self.sub_frame };
        render_tree(root, &mut build, &self.transform, frame_no, draw);
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
fn build_node(packs: &HashMap<String, SSPack>, atlas_keys: &HashMap<String, Arc<AtlasKeys>>, pack: &str, anime: &str) -> Option<PlaybackNode> {
    let resource = packs.get(pack)?.resource();
    let res_ptr = resource.resource_static_ptr();
    if res_ptr.is_null() {
        return None;
    }
    // SAFETY: the resource lives in `packs` for the player's lifetime.
    let static_res = unsafe { &*res_ptr };
    let mut context = Context::new();
    context.bind_resource(static_res);
    // Y-down, matching every sibling Player that draws into a Y-down target
    // (ForWeb, ForFlutter, ForGodot, ForRenPy, ForReactNative). The Brain owns
    // the flip and applies it once at the source — it mirrors the hierarchy by
    // negating `position_y`, `rotation_z` and `pivot_y` — so the baked geometry,
    // the world matrices [`SSPlayer::part_transform`] hands out and the effect
    // particles all agree about which way is up. The projection undoes it for a
    // Y-up target (see `SSScene::set_viewport`), which is the one place the
    // target's own sense is known.
    context.set_coordinate_system(ssruntime::SsCoordinateSystem::YDown);
    context.setup_animation(anime).ok()?;
    let (instances, effects, mask_flags) = build_rig(&context, static_res, packs, atlas_keys);
    Some(PlaybackNode { context, instances, effects, mask_flags, pack: pack.to_string(), atlas_keys: node_atlas_keys(atlas_keys, pack), res: static_res })
}

/// The per-animation half of a node: which instance children it mounts, which
/// effect slots it owns, and the static mask flags its parts carry.
///
/// Split out of [`build_node`] because it is the only half a *switch* has to
/// redo. The context is the animation-independent half — it belongs to the pack,
/// and handing it a new clip through `setup_animation` is what keeps the speed,
/// the frame-rate override, the frame skip and the playback direction across the
/// switch, exactly as every sibling Player gets them kept.
fn build_rig(
    context: &Context<'static>,
    static_res: &'static ssruntime::AnimationResource<'static>,
    packs: &HashMap<String, SSPack>,
    atlas_keys: &HashMap<String, Arc<AtlasKeys>>,
) -> (HashMap<usize, InstanceChild>, HashMap<usize, EffectPart>, Vec<PartMaskFlags>) {
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

            // Static mask flags. Geometry kind picks the
            // writer builder; Text/Nines can't write coverage yet.
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
                        && let Some(child) = build_node(packs, atlas_keys, inst.ref_anime_pack(), inst.ref_anime_name())
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

    (instances, effects, mask_flags)
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
/// not what SpriteStudio means by independent playback (instance doc §Supporting
/// Independent Playback, effect doc). The slots are built from the pack's part
/// list in [`build_node`], so they are all here to walk.
///
/// What the draw walk needs afterwards is left on each slot: the child's frame
/// and visibility, and the effect's draw plan.
fn step_slots(node: &mut PlaybackNode, frame_no: f32, delta_secs: f32, node_looped: bool) {
    let fps = node.context.get_frame_rate() as f32;
    for (&part_idx, effect) in node.effects.iter_mut() {
        let ev = node.context.get_active_effect_event(part_idx as u32);
        // vert_stride=2 (packed XY), y_flip=true: the context runs Y-down, so the
        // particles are mirrored with the parts around them rather than against.
        let r = effect.slot.step(ev, frame_no, delta_secs, fps, node_looped, true, 2);
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

        // On a transition edge the slot configures the child — section, direction,
        // loop count, and the frame it starts on — but does not start it: play()
        // does, so independent playback actually advances.
        if r.transitioned {
            child.node.context.play();
        }
        // The slot's frame, the transition tick included. Reading the controller
        // back after that play() would put a synced child at its section start for
        // one tick instead of where the parent has it.
        // Raw, never rounded: rounding here would truncate sub-frame progress on
        // every tick and freeze the child when a tick is worth less than a frame.
        child.frame_no = r.child_frame_no;
        child.visible = r.visible;

        // Recurse regardless of visibility, and re-arm the grandchildren on the
        // *child's* loop rather than the parent animation's: an independent child
        // runs on its own clock, so that is the edge they must re-fire on.
        step_slots(&mut child.node, child.frame_no, child_delta, r.child_looped);
    }
}

/// The two halves of layer 2, bundled for the tree walk: the scene being
/// filled, and the device-scoped renderer it reads texture
/// sizes from. Only the scene is mutated — pipelines are compiled later, in
/// `end_frame`, which is where the renderer is borrowed mutably.
struct FrameBuild<'a> {
    scene: &'a mut FrameChunk,
    renderer: &'a SSRenderer,
    /// The owning player's [`SSPlayer::set_sub_frame_enabled`]. Carried here rather than
    /// passed down, so it reaches every node of the tree — an instance child is
    /// drawn at its own playhead and has to be floored on the same terms.
    sub_frame: bool,
}

/// How one node of the tree is drawn: how opaque it is and how it is tinted.
///
/// `alpha` changes per recursion level (each instance child folds in its
/// part's opacity), `tint` is the owning player's and is constant for the whole
/// walk. Where a node sits is decided before the walk, in [`prepare_node`].
#[derive(Clone, Copy)]
struct NodeDraw {
    /// Player-level fade; see [`BatchInfo::alpha`]. Never applied to mask
    /// coverage — a faded player clips exactly as it would at full opacity.
    alpha: f32,
    /// Player-level tint, already in the shader's byte order; see
    /// [`BatchInfo::tint`]. Constant for the whole walk.
    tint: u32,
}

/// The mask flags an instance part hands the sub-animation it draws.
///
/// **One level, never a chain**: a sub-animation's parts compose with their own
/// instance part's `mask_influence` (AND) and `visible_inside_mask` (OR), and
/// with nothing above it — a grandparent instance part that opts out of masking
/// does not opt its grandchildren out. And the flags reach the sub-animation
/// whether or not anything above masks. SpriteStudio 7.5 draws it this way; the
/// data that tells the two apart nests an instance that opts out around one that
/// opts in. The identity — a top-level animation called by no instance — is
/// [`InheritedMask::ROOT`].
#[derive(Clone, Copy)]
struct InheritedMask {
    influence: bool,
    visible_inside: bool,
}

impl InheritedMask {
    /// The identity a top-level animation starts from.
    const ROOT: Self = Self { influence: true, visible_inside: false };

    /// What `part`, an instance part, hands its sub-animation: its own flags,
    /// and none of what its own animation inherited.
    fn of_instance(part: &PartMaskFlags) -> Self {
        Self { influence: part.mask_influence, visible_inside: part.visible_inside_mask }
    }

    /// Whether a drawn part is a mask target — its effective `mask_influence`
    /// (own AND inherited) is set. `mask_write` does not make it a target; that
    /// is the writer's own fact.
    fn targets(self, part: &PartMaskFlags) -> bool {
        self.influence && part.mask_influence
    }

    /// Whether a target draws inside the coverage (own OR inherited).
    fn inside(self, part: &PartMaskFlags) -> bool {
        self.visible_inside || part.visible_inside_mask
    }
}

/// One node of the instance tree, made ready for this frame before anything is
/// drawn.
///
/// The whole tree is prepared first because the mask state is the tree's, not a
/// node's: a clipping mask written inside an instance keeps clipping its callers'
/// later parts however deep it sits, so every writer and every part's place in
/// the drawing order has to be known before the first part draws. Preparing is
/// also where each node's frame is fetched and copied into the arena, **once** —
/// fetching a frame again rebuilds the runtime's buffers under any slice still
/// pointing into them.
struct NodePrep<'r> {
    frame: FrameView<'r>,
    slot: NodeSlot,
    /// Where the node draws: the player's placement composed with every instance
    /// part above it.
    transform: [f32; 16],
    /// Node-local to player-local: the chain of instance placements alone. The
    /// coverage bitmap is pinned to the player, not to the screen.
    to_root: [f32; 16],
    ctx: InheritedMask,
    /// Each rank's place on the tree sequence (see [`ScopedWriter`]). Empty when
    /// the scene does not mask.
    seq: Vec<i32>,
    /// `(instance part index, prep index)` for each visible instance child.
    children: Vec<(usize, usize)>,
}

/// Where a pure mask of the top animation opens: before the first part.
const TREE_START: i32 = -1;
/// Where every clipping writer closes: past the last part of the tree.
const TREE_END: i32 = i32::MAX;

/// How one drawn part meets the mask this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PartMask {
    /// Not tested: no mask reaches it, or it opts out.
    Plain,
    /// Tested with this polarity.
    Tested(StencilTest),
    /// Draws only inside a mask and no mask covers it, so it draws nowhere —
    /// also in an animation with no mask at all.
    Hidden,
}

/// The player's mask state for one frame: every writer of the instance tree,
/// placed on the tree sequence, and — for the stencil — how far the drawing has
/// got through adding and removing them.
struct TreeMask {
    backend: crate::MaskBackend,
    /// Capped at [`crate::mask::MAX_MASK_WRITERS`], in drawing order.
    writers: Vec<ScopedWriter>,
    /// The prep each writer's geometry comes from, parallel to `writers`.
    writer_nodes: Vec<usize>,
    /// Stencil only: `(seq, writer, add)`, applied once the drawing passes `seq`.
    events: Vec<(i32, usize, bool)>,
    cursor: usize,
    /// Coverage only: the player-local -> coverage-UV affine and the layer the
    /// tree baked into, for the effect slots the walk points at them.
    coverage: Option<([f32; 6], u32)>,
}

impl TreeMask {
    fn new(backend: crate::MaskBackend) -> Self {
        Self { backend, writers: Vec::new(), writer_nodes: Vec::new(), events: Vec::new(), cursor: 0, coverage: None }
    }

    fn masking(&self) -> bool {
        self.backend != crate::MaskBackend::None
    }

    /// Place one of a node's writers, found at `seq`, on the tree sequence.
    /// `open` is where the node's own animation starts.
    fn place(&mut self, writer: &MaskWriter, seq: i32, open: i32, node: usize, ctx: InheritedMask) {
        if self.writers.len() >= crate::mask::MAX_MASK_WRITERS {
            return;
        }
        // The op follows the effective `mask_influence` — one level, like a
        // target's: inside a `mask_influence == 0` instance a writer lands on the
        // counter plane and adds to its caller's masks instead of cancelling them.
        let writer = MaskWriter { op_invert: writer.op_invert && ctx.influence, ..*writer };
        let (open, close) = if writer.is_clipping { (seq, TREE_END) } else { (open, seq) };
        self.writers.push(ScopedWriter { writer, open, close });
        self.writer_nodes.push(node);
    }

    /// Whether any writer holds its bit at `seq`.
    fn covers(&self, seq: i32) -> bool {
        self.writers.iter().any(|w| w.open < seq && seq < w.close)
    }

    /// How the part at `seq` meets the mask, given its own flags and the ones its
    /// instance part handed down.
    fn part(&self, flags: &PartMaskFlags, ctx: InheritedMask, seq: i32) -> PartMask {
        if !self.masking() || !ctx.targets(flags) {
            return PartMask::Plain;
        }
        let inside = ctx.inside(flags);
        if self.covers(seq) {
            PartMask::Tested(if inside { StencilTest::Inside } else { StencilTest::Outside })
        } else if inside {
            PartMask::Hidden
        } else {
            PartMask::Plain
        }
    }

    /// Stencil only: order every writer's add and removal along the tree.
    ///
    /// A single hardware stencil holds one state at a time, so each writer is
    /// added when the drawing passes its `open` and removed when it passes its
    /// `close`: a pure mask is written before its own animation's first part and
    /// taken out at itself, a clipping mask put in at itself and left for
    /// [`TreeMask::finish`]. Incr/Decr and Invert each stay in their own plane and
    /// commute within it, so writers that share a point may go in any order.
    fn schedule(&mut self) {
        for (i, w) in self.writers.iter().enumerate() {
            self.events.push((w.open, i, true));
            self.events.push((w.close, i, false));
        }
        self.events.sort_by_key(|&(seq, _, _)| seq);
    }

    /// Stencil only: apply every add and removal that falls before `seq`, so the
    /// buffer holds exactly the writers in scope for the part drawn there.
    fn sync(&mut self, build: &mut FrameBuild<'_>, preps: &[NodePrep<'_>], seq: i32) {
        while self.cursor < self.events.len() && self.events[self.cursor].0 < seq {
            self.apply_next(build, preps);
        }
    }

    /// Draw the next scheduled add or removal into the stencil.
    fn apply_next(&mut self, build: &mut FrameBuild<'_>, preps: &[NodePrep<'_>]) {
        let (_, i, add) = self.events[self.cursor];
        let w = &self.writers[i].writer;
        // Increment pairs with decrement; invert is its own inverse.
        let op = match (w.op_invert, add) {
            (true, _) => MaskOp::Invert,
            (false, true) => MaskOp::Incr,
            (false, false) => MaskOp::Decr,
        };
        let prep = &preps[self.writer_nodes[i]];
        build.scene.append_mask_writer(&prep.frame, &prep.slot, w, op);
        self.cursor += 1;
    }

    /// Stencil only: take back everything still applied — the clipping writers,
    /// which hold to the end of the tree, and whatever the drawing never reached.
    ///
    /// Every writer is added once and removed once, so the player leaves the
    /// stencil buffer as it found it. Without this the invariant only holds by
    /// accident: one player per render pass, with `LoadOp::Clear(0)` wiping the
    /// residue between frames. Stack a second player into the same pass and the
    /// first one's mask clips it.
    fn finish(&mut self, build: &mut FrameBuild<'_>, preps: &[NodePrep<'_>]) {
        while self.cursor < self.events.len() {
            self.apply_next(build, preps);
        }
    }
}

/// Transform a node-local bounding rectangle by `m`, returning the rectangle
/// that encloses it. Only the affine 2D part of `m` matters (the runtime's local
/// Z is zero).
fn transform_bounds(bounds: [f32; 4], m: &[f32; 16]) -> [f32; 4] {
    let (m0, m1, m4, m5, m12, m13) = (m[0], m[1], m[4], m[5], m[12], m[13]);
    let [min_x, min_y, max_x, max_y] = bounds;
    let mut out = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for (x, y) in [(min_x, min_y), (max_x, min_y), (min_x, max_y), (max_x, max_y)] {
        let px = m0 * x + m4 * y + m12;
        let py = m1 * x + m5 * y + m13;
        out[0] = out[0].min(px);
        out[1] = out[1].min(py);
        out[2] = out[2].max(px);
        out[3] = out[3].max(py);
    }
    out
}

/// A player-local -> coverage-UV affine, pulled back through `m` (node-local to
/// player-local) so a node's own coordinates land where the writers were baked.
/// Only the 2D part matters: coverage is a flat bitmap and the runtime's local Z
/// is zero.
fn compose_uv(uv: &[f32; 6], m: &[f32; 16]) -> [f32; 6] {
    let [a, b, c, d, tx, ty] = *uv;
    let (m0, m1, m4, m5, m12, m13) = (m[0], m[1], m[4], m[5], m[12], m[13]);
    [a * m0 + c * m1, b * m0 + d * m1, a * m4 + c * m5, b * m4 + d * m5, a * m12 + c * m13 + tx, b * m12 + d * m13 + ty]
}

/// A column-major 4x4 identity: where the top node sits relative to itself.
const IDENTITY_MATRIX: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Draw one player's instance tree: prepare every node, settle the tree's mask,
/// then walk it in drawing order.
fn render_tree(root: &mut PlaybackNode, build: &mut FrameBuild<'_>, transform: &[f32; 16], frame_no: f32, draw: NodeDraw) {
    let mut mask = TreeMask::new(build.scene.mask_backend());
    let mut preps = Vec::new();
    let mut next_seq = 0;
    let Some(top) = prepare_node(root, build, frame_no, *transform, IDENTITY_MATRIX, InheritedMask::ROOT, TREE_START, &mut next_seq, &mut preps, &mut mask) else { return };

    match mask.backend {
        crate::MaskBackend::Coverage => bake_coverage(build, &mut preps, &mut mask),
        crate::MaskBackend::Stencil => mask.schedule(),
        crate::MaskBackend::None => {}
    }

    render_node(root, top, &preps, build, draw, &mut mask);

    if mask.backend == crate::MaskBackend::Stencil {
        mask.finish(build, &preps);
    }
}

/// Fetch a node's frame, copy it into the arena, number its ranks on the tree
/// sequence and place its writers — then the same for each visible instance
/// child, in its instance part's place. Returns the node's index in `preps`.
///
/// `open` is where this node's animation starts on the sequence: the top
/// animation's before its first part, a sub-animation's at its instance part.
#[allow(clippy::too_many_arguments)]
fn prepare_node<'r>(node: &mut PlaybackNode, build: &mut FrameBuild<'_>, frame_no: f32, transform: [f32; 16], to_root: [f32; 16], ctx: InheritedMask, open: i32, next_seq: &mut i32, preps: &mut Vec<NodePrep<'r>>, mask: &mut TreeMask) -> Option<usize> {
    // The DISPLAY frame. The playhead keeps its fraction whatever this is set to;
    // flooring it here is the whole of what turning sub-frame off does.
    let frame_no = if build.sub_frame { frame_no } else { frame_no.floor() };
    let frame_data: &'r [u8] = match node.context.get_frame_data(frame_no) {
        Ok(bytes) => unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) },
        Err(_) => return None,
    };
    let frame = frame_view(&mut node.context, frame_data);
    // Hand this node's runtime buffers to the scene once, up front: everything
    // after refers to them by arena offset rather than expanding them.
    let slot = build.scene.begin_node(&frame, &transform);
    let index = preps.len();
    preps.push(NodePrep { frame, slot, transform, to_root, ctx, seq: Vec::new(), children: Vec::new() });

    let mut own = if mask.masking() { plan_writers(&frame, &node.mask_flags) } else { MaskWriters::default() };
    retarget(&mut own, &node.atlas_keys);
    // `plan_writers` hands them back in ascending rank.
    let mut own = own.iter().peekable();

    let fd = unsafe { root_as_frame_data_unchecked(frame.frame_data) };
    let draw_order = fd.draw_order();
    let mut seq = Vec::with_capacity(if mask.masking() { draw_order.len() } else { 0 });
    let mut children = Vec::new();
    for rank in 0..draw_order.len() {
        let here = *next_seq;
        *next_seq += 1;
        if mask.masking() {
            seq.push(here);
        }
        while let Some(w) = own.next_if(|w| w.rank <= rank) {
            mask.place(w, here, open, index, ctx);
        }

        // A visible instance's sub-animation is drawn in its instance part's
        // place, so it is numbered right there, ahead of the next part.
        let part_idx = draw_order.get(rank) as usize;
        let Some(child) = node.instances.get_mut(&part_idx) else { continue };
        if !child.visible || (part_idx + 1) * 16 > frame.world_matrices.len() {
            continue;
        }
        let m: [f32; 16] = frame.world_matrices[part_idx * 16..part_idx * 16 + 16].try_into().unwrap();
        let child_ctx = InheritedMask::of_instance(&node.mask_flags.get(part_idx).copied().unwrap_or_default());
        let child_frame_no = child.frame_no;
        if let Some(ci) = prepare_node(&mut child.node, build, child_frame_no, mat4_mul(&transform, &m), mat4_mul(&to_root, &m), child_ctx, here, next_seq, preps, mask) {
            children.push((part_idx, ci));
        }
    }
    preps[index].seq = seq;
    preps[index].children = children;
    Some(index)
}

/// Rasterise every writer of the tree into one coverage bitmap, owned by the top
/// node, and point every other node at it through its own placement.
///
/// The replay needs no running state: each writer's record carries its scope on
/// the tree sequence, and a masked part carries its own place on it.
fn bake_coverage(build: &mut FrameBuild<'_>, preps: &mut [NodePrep<'_>], mask: &mut TreeMask) {
    // Bounds enclose every writer, each mapped through its node's instance
    // placements into the player's local space, so the bitmap covers them all.
    let mut bounds: Option<[f32; 4]> = None;
    for (w, &n) in mask.writers.iter().zip(&mask.writer_nodes) {
        let Some(b) = crate::mask::writer_bounds(&preps[n].frame, std::slice::from_ref(&w.writer)) else { continue };
        let b = transform_bounds(b, &preps[n].to_root);
        bounds = Some(match bounds {
            Some([a, bb, c, d]) => [a.min(b[0]), bb.min(b[1]), c.max(b[2]), d.max(b[3])],
            None => b,
        });
    }
    let Some([min_x, min_y, max_x, max_y]) = bounds else {
        // Nothing to rasterise, so nothing covers anything: parts drawn only
        // inside a mask must draw nowhere rather than untested.
        mask.writers.clear();
        mask.writer_nodes.clear();
        return;
    };
    let (w, h) = ((max_x - min_x).max(f32::EPSILON), (max_y - min_y).max(f32::EPSILON));
    let uv = [1.0 / w, 0.0, 0.0, 1.0 / h, -min_x / w, -min_y / h];

    let (top, rest) = preps.split_first_mut().expect("the top node is prepared first");
    build.scene.set_node_coverage(&mut top.slot, &uv, (w, h), &mask.writers);
    let layer = build.scene.node_coverage_layer(&top.slot);
    mask.coverage = Some((uv, layer));
    for prep in rest {
        build.scene.inherit_node_coverage(&mut prep.slot, &compose_uv(&uv, &prep.to_root), layer, &mask.writers);
    }
    // Each writer bakes through its own node's block, whose affine lands its
    // node-local geometry in the player's bitmap; the bit is its record's index.
    for (bit, (w, &n)) in mask.writers.iter().zip(&mask.writer_nodes).enumerate() {
        build.scene.append_coverage_writer(&preps[n].frame, &preps[n].slot, &w.writer, bit);
    }
}

/// Append one prepared node's geometry, recursing into its instance children in
/// their instance parts' places.
///
/// Draw only: every slot was advanced by [`step_slots`] and every node prepared
/// by [`prepare_node`] before this walk began, so this reads what they left
/// behind rather than driving it.
///
/// Masking is decided per part, the same way at every depth: whether it is a
/// target and its polarity from its own flags and its instance part's
/// ([`InheritedMask`]), and whether any writer of the tree is in scope from its
/// place on the tree sequence ([`TreeMask::part`]).
fn render_node(node: &mut PlaybackNode, index: usize, preps: &[NodePrep<'_>], build: &mut FrameBuild<'_>, draw: NodeDraw, mask: &mut TreeMask) {
    let NodeDraw { alpha, tint } = draw;
    let prep = &preps[index];
    let (frame, node_slot, transform, ctx) = (&prep.frame, &prep.slot, &prep.transform, prep.ctx);
    let seq_of = |rank: usize| prep.seq.get(rank).copied().unwrap_or(0);

    let fd = unsafe { root_as_frame_data_unchecked(frame.frame_data) };
    let draw_order = fd.draw_order();
    for batch in fd.draw_batches() {
        let start_rank = batch.start_rank() as usize;
        let count = batch.count() as usize;
        let kind = batch.kind();
        match kind {
            DrawBatchKind::Normal | DrawBatchKind::Mesh | DrawBatchKind::Shape => {
                // Pure masks (PartTypeMask / draw_as_mask) contribute coverage but
                // never draw their own colour. The runtime still
                // emits a content batch for them, so skip it here.
                let is_pure = |pi: usize| node.mask_flags.get(pi).map(|f| f.pure_mask()).unwrap_or(false);
                let part_mask = |rank: usize, mask: &TreeMask| {
                    let f = node.mask_flags.get(draw_order.get(rank) as usize).copied().unwrap_or_default();
                    mask.part(&f, ctx, seq_of(rank))
                };
                let texture_hash = node.atlas_keys.resolve(batch.texture_hash());
                let whole = (start_rank..start_rank + count).all(|r| !is_pure(draw_order.get(r) as usize) && part_mask(r, mask) == PartMask::Plain);

                if whole {
                    let info = BatchInfo { start_rank, count, texture_hash, blend_type: batch.blend_type(), kind, stencil: StencilTest::None, mask_seq: 0, alpha, tint };
                    build.scene.append_batch(build.renderer, frame, node_slot, &info);
                    continue;
                }
                // Polarity and the writers in scope vary per part → draw one at a time.
                for rank in start_rank..start_rank + count {
                    if is_pure(draw_order.get(rank) as usize) {
                        continue;
                    }
                    let seq = seq_of(rank);
                    let stencil = match part_mask(rank, mask) {
                        PartMask::Hidden => continue,
                        PartMask::Plain => StencilTest::None,
                        PartMask::Tested(test) => {
                            mask.sync(build, preps, seq);
                            test
                        }
                    };
                    let info = BatchInfo { start_rank: rank, count: 1, texture_hash, blend_type: batch.blend_type(), kind, stencil, mask_seq: seq.max(0) as u32, alpha, tint };
                    build.scene.append_batch(build.renderer, frame, node_slot, &info);
                }
            }
            DrawBatchKind::Instance => {
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    // Prepared only when visible; the prep is the draw's go-ahead.
                    let Some(&(_, child_index)) = prep.children.iter().find(|&&(p, _)| p == part_idx) else { continue };
                    let Some(child) = node.instances.get_mut(&part_idx) else { continue };
                    // Fold the instance part's own opacity into the chain the child
                    // draws with. SpriteStudio modulates a sub-animation by its
                    // instance part's effective alpha (SS6/SS7.5 `RateOpacity`, an
                    // unconditional multiply cascade); `part.alpha()` is that
                    // hierarchical value. Colour deliberately does not cross the
                    // boundary — no player propagates an instance part's tint.
                    let child_alpha = alpha * fd.parts().get(part_idx).alpha();
                    render_node(&mut child.node, child_index, preps, build, NodeDraw { alpha: child_alpha, tint }, mask);
                }
            }
            DrawBatchKind::Effect => {
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    // Whatever `step_slots` produced for this part earlier in the
                    // frame; empty when the effect had nothing to draw.
                    let Some(plan) = node.effects.get(&part_idx).map(|e| e.plan.as_slice()).filter(|p| !p.is_empty()) else { continue };
                    // Each emitter meets the mask on its own flags and place, like
                    // any drawn part — it draws into its own slot anyway.
                    let seq = seq_of(rank);
                    let f = node.mask_flags.get(part_idx).copied().unwrap_or_default();
                    let eff_stencil = match mask.part(&f, ctx, seq) {
                        PartMask::Hidden => continue,
                        PartMask::Plain => StencilTest::None,
                        PartMask::Tested(test) => {
                            mask.sync(build, preps, seq);
                            test
                        }
                    };
                    let m: [f32; 16] = frame.world_matrices[part_idx * 16..part_idx * 16 + 16].try_into().unwrap();
                    let eff_transform = mat4_mul(transform, &m);
                    // The effect draws from its own plan, so it needs its own
                    // arena slot rather than this node's.
                    let mut eff_slot = build.scene.push_transform(&eff_transform);
                    // Under coverage the slot needs its own view of the bitmap too:
                    // the particles are in the effect part's local space, one
                    // placement below this node's. Without it the replay has no
                    // block to read, and the effect draws unclipped — unlike the
                    // stencil, which clips it by pipeline state alone.
                    if eff_stencil != StencilTest::None
                        && let Some((uv, layer)) = mask.coverage
                    {
                        build.scene.inherit_node_coverage(&mut eff_slot, &compose_uv(&uv, &mat4_mul(&prep.to_root, &m)), layer, &mask.writers);
                    }
                    // Same opacity fold as an instance part: the effect emitter's own
                    // alpha scales every particle it draws (the particles carry their
                    // own per-vertex alpha on top).
                    let eff_alpha = alpha * fd.parts().get(part_idx).alpha();
                    build.scene.append_effect(plan, &eff_slot, eff_stencil, seq.max(0) as u32, eff_alpha, tint, &node.atlas_keys);
                }
            }
            // Mask batches emit no colour (coverage is handled by the writers
            // above). Text / Nines: not yet handled.
            _ => {}
        }
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

/// Decode a flush's whole batch of atlases and register them **in `jobs` order**.
///
/// One call for the batch rather than one per image so the decodes can overlap: a
/// character with three 4096² atlases otherwise pays for them end to end on the
/// host's thread, which is the load stall this exists to avoid. The registration
/// order is still the caller's, not the order the decodes happened to finish in.
/// Whether the renderer's entry for `key` still has to be built from `encoded`.
///
/// Three answers, and the middle one is the whole point:
///
/// - **Nothing there** — build it.
/// - **Something there that a load put there, from different bytes** — build it.
///   Same path, same key, re-exported pixels: the viewer's convert / drop / look
///   / convert again round, which otherwise keeps showing the old atlas.
/// - **Something there from these same bytes, or something a host registered** —
///   leave it. The first is what keeps N players of one pack to one upload
///   between them; the second is not ours to overwrite.
fn wanted_now(renderer: &SSRenderer, key: u32, encoded: &[u8]) -> bool {
    if renderer.texture_size(key).is_none() {
        return true;
    }
    renderer.texture_digest(key).is_some_and(|held| held != fnv1a_bytes(encoded))
}

#[cfg(feature = "embedded")]
fn decode_and_register(renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, jobs: &[(u32, &[u8], Retention)]) {
    let encoded: Vec<&[u8]> = jobs.iter().map(|(_, bytes, _)| *bytes).collect();
    // Premultiplied on the worker, so all that is left for this thread
    // is creating the texture and writing it.
    for (decoded, (hash, bytes, retention)) in crate::decode::decode_all(&encoded, true).into_iter().zip(jobs) {
        let Some(decoded) = decoded else {
            eprintln!("[ssplayer-wgpu] an image for {hash:#010x} could not be decoded");
            continue;
        };
        renderer.register_premultiplied(device, queue, *hash, decoded.rgba, (decoded.width, decoded.height), crate::renderer::Provenance { retention: *retention, digest: Some(fnv1a_bytes(bytes)) });
    }
}

/// Without a decoder there is nothing to do here: on that build the host supplies
/// textures itself (the web face decodes with `createImageBitmap`).
#[cfg(not(feature = "embedded"))]
fn decode_and_register(_: &mut SSRenderer, _: &wgpu::Device, _: &wgpu::Queue, _: &[(u32, &[u8], Retention)]) {}
