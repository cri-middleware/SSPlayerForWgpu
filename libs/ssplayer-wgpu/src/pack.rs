//! The shared unit of content: one loaded `.ssab`, referenced by any
//! number of players.
//!
//! **An `SSPack` holds CPU data only — never a GPU texture.** Owning textures
//! here would compete with the host's own asset lifecycle (in Bevy,
//! `Assets<Image>` / `RenderAssets<GpuImage>`), so a pack *describes* the
//! textures it needs ([`SSPack::textures`]) and can *hand over pixels*
//! ([`SSPack::decode_embedded`]); whoever owns GPU memory registers them with
//! [`crate::SSRenderer`].
//!
//! "The same character twenty times" costs one parse and one atlas, however the
//! twenty arrived: cloning a pack is an `Arc` bump and two small shared slices,
//! and loading bytes that a live pack already holds shares that pack's parse.

use ssab::ssab::ss::format::root_as_ss_anime_binary_unchecked;
use ssruntime::error::SSRuntimeError;
use ssruntime::{SsabData, SsabResource};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// Every parse a pack still holds, so that loading the same bytes again shares
/// it instead of building a second.
///
/// The runtime keeps everything it derives from a pack on the parse — the
/// decoded keys, each played animation's part caches and setup pose, the mesh
/// data — so twenty players that each load Ringo would otherwise hold twenty
/// copies of it. Held weakly: the last pack to drop a parse frees it.
static PARSES: Mutex<Vec<Weak<SsabResource>>> = Mutex::new(Vec::new());

/// The live parse of `bytes`, or a new one.
///
/// Matched on the bytes themselves, not on where they came from: the same file
/// reached by two paths is one parse, and a file re-exported under the same path
/// is a new one.
///
/// # Safety
///
/// `bytes` must be a valid `.ssab`, as for [`SsabResource::new`].
unsafe fn parse(bytes: &[u8]) -> Result<Arc<SsabResource>, SSRuntimeError> {
    let mut parses = PARSES.lock().unwrap_or_else(PoisonError::into_inner);
    parses.retain(|parse| parse.strong_count() > 0);
    if let Some(shared) = parses.iter().filter_map(Weak::upgrade).find(|parse| parse.data().as_slice() == bytes) {
        return Ok(shared);
    }
    // SAFETY: the caller's contract.
    let resource = unsafe { SsabResource::new(SsabData::load_copy(bytes)?) };
    if resource.resource_static_ptr().is_null() {
        return Err(SSRuntimeError::NotLoaded);
    }
    parses.push(Arc::downgrade(&resource));
    Ok(resource)
}

/// One atlas image decoded out of an embedded `.ssab`.
///
/// The pixels are **straight-alpha** RGBA8, exactly as the PNG decoded: whoever
/// takes them decides what to do about premultiplication.
/// [`crate::SSRenderer::upload_texture`] premultiplies on the way to the GPU; an
/// engine that puts them in its own asset system declares
/// [`crate::AlphaMode::Straight`] instead.
pub struct EmbeddedTexture {
    /// The cellmap's `name_hash` — what `DrawBatch.texture_hash` carries.
    ///
    /// A slot **inside this pack**, not an identity for the image: hand it to
    /// [`crate::SSPlayer::texture_key`] for what [`crate::SSRenderer`] is keyed
    /// by. The two differ whenever two packs could name one atlas alike, which
    /// a tool that names every project's atlas `spritemap1.png` does.
    pub slot: u32,
    /// Straight-alpha RGBA8, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// One atlas an embedded `.ssab` carries, still in the form the exporter wrote
/// it: **encoded image bytes (PNG), not pixels**.
///
/// This is the *parsing* half of embedded support, and it deliberately has no
/// decoder behind it — which is why it is available without the `embedded`
/// feature. A host with a decoder
/// already — a browser, or an engine's asset pipeline — wants this;
/// [`SSPack::decode_embedded`] is for hosts that would rather the player do it.
///
/// The bytes borrow the pack's `.ssab`, so nothing is copied to produce them.
pub struct EmbeddedImage<'a> {
    /// The cellmap's `name_hash` — what `DrawBatch.texture_hash` carries.
    ///
    /// A slot **inside this pack**, not an identity for the image: hand it to
    /// [`crate::SSPlayer::texture_key`] for what [`crate::SSRenderer`] is keyed
    /// by. The two differ whenever two packs could name one atlas alike, which
    /// a tool that names every project's atlas `spritemap1.png` does.
    pub slot: u32,
    /// The encoded image exactly as the exporter embedded it.
    pub bytes: &'a [u8],
}

