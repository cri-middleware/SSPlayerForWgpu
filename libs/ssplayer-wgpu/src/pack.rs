//! The shared unit of content (SPEC §8.2): one loaded `.ssab`, referenced by any
//! number of players.
//!
//! **An `SSPack` holds CPU data only — never a GPU texture.** That is a design
//! decision, not an omission (SPEC §8.2): owning textures here would put a second
//! lifecycle in competition with the host's. In Bevy, `Assets<Image>` /
//! `Handle<Image>` / `RenderAssets<GpuImage>` already own upload, hot reload and
//! unload; a pack that also owned them would upload every atlas twice, free at the
//! wrong moment, and take the format / mip / sampler decisions away from the
//! engine. So a pack *describes* the textures it needs ([`SSPack::textures`])
//! and can *hand over pixels* ([`SSPack::decode_embedded`]), and whoever owns GPU
//! memory registers them with [`crate::SSRenderer`].
//!
//! Cloning a pack is cheap — an `Arc` bump and two small shared slices — so "the
//! same character twenty times" costs one parse and one atlas.

use ssab::ssab::ss::format::root_as_ss_anime_binary_unchecked;
use ssruntime::error::SSRuntimeError;
use ssruntime::{SsabData, SsabResource};
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

/// One atlas an embedded `.ssab` carries, still in the form the exporter wrote
/// it: **encoded image bytes (PNG), not pixels** (SPEC §8.1).
///
/// This is the *parsing* half of embedded support, and it deliberately has no
/// decoder behind it — which is why it is available without the `embedded`
/// feature. The sibling players split the same way: `ssplayer-core`'s
/// `extractEmbeddedTextures` hands out bytes and `ssplayer-webgl` decodes them
/// with `createImageBitmap`, which is how the WebGL player supports embedded
/// packs while shipping no image decoder of its own. A host with a decoder
/// already — a browser, or an engine's asset pipeline — wants this;
/// [`SSPack::decode_embedded`] is for hosts that would rather the player do it.
///
/// The bytes borrow the pack's `.ssab`, so nothing is copied to produce them.
pub struct EmbeddedImage<'a> {
    /// The cellmap's `name_hash` — what `DrawBatch.texture_hash` carries, and the
    /// key [`crate::SSRenderer`] looks textures up by.
    pub hash: u32,
    /// The encoded image exactly as the exporter embedded it.
    pub bytes: &'a [u8],
}

/// One atlas a pack asks for, described the way the `.ssab` itself describes it
/// (SPEC §8.1).
///
/// This is what makes reference mode loadable without the host guessing: the
/// pack answers *what* it needs and *where the exporter put it*, and the host —
/// the only side that has a filesystem or a network — does the reading. The
/// sibling players work exactly this way (`ssplayer-core`'s
/// `extractTexturePaths` + `loadTextures`), and a host here should too: resolve
/// [`TextureRef::path`] against the directory the `.ssab` came from, skipping
/// the entries that are [`TextureRef::embedded`] because those arrive with the
/// pack.
#[derive(Clone, Debug)]
pub struct TextureRef {
    /// The cellmap's `name_hash` — what `DrawBatch.texture_hash` carries, and the
    /// key [`crate::SSRenderer`] looks textures up by.
    pub hash: u32,
    /// The image path the exporter wrote, relative to the `.ssab`'s own location.
    pub path: String,
    /// Whether this image rides inside the `.ssab` (embedded mode, SPEC §8.1).
    /// `true` means the host has nothing to load: [`SSPack::decode_embedded`]
    /// already produces it, and a player registers it on the next update.
    pub embedded: bool,
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
    /// Every cellmap's atlas, in pack order.
    textures: Arc<[TextureRef]>,
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
        let mut textures = Vec::new();
        let mut has_embedded = false;
        if let Some(cellmaps) = ab.cellmaps() {
            let embedded: Vec<u32> = ab.embedded_assets().map(|assets| assets.iter().map(|a| a.name_hash()).collect()).unwrap_or_default();
            for cm in cellmaps {
                let is_embedded = embedded.contains(&cm.image_path_hash());
                has_embedded |= is_embedded;
                textures.push(TextureRef { hash: cm.name_hash(), path: cm.image_path().to_string(), embedded: is_embedded });
            }
        }

        Ok(Self { name, resource, textures: textures.into(), has_embedded })
    }

    /// The name instance references resolve against.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The atlases this pack's draw batches will ask for, in pack order.
    ///
    /// A reference-mode host walks this to know what to load, and from where,
    /// before playing (see [`TextureRef`]); combined with
    /// [`crate::SSRenderer::texture_size`] it also answers "is this pack ready to
    /// draw".
    pub fn textures(&self) -> &[TextureRef] {
        &self.textures
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

    /// The encoded atlases this pack carries, in cellmap order (SPEC §8.1).
    /// Empty for a reference-mode pack.
    ///
    /// Parsing only — no image decoder is involved, so this works in every build
    /// and is the route a host with its own decoder takes (see [`EmbeddedImage`]).
    /// On the web that host is the browser: `createImageBitmap` on these bytes
    /// decodes off the main thread and premultiplies for free, which
    /// [`SSPack::decode_embedded`] cannot do and pays for in wasm size.
    ///
    /// The two hashes are different things and must not be swapped (SPEC §8.1):
    /// the blob is keyed by `image_path_hash`, and what comes back is keyed by
    /// `name_hash` — the one a draw batch asks for.
    pub fn embedded_images(&self) -> Vec<EmbeddedImage<'_>> {
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
            if let Some(&bytes) = bytes_by_hash.get(&cm.image_path_hash()) {
                out.push(EmbeddedImage { hash: cm.name_hash(), bytes });
            }
        }
        out
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
        let mut out = Vec::new();
        for embedded in self.embedded_images() {
            if !wanted(embedded.hash) {
                continue;
            }
            let Ok(img) = image::load_from_memory(embedded.bytes) else { continue };
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            out.push(EmbeddedTexture { hash: embedded.hash, rgba: rgba.into_raw(), width, height });
        }
        out
    }

    /// Without the `embedded` feature the player carries no image decoder, so the
    /// host decodes instead — in the browser that is `createImageBitmap` over
    /// [`SSPack::embedded_images`] (SPEC §10), which still needs no sibling files.
    #[cfg(not(feature = "embedded"))]
    pub fn decode_embedded_where(&self, _wanted: impl Fn(u32) -> bool) -> Vec<EmbeddedTexture> {
        Vec::new()
    }

    /// The runtime resource this pack wraps, for building playback contexts.
    pub(crate) fn resource(&self) -> &Arc<SsabResource> {
        &self.resource
    }
}
