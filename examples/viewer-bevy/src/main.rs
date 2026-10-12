//! Bevy development host for `ssplayer-wgpu` — the Bevy sibling of the native
//! winit viewer (`examples/viewer`) and the C++ hosts (`examples/viewer-*-cpp`).
//!
//! **It shows a cast, not a character.** Bevy owns the device; the adapter draws
//! every `SsPlayer` entity into Bevy's own 2d pass, so N characters share one
//! target and one pass. The multi-player faults the eye catches live here: one
//! player's mask clipping the next (the `Stack` layout piles them on top of each
//! other on purpose), and who ends up in front when `order` is reversed.
//!
//! Run from the repository root, which is where `assets/` is:
//!
//! ```text
//! cargo run -p ssplayer-bevy-viewer
//! ```
//!
//! `scripts/deploy-examples.sh` (or `.ps1`) generates `assets/` first.
//!
//! Space   pause / resume          +/-     add / remove a character (1–8)
//! Left/Right   prev / next pack    Tab     cycle focus (all / #1 / #2 …)
//! Up/Down prev / next animation    V       toggle visibility
//! R       restart animation        [ / ]   alpha down / up
//! L       layout: Row / Stack      O       reverse draw order
//! F       flip Y (vertical mirror) Esc     quit
//! drop    play a dropped .ssab, or every .ssab in a dropped folder
//!
//! Tracks — the same keys as the native viewer, sent as `SsTrackCommand` and read
//! back from `SsTrackState`, so what the HUD shows is a frame behind:
//!
//! C       crossfade track 0 to the next animation (400ms), by changing
//!         `SsPlayer::animation` with `transition_ms` set — the one way track 0
//!         changes hands; Up/Down switch it with `transition_ms` at 0
//! S       cycle which animation sits on track 1, or take it off
//! 1 / 2   track 1 alpha down / up      A   track 1 mix / additive
//! 0       fade track 0 to the rest pose and back
//! , / .   scrub track 1's frame
//! Y       track sync free / start, for the whole cast
//! 3       track 0 speed 1x / 2x / 0.5x      T   track 0 loops once / forever
//! 4       track 1 loops once / forever      E   outgoing side of a track 0
//!         crossfade fires events: never / through the first half
//!
//! The pack / animation / V / `[` `]` and track keys act on the **focused** member;
//! focus starts at "all", so they drive the whole cast until you press Tab.
//!
//! One native-viewer key is deliberately absent: **K (mask backend)**. Bevy's 2d
//! depth is `Depth32Float`, which carries no stencil, so the adapter always uses
//! the coverage backend; there is nothing to toggle to.

use bevy::asset::{AssetPlugin, UnapprovedPathMode};
use bevy::audio::{AudioPlayer, PlaybackSettings};
use bevy::camera::CompositingSpace;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::window::FileDragAndDrop;
use ssplayer_bevy::{SsEvent, SsEventMessage, SsFinished, SsLooped, SsPlayback, SsPlaybackState, SsPlayer, SsPlayerPlugin, SsStarted, SsTrackCommand, SsTrackState, SsTransform, TrackBlend, TrackCommand, TrackSync};
use ssplayer_wgpu::SSPack;
use std::path::{Path, PathBuf};

/// The repo's `assets/`, absolute so the viewer is launchable from anywhere. It is
/// Bevy's asset root, so the packs and the audio files they cue are asset paths
/// into it. This example carries no `assets/` of its own; it points at the shared,
/// generated tree, exactly as `examples/viewer` does.
const ASSET_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

/// Cast size at startup. Two rather than one, so a regression in player isolation
/// is on screen from the first frame rather than behind a keypress.
const INITIAL_CAST: usize = 2;
/// Most characters `+` will add. Interference is what this is for, and past a
/// handful there is nothing new to see.
const MAX_CAST: usize = 8;
/// Nominal canvas size used to fit characters, since the real canvas size lives on
/// the render-world player and is not readable from here. The demo packs are close
/// enough to this that a single figure lays them out sensibly.
const BASE_CANVAS: f32 = 320.0;

/// How the cast is arranged on screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Side by side, each scaled to its slot. For reading one character.
    Row,
    /// Piled at the centre with a small stagger, so they overlap almost
    /// completely — interference between players only shows where they overlap.
    Stack,
}

impl Layout {
    fn name(self) -> &'static str {
        match self {
            Layout::Row => "Row",
            Layout::Stack => "Stack (overlapping)",
        }
    }
}

