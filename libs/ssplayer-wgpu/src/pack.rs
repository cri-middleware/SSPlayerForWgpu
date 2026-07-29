//! The shared unit of content (SPEC §8.2): one loaded `.ssab`, referenced by any
//! number of players.
//!
//! **An `SSPack` holds CPU data only — never a GPU texture.** That is a design
//! decision, not an omission (SPEC §8.2): owning textures here would put a second
//! lifecycle in competition with the host's. In Bevy, `Assets<Image>` /
//! `Handle<Image>` / `RenderAssets<GpuImage>` already own upload, hot reload and
//! unload; a pack that also owned them would upload every atlas twice, free at the
//! wrong moment, and take the format / mip / sampler decisions away from the
//! engine. So a pack *describes* the textures it needs
//! ([`SSPack::required_textures`]) and can *hand over pixels*
//! ([`SSPack::decode_embedded`]), and whoever owns GPU memory registers them with
//! [`crate::SSRenderer`].
//!
//! Cloning a pack is cheap — an `Arc` bump and two small shared slices — so "the
//! same character twenty times" costs one parse and one atlas.

use ssab::ssab::ss::format::root_as_ss_anime_binary_unchecked;
use ssruntime::error::SSRuntimeError;
use ssruntime::{SsabData, SsabResource};
#[cfg(feature = "embedded")]
use std::collections::HashMap;
use std::sync::Arc;

/// One atlas image decoded out of an embedded `.ssab` (SPEC §8.1).
///
/// The pixels are **straight-alpha** RGBA8, exactly as the PNG decoded: whoever
/// takes them decides what to do about premultiplication.
/// [`crate::SSRenderer::upload_texture`] premultiplies on the way to the GPU; an
/// engine that puts them in its own asset system declares
/// [`crate::AlphaMode::Straight`] instead.
pub struct EmbeddedTexture {
    /// The cellmap's `name_hash` — what `DrawBatch.texture_hash` carries, and the
    /// key [`crate::SSRenderer`] looks textures up by.
    pub hash: u32,
    /// Straight-alpha RGBA8, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A loaded `.ssab`, shareable between players (SPEC §8.2).
///
/// Loading costs a parse; sharing costs an `Arc` clone. [`SSPlayer::set_pack`]
/// selects the pack to play from and [`SSPlayer::add_pack`] registers the extra
/// packs that external instance parts reference by name (SPEC §7.1).
///
/// [`SSPlayer::set_pack`]: crate::SSPlayer::set_pack
/// [`SSPlayer::add_pack`]: crate::SSPlayer::add_pack
#[derive(Clone)]
pub struct SSPack {
    name: Arc<str>,
    resource: Arc<SsabResource>,
    /// Every cellmap's `name_hash`, in pack order.
    required_textures: Arc<[u32]>,
    /// Whether any cellmap's image is carried inside the `.ssab` (SPEC §8.1).
    has_embedded: bool,
}

impl SSPack {
    /// Parse `bytes`, naming the pack after the `.ssab` itself.
    ///
    /// The name matters because instance parts resolve their sub-animation by
    /// pack name (`ref_anime_pack`, SPEC §7.1), and the exporter writes that same
    /// name into each pack — so the two agree without the host having to derive
    /// one from a file path. Use [`SSPack::load_named`] when the host names packs
    /// itself.
    pub fn load(bytes: &[u8]) -> Result<Self, SSRuntimeError> {
        Self::build(None, bytes)
    }

    /// Parse `bytes` under a name the caller chooses, overriding the one in the
    /// `.ssab`. What `add_ssab` / `load_ssab` use.
    pub fn load_named(name: &str, bytes: &[u8]) -> Result<Self, SSRuntimeError> {
        Self::build(Some(name), bytes)
    }

