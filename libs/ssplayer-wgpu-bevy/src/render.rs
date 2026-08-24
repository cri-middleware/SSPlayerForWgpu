//! The render-world half: the playback trees, the device-scoped renderer, the
//! target-scoped scene, and the one system that draws them.
//!
//! Everything here follows the player's borrow model — the renderer and scene
//! never own the `Device` / `Queue` / target, Bevy does, and lends them per
//! frame exactly as the winit and C++ hosts do.

use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_resource::RenderPassDescriptor;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::ViewTarget;
use ssplayer_wgpu::{MaskBackend, SSPlayer, SSRenderer, SSScene, SceneConfig};

use crate::extract::ExtractedPlayers;

/// One character's playback tree, plus what it was last built from.
///
/// `src` and `animation` are kept so a change to either can be told from the
/// frame-to-frame traffic: rebuilding the tree allocates, the rest does not.
struct PlayerSlot {
    entity: MainEntity,
    player: SSPlayer,
    src: String,
    animation: String,
}

/// Everything the render world owns on this crate's behalf.
///
/// `SSRenderer` is device-scoped and `SSScene` is target-scoped, so there is one
/// renderer for the whole app and — once multi-view lands — one scene per view.
/// For now there is one scene, and a second view is refused rather than silently
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
    /// The view `scene` belongs to; anything else is refused.
    bound_view: Option<Entity>,
    warned_second_view: bool,
}

pub(crate) fn init_state(mut commands: Commands, device: Res<RenderDevice>) {
    commands.insert_resource(SsRenderState { renderer: SSRenderer::new(device.wgpu_device()), scene: None, scene_format: None, slots: Vec::new(), bound_view: None, warned_second_view: false });
}

/// Apply this frame's extracted state to the playback trees.
///
/// Runs before anything touches the GPU: loading reads files and builds the
/// tree, and the borrow model lends no device until `update`, so what a load
/// reads waits in the player until the first frame that draws it.
pub(crate) fn sync_players(mut state: Option<ResMut<SsRenderState>>, extracted: Res<ExtractedPlayers>) {
    let Some(state) = state.as_deref_mut() else {
        return;
    };

    // Drop the trees whose entities went away. The extracted list is this
    // frame's whole truth, so anything absent from it is gone.
    state.slots.retain(|slot| extracted.players.iter().any(|player| player.entity == slot.entity));

    for extracted_player in &extracted.players {
        let position = state.slots.iter().position(|slot| slot.entity == extracted_player.entity);
        let index = match position {
            Some(index) => index,
            None => {
                state.slots.push(PlayerSlot { entity: extracted_player.entity, player: SSPlayer::new(), src: String::new(), animation: String::new() });
                state.slots.len() - 1
            }
        };
        let slot = &mut state.slots[index];

        // The two allocating steps, each guarded by what it was last given.
        if slot.src != extracted_player.src {
            slot.src.clone_from(&extracted_player.src);
            slot.animation.clear();
            if let Err(err) = slot.player.load(&extracted_player.src) {
                error!("ssplayer-wgpu-bevy: could not load {:?}: {err:?}", extracted_player.src);
                continue;
            }
            for missing in slot.player.missing_assets() {
                warn!("ssplayer-wgpu-bevy: {:?} is missing {:?}", extracted_player.src, missing);
            }
        }
        if !extracted_player.animation.is_empty() && slot.animation != extracted_player.animation {
            slot.animation.clone_from(&extracted_player.animation);
            if let Err(err) = slot.player.set_animation(&extracted_player.animation) {
                // The names come from the pack, so listing them turns a typo
                // into a one-line fix rather than a hunt through the .ssab.
                error!("ssplayer-wgpu-bevy: no animation {:?} in {:?} ({err:?}); it has {:?}", extracted_player.animation, extracted_player.src, slot.player.animation_names().unwrap_or_default());
                continue;
            }
        }

        // Allocation-free from here down.
        let transform = extracted_player.transform;
        slot.player.set_transform(transform.x, transform.y, transform.scale_x, transform.scale_y, transform.rotation);

        let playback = extracted_player.playback;
        slot.player.set_speed(playback.speed);
        slot.player.set_order(playback.order);
        slot.player.set_alpha(playback.alpha);
        slot.player.set_visible(playback.visible);
        if playback.playing != slot.player.is_playing() {
            if playback.playing {
                slot.player.play();
            } else {
                slot.player.pause();
            }
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
pub(crate) fn draw_players(mut state: Option<ResMut<SsRenderState>>, extracted: Res<ExtractedPlayers>, device: Res<RenderDevice>, queue: Res<RenderQueue>, view: ViewQuery<(&ExtractedCamera, &ViewTarget)>, mut ctx: RenderContext) {
    let Some(state) = state.as_deref_mut() else {
        return;
    };
    if state.slots.is_empty() {
        return;
    }

    let view_entity = view.entity();
    let (camera, target) = view.into_inner();

    match state.bound_view {
        Some(bound) if bound != view_entity => {
            if !state.warned_second_view {
                state.warned_second_view = true;
                warn!(
                    "ssplayer-wgpu-bevy: more than one view is rendering; only the first draws. \
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
