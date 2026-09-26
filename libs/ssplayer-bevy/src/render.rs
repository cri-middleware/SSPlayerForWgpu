//! The render-world half: the playback trees, the device-scoped renderer, the
//! target-scoped scene, and the one system that draws them.
//!
//! Everything here follows the player's borrow model — the renderer and scene
//! never own the `Device` / `Queue` / target, Bevy does, and lends them per
//! frame exactly as the winit and C++ hosts do.

use bevy::asset::AssetId;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{RenderPassDescriptor, TextureId};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
// `AlphaMode` is aliased: `bevy::prelude` has one of its own, and a Bevy image is
// straight-alpha, which is the form the player must be told about.
use ssplayer_wgpu::{AlphaMode as SsAlphaMode, MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};
use std::collections::{HashMap, HashSet};

use crate::asset::{Keys, RenderSsPack, SsPack};
use crate::extract::ExtractedPlayers;
use crate::feedback::{FeedbackChannel, PlaybackFeedback};

/// One character's playback tree, plus what it was last built from.
///
/// `pack` and `animation` are kept so a change to either can be told from the
/// frame-to-frame traffic: rebuilding the tree allocates, the rest does not.
///
/// `started` / `playing` / `restart` track transport, because the player's
/// `play()` rewinds (it is a start, not a resume) and `is_playing()` stays true
/// through a pause — so neither can be used to tell "carry on" from "restart".
/// `started` is whether this tree has been `play()`ed at all; `playing` is the
/// last `SsPlayback::playing` applied, for edge-detecting pause vs resume;
/// `restart` is the last `SsPlayback::restart` counter seen, for detecting an
/// explicit replay. `started`/`playing` reset when the tree rebuilds.
struct PlayerSlot {
    entity: MainEntity,
    player: SSPlayer,
    /// The pack the tree was built from, and which load of it — a hot reload
    /// replaces the asset under the same id with a new generation.
    pack: Option<(AssetId<SsPack>, u64)>,
    animation: String,
    started: bool,
    playing: bool,
    restart: u32,
    /// Runtime texture overrides currently bound, cellmap name -> (the image
    /// applied, the private renderer key it went in under). Diffed against the
    /// character's `SsCellmapTextures` each frame: a new or changed image is
    /// registered, a removed one is restored, and the whole set is dropped when
    /// the pack changes (its cellmaps do).
    applied_overrides: HashMap<String, (AssetId<Image>, u32)>,
}

/// Everything the render world owns on this crate's behalf.
///
/// `SSRenderer` is device-scoped and `SSScene` is target-scoped, so there is one
/// renderer for the whole app and one scene per view it draws into.
/// There is one scene here, and a second view is refused rather than silently
/// double-advancing every timeline: `SSScene::update` fuses the timeline advance
/// with the geometry build, so running it twice a frame would play every
/// animation at double speed.
#[derive(Resource)]
pub(crate) struct SsRenderState {
    renderer: SSRenderer,
    scene: Option<SSScene>,
    /// What `scene` was built against. A view that changes format (HDR toggled)
    /// invalidates the pipelines baked into it.
    scene_format: Option<wgpu::TextureFormat>,
    /// Insertion-ordered, because this order is what breaks `order` ties.
    slots: Vec<PlayerSlot>,
    /// Every atlas lent to `renderer`, by renderer key, and the Bevy texture
    /// lent under it. Device-scoped like the renderer, so the characters sharing
    /// a pack share one binding.
    lent_atlases: HashMap<u32, TextureId>,
    /// The view `scene` belongs to; anything else is refused.
    bound_view: Option<Entity>,
    warned_second_view: bool,
    warned_srgb: bool,
    warned_msaa: bool,
}

