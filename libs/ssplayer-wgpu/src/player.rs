//! Layer 3 (operation): SSPlayer — the primary public API.
//!
//! Owns a tree of runtime `Context`s (one per animation instance) and a single
//! shared `SSInternalPlayer`. Borrow model: it holds no `Device`/`Queue`/
//! `Surface`; the host lends them on every `update`/`draw`.
//!
//! Instance parts reference a sub-animation in another `.ssab` pack. Each
//! instance child gets its **own** `Context` (hence its own SoA buffer), which
//! is how the single-buffer overwrite problem is avoided (see SPEC §7). The
//! host registers every pack via [`SSPlayer::add_ssab`]; external references are
//! resolved by pack name.

use crate::frame::FrameView;
use crate::internal::{BatchInfo, RendererConfig, Retention, SSInternalPlayer, SSRenderer};
use crate::mask::{GeomKind, MaskOp, MaskWriter, PartMaskFlags, StencilTest, plan_writers};
use ssab::ssab::ss::format::{PartType, root_as_ss_anime_binary_unchecked};
use ssruntime::error::SSRuntimeError;
use ssruntime::runtime::framedata::ss::runtime::{DrawBatchKind, root_as_frame_data_unchecked};
use ssruntime::{Context, EffectSlot, InstanceSlot, SsabData, SsabResource};
use std::collections::HashMap;
use std::sync::Arc;

/// Column-major 4x4 identity.
const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Guards against cyclic instance references blowing the stack.
const MAX_INSTANCE_DEPTH: u32 = 12;

/// Construction config for [`SSPlayer`], matched to the host's render target.
pub struct SSPlayerConfig {
    /// Target color format. Forwarded to the renderer so pipelines match.
    pub color_format: wgpu::TextureFormat,
    /// Depth/stencil format of the host's render target, or `None` to run
    /// without masking (SPEC §6). When `Some`, the host must attach a matching
    /// depth/stencil buffer to every render pass, and mask batches are
    /// rasterised into the stencil buffer.
    pub stencil_format: Option<wgpu::TextureFormat>,
}

/// One node in the playback tree: a runtime context plus its instance children
/// and effect slots.
struct PlaybackNode {
    context: Context<'static>,
    /// Instance children keyed by part index (index into parts/draw_order/SoA).
    instances: HashMap<usize, InstanceChild>,
    /// Effect slots keyed by part index. Effects reuse this node's context
    /// (no separate context, unlike instances).
    effects: HashMap<usize, EffectSlot<'static>>,
    /// Static per-part mask flags, indexed by part index. Read once from ssab
    /// `PartData` at build time (SPEC §6). Empty when the pack has no ssab data.
    mask_flags: Vec<PartMaskFlags>,
}

struct InstanceChild {
    slot: InstanceSlot,
    node: Box<PlaybackNode>,
}

/// The SpriteStudio player. One instance owns one animation-instance tree.
pub struct SSPlayer {
    internal: SSInternalPlayer,
    /// All registered packs, keyed by pack name (matches `ref_anime_pack`).
    resources: HashMap<String, Arc<SsabResource>>,
    main_pack: Option<String>,
    root: Option<PlaybackNode>,
    current_animation: Option<String>,
    /// Root frame_no from the previous `update`, used to detect the loop edge
    /// that instance slots need (`parent_looped`).
    last_root_frame: f32,
    /// Embedded-mode textures decoded at `add_ssab` (CPU), awaiting GPU upload on
    /// the next `update` (the borrow model means load has no `Device`/`Queue`).
    /// Each entry is `(cellmap name_hash, straight-alpha RGBA, width, height)`.
    pending_textures: Vec<(u32, Vec<u8>, u32, u32)>,
    /// Last viewport passed to [`SSPlayer::set_viewport`], re-applied on restore
    /// (the projection lives in a uniform buffer that dies with the device).
    viewport: Option<(f32, f32, bool)>,
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

impl SSPlayer {
    /// Create a player against a borrowed `device` and the host's `renderer`.
    /// Neither is retained (SPEC §1.2): the renderer is device-scoped and shared,
    /// so it is lent back on every `update` / `draw`.
    pub fn new(device: &wgpu::Device, renderer: &SSRenderer, config: &SSPlayerConfig) -> Self {
        let internal = SSInternalPlayer::new(device, renderer, &RendererConfig { color_format: config.color_format, stencil_format: config.stencil_format });
        Self { internal, resources: HashMap::new(), main_pack: None, root: None, current_animation: None, last_root_frame: 0.0, pending_textures: Vec::new(), viewport: None, transform: IDENTITY, order: 0, visible: true, alpha: 1.0 }
    }