/// One atlas a pack asks for, described the way the `.ssab` itself describes it.
///
/// This is what makes reference mode loadable without the host guessing: the
/// pack answers *what* it needs and *where the exporter put it*, and the host —
/// the only side that has a filesystem or a network — does the reading. A host
/// resolves
/// [`TextureRef::path`] against the directory the `.ssab` came from, skipping
/// the entries that are [`TextureRef::embedded`] because those arrive with the
/// pack.
#[derive(Clone, Debug)]
pub struct TextureRef {
    /// What `DrawBatch.texture_hash` carries: a cellmap's `name_hash`, or a
    /// texture file name's hash for a TextureChange target (see
    /// [`SSPack::textures`]).
    ///
    /// A slot **inside this pack**, not an identity for the image — two packs
    /// from two projects one tool exported can both name theirs `spritemap1`. Hand it to
    /// [`crate::SSPlayer::texture_key`] for what [`crate::SSRenderer`] is keyed
    /// by, which is what a host supplying its own pixels registers under.
    pub slot: u32,
    /// The image path the exporter wrote, relative to the `.ssab`'s own location.
    pub path: String,
    /// Whether this image rides inside the `.ssab` (embedded mode).
    /// `true` means the host has nothing to load: [`SSPack::decode_embedded`]
    /// already produces it, and a player registers it on the next update.
    pub embedded: bool,
}

/// A loaded `.ssab`, shareable between players.
///
/// Loading costs a parse, unless a live pack already holds the same bytes;
/// sharing costs an `Arc` clone. [`SSPlayer::set_pack`]
/// selects the pack to play from and [`SSPlayer::add_pack`] registers the extra
/// packs that external instance parts reference by name.
///
/// [`SSPlayer::set_pack`]: crate::SSPlayer::set_pack
/// [`SSPlayer::add_pack`]: crate::SSPlayer::add_pack
#[derive(Clone)]
pub struct SSPack {
    name: Arc<str>,
    resource: Arc<SsabResource>,
    /// Every cellmap's atlas, in pack order.
    textures: Arc<[TextureRef]>,
    /// Whether any cellmap's image is carried inside the `.ssab`.
    has_embedded: bool,
}

impl SSPack {
    /// Parse `bytes`, naming the pack after the `.ssab` itself.
    ///
    /// The name matters because instance parts resolve their sub-animation by
    /// pack name (`ref_anime_pack`), and the exporter writes that same
    /// name into each pack — so the two agree without the host having to derive
    /// one from a file path. Use [`SSPack::load_named`] when the host names packs
    /// itself.
    ///
    /// The bytes are not verified — not here, and not by the runtime — so a malformed or
    /// crafted `.ssab` can cause out-of-bounds reads. Load only packs you trust (SECURITY.md).
    pub fn load(bytes: &[u8]) -> Result<Self, SSRuntimeError> {
        Self::build(None, bytes)
    }

    /// Parse `bytes` under a name the caller chooses, overriding the one in the
    /// `.ssab`. What `add_ssab` / `load_ssab` use.
    ///
    /// The bytes are not verified — not here, and not by the runtime — so a malformed or
    /// crafted `.ssab` can cause out-of-bounds reads. Load only packs you trust (SECURITY.md).
    pub fn load_named(name: &str, bytes: &[u8]) -> Result<Self, SSRuntimeError> {
        Self::build(Some(name), bytes)
    }

    fn build(name: Option<&str>, bytes: &[u8]) -> Result<Self, SSRuntimeError> {
        // SAFETY: `load` and `load_named` require the host to pass a valid `.ssab` (their
        // docs); neither this crate nor the runtime checks it.
        let resource = unsafe { parse(bytes)? };

        // SAFETY: the same bytes, which the host guaranteed are a valid `.ssab` (above), read
        // from the copy the resource owns, which outlives this borrow.
        let ab = unsafe { root_as_ss_anime_binary_unchecked(resource.data().as_slice()) };
        let name: Arc<str> = Arc::from(name.unwrap_or_else(|| ab.name()));

        // The two hashes are different things and must not be swapped:
        // `name_hash` is what a draw batch asks for, `image_path_hash` is the key
        // of the embedded byte blob.
        let embedded: Vec<u32> = ab.embedded_assets().map(|assets| assets.iter().map(|a| a.name_hash()).collect()).unwrap_or_default();
        let mut textures = Vec::new();
        let mut has_embedded = false;
        if let Some(cellmaps) = ab.cellmaps() {
            for cm in cellmaps {
                let is_embedded = embedded.contains(&cm.image_path_hash());
                has_embedded |= is_embedded;
                textures.push(TextureRef { slot: cm.name_hash(), path: cm.image_path().to_string(), embedded: is_embedded });
            }
        }
        // The second table, and not optional: a `DrawBatch.texture_hash` is a
        // cellmap's `name_hash` *or* a texture file name's hash, the latter for a
        // part the author gave a TextureChange (`TCHG`). Miss these and those
        // batches resolve to nothing — here that means the 1x1 white fallback,
        // so the part turns into a white silhouette rather than disappearing.
        // Both tables go into one list under the key a draw batch will ask for.
        //
        // An `ExternalTexture`'s `name_hash` is the hash of its file name, so it
        // is also the key of its embedded blob — no join needed, unlike a
        // cellmap. The same file can therefore appear twice in this list under
        // two different hashes (once as a cellmap's image, once by name); a host
        // that fetches per entry should dedupe by [`TextureRef::path`].
        if let Some(externals) = ab.external_textures() {
            for et in externals {
                let Some(name) = et.name() else { continue };
                let is_embedded = embedded.contains(&et.name_hash());
                has_embedded |= is_embedded;
                textures.push(TextureRef { slot: et.name_hash(), path: name.to_string(), embedded: is_embedded });
            }
        }

        Ok(Self { name, resource, textures: textures.into(), has_embedded })
    }