/// One pack in the picker: its name, where it is, and what it can play.
///
/// Parsed once from the `.ssab` in the main world — the same thing the native
/// viewer does — so the input systems can name an animation without reaching into
/// the render world, which is the only place the live playback tree exists.
#[derive(Clone)]
struct PackInfo {
    name: String,
    /// What `AssetServer::load` takes: relative to [`ASSET_ROOT`] for the shared
    /// set, absolute for a dropped file. The pack is loaded when a character
    /// switches to it, not when it enters the picker.
    path: String,
    anims: Vec<String>,
}

/// The parsed catalogue and which entry a fresh character starts on.
#[derive(Resource)]
struct Catalogue {
    packs: Vec<PackInfo>,
    initial: usize,
}

/// Which pack and animation one character is on — its index into [`Catalogue`].
/// Kept on the entity rather than in a side table so add / remove need no
/// bookkeeping beyond the spawn.
#[derive(Component, Clone, Copy)]
struct Slot {
    pack_idx: usize,
    anim_idx: usize,
    /// What the track keys last laid on track 1, as an index into the pack's
    /// animations; `None` with nothing there. Kept here because the stack read
    /// back from the render world is a frame late, and a key pressed twice in a
    /// row must not act on the state from before the first press.
    overlay_idx: Option<usize>,
    /// Track 1's frame while it is being scrubbed; `None` while it plays.
    overlay_frame: Option<f32>,
    /// Track 1's alpha and blend as the keys last set them.
    overlay_alpha: f32,
    overlay_additive: bool,
    /// Whether `0` has faded track 0 out to the rest pose.
    at_rest: bool,
    /// Whether `4` asked track 1 to play once. The count belongs to the
    /// animation on the track, so it is sent again with every overlay laid.
    overlay_once: bool,
    /// Whether `E` lets the outgoing side of a track 0 crossfade fire through
    /// the first half of the fade. Track 0 keeps it across a new animation.
    outgoing_half: bool,
}

impl Slot {
    fn new(pack_idx: usize, anim_idx: usize) -> Self {
        Self { pack_idx, anim_idx, overlay_idx: None, overlay_frame: None, overlay_alpha: 0.5, overlay_additive: false, at_rest: false, overlay_once: false, outgoing_half: false }
    }

    /// A new animation or pack collapses the stack to track 0 alone. Track 0's
    /// outgoing event threshold stays: the runtime keeps it across a setup.
    fn clear_tracks(&mut self) {
        *self = Self { outgoing_half: self.outgoing_half, ..Self::new(self.pack_idx, self.anim_idx) };
    }
}

/// Viewer-wide state the render output does not carry: who is focused, how the
/// cast is laid out, and the cast in add-order (draw order is `SsPlayback::order`,
/// set in [`apply_layout`]).
#[derive(Resource)]
struct ViewerState {
    /// Which member the pack / animation / appearance keys drive. `None` is "all".
    focus: Option<usize>,
    layout: Layout,
    reverse_order: bool,
    y_flip: bool,
    /// The characters, in the order they were added.
    cast: Vec<Entity>,
    /// The last timeline event, for the HUD.
    last_event: String,
    /// What `Y` set for tracks 0 and 1 of the whole cast. Sent again whenever a
    /// member's stack reads back otherwise — a new animation or pack resets it.
    track_sync: TrackSync,
}

/// Marks the HUD text so a system can find it.
#[derive(Component)]
struct Hud;