/// Build the render-world state on the device Bevy just acquired.
///
/// Deliberately in `RenderStartup`, which Bevy re-runs every time it acquires a
/// `RenderDevice` — including after recovering a lost one. That is the whole
/// device-loss story for this adapter: a fresh `SSRenderer` on the new device,
/// `slots` and the lent atlases cleared, and `sync_players` rebuilds the players
/// from the packs already in the render world next frame, lending the atlases
/// again once Bevy has re-prepared its images. So recovery needs no `restore_device` here —
/// but it is the host's to opt into, because Bevy's default error handler quits
/// on device loss rather than recovering. Do not move this to a run-once startup:
/// that is what would break recovery.
pub(crate) fn init_state(mut commands: Commands, device: Res<RenderDevice>) {
    commands.insert_resource(SsRenderState {
        renderer: SSRenderer::new(device.wgpu_device()),
        scene: None,
        scene_format: None,
        slots: Vec::new(),
        lent_atlases: HashMap::new(),
        bound_view: None,
        warned_second_view: false,
        warned_srgb: false,
        warned_msaa: false,
    });
}

/// Apply this frame's extracted state to the playback trees.
///
/// Runs before anything touches the GPU. Nothing here reads a file: the pack
/// arrived through the asset server, and building a tree from it takes its
/// already-parsed packs. An embedded atlas the pack carries waits in the player
/// until the first frame that draws it, because the borrow model lends no device
/// until `update`.
pub(crate) fn sync_players(mut state: Option<ResMut<SsRenderState>>, extracted: Res<ExtractedPlayers>, device: Res<RenderDevice>, images: Res<RenderAssets<GpuImage>>, packs: Res<RenderAssets<RenderSsPack>>) {
    let Some(state) = state.as_deref_mut() else {
        return;
    };

    // Bind the view afresh each frame. The second-view guard only needs to hold
    // within a frame, and latching it across frames would freeze playback forever
    // if the bound camera is ever despawned and replaced (a scene transition).
    state.bound_view = None;

    // Drop the trees whose entities went away. The extracted list is this
    // frame's whole truth, so anything absent from it is gone.
    let before = state.slots.len();
    state.slots.retain(|slot| extracted.players.iter().any(|player| player.entity == slot.entity));
    let mut packs_changed = state.slots.len() != before;

    // Split the borrow: the override reconcile needs the device-scoped renderer
    // and one slot at once, which are disjoint fields.
    let SsRenderState { renderer, slots, lent_atlases, .. } = state;
    let device = device.wgpu_device();

    for extracted_player in &extracted.players {
        let position = slots.iter().position(|slot| slot.entity == extracted_player.entity);
        let index = match position {
            Some(index) => index,
            None => {
                slots.push(PlayerSlot { entity: extracted_player.entity, player: SSPlayer::new(), pack: None, animation: String::new(), started: false, playing: false, restart: 0, applied_overrides: HashMap::new() });
                slots.len() - 1
            }
        };
        let slot = &mut slots[index];

        // The two allocating steps, each guarded by what it was last given.
        //
        // A pack is taken only once it has settled, so the tree is never built
        // before its atlases can be lent — no frame of white silhouettes. Until a
        // newer load settles the character keeps the tree it has, which is what
        // keeps a hot reload from blinking it out.
        let mut pack_changed = false;
        if extracted_player.settled
            && let Some(pack) = packs.get(extracted_player.pack)
            && slot.pack != Some((extracted_player.pack, pack.generation))
        {
            slot.pack = Some((extracted_player.pack, pack.generation));
            // A fresh player rather than a re-registration: a player only ever
            // accumulates packs, and the old ones are this pack's no longer.
            slot.player = SSPlayer::new();
            slot.player.set_pack(&pack.packs[0]);
            for sibling in &pack.packs[1..] {
                slot.player.add_pack(sibling);
            }
            // Key each pack's atlases the way the asset loader did, so the draw
            // batches resolve to the textures lent below. Nothing is read.
            let mut keys = Keys { base: &pack.path };
            for each in &pack.packs {
                bevy::tasks::block_on(slot.player.load_assets(&mut keys, each.name()));
            }
            slot.animation.clear();
            // A different pack means a different tree; transport starts over.
            slot.started = false;
            slot.playing = false;
            pack_changed = true;
            packs_changed = true;
        }
        // Only against a tree built from the pack the character names now. A switch
        // changes `pack` and `animation` together, and until the new pack settles
        // the tree is still the old one — which has no such animation.
        let built_from_named_pack = slot.pack.is_some_and(|(id, _)| id == extracted_player.pack);
        if built_from_named_pack && !extracted_player.animation.is_empty() && slot.animation != extracted_player.animation {
            slot.animation.clone_from(&extracted_player.animation);
            if let Err(err) = slot.player.set_animation(&extracted_player.animation) {
                // The names come from the pack, so listing them turns a typo
                // into a one-line fix rather than a hunt through the .ssab.
                let path = packs.get(extracted_player.pack).map(|pack| pack.path.to_string()).unwrap_or_default();
                error!("ssplayer-bevy: no animation {:?} in {path:?} ({err:?}); it has {:?}", extracted_player.animation, slot.player.animation_names().unwrap_or_default());
                continue;
            }
            // A fresh tree, stopped at its head; transport starts over.
            slot.started = false;
            slot.playing = false;
        }

        // Allocation-free from here down.
        let transform = extracted_player.transform;
        slot.player.set_transform(transform.x, transform.y, transform.scale_x, transform.scale_y, transform.rotation);

        let playback = extracted_player.playback;
        slot.player.set_speed_scale(playback.speed);
        slot.player.set_order(playback.order);
        slot.player.set_alpha(playback.alpha);
        slot.player.set_visible(playback.visible);
        // An explicit replay: `play()` rewinds but is otherwise only the initial
        // start, and the `playing` toggle only pauses/resumes — so a bumped
        // restart counter is the one way to re-trigger a finished animation.
        if slot.restart != playback.restart {
            slot.restart = playback.restart;
            slot.player.play();
            slot.started = true;
            slot.playing = true;
        }

        // Transport under the player's split play/resume semantics: `play()`
        // rewinds and re-arms, so it is only the initial start; `pause()` holds
        // and `resume()` lifts, neither rewinding. `is_playing()` stays true
        // through a pause, so the wanted state is tracked here and acted on at the
        // edge rather than compared against it.
        let want = playback.playing;
        if !slot.started {
            if want {
                slot.player.play();
                slot.started = true;
            }
        } else if want != slot.playing {
            if want {
                slot.player.resume();
            } else {
                slot.player.pause();
            }
        }
        slot.playing = want;

        reconcile_overrides(slot, renderer, device, &images, &extracted_player.overrides, pack_changed);
    }

    lend_atlases(renderer, device, lent_atlases, slots, &packs, &images, packs_changed);
}