    /// The name instance references resolve against.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The atlases this pack's draw batches will ask for: every cellmap's image,
    /// then every `external_textures()` entry.
    ///
    /// A reference-mode host walks this to know what to load, and from where,
    /// before playing (see [`TextureRef`]); combined with
    /// [`crate::SSRenderer::texture_size`] it also answers "is this pack ready to
    /// draw".
    ///
    /// **Both tables, always.** `DrawBatch.texture_hash` is a cellmap's
    /// `name_hash` for an ordinary part but a texture *file name*'s hash for one
    /// the author gave a TextureChange (`TCHG`), and the two tables are the only
    /// places those keys come from. An external texture can be a cellmap's own
    /// image under a second hash. Loading only the cellmaps leaves the changed
    /// parts on the white fallback.
    pub fn textures(&self) -> &[TextureRef] {
        &self.textures
    }

    /// Whether the atlases ride inside the `.ssab` (embedded mode).
    /// `false` means the host supplies them (reference mode).
    pub fn has_embedded_textures(&self) -> bool {
        self.has_embedded
    }

    /// The names of the packs this one's instance parts play from,
    /// deduplicated, in `external_instances()` order.
    ///
    /// The counterpart of [`SSPack::textures`] for playback data rather than
    /// images: a pack answers what else it needs, and the host — the only side
    /// with a filesystem or a network — fetches it and hands it to
    /// [`SSPlayer::add_pack`]. The exporter writes the referenced pack's own name
    /// here, which is both the file's stem and the key `ref_anime_pack` resolves
    /// against, so `<the .ssab's directory>/<name>.ssab` finds it.
    ///
    /// Only genuinely external references appear: `external_instances()` is the
    /// exporter's own list, so a pack whose instance parts play its own
    /// animations reports nothing.
    ///
    /// [`SSPlayer::add_pack`]: crate::SSPlayer::add_pack
    pub fn instance_packs(&self) -> Vec<String> {
        // SAFETY: as in `build` — the resource owns the buffer being read.
        let ab = unsafe { root_as_ss_anime_binary_unchecked(self.resource.data().as_slice()) };
        let Some(externals) = ab.external_instances() else { return Vec::new() };
        let mut out: Vec<String> = Vec::new();
        for entry in externals {
            let Some(name) = entry.anime_pack_name() else { continue };
            if name != &*self.name && !out.iter().any(|seen| seen == name) {
                out.push(name.to_string());
            }
        }
        out
    }

    /// Names of every animation in the pack.
    pub fn animation_names(&self) -> Vec<String> {
        self.resource.resource().map(|r| r.get_animation_names()).unwrap_or_default()
    }

    /// The encoded atlases this pack carries, in cellmap order.
    /// Empty for a reference-mode pack.
    ///
    /// Parsing only — no image decoder is involved, so this works in every build
    /// and is the route a host with its own decoder takes (see [`EmbeddedImage`]).
    /// On the web that host is the browser: `createImageBitmap` on these bytes
    /// decodes off the main thread and premultiplies for free, which
    /// [`SSPack::decode_embedded`] cannot do and pays for in wasm size.
    ///
    /// The two hashes are different things and must not be swapped:
    /// the blob is keyed by `image_path_hash`, and what comes back is keyed by
    /// `name_hash` — the one a draw batch asks for.
    pub fn embedded_images(&self) -> Vec<EmbeddedImage<'_>> {
        if !self.has_embedded {
            return Vec::new();
        }
        // SAFETY: as in `build` — the resource owns the buffer being read.
        let ab = unsafe { root_as_ss_anime_binary_unchecked(self.resource.data().as_slice()) };
        let Some(assets) = ab.embedded_assets() else { return Vec::new() };
        let mut bytes_by_hash: HashMap<u32, &[u8]> = HashMap::new();
        for asset in assets {
            if let Some(bytes) = asset.data() {
                bytes_by_hash.insert(asset.name_hash(), bytes.bytes());
            }
        }