fn main() {
    App::new()
        // A dropped `.ssab` can be anywhere on disk, outside the asset root; a
        // development viewer allows that, which a shipped app should not.
        .add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(Window { title: "SpriteStudio in Bevy — viewer".into(), ..default() }), ..default() }).set(AssetPlugin { file_path: ASSET_ROOT.to_string(), unapproved_path_mode: UnapprovedPathMode::Allow, ..default() }))
        .add_plugins(SsPlayerPlugin)
        .add_systems(Startup, setup)
        // Chained: input mutates the cast, drop grows the catalogue, layout reads
        // both and places every member, then the HUD reads the settled state.
        .add_systems(Update, (handle_input, handle_track_input, keep_track_sync, handle_drop, playback_events, apply_layout, refresh_hud).chain())
        .run();
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // Msaa::Off is required: the player's pipelines are single-sampled.
    // CompositingSpace::Srgb is required for correct colour: SpriteStudio art is
    // authored in sRGB and the player writes those bytes verbatim, so a default
    // (sRGB-target) camera would encode them a second time and come out too bright.
    commands.spawn((Camera2d, Msaa::Off, CompositingSpace::Srgb));

    // An opaque backdrop, large enough to fill any window: additive blend modes
    // read the target, so a transparent clear would let them blow out.
    commands.spawn(Sprite { color: Color::srgb(0.08, 0.08, 0.12), custom_size: Some(Vec2::splat(100_000.0)), ..default() });

    commands.spawn((Text::new(""), TextFont { font_size: bevy::text::FontSize::Px(14.0), ..default() }, TextColor(Color::WHITE), Node { position_type: PositionType::Absolute, top: Val::Px(8.0), left: Val::Px(10.0), ..default() }, Hud));

    let catalogue = load_catalogue();
    if catalogue.packs.is_empty() {
        error!("no .ssab found under {ASSET_ROOT}/overall — run scripts/deploy-examples.sh first");
    }

    // Spawn the initial cast on the preferred pack. apply_layout places them next
    // frame; the one-frame default transform is imperceptible.
    let mut cast = Vec::new();
    let start = catalogue.initial;
    if let Some(info) = catalogue.packs.get(start) {
        for i in 0..INITIAL_CAST {
            let entity = commands.spawn((SsPlayer { pack: asset_server.load(&info.path), animation: info.anims.first().cloned().unwrap_or_default(), ..default() }, Slot::new(start, 0), SsPlayback { order: i as i32, ..default() })).id();
            cast.push(entity);
        }
    }

    commands.insert_resource(catalogue);
    commands.insert_resource(ViewerState { focus: None, layout: Layout::Row, reverse_order: false, y_flip: false, cast, last_event: "-".into(), track_sync: TrackSync::Free });
}

/// The members the pack / animation / appearance keys act on: the focused one, or
/// all of them when focus is "all".
fn targets(state: &ViewerState) -> Vec<Entity> {
    match state.focus {
        Some(i) if i < state.cast.len() => vec![state.cast[i]],
        _ => state.cast.clone(),
    }
}