    /// Rebuild every GPU resource against a freshly created `device` after the
    /// previous one was lost (SPEC §14 — the counterpart of the WebGL player's
    /// `SSWebGLRenderer.restoreContext()` + `SSWebGLResourceManager.restoreContext()`).
    ///
    /// The borrow model puts device creation on the host, so device loss is the
    /// host's to detect (`GPUDevice.lost` on the web, `wgpu::Device::set_device_lost_callback`
    /// natively) and its to recover from: create a new device, reconfigure the
    /// surface, then call this. Pipelines, buffers and bind groups are rebuilt for
    /// the same target formats the player was constructed with, every texture is
    /// re-uploaded from its retained source, and the last viewport is re-applied —
    /// so the next `update`/`draw` pair renders the same frame it would have.
    ///
    /// Playback state (loaded packs, the instance tree, the current frame) lives on
    /// the CPU and is untouched: the animation resumes where it was interrupted.
    ///
    /// A target whose *formats* changed needs a new [`SSPlayer`], not this call.
    pub fn restore_device(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        // The projection buffer and the frame arena belonged to the dead device;
        // the target-scoped half is cheapest to rebuild whole. The renderer's own
        // pipelines and textures are restored by `SSRenderer::restore_device`,
        // which the host calls once for the whole device — not once per player.
        self.internal = SSInternalPlayer::new(device, renderer, &self.internal.config());

        // Anything queued for the old device is stale — the decode below covers it.
        self.pending_textures.clear();
        // Embedded packs (SPEC §8.1): re-decode from the ssab bytes we still own,
        // which is why they upload as `Reproducible` and cost no retained copy.
        // Sorted by pack name because later packs overwrite earlier ones on a
        // texture-hash collision, and a restore must not land on a different
        // winner than the last one did.
        let mut packs: Vec<(&String, &Arc<SsabResource>)> = self.resources.iter().collect();
        packs.sort_by_key(|(name, _)| *name);
        let packs: Vec<Arc<SsabResource>> = packs.into_iter().map(|(_, r)| Arc::clone(r)).collect();
        for resource in &packs {
            self.decode_embedded_textures(resource.data().as_slice());
        }
        self.flush_pending_textures(renderer, device, queue);

        if let Some((width, height, y_down)) = self.viewport {
            self.internal.update_projection(queue, width, height, y_down);
        }
    }

    /// Register an `.ssab` pack under `pack_name` (used to resolve external
    /// instance references). Does not change what is playing.
    pub fn add_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError> {
        let ssab_data = SsabData::load_copy(data)?;
        let resource = SsabResource::new(ssab_data);
        if resource.resource_static_ptr().is_null() {
            return Err(SSRuntimeError::NotLoaded);
        }
        self.resources.insert(pack_name.to_string(), resource);
        // Embedded ssab (SPEC §8.1): decode the atlas PNGs the pack carries and
        // queue them for GPU upload on the next `update`. Reference ssab has no
        // embedded_assets, so this is a no-op and the host supplies textures via
        // `upload_texture`. Both modes funnel into the same renderer texture map.
        self.decode_embedded_textures(data);
        Ok(())
    }

    /// Decode a pack's embedded atlas textures (SPEC §8.1) into `pending_textures`
    /// for upload on the next `update`. Each `CellMap` names its texture by
    /// `name_hash` (what `DrawBatch.texture_hash` carries) and points at its bytes
    /// via `image_path_hash` (the `embedded_assets` key). No-op without the
    /// `embedded` feature (host decodes texture-side instead).
    #[cfg(feature = "embedded")]
    fn decode_embedded_textures(&mut self, data: &[u8]) {
        let ab = unsafe { root_as_ss_anime_binary_unchecked(data) };
        let (Some(cellmaps), Some(assets)) = (ab.cellmaps(), ab.embedded_assets()) else {
            return;
        };
        // image_path_hash -> raw PNG bytes.
        let mut bytes_by_hash: HashMap<u32, &[u8]> = HashMap::new();
        for asset in assets {
            if let Some(bytes) = asset.data() {
                bytes_by_hash.insert(asset.name_hash(), bytes.bytes());
            }
        }
        for cm in cellmaps {
            let Some(png) = bytes_by_hash.get(&cm.image_path_hash()) else {
                continue;
            };
            let Ok(img) = image::load_from_memory(png) else {
                continue;
            };
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            // Keyed by the cellmap name_hash — the hash the renderer looks up.
            self.pending_textures.push((cm.name_hash(), rgba.into_raw(), w, h));
        }
    }

