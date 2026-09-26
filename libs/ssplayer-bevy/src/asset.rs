//! `.ssab` as a Bevy asset.
//!
//! A pack loads through the `AssetServer` like any other asset: on Bevy's IO
//! pool, from whatever source the app configured (the filesystem, an embedded or
//! packed source, the web), and again when its file changes if the app enables
//! hot reloading. What reaches the render world is parsed once per pack, not
//! once per character, and its atlases are Bevy [`Image`]s — decoded by Bevy,
//! uploaded once, and lent to the player as texture views.
//!
//! The walk that discovers what a pack depends on is the player's own
//! (`SSPlayer::load_assets`), run over Bevy's reader; nothing here decides which
//! files a pack needs.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetId, AssetLoader, AssetPath, LoadContext};
use bevy::ecs::system::SystemParamItem;
use bevy::image::{Image, ImageLoaderSettings};
use bevy::log::warn;
use bevy::prelude::Handle;
use bevy::reflect::TypePath;
use bevy::render::render_asset::{PrepareAssetError, RenderAsset};
use ssplayer_wgpu::{SSPack, SSPlayer, SSRuntimeError};

/// A `.ssab`, with every pack its instance parts play from and every atlas it
/// draws with.
///
/// Load it with `asset_server.load("Ringo/Ringo.ssab")` and hand the handle to
/// [`crate::SsPlayer`]. Characters sharing a handle share one parse of the pack
/// and one upload of each atlas.
#[derive(Asset, TypePath, Clone)]
pub struct SsPack {
    /// The pack itself first, then the packs its instance parts play from,
    /// transitively. Cloning one is an `Arc` bump, which is what a character
    /// takes.
    pub(crate) packs: Vec<SSPack>,
    /// Where the pack was loaded from. An atlas is keyed by its path resolved
    /// against this, in the main world when the pack loads and again in the
    /// render world when a character is built, so both sides must resolve alike.
    pub(crate) path: AssetPath<'static>,
    /// The reference-mode atlases, in the order of `atlas_keys`.
    #[dependency]
    pub(crate) atlases: Vec<Handle<Image>>,
    /// The renderer key each of `atlases` is lent under.
    pub(crate) atlas_keys: Vec<u32>,
}

impl SsPack {
    /// The pack itself: its name and the animations it can play.
    pub fn pack(&self) -> &SSPack {
        &self.packs[0]
    }
}

/// An [`SsPack`] as the render world holds it: the packs and where each atlas
/// comes from, with a generation that changes whenever the asset is replaced —
/// a hot reload rebuilds every character playing it.
pub(crate) struct RenderSsPack {
    pub(crate) packs: Vec<SSPack>,
    pub(crate) path: AssetPath<'static>,
    pub(crate) atlases: Vec<(u32, AssetId<Image>)>,
    pub(crate) generation: u64,
}

impl RenderAsset for RenderSsPack {
    type SourceAsset = SsPack;
    type Param = ();

    fn prepare_asset(source: SsPack, _id: AssetId<SsPack>, _param: &mut SystemParamItem<()>, _previous: Option<&Self>) -> Result<Self, PrepareAssetError<SsPack>> {
        static GENERATION: AtomicU64 = AtomicU64::new(0);
        let atlases = source.atlas_keys.iter().copied().zip(source.atlases.iter().map(Handle::id)).collect();
        Ok(Self { packs: source.packs, path: source.path, atlases, generation: GENERATION.fetch_add(1, Ordering::Relaxed) })
    }
}

/// Why a `.ssab` did not load.
#[derive(Debug)]
pub enum SsPackLoadError {
    Io(std::io::Error),
    Parse(SSRuntimeError),
}

impl std::fmt::Display for SsPackLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "could not read the .ssab: {err}"),
            Self::Parse(err) => write!(f, "not a valid .ssab: {err}"),
        }
    }
}

impl std::error::Error for SsPackLoadError {}

/// Loads `.ssab` files as [`SsPack`].
#[derive(Default, TypePath)]
pub struct SsPackLoader;

impl AssetLoader for SsPackLoader {
    type Asset = SsPack;
    type Settings = ();
    type Error = SsPackLoadError;