/// Every key. Each branch is a plain edit of the adapter's components, which is
/// the whole point of the demo: the characters are driven entirely through
/// `SsPlayer` / `SsPlayback` / `Slot`.
fn handle_input(
    keys: Res<ButtonInput<KeyCode>>,
    asset_server: Res<AssetServer>,
    mut state: ResMut<ViewerState>,
    catalogue: Res<Catalogue>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut players: Query<(&mut SsPlayer, &mut SsPlayback, &mut Slot)>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
        return;
    }
    if catalogue.packs.is_empty() {
        return;
    }
    let targets = targets(&state);

    if keys.just_pressed(KeyCode::Space) {
        for &e in &targets {
            if let Ok((_, mut pb, _)) = players.get_mut(e) {
                pb.playing = !pb.playing;
            }
        }
    }
    if keys.just_pressed(KeyCode::KeyR) {
        for &e in &targets {
            if let Ok((_, mut pb, _)) = players.get_mut(e) {
                // A generation counter: bumping it replays from the head, which
                // pausing and resuming cannot do.
                pb.restart = pb.restart.wrapping_add(1);
            }
        }
    }
    if keys.just_pressed(KeyCode::KeyV) {
        for &e in &targets {
            if let Ok((_, mut pb, _)) = players.get_mut(e) {
                pb.visible = !pb.visible;
            }
        }
    }

    // Horizontal: switch pack. Changing `pack` rebuilds the tree from the new pack
    // once it has loaded, which is the allocating edit; the animation is clamped
    // into the new range.
    let pack_step = keys.just_pressed(KeyCode::ArrowRight) as i32 - keys.just_pressed(KeyCode::ArrowLeft) as i32;
    if pack_step != 0 {
        let n = catalogue.packs.len() as i32;
        for &e in &targets {
            if let Ok((mut player, _, mut slot)) = players.get_mut(e) {
                slot.pack_idx = (((slot.pack_idx as i32 + pack_step) % n + n) % n) as usize;
                let info = &catalogue.packs[slot.pack_idx];
                slot.anim_idx = slot.anim_idx.min(info.anims.len().saturating_sub(1));
                player.pack = asset_server.load(&info.path);
                player.animation = info.anims.get(slot.anim_idx).cloned().unwrap_or_default();
                slot.clear_tracks();
            }
        }
    }

    // Vertical: switch animation within the current pack. Down is next.
    let anim_step = keys.just_pressed(KeyCode::ArrowDown) as i32 - keys.just_pressed(KeyCode::ArrowUp) as i32;
    if anim_step != 0 {
        for &e in &targets {
            if let Ok((mut player, _, mut slot)) = players.get_mut(e) {
                let info = &catalogue.packs[slot.pack_idx];
                if !info.anims.is_empty() {
                    let m = info.anims.len() as i32;
                    slot.anim_idx = ((slot.anim_idx as i32 + anim_step) % m + m) as usize % info.anims.len();
                    // A switch, rebuilding the tree; C is the crossfade.
                    player.transition_ms = 0.0;
                    player.animation = info.anims[slot.anim_idx].clone();
                    slot.clear_tracks();
                }
            }
        }
    }

    // `[` / `]`: player-level alpha, folded into every vertex.
    let alpha_step = keys.just_pressed(KeyCode::BracketRight) as i32 - keys.just_pressed(KeyCode::BracketLeft) as i32;
    if alpha_step != 0 {
        for &e in &targets {
            if let Ok((_, mut pb, _)) = players.get_mut(e) {
                pb.alpha = (pb.alpha + alpha_step as f32 * 0.1).clamp(0.0, 1.0);
            }
        }
    }

    // Cast size. `=` doubles for `+` so it works without Shift.
    if (keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd)) && state.cast.len() < MAX_CAST {
        // Start the newcomer on whatever the last one plays.
        let (pack_idx, anim_idx) = state.cast.last().and_then(|&e| players.get(e).ok().map(|(_, _, slot)| (slot.pack_idx, slot.anim_idx))).unwrap_or((catalogue.initial, 0));
        if let Some(info) = catalogue.packs.get(pack_idx) {
            let entity = commands
                .spawn((SsPlayer { pack: asset_server.load(&info.path), animation: info.anims.get(anim_idx).cloned().unwrap_or_default(), ..default() }, Slot::new(pack_idx, anim_idx), SsPlayback { order: state.cast.len() as i32, ..default() }))
                .id();
            state.cast.push(entity);
        }
    }
    // One always stays, or there is nothing to look at.
    if (keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract)) && state.cast.len() > 1 {
        if let Some(e) = state.cast.pop() {
            commands.entity(e).despawn();
        }
        if state.focus.is_some_and(|i| i >= state.cast.len()) {
            state.focus = None;
        }
    }

    // Focus: all → #1 → … → #N → all.
    if keys.just_pressed(KeyCode::Tab) {
        let len = state.cast.len();
        state.focus = match state.focus {
            None if len > 0 => Some(0),
            Some(i) if i + 1 < len => Some(i + 1),
            _ => None,
        };
    }

    // Layout / order / flip live in state; apply_layout reads them every frame.
    if keys.just_pressed(KeyCode::KeyL) {
        state.layout = if state.layout == Layout::Row { Layout::Stack } else { Layout::Row };
    }
    if keys.just_pressed(KeyCode::KeyO) {
        state.reverse_order = !state.reverse_order;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        // Not the native viewer's projection-handedness flip — the adapter owns the
        // projection (it follows the camera), so this mirrors the character
        // vertically via a negative scale instead. Enough to sanity-check that
        // negative scale draws.
        state.y_flip = !state.y_flip;
    }
}