    #[cfg(not(feature = "embedded"))]
    fn decode_embedded_textures(&mut self, _data: &[u8]) {}

    /// Register `pack_name` and make it the main pack to play from.
    pub fn load_ssab(&mut self, pack_name: &str, data: &[u8]) -> Result<(), SSRuntimeError> {
        self.add_ssab(pack_name, data)?;
        self.main_pack = Some(pack_name.to_string());
        self.root = None;
        self.current_animation = None;
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
        let root = build_node(&self.resources, &main_pack, anim_name, 0).ok_or(SSRuntimeError::NotLoaded)?;
        self.last_root_frame = root.context.get_frame_no();
        self.root = Some(root);
        self.current_animation = Some(anim_name.to_string());
        Ok(())
    }

    /// Start — or, after [`SSPlayer::pause`], resume — the selected animation.
    /// No-op until [`SSPlayer::set_animation`] has succeeded.
    pub fn play(&mut self) {
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

    /// Place this player: translation, scale, then rotation in radians, composed
    /// into the root transform of its tree.
    ///
    /// This is the per-player half of SPEC §5.4 — the projection belongs to the
    /// render target ([`SSPlayer::set_viewport`]), the placement to the player.
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

    /// Skip this player entirely when false: [`SSPlayer::update`] advances no
    /// timeline and produces no geometry, so [`SSPlayer::draw`] records nothing.
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
        let anim_resource = self.resources.get(self.main_pack.as_ref()?)?.resource()?;
        Some(anim_resource.get_animation_names())
    }

    /// Canvas size of the currently playing animation.
    pub fn canvas_size(&self) -> Option<(f32, f32)> {
        let name = self.current_animation.as_ref()?;
        let anim_resource = self.resources.get(self.main_pack.as_ref()?)?.resource()?;
        let idx = anim_resource.find_animation_index(name)?;
        let anim_meta = &anim_resource.animations()[idx];
        Some((anim_meta.canvas_width, anim_meta.canvas_height))
    }

    /// Set the projection to the render target's pixel dimensions. Remembered, so
    /// [`SSPlayer::restore_device`] can re-apply it to a rebuilt uniform buffer.
    pub fn set_viewport(&mut self, queue: &wgpu::Queue, width: f32, height: f32, y_down: bool) {
        self.internal.update_projection(queue, width, height, y_down);
        self.viewport = Some((width, height, y_down));
    }

    /// Upload everything `decode_embedded_textures` queued. Called at the start of
    /// each `update` (the earliest point the borrow model lends a `Queue`) and by
    /// `restore_device`, which re-queues the packs against the new device.
    fn flush_pending_textures(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        for (hash, rgba, w, h) in std::mem::take(&mut self.pending_textures) {
            // Reproducible: this player still owns the `.ssab` and can decode the
            // atlas again, so the renderer need not retain a second copy.
            renderer.upload_texture(device, queue, hash, &rgba, (w, h), Retention::Reproducible);
        }
    }

    /// Advance the tree by `delta_secs` and build GPU buffers for the frame.
    pub fn update(&mut self, renderer: &mut SSRenderer, device: &wgpu::Device, queue: &wgpu::Queue, delta_secs: f32) {
        // Flush any embedded textures decoded at load time (§8.1); the first
        // `update` is the earliest point the borrow model lends a Queue.
        self.flush_pending_textures(renderer, device, queue);

        self.internal.begin_frame();

        // `visible == false` means "this player cannot be seen": no timeline
        // advance and no geometry, so the frame ends empty and `draw` records
        // nothing. Textures are still flushed above — a player can be made
        // visible again without re-loading anything.
        if !self.visible {
            self.internal.end_frame(renderer, device, queue);
            return;
        }

        if let Some(root) = &mut self.root {
            // The runtime works in milliseconds for the primary context.
            root.context.update(delta_secs * 1000.0);
            let frame_no = root.context.get_frame_no();
            // Detect the loop edge (forward playback wrapping past the end) so
            // instance slots can persist/re-arm their children correctly.
            let looped = frame_no + 0.5 < self.last_root_frame;
            self.last_root_frame = frame_no;
            let draw = NodeDraw { transform: &self.transform, alpha: self.alpha };
            let mut build = FrameBuild { internal: &mut self.internal, renderer };
            render_node(root, &mut build, draw, frame_no, delta_secs, looped, StencilTest::None);
        }

        self.internal.end_frame(renderer, device, queue);
    }