    fn build(name: Option<&str>, bytes: &[u8]) -> Result<Self, SSRuntimeError> {
        let resource = SsabResource::new(SsabData::load_copy(bytes)?);
        if resource.resource_static_ptr().is_null() {
            return Err(SSRuntimeError::NotLoaded);
        }

        // SAFETY: `SsabData::load_copy` accepted the buffer as an `SsAnimeBinary`,
        // and the copy it owns outlives this borrow.
        let ab = unsafe { root_as_ss_anime_binary_unchecked(resource.data().as_slice()) };
        let name: Arc<str> = Arc::from(name.unwrap_or_else(|| ab.name()));

        // The two hashes are different things and must not be swapped (SPEC §8.1):
        // `name_hash` is what a draw batch asks for, `image_path_hash` is the key
        // of the embedded byte blob.
        let mut required_textures = Vec::new();
        let mut has_embedded = false;
        if let Some(cellmaps) = ab.cellmaps() {
            let embedded: Vec<u32> = ab.embedded_assets().map(|assets| assets.iter().map(|a| a.name_hash()).collect()).unwrap_or_default();
            for cm in cellmaps {
                required_textures.push(cm.name_hash());
                has_embedded |= embedded.contains(&cm.image_path_hash());
            }
        }

        Ok(Self { name, resource, required_textures: required_textures.into(), has_embedded })
    }

    /// The name instance references resolve against.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The texture hashes this pack's draw batches will ask for.
    ///
    /// A reference-mode host uses this to know what to load before playing;
    /// combined with [`crate::SSRenderer::texture_size`] it also answers "is this
    /// pack ready to draw".
    pub fn required_textures(&self) -> &[u32] {
        &self.required_textures
    }

    /// Whether the atlases ride inside the `.ssab` (embedded mode, SPEC §8.1).
    /// `false` means the host supplies them (reference mode).
    pub fn has_embedded_textures(&self) -> bool {
        self.has_embedded
    }

    /// Names of every animation in the pack.
    pub fn animation_names(&self) -> Vec<String> {
        self.resource.resource().map(|r| r.get_animation_names()).unwrap_or_default()
    }

    /// Decode the atlases this pack carries (SPEC §8.1). Empty for a
    /// reference-mode pack, and empty without the `embedded` feature.
    ///
    /// Decoding is not cached: the pixels are handed over so the caller can put
    /// them wherever GPU memory is owned (`upload_texture`, or an engine's asset
    /// system, §8.2). Call it once per pack, not once per player.
    pub fn decode_embedded(&self) -> Vec<EmbeddedTexture> {
        self.decode_embedded_where(|_| true)
    }

    /// [`SSPack::decode_embedded`], restricted to the hashes `wanted` accepts.
    ///
    /// The filter runs *before* the PNG decode, which is the whole point: a player
    /// joining a scene where its atlas is already registered skips the decode
    /// entirely, so N players of one pack cost one decode and one upload.
    #[cfg(feature = "embedded")]
    pub fn decode_embedded_where(&self, wanted: impl Fn(u32) -> bool) -> Vec<EmbeddedTexture> {
        if !self.has_embedded {
            return Vec::new();
        }
        // SAFETY: as in `build` — the resource owns the buffer being read.
        let ab = unsafe { root_as_ss_anime_binary_unchecked(self.resource.data().as_slice()) };
        let (Some(cellmaps), Some(assets)) = (ab.cellmaps(), ab.embedded_assets()) else {
            return Vec::new();
        };
        let mut bytes_by_hash: HashMap<u32, &[u8]> = HashMap::new();
        for asset in assets {
            if let Some(bytes) = asset.data() {
                bytes_by_hash.insert(asset.name_hash(), bytes.bytes());
            }
        }

        let mut out = Vec::new();
        for cm in cellmaps {
            let hash = cm.name_hash();
            if !wanted(hash) {
                continue;
            }
            let Some(png) = bytes_by_hash.get(&cm.image_path_hash()) else { continue };
            let Ok(img) = image::load_from_memory(png) else { continue };
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            out.push(EmbeddedTexture { hash, rgba: rgba.into_raw(), width, height });
        }
        out
    }

    /// Without the `embedded` feature the player carries no image decoder, so the
    /// host decodes instead — in the browser that is `createImageBitmap` (SPEC §10).
    #[cfg(not(feature = "embedded"))]
    pub fn decode_embedded_where(&self, _wanted: impl Fn(u32) -> bool) -> Vec<EmbeddedTexture> {
        Vec::new()
    }

    /// The runtime resource this pack wraps, for building playback contexts.
    pub(crate) fn resource(&self) -> &Arc<SsabResource> {
        &self.resource
    }
}