/// The track keys, the native viewer's. Each sends an `SsTrackCommand`; what the
/// stack then holds comes back in `SsTrackState` a frame later, so the keys act on
/// what `Slot` remembers they last did rather than on that.
fn handle_track_input(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<ViewerState>, catalogue: Res<Catalogue>, mut slots: Query<(&mut Slot, &mut SsPlayer, &mut SsPlayback)>, mut tracks: MessageWriter<SsTrackCommand>) {
    if catalogue.packs.is_empty() {
        return;
    }
    if keys.just_pressed(KeyCode::KeyY) {
        // `keep_track_sync` sends it, to the whole cast and again after a reset.
        state.track_sync = match state.track_sync {
            TrackSync::Free => TrackSync::Start,
            _ => TrackSync::Free,
        };
    }

    for entity in targets(&state) {
        let Ok((mut slot, mut player, mut playback)) = slots.get_mut(entity) else { continue };
        let anims = &catalogue.packs[slot.pack_idx].anims;
        if anims.is_empty() {
            continue;
        }
        let mut send = |command| {
            tracks.write(SsTrackCommand { entity, command });
        };

        // C: crossfade track 0 to the next animation, without rebuilding the
        // tree. Track 0 changes hands through `animation` alone; `transition_ms`
        // makes the change a crossfade. `Slot` follows so the HUD names it and
        // Up/Down carry on from it.
        if keys.just_pressed(KeyCode::KeyC) {
            slot.anim_idx = (slot.anim_idx + 1) % anims.len();
            player.transition_ms = 400.0;
            player.animation = anims[slot.anim_idx].clone();
        }
        // 3: track 0's speed, which `SsPlayback` owns.
        if keys.just_pressed(KeyCode::Digit3) {
            playback.speed_scale = if playback.speed_scale == 1.0 {
                2.0
            } else if playback.speed_scale == 2.0 {
                0.5
            } else {
                1.0
            };
        }
        // T: track 0's loop count, which `SsPlayback` owns too.
        if keys.just_pressed(KeyCode::KeyT) {
            playback.loop_count = if playback.loop_count == 1 { -1 } else { 1 };
        }
        // E: whether the outgoing side of a track 0 crossfade fires events.
        if keys.just_pressed(KeyCode::KeyE) {
            slot.outgoing_half = !slot.outgoing_half;
            let threshold = if slot.outgoing_half { 0.5 } else { f32::INFINITY };
            send(TrackCommand::SetOutgoingEventThreshold { track: 0, threshold });
        }
        // S: walk track 1 through the pack's animations, then back to empty.
        if keys.just_pressed(KeyCode::KeyS) {
            let next = match slot.overlay_idx {
                Some(i) if (i + 1) % anims.len() == slot.anim_idx => None,
                Some(i) => Some((i + 1) % anims.len()),
                None => Some((slot.anim_idx + 1) % anims.len()),
            };
            match next {
                Some(i) => {
                    send(TrackCommand::SetAnimation { track: 1, animation: anims[i].clone() });
                    // Half strength: the point is to see partial blending.
                    send(TrackCommand::SetAlpha { track: 1, alpha: slot.overlay_alpha });
                    let blend = if slot.overlay_additive { TrackBlend::Additive } else { TrackBlend::Mix };
                    send(TrackCommand::SetBlend { track: 1, blend });
                    if slot.overlay_once {
                        send(TrackCommand::SetLoopCount { track: 1, count: 1 });
                    }
                    slot.overlay_frame = None;
                }
                None => send(TrackCommand::Clear { track: 1 }),
            }
            slot.overlay_idx = next;
        }
        if slot.overlay_idx.is_none() {
            continue;
        }
        // 1 / 2: track 1's alpha. Not clamped at the top: above 1 extrapolates.
        let alpha_step = keys.just_pressed(KeyCode::Digit2) as i32 - keys.just_pressed(KeyCode::Digit1) as i32;
        if alpha_step != 0 {
            slot.overlay_alpha = (slot.overlay_alpha + alpha_step as f32 * 0.1).max(0.0);
            send(TrackCommand::SetAlpha { track: 1, alpha: slot.overlay_alpha });
        }
        // 4: track 1 plays once and holds its last frame, or loops forever.
        if keys.just_pressed(KeyCode::Digit4) {
            slot.overlay_once = !slot.overlay_once;
            send(TrackCommand::SetLoopCount { track: 1, count: if slot.overlay_once { 1 } else { -1 } });
        }
        // A: track 1 mix / additive.
        if keys.just_pressed(KeyCode::KeyA) {
            slot.overlay_additive = !slot.overlay_additive;
            let blend = if slot.overlay_additive { TrackBlend::Additive } else { TrackBlend::Mix };
            send(TrackCommand::SetBlend { track: 1, blend });
        }
        // , / .: scrub track 1 instead of playing it — how a continuous axis is driven.
        let scrub = keys.just_pressed(KeyCode::Period) as i32 - keys.just_pressed(KeyCode::Comma) as i32;
        if scrub != 0 {
            let frame = (slot.overlay_frame.unwrap_or(0.0) + scrub as f32).max(0.0);
            slot.overlay_frame = Some(frame);
            send(TrackCommand::SetSpeed { track: 1, speed: 0.0 });
            send(TrackCommand::SetFrame { track: 1, frame });
        }
    }

    // 0: fade track 0 to the rest pose, and back. Outside the overlay guard: it
    // needs no overlay.
    if keys.just_pressed(KeyCode::Digit0) {
        for entity in targets(&state) {
            let Ok((mut slot, _, _)) = slots.get_mut(entity) else { continue };
            slot.at_rest = !slot.at_rest;
            let target = if slot.at_rest { 0.0 } else { 1.0 };
            tracks.write(SsTrackCommand { entity, command: TrackCommand::FadeTrack { track: 0, target, duration_ms: 500.0 } });
        }
    }
}