/// Lend the renderer every atlas the characters' packs draw with, as the texture
/// Bevy prepared for it.
///
/// Borrowed rather than uploaded, so an atlas is decoded once (by Bevy) and held
/// once (on the GPU): the renderer keeps no pixels of its own for it. Re-lent when
/// Bevy replaces the texture — a hot-reloaded image — and, when the set of packs
/// in play changed, taken back from the renderer once no character's pack names
/// it, which is what lets Bevy free it.
fn lend_atlases(renderer: &mut SSRenderer, device: &wgpu::Device, lent: &mut HashMap<u32, TextureId>, slots: &[PlayerSlot], packs: &RenderAssets<RenderSsPack>, images: &RenderAssets<GpuImage>, packs_changed: bool) {
    let in_play = || slots.iter().filter_map(|slot| slot.pack).filter_map(|(id, _)| packs.get(id));
    for pack in in_play() {
        for &(key, image) in &pack.atlases {
            let Some(gpu) = images.get(image) else { continue };
            if lent.get(&key) == Some(&gpu.texture.id()) {
                continue;
            }
            let size = gpu.texture.size();
            renderer.set_texture(device, key, &gpu.texture_view, (size.width, size.height), SsAlphaMode::Straight);
            lent.insert(key, gpu.texture.id());
        }
    }
    if packs_changed {
        for (key, _) in lent.extract_if(|key, _| !in_play().any(|pack| pack.atlases.iter().any(|(k, _)| k == key))) {
            renderer.forget_texture(key);
        }
    }
}