    async fn load(&self, reader: &mut dyn Reader, _settings: &(), load_context: &mut LoadContext<'_>) -> Result<SsPack, SsPackLoadError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.map_err(SsPackLoadError::Io)?;
        let main = SSPack::load(&bytes).map_err(SsPackLoadError::Parse)?;
        let path = load_context.path().clone_owned();

        // The player walks the pack; this only answers its reads. The player is
        // thrown away afterwards — what the asset keeps is the packs and the
        // atlas handles the walk turned up.
        let mut walk = Walk { ctx: load_context, base: path.clone(), siblings: Vec::new(), atlases: RefCell::new(Vec::new()) };
        let mut player = SSPlayer::new();
        player.add_pack(&main);
        player.load_assets(&mut walk, main.name()).await;

        let Walk { ctx, siblings, atlases, .. } = walk;
        let mut atlas_keys = Vec::new();
        let mut handles = Vec::new();
        for (key, atlas) in atlases.into_inner() {
            // Two packs of one project name the same image; one handle serves both.
            if atlas_keys.contains(&key) {
                continue;
            }
            // Not sRGB: the player takes atlas bytes as they were authored, the
            // same way it uploads the ones it decodes itself.
            let handle = ctx.load_builder().with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false).load(atlas);
            atlas_keys.push(key);
            handles.push(handle);
        }

        let mut packs = vec![main];
        packs.extend(siblings);
        Ok(SsPack { packs, path, atlases: handles, atlas_keys })
    }

    fn extensions(&self) -> &[&str] {
        &["ssab"]
    }
}

/// The renderer key an atlas is lent under: its resolved asset path, hashed, so
/// two packs naming one image share it and two images of one name do not.
pub(crate) fn atlas_key(path: &AssetPath<'_>) -> u32 {
    // fnv1a, the family's hash — the value only has to be stable and spread.
    path.to_string().bytes().fold(2166136261u32, |hash, byte| (hash ^ byte as u32).wrapping_mul(16777619))
}

/// The player's walk, answered from Bevy's asset reader.
///
/// Atlases are never read here: `has_texture` answers yes for every one, so the
/// walk only keys them, and the keys it asks for are recorded so the loader can
/// request each image from Bevy. The only reads are the sibling packs, and
/// reading them through the context is what makes a change to one reload this
/// pack.
struct Walk<'a, 'ctx> {
    ctx: &'a mut LoadContext<'ctx>,
    base: AssetPath<'static>,
    /// Sibling packs, parsed as they are read. The walk's player parses its own
    /// copy and is dropped; this is the one the asset keeps.
    siblings: Vec<SSPack>,
    /// Every reference-mode atlas the walk keyed, with where it resolves to. A
    /// `RefCell` because the walk asks for keys through `&self`.
    atlases: RefCell<Vec<(u32, AssetPath<'static>)>>,
}

impl ssplayer_wgpu::AssetLoader for Walk<'_, '_> {
    // `load_assets` roots nothing; every path resolves against `base`.
    fn reset_base(&mut self) {}

    fn set_base(&mut self, _src: &str) {}

    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        let mut out = Vec::with_capacity(paths.len());
        for path in paths {
            let bytes = match self.base.resolve_embed_str(path) {
                Ok(resolved) => self.ctx.read_asset_bytes(resolved).await.ok(),
                Err(_) => None,
            };
            if let Some(pack) = bytes.as_deref().and_then(|bytes| SSPack::load(bytes).ok()) {
                self.siblings.push(pack);
            }
            out.push(bytes);
        }
        out
    }

    fn texture_key(&self, path: &str, cellmap_hash: u32) -> u32 {
        let Ok(resolved) = self.base.resolve_embed_str(path) else { return cellmap_hash };
        let key = atlas_key(&resolved);
        self.atlases.borrow_mut().push((key, resolved));
        key
    }

    fn has_texture(&self, _key: u32) -> bool {
        true
    }

    async fn supply_texture(&mut self, _key: u32, _encoded: &[u8]) {}

    fn warn(&self, message: &str) {
        warn!("ssplayer-bevy: {} — {message}", self.base);
    }
}

/// The same keys, in the render world, for a character being built from an
/// [`SsPack`] whose packs are already registered: nothing is read, only keyed.
pub(crate) struct Keys<'a> {
    pub(crate) base: &'a AssetPath<'static>,
}

impl ssplayer_wgpu::AssetLoader for Keys<'_> {
    fn reset_base(&mut self) {}

    fn set_base(&mut self, _src: &str) {}

    async fn read(&mut self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        vec![None; paths.len()]
    }

    fn texture_key(&self, path: &str, cellmap_hash: u32) -> u32 {
        self.base.resolve_embed_str(path).map(|resolved| atlas_key(&resolved)).unwrap_or(cellmap_hash)
    }

    fn has_texture(&self, _key: u32) -> bool {
        true
    }

    async fn supply_texture(&mut self, _key: u32, _encoded: &[u8]) {}

    fn warn(&self, message: &str) {
        warn!("ssplayer-bevy: {} — {message}", self.base);
    }
}