/// Keep tracks 0 and 1 of every member on the sync `Y` chose. Compared against
/// what the stack reads back rather than sent once, because a new animation or
/// pack resets the stack — and a pack switch does so only once the new pack has
/// loaded, frames after the key that asked for it.
fn keep_track_sync(state: Res<ViewerState>, stacks: Query<&SsTrackState>, mut tracks: MessageWriter<SsTrackCommand>) {
    for &entity in &state.cast {
        let Ok(stack) = stacks.get(entity) else { continue };
        let Some(track0) = stack.tracks.first() else { continue };
        let track1 = stack.tracks.get(1).map(|t| t.sync);
        if track0.sync != state.track_sync || track1 != Some(state.track_sync) {
            for track in [0, 1] {
                tracks.write(SsTrackCommand { entity, command: TrackCommand::SetSync { track, sync: state.track_sync } });
            }
        }
    }
}

/// Play a `.ssab` dropped on the window, or every `.ssab` directly inside a
/// dropped folder. Drop the folder rather than the file when a pack has external
/// instance references: those resolve by pack name against the pack's own
/// directory, and the player's loader finds the siblings there.
fn handle_drop(mut drops: MessageReader<FileDragAndDrop>, asset_server: Res<AssetServer>, mut state: ResMut<ViewerState>, mut catalogue: ResMut<Catalogue>, mut players: Query<(&mut SsPlayer, &mut SsPlayback, &mut Slot)>) {
    for event in drops.read() {
        let FileDragAndDrop::DroppedFile { path_buf, .. } = event else { continue };

        let mut paths = Vec::new();
        if path_buf.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path_buf) {
                paths.extend(entries.flatten().map(|e| e.path()).filter(|p| is_ssab(p)));
            }
            paths.sort();
        } else if is_ssab(path_buf) {
            paths.push(path_buf.clone());
        }

        let mut first = None;
        for path in paths {
            let Some(info) = parse_pack(&path, path.to_string_lossy().into_owned()) else { continue };
            info!("dropped pack {:?} / {} animation(s)", info.name, info.anims.len());
            // A pack dropped twice replaces its own entry rather than growing the
            // list, and is read again: the asset server would otherwise hand back
            // what it loaded the first time. That is what makes "convert, drop,
            // look, convert again" work.
            let idx = catalogue.packs.iter().position(|p| p.name == info.name).unwrap_or(catalogue.packs.len());
            if idx == catalogue.packs.len() {
                catalogue.packs.push(info);
            } else {
                asset_server.reload(&info.path);
                catalogue.packs[idx] = info;
            }
            first.get_or_insert(idx);
        }

        // Show the first of them, on whoever the pack keys drive.
        if let Some(idx) = first {
            for &e in &targets(&state) {
                if let Ok((mut player, _, mut slot)) = players.get_mut(e) {
                    slot.pack_idx = idx;
                    slot.anim_idx = 0;
                    slot.clear_tracks();
                    let info = &catalogue.packs[idx];
                    player.pack = asset_server.load(&info.path);
                    player.animation = info.anims.first().cloned().unwrap_or_default();
                }
            }
            state.last_event = "-".into();
        }
    }
}