/// Bring one character's bound texture overrides in line with what it asks for
/// this frame.
///
/// The character names its overrides by cellmap name in [`crate::SsCellmapTextures`];
/// this registers each new or changed image in the device-scoped renderer under
/// the player's own private key, restores the ones it dropped, and — when the
/// pack changed under it — clears the lot, because the new pack's cellmaps are not
/// the old one's. An image Bevy has not finished preparing is left for a later
/// frame, so binding a freshly-loaded handle is safe.
fn reconcile_overrides(slot: &mut PlayerSlot, renderer: &mut SSRenderer, device: &wgpu::Device, images: &RenderAssets<GpuImage>, wanted: &[(String, Handle<Image>)], pack_changed: bool) {
    if pack_changed && !slot.applied_overrides.is_empty() {
        for (name, (_, key)) in std::mem::take(&mut slot.applied_overrides) {
            slot.player.restore_cellmap_texture(None, &name);
            renderer.forget_texture(key);
        }
    }

    // Apply or refresh each wanted override whose image is ready. A straight-alpha
    // Bevy image, borrowed through `set_texture` — so device loss is Bevy's to
    // repair (it re-prepares the image and this re-binds it next frame).
    for (name, handle) in wanted {
        let id = handle.id();
        if slot.applied_overrides.get(name).is_some_and(|(applied, _)| *applied == id) {
            continue;
        }
        let Some(gpu) = images.get(id) else { continue };
        let Some(key) = slot.player.override_cellmap_texture_key(None, name) else { continue };
        let size = gpu.texture.size();
        renderer.set_texture(device, key, &gpu.texture_view, (size.width, size.height), SsAlphaMode::Straight);
        slot.applied_overrides.insert(name.clone(), (id, key));
    }

    // Restore any override the character dropped from its component.
    let wanted_names: HashSet<&str> = wanted.iter().map(|(name, _)| name.as_str()).collect();
    let stale: Vec<String> = slot.applied_overrides.keys().filter(|name| !wanted_names.contains(name.as_str())).cloned().collect();
    for name in stale {
        if let Some((_, key)) = slot.applied_overrides.remove(&name) {
            slot.player.restore_cellmap_texture(None, &name);
            renderer.forget_texture(key);
        }
    }
}