    /// Record the prepared frame into a RenderPass opened by the host.
    pub fn draw<'a>(&'a self, renderer: &'a SSRenderer, pass: &mut wgpu::RenderPass<'a>) -> usize {
        self.internal.draw(renderer, pass)
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
fn build_node(resources: &HashMap<String, Arc<SsabResource>>, pack: &str, anime: &str, depth: u32) -> Option<PlaybackNode> {
    if depth > MAX_INSTANCE_DEPTH {
        return None;
    }
    let resource = resources.get(pack)?;
    let res_ptr = resource.resource_static_ptr();
    if res_ptr.is_null() {
        return None;
    }
    // SAFETY: the resource lives in `resources` for the player's lifetime.
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
                        && let Some(child) = build_node(resources, inst.ref_anime_pack(), inst.ref_anime_name(), depth + 1)
                    {
                        instances.insert(i, InstanceChild { slot: InstanceSlot::new(), node: Box::new(child) });
                    }
                }
                PartType::PartTypeEffect => {
                    if let Some(eff) = part.part_type_as_part_type_effect()
                        && let Ok(slot) = EffectSlot::new(static_res, eff.ref_effect_name_hash(), i as u32)
                    {
                        effects.insert(i, slot);
                    }
                }
                _ => {}
            }
        }
    }

    Some(PlaybackNode { context, instances, effects, mask_flags })
}

/// `DrawBatch.flags` bit set when a content batch is a masked target
/// (mirrors `FLAG_MASK_INFLUENCE` in the runtime).
const FLAG_MASK_INFLUENCE: u8 = 0x01;

/// Advance the mask cursor to `up_to_rank`, appending stencil writes for every
/// planned writer whose rank is below it (mask doc §2-1, WebGL `syncMasksTo`):
/// a pure mask's backward scope has ended → remove it (Decr/Invert); a clipping
/// writer's forward scope has begun → add it (Incr/Invert). Monotonic in
/// `cursor`; ops are baked per pipeline, so ref is always 0.
fn sync_masks_to(build: &mut FrameBuild<'_>, frame: &FrameView, transform: &[f32; 16], writers: &[MaskWriter], cursor: &mut usize, up_to_rank: usize) {
    while *cursor < writers.len() && writers[*cursor].rank < up_to_rank {
        let w = &writers[*cursor];
        let op = if w.is_clipping {
            if w.op_invert { MaskOp::Invert } else { MaskOp::Incr }
        } else if w.op_invert {
            MaskOp::Invert
        } else {
            MaskOp::Decr
        };
        build.internal.append_mask_writer(build.renderer, frame, transform, w, op);
        *cursor += 1;
    }
}

/// The two halves of layer 2, bundled for the tree walk: the target-scoped
/// accumulator being filled, and the device-scoped renderer it reads texture
/// sizes from. Only `internal` is mutated — pipelines are compiled later, in
/// `end_frame`, which is where the renderer is borrowed mutably.
struct FrameBuild<'a> {
    internal: &'a mut SSInternalPlayer,
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