/// The return trip: timeline events come back as messages. This keeps the last one
/// for the HUD and — for an audio cue — actually plays the sound, because the
/// player only says *what* to play.
fn playback_events(
    mut started: MessageReader<SsStarted>,
    mut events: MessageReader<SsEventMessage>,
    mut finished: MessageReader<SsFinished>,
    mut looped: MessageReader<SsLooped>,
    mut state: ResMut<ViewerState>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for _ in started.read() {
        state.last_event = "started".into();
    }
    for message in events.read() {
        match &message.event {
            SsEvent::Audio(audio) => {
                state.last_event = format!("audio {:?} x{}", audio.sound_name, audio.loop_num);
                if let Some(path) = resolve_sound(&audio.sound_name) {
                    // DESPAWN plays once and cleans up. SpriteStudio's loop_num is a
                    // repeat count Bevy has no single-shot equivalent for, so this
                    // honours the trigger rather than the count.
                    commands.spawn((AudioPlayer::new(asset_server.load(path)), PlaybackSettings::DESPAWN));
                }
            }
            SsEvent::UserData(data) => state.last_event = format!("user data on part {}", data.part_index),
            SsEvent::Signal(signal) => state.last_event = format!("signal {:?}", signal.command_id),
        }
        // Which source fired it: on a track 0 crossfade, the main source is the
        // incoming side and any other the outgoing one.
        state.last_event.push_str(&format!(" from source {}", message.event.source()));
    }
    for _ in finished.read() {
        state.last_event = "finished".into();
    }
    for _ in looped.read() {
        state.last_event = "looped".into();
    }
}

/// Place the cast and stack it front to back, every frame. Cheap for a handful of
/// entities, and re-reading the window size here is what makes the layout follow a
/// resize with no resize handler of its own.
///
/// Only `SsTransform` and `SsPlayback::order` are touched — never `src` /
/// `animation`, which would rebuild the tree, nor the other `SsPlayback` fields
/// the input keys own.
fn apply_layout(state: Res<ViewerState>, windows: Query<&Window>, mut players: Query<(&mut SsTransform, &mut SsPlayback)>) {
    let Ok(window) = windows.single() else { return };
    let (width, height) = (window.width(), window.height());
    let count = state.cast.len();
    if count == 0 {
        return;
    }
    let last = (count - 1) as f32;

    for (i, &entity) in state.cast.iter().enumerate() {
        let Ok((mut transform, mut playback)) = players.get_mut(entity) else { continue };
        let offset = i as f32 - last * 0.5;
        let (x, y, scale) = match state.layout {
            Layout::Row => {
                let slot = width / count as f32;
                (offset * slot, 0.0, (slot * 0.9 / BASE_CANVAS).min(height * 0.9 / BASE_CANVAS).clamp(0.05, 1.0))
            }
            Layout::Stack => {
                let step = (width * 0.04).min(48.0);
                (offset * step, offset * step, (width * 0.5 / BASE_CANVAS).min(height * 0.8 / BASE_CANVAS).clamp(0.05, 1.5))
            }
        };
        transform.x = x;
        transform.y = y;
        transform.scale_x = scale;
        transform.scale_y = if state.y_flip { -scale } else { scale };
        playback.order = if state.reverse_order { (count - 1 - i) as i32 } else { i as i32 };
    }
}