/// Advance, build and draw every player into the view being rendered.
///
/// Ordered after [`bevy::core_pipeline::core_2d::main_transparent_pass_2d`]
/// rather than merely inside
/// `Core2dSystems::MainPass`: two systems that each take a [`RenderContext`] get
/// their own command encoder, and only an explicit edge decides which of the two
/// buffers reaches the queue first.
pub(crate) fn draw_players(
    mut state: Option<ResMut<SsRenderState>>,
    extracted: Res<ExtractedPlayers>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    channel: Res<FeedbackChannel>,
    view: ViewQuery<(&ExtractedCamera, &ViewTarget)>,
    mut ctx: RenderContext,
) {
    let Some(state) = state.as_deref_mut() else {
        return;
    };
    if state.slots.is_empty() {
        // Clear last frame's feedback so the main world stops answering for
        // characters that are all gone.
        channel.store(Vec::new());
        return;
    }

    let view_entity = view.entity();
    let (camera, target) = view.into_inner();

    // The player's pipelines are single-sampled; a multisampled target is a wgpu
    // validation panic, not a soft failure. Bevy's default MSAA is on (Sample4),
    // so warn once and skip rather than crash — the fix is the camera's `Msaa::Off`.
    if target.sampled_main_texture_view().is_some() {
        if !state.warned_msaa {
            state.warned_msaa = true;
            warn!(
                "ssplayer-bevy: the view is multisampled (MSAA on); the player's pipelines \
                 are single-sampled, so nothing is drawn. Add `Msaa::Off` to the camera."
            );
        }
        return;
    }

    match state.bound_view {
        Some(bound) if bound != view_entity => {
            if !state.warned_second_view {
                state.warned_second_view = true;
                warn!(
                    "ssplayer-bevy: more than one view is rendering; only the first draws. \
                     A scene per view needs the timeline advance split off from the geometry build."
                );
            }
            return;
        }
        Some(_) => {}
        None => state.bound_view = Some(view_entity),
    }

    let device = device.wgpu_device();
    let queue: &wgpu::Queue = &queue;

    // The format is a property of the view, so it is only knowable here — and a
    // view that changed it has invalidated every pipeline baked into the scene.
    let format = target.main_texture_format();

    // SpriteStudio atlases are authored in sRGB and the player writes those bytes
    // verbatim, expecting a non-sRGB target (as the native and C++ hosts give it).
    // An sRGB target encodes them a second time on write, so everything comes out
    // too bright. The camera decides this: `CompositingSpace::Srgb` makes Bevy pick
    // a non-sRGB (`Rgba8Unorm`) target and composite in sRGB space, which matches
    // the player. Warn once rather than silently miscolour.
    if format.is_srgb() && !state.warned_srgb {
        state.warned_srgb = true;
        warn!(
            "ssplayer-bevy: rendering into an sRGB target ({format:?}); SpriteStudio art \
             is authored in sRGB and will look too bright. Add `CompositingSpace::Srgb` to the \
             camera to composite in sRGB (a non-sRGB target)."
        );
    }

    if state.scene_format != Some(format) {
        // Coverage, not stencil: Bevy's 2d depth texture is `Depth32Float`, which
        // carries no stencil to lend, and this crate draws into Bevy's own pass
        // rather than opening one with attachments of its own.
        state.scene = Some(SSScene::new(device, &SceneConfig { color_format: format, stencil_format: None, mask: MaskBackend::Coverage }));
        state.scene_format = Some(format);
    }

    let SsRenderState { renderer, scene, slots, .. } = state;
    let Some(scene) = scene.as_mut() else {
        return;
    };

    let size = camera.physical_viewport_size.unwrap_or_else(|| {
        let size = target.main_texture().size();
        UVec2::new(size.width, size.height)
    });
    // Y-up, matching Bevy's 2D world axes.
    scene.set_viewport(queue, size.x as f32, size.y as f32, false);

    let mut players: Vec<&mut SSPlayer> = slots.iter_mut().map(|slot| &mut slot.player).collect();
    scene.update(renderer, device, queue, extracted.delta_secs, &mut players);

    // Read back what the advance produced, before drawing, and hand it to the
    // main world. `update` cleared the events of any culled player, so an
    // invisible character reports nothing — reading every slot is still correct.
    drop(players);
    channel.store(
        slots
            .iter()
            .map(|slot| {
                let player = &slot.player;
                PlaybackFeedback {
                    entity: slot.entity.id(),
                    frame_no: player.frame_no(),
                    is_finished: player.is_finished(),
                    started_edge: player.just_started(),
                    finished_edge: player.just_finished(),
                    looped: player.just_looped(),
                    events: player.events().to_vec(),
                }
            })
            .collect(),
    );

    // Bake the mask bitmaps into the same encoder the pass below is opened on.
    // Passes in one encoder run in order, so this frame's coverage is what this
    // frame's content reads — keeping the bake and the draw in one system is
    // what guarantees that, because two systems would each get their own encoder
    // and the order between them would be a race.
    scene.encode_mask_coverage(renderer, device, ctx.command_encoder());

    // `get_color_attachment` hands back a load op that is already `Load` here:
    // the attachment records that Bevy's own 2d passes cleared it this frame.
    let color_attachments = [Some(target.get_color_attachment())];
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("spritestudio_2d"),
        color_attachments: &color_attachments,
        // The player masks with a coverage bitmap of its own, so it asks Bevy for
        // no depth-stencil at all.
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if let Some(viewport) = camera.viewport.as_ref() {
        pass.set_camera_viewport(viewport);
    }
    scene.draw(renderer, pass.wgpu_pass());
}