        // Each table on its own: a pack can have one and not the other. An
        // embedded pack whose parts only draw TextureChange targets has no
        // cellmaps at all (`assets/overall_emb/Instance.ssab` is one), and
        // requiring both would leave it carrying nothing — every part on the white
        // fallback, with its atlas sitting unread inside the file.
        let mut out = Vec::new();
        if let Some(cellmaps) = ab.cellmaps() {
            for cm in cellmaps {
                if let Some(&bytes) = bytes_by_hash.get(&cm.image_path_hash()) {
                    out.push(EmbeddedImage { slot: cm.name_hash(), bytes });
                }
            }
        }
        // TextureChange targets, keyed by file name — which for an
        // `ExternalTexture` is its `name_hash`, the blob key as well. One blob can
        // come back twice under two hashes (as a cellmap's image and by name);
        // that is what the draw batches ask for, so both entries are wanted.
        if let Some(externals) = ab.external_textures() {
            for et in externals {
                if let Some(&bytes) = bytes_by_hash.get(&et.name_hash()) {
                    out.push(EmbeddedImage { slot: et.name_hash(), bytes });
                }
            }
        }
        out
    }

    /// Decode the atlases this pack carries. Empty for a
    /// reference-mode pack, and empty without the `embedded` feature.
    ///
    /// Decoding is not cached: the pixels are handed over so the caller can put
    /// them wherever GPU memory is owned (`upload_texture`, or an engine's asset
    /// system). Call it once per pack, not once per player.
    pub fn decode_embedded(&self) -> Vec<EmbeddedTexture> {
        self.decode_embedded_where(|_| true)
    }

    /// [`SSPack::decode_embedded`], restricted to the hashes `wanted` accepts.
    ///
    /// The filter runs *before* the PNG decode, which is the whole point: a player
    /// joining a scene where its atlas is already registered skips the decode
    /// entirely, so N players of one pack cost one decode and one upload.
    /// The decodes overlap where there is more than one image; the result stays in
    /// pack order, so an image that will not decode drops out without shifting the
    /// ones after it.
    #[cfg(feature = "embedded")]
    pub fn decode_embedded_where(&self, wanted: impl Fn(u32) -> bool) -> Vec<EmbeddedTexture> {
        let images: Vec<EmbeddedImage<'_>> = self.embedded_images().into_iter().filter(|i| wanted(i.slot)).collect();
        let encoded: Vec<&[u8]> = images.iter().map(|i| i.bytes).collect();
        crate::decode::decode_all(&encoded, false).into_iter().zip(&images).filter_map(|(decoded, image)| decoded.map(|d| EmbeddedTexture { slot: image.slot, rgba: d.rgba, width: d.width, height: d.height })).collect()
    }

    /// Without the `embedded` feature the player carries no image decoder, so the
    /// host decodes instead — in the browser that is `createImageBitmap` over
    /// [`SSPack::embedded_images`], which still needs no sibling files.
    #[cfg(not(feature = "embedded"))]
    pub fn decode_embedded_where(&self, _wanted: impl Fn(u32) -> bool) -> Vec<EmbeddedTexture> {
        Vec::new()
    }

    /// The runtime resource this pack wraps, for building playback contexts.
    pub(crate) fn resource(&self) -> &Arc<SsabResource> {
        &self.resource
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Option<Vec<u8>> {
        std::fs::read(format!("{}/../../assets/overall/{name}.ssab", env!("CARGO_MANIFEST_DIR"))).ok()
    }

    /// Players that each load the same `.ssab` hold one parse between them,
    /// whatever they name it, and the parse goes with the last pack holding it.
    #[test]
    fn loading_the_same_bytes_again_shares_the_parse() {
        let (Some(basic), Some(mask)) = (fixture("Basic"), fixture("Mask")) else {
            eprintln!("fixture missing; run scripts/deploy-examples.sh. skipping.");
            return;
        };
        let first = SSPack::load(&basic).unwrap();
        let named = SSPack::load_named("renamed", &basic).unwrap();
        assert!(Arc::ptr_eq(first.resource(), named.resource()), "the same bytes were parsed twice");
        assert_eq!(named.name(), "renamed");
        assert!(!Arc::ptr_eq(first.resource(), SSPack::load(&mask).unwrap().resource()), "different bytes shared a parse");

        let parse = Arc::downgrade(first.resource());
        drop((first, named));
        assert!(parse.upgrade().is_none(), "the parse outlived every pack holding it");
    }
}