/// Draw the HUD from what the main world knows: the cast, the layout, and each
/// member's component state plus its read-back frame. GPU / primitive counters are
/// not shown — the adapter does not surface `SSScene::stats()` to the main world.
fn refresh_hud(state: Res<ViewerState>, catalogue: Res<Catalogue>, members: Query<(&Slot, &SsPlayback, &SsPlaybackState, &SsTrackState)>, mut hud: Query<&mut Text, With<Hud>>) {
    let Ok(mut text) = hud.single_mut() else { return };

    let focus = match state.focus {
        Some(i) => format!("#{}", i + 1),
        None => "all".to_string(),
    };
    let order = if state.reverse_order { ", order reversed" } else { "" };
    let flip = if state.y_flip { ", Y-flipped" } else { "" };
    let sync = if state.track_sync == TrackSync::Start { "start (at track 0's phase)" } else { "free (from the head)" };
    let mut out = format!("Cast: {} in 1 pass   focus {focus}   layout {}{order}{flip}   track sync {sync}\n", state.cast.len(), state.layout.name());

    for (i, &entity) in state.cast.iter().enumerate() {
        let Ok((slot, playback, playstate, stack)) = members.get(entity) else { continue };
        let marker = if state.focus == Some(i) { '>' } else { ' ' };
        let pack = catalogue.packs.get(slot.pack_idx).map(|p| p.name.as_str()).unwrap_or("none");
        let anim = catalogue.packs.get(slot.pack_idx).and_then(|p| p.anims.get(slot.anim_idx)).map(String::as_str).unwrap_or("none");
        let hidden = if playback.visible { "" } else { " [hidden]" };
        let paused = if playback.playing { "" } else { " [paused]" };
        out.push_str(&format!("{marker} #{} {pack} / {anim}   order {} alpha {:.2} frame {:.0}{hidden}{paused}\n", i + 1, playback.order, playback.alpha, playstate.frame_no));

        // The stack as the render world last left it, a frame behind.
        if let Some(track0) = stack.tracks.first() {
            let fading = track0.fade_progress.map(|p| format!(", fading {:.0}%", p * 100.0)).unwrap_or_default();
            let main = track0.main_source.map(|h| h.to_string()).unwrap_or_else(|| "-".into());
            let loops = match track0.loop_count {
                Some(n) if n > 0 => format!("{n}x"),
                _ => "forever".into(),
            };
            let finished = if playstate.finished { ", finished" } else { "" };
            let outgoing = if track0.outgoing_event_threshold.is_finite() { format!("{:.1}", track0.outgoing_event_threshold) } else { "never".into() };
            out.push_str(&format!("      track 0 source {main} alpha {:.2}{fading}, speed {:.1}x, loops {loops}{finished}, outgoing fires {outgoing}\n", track0.alpha, track0.speed));
        }
        if let (Some(oi), Some(track1)) = (slot.overlay_idx, stack.tracks.get(1)) {
            let name = catalogue.packs.get(slot.pack_idx).and_then(|p| p.anims.get(oi)).map(String::as_str).unwrap_or("?");
            let blend = if track1.blend == TrackBlend::Additive { "additive" } else { "mix" };
            let at = match slot.overlay_frame {
                Some(f) => format!(", scrubbed to frame {f:.0}"),
                None => track1.frame.map(|f| format!(", frame {f:.0}")).unwrap_or_default(),
            };
            let loops = match track1.loop_count {
                Some(n) if n > 0 => format!(", loops {n}x"),
                _ => String::new(),
            };
            let finished = if track1.finished { ", finished" } else { "" };
            out.push_str(&format!("      track 1 {name} alpha {:.2} {blend}{at}{loops}{finished}\n", track1.alpha));
        }
    }

    out.push_str(&format!("last event: {}\n", state.last_event));
    out.push_str("Space pause  Left/Right pack  Up/Down anim  R restart  +/- cast  Tab focus\nL layout  O order  V hide  [ ] alpha  F flip-Y  Esc quit  (drop a .ssab to play it)\nC crossfade  S track 1  1/2 track 1 alpha  A mix/additive  0 rest pose  , . scrub  Y sync\n3 speed  T track 0 loops  4 track 1 loops  E outgoing events");
    **text = out;
}

// --- catalogue ------------------------------------------------------------

fn is_ssab(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("ssab")
}

/// Parse the `.ssab` at `path` into a [`PackInfo`] that loads it from `asset_path`.
/// The pack is never renamed — the name inside it is what instance references
/// resolve against.
fn parse_pack(path: &Path, asset_path: String) -> Option<PackInfo> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("failed to read {:?}: {e:?}", path);
            return None;
        }
    };
    match SSPack::load(&bytes) {
        Ok(pack) => Some(PackInfo { name: pack.name().to_owned(), path: asset_path, anims: pack.animation_names() }),
        Err(e) => {
            warn!("failed to parse {:?}: {e:?}", path);
            None
        }
    }
}

/// Parse every `.ssab` in `assets/overall`, preferring `Basic` as the first pack.
fn load_catalogue() -> Catalogue {
    let dir = PathBuf::from(format!("{ASSET_ROOT}/overall"));
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        paths.extend(entries.flatten().map(|e| e.path()).filter(|p| is_ssab(p)));
    }
    paths.sort();

    let packs: Vec<PackInfo> = paths.iter().filter_map(|p| parse_pack(p, format!("overall/{}", p.file_name()?.to_string_lossy()))).collect();
    let initial = packs.iter().position(|p| p.name == "Basic").unwrap_or(0);
    Catalogue { packs, initial }
}

/// Map a cue's `sound_name` (a bare stem) to an asset path, picking the extension
/// that exists on disk. Returned relative to the `assets/` root the Bevy
/// `AssetServer` loads from.
fn resolve_sound(name: &str) -> Option<String> {
    ["mp3", "wav", "ogg", "flac"].into_iter().find(|ext| Path::new(&format!("{ASSET_ROOT}/overall/sound/{name}.{ext}")).exists()).map(|ext| format!("overall/sound/{name}.{ext}"))
}