/// Recursively append a node's geometry to the renderer, driving instance slots
/// and composing transforms. The node's context must already be advanced to
/// `frame_no` (root by `update`, children by the parent's `InstanceSlot::step`).
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
fn render_node(node: &mut PlaybackNode, build: &mut FrameBuild<'_>, draw: NodeDraw, frame_no: f32, delta_secs: f32, node_looped: bool, inherited_stencil: StencilTest) {
    let NodeDraw { transform, alpha } = draw;
    let frame_data = match node.context.get_frame_data(frame_no) {
        Ok(bytes) => unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) },
        Err(_) => return,
    };
    let frame = frame_view(&mut node.context, frame_data);

    // Own masking runs only at the root scope; a masked sub-animation inherits.
    let masking = build.internal.stencil_enabled() && inherited_stencil == StencilTest::None;
    let writers = if masking { plan_writers(&frame, &node.mask_flags) } else { Vec::new() };

    // Pre-write pure masks so they are active for lower-rank content (§2-1).
    for w in &writers {
        if !w.is_clipping {
            let op = if w.op_invert { MaskOp::Invert } else { MaskOp::Incr };
            build.internal.append_mask_writer(build.renderer, &frame, transform, w, op);
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
                        sync_masks_to(build, &frame, transform, &writers, &mut cursor, rank);
                        if is_pure(part_idx) {
                            continue;
                        }
                        let stencil = node.mask_flags.get(part_idx).map(|f| f.polarity()).unwrap_or(StencilTest::None);
                        let info = BatchInfo { start_rank: rank, count: 1, texture_hash: batch.texture_hash(), blend_type: batch.blend_type(), kind, stencil, alpha };
                        build.internal.append_batch(build.renderer, &frame, transform, &info);
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
                        build.internal.append_batch(build.renderer, &frame, transform, &info);
                    }
                } else {
                    let info = BatchInfo { start_rank, count, texture_hash: batch.texture_hash(), blend_type: batch.blend_type(), kind, stencil: inherited_stencil, alpha };
                    build.internal.append_batch(build.renderer, &frame, transform, &info);
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
                            sync_masks_to(build, &frame, transform, &writers, &mut cursor, rank);
                            f.polarity()
                        } else {
                            StencilTest::None
                        }
                    } else {
                        inherited_stencil
                    };

                    let Some(child) = node.instances.get_mut(&part_idx) else { continue };

                    let ev = node.context.get_active_event_instance(part_idx as u32);
                    let result = child.slot.step(ev, &mut child.node.context, frame_no, delta_secs, node_looped);

                    // On a transition edge the slot configures the child's section
                    // but leaves the frame snap to the host: play() moves the child
                    // to its start frame so independent playback actually advances.
                    if result.transitioned {
                        child.node.context.play();
                    }
                    let child_frame_no = child.node.context.get_frame_no();

                    if result.visible {
                        // Compose the instance part's world matrix onto the parent transform.
                        let m: [f32; 16] = frame.world_matrices[part_idx * 16..part_idx * 16 + 16].try_into().unwrap();
                        let child_transform = mat4_mul(transform, &m);
                        let child_draw = NodeDraw { transform: &child_transform, alpha };
                        render_node(&mut child.node, build, child_draw, child_frame_no, delta_secs, result.child_looped, child_inherited);
                    }
                }
            }
            DrawBatchKind::Effect => {
                // A masked effect (or one inside an inherited-masked subtree) tests
                // against the screen-space stencil; sync coverage once at its rank.
                let eff_stencil = if masked_target {
                    let part_idx = draw_order.get(start_rank) as usize;
                    sync_masks_to(build, &frame, transform, &writers, &mut cursor, start_rank);
                    node.mask_flags.get(part_idx).map(|f| f.polarity()).unwrap_or(StencilTest::None)
                } else {
                    inherited_stencil
                };
                for rank in start_rank..start_rank + count {
                    let part_idx = draw_order.get(rank) as usize;
                    let Some(slot) = node.effects.get_mut(&part_idx) else { continue };

                    let ev = node.context.get_active_effect_event(part_idx as u32);
                    let fps = node.context.get_fps() as f32;
                    // vert_stride=2 (packed XY), y_flip=false (Y handled by the world matrix).
                    let r = slot.step(ev, frame_no, delta_secs, fps, node_looped, false, 2);

                    if r.visible && !r.draw_plan_buf.is_null() && r.draw_plan_len > 0 {
                        // The plan buffer is owned by the slot and valid until its
                        // next step; append_effect copies it immediately.
                        let plan = unsafe { std::slice::from_raw_parts(r.draw_plan_buf, r.draw_plan_len) };
                        let m: [f32; 16] = frame.world_matrices[part_idx * 16..part_idx * 16 + 16].try_into().unwrap();
                        let eff_transform = mat4_mul(transform, &m);
                        build.internal.append_effect(plan, &eff_transform, eff_stencil, alpha);
                    }
                }
            }
            // Mask batches emit no colour (coverage is handled by the writer
            // pre-write / scoped replay above). Text / Nines: not yet handled.
            _ => {}
        }
    }

    if masking {
        unwind_masks(build, &frame, transform, &writers, cursor);
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
fn unwind_masks(build: &mut FrameBuild<'_>, frame: &FrameView, transform: &[f32; 16], writers: &[MaskWriter], cursor: usize) {
    let remove = |w: &MaskWriter| if w.op_invert { MaskOp::Invert } else { MaskOp::Decr };
    // Consumed clipping writers: `sync_masks_to` added their coverage.
    for w in writers[..cursor].iter().filter(|w| w.is_clipping) {
        build.internal.append_mask_writer(build.renderer, frame, transform, w, remove(w));
    }
    // Unconsumed pure masks: pre-written before the batch loop, never removed.
    for w in writers[cursor..].iter().filter(|w| !w.is_clipping) {
        build.internal.append_mask_writer(build.renderer, frame, transform, w, remove(w));
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
