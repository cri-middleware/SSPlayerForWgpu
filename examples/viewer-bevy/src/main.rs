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
//! cargo run -p ssplayer-wgpu-viewer-bevy
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
//! The pack / animation / V / `[` `]` keys act on the **focused** member; focus
//! starts at "all", so they drive the whole cast until you press Tab.
//!
//! Two native-viewer keys are deliberately absent, because the Bevy adapter's
//! surface does not reach what they drive:
//! - **K (mask backend)** — Bevy's 2d depth is `Depth32Float`, which carries no
//!   stencil, so the adapter always uses the coverage backend; there is nothing to
//!   toggle to.
//! - **the track keys (C / S / A / 1 / 2 / 0 / , / .)** — track mixing is a method
//!   on the render-world player, and the adapter exposes only `SsPlayer` /
//!   `SsTransform` / `SsPlayback`, none of which carries a track stack yet.

use bevy::asset::AssetPlugin;
use bevy::audio::{AudioPlayer, PlaybackSettings};
use bevy::camera::CompositingSpace;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::window::FileDragAndDrop;
use ssplayer_bevy::{SsEvent, SsEventMessage, SsFinished, SsLooped, SsPlayback, SsPlaybackState, SsPlayer, SsPlayerPlugin, SsTransform};
use ssplayer_wgpu::SSPack;
use std::path::{Path, PathBuf};

/// The repo's `assets/`, absolute so the viewer is launchable from anywhere and so
/// Bevy's asset root (for the audio files) points at the same tree the player
/// loads packs from. This example carries no `assets/` of its own; it points at
/// the shared, generated tree, exactly as `examples/viewer` does.
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

/// One pack in the picker: its name, where it is on disk, and what it can play.
///
/// Parsed once from the `.ssab` in the main world — the same thing the native
/// viewer does — so the input systems can name an animation without reaching into
/// the render world, which is the only place the live playback tree exists.
#[derive(Clone)]
struct PackInfo {
    name: String,
    /// Absolute path, as [`ssplayer_wgpu::SSPlayer::load`] takes it; the loader
    /// roots on this string and finds the atlases and referenced packs beside it.
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
}

/// Marks the HUD text so a system can find it.
#[derive(Component)]
struct Hud;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(Window { title: "SpriteStudio in Bevy — viewer".into(), ..default() }), ..default() }).set(AssetPlugin { file_path: ASSET_ROOT.to_string(), ..default() }))
        .add_plugins(SsPlayerPlugin)
        .add_systems(Startup, setup)
        // Chained: input mutates the cast, drop grows the catalogue, layout reads
        // both and places every member, then the HUD reads the settled state.
        .add_systems(Update, (handle_input, handle_drop, playback_events, apply_layout, refresh_hud).chain())
        .run();
}

fn setup(mut commands: Commands) {
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
            let entity = commands.spawn((SsPlayer { src: info.path.clone(), animation: info.anims.first().cloned().unwrap_or_default() }, Slot { pack_idx: start, anim_idx: 0 }, SsPlayback { order: i as i32, ..default() })).id();
            cast.push(entity);
        }
    }

    commands.insert_resource(catalogue);
    commands.insert_resource(ViewerState { focus: None, layout: Layout::Row, reverse_order: false, y_flip: false, cast, last_event: "—".into() });
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
fn handle_input(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<ViewerState>, catalogue: Res<Catalogue>, mut commands: Commands, mut exit: MessageWriter<AppExit>, mut players: Query<(&mut SsPlayer, &mut SsPlayback, &mut Slot)>) {
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

    // Horizontal: switch pack. Changing `src` rebuilds the tree from the new pack,
    // which is the allocating edit; the animation is clamped into the new range.
    let pack_step = keys.just_pressed(KeyCode::ArrowRight) as i32 - keys.just_pressed(KeyCode::ArrowLeft) as i32;
    if pack_step != 0 {
        let n = catalogue.packs.len() as i32;
        for &e in &targets {
            if let Ok((mut player, _, mut slot)) = players.get_mut(e) {
                slot.pack_idx = (((slot.pack_idx as i32 + pack_step) % n + n) % n) as usize;
                let info = &catalogue.packs[slot.pack_idx];
                slot.anim_idx = slot.anim_idx.min(info.anims.len().saturating_sub(1));
                player.src = info.path.clone();
                player.animation = info.anims.get(slot.anim_idx).cloned().unwrap_or_default();
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
                    player.animation = info.anims[slot.anim_idx].clone();
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
            let entity = commands.spawn((SsPlayer { src: info.path.clone(), animation: info.anims.get(anim_idx).cloned().unwrap_or_default() }, Slot { pack_idx, anim_idx }, SsPlayback { order: state.cast.len() as i32, ..default() })).id();
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

/// Play a `.ssab` dropped on the window, or every `.ssab` directly inside a
/// dropped folder. Drop the folder rather than the file when a pack has external
/// instance references: those resolve by pack name against the pack's own
/// directory, and the player's loader finds the siblings there.
fn handle_drop(mut drops: MessageReader<FileDragAndDrop>, mut state: ResMut<ViewerState>, mut catalogue: ResMut<Catalogue>, mut players: Query<(&mut SsPlayer, &mut SsPlayback, &mut Slot)>) {
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
            let Some(info) = parse_pack(&path) else { continue };
            info!("dropped pack {:?} / {} animation(s)", info.name, info.anims.len());
            // A pack dropped twice replaces its own entry rather than growing the
            // list — what makes "convert, drop, look, convert again" work.
            let idx = catalogue.packs.iter().position(|p| p.name == info.name).unwrap_or(catalogue.packs.len());
            if idx == catalogue.packs.len() {
                catalogue.packs.push(info);
            } else {
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
                    let info = &catalogue.packs[idx];
                    player.src = info.path.clone();
                    player.animation = info.anims.first().cloned().unwrap_or_default();
                }
            }
            state.last_event = "—".into();
        }
    }
}

/// The return trip: timeline events come back as messages. This keeps the last one
/// for the HUD and — for an audio cue — actually plays the sound, because the
/// player only says *what* to play.
fn playback_events(mut events: MessageReader<SsEventMessage>, mut finished: MessageReader<SsFinished>, mut looped: MessageReader<SsLooped>, mut state: ResMut<ViewerState>, asset_server: Res<AssetServer>, mut commands: Commands) {
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
fn refresh_hud(state: Res<ViewerState>, catalogue: Res<Catalogue>, members: Query<(&Slot, &SsPlayback, &SsPlaybackState)>, mut hud: Query<&mut Text, With<Hud>>) {
    let Ok(mut text) = hud.single_mut() else { return };

    let focus = match state.focus {
        Some(i) => format!("#{}", i + 1),
        None => "all".to_string(),
    };
    let order = if state.reverse_order { ", order reversed" } else { "" };
    let flip = if state.y_flip { ", Y-flipped" } else { "" };
    let mut out = format!("Cast: {} in 1 pass   focus {focus}   layout {}{order}{flip}\n", state.cast.len(), state.layout.name());

    for (i, &entity) in state.cast.iter().enumerate() {
        let Ok((slot, playback, playstate)) = members.get(entity) else { continue };
        let marker = if state.focus == Some(i) { '>' } else { ' ' };
        let pack = catalogue.packs.get(slot.pack_idx).map(|p| p.name.as_str()).unwrap_or("none");
        let anim = catalogue.packs.get(slot.pack_idx).and_then(|p| p.anims.get(slot.anim_idx)).map(String::as_str).unwrap_or("none");
        let hidden = if playback.visible { "" } else { " [hidden]" };
        let paused = if playback.playing { "" } else { " [paused]" };
        out.push_str(&format!("{marker} #{} {pack} / {anim}   order {} alpha {:.2} frame {:.0}{hidden}{paused}\n", i + 1, playback.order, playback.alpha, playstate.frame_no));
    }

    out.push_str(&format!("last event: {}\n", state.last_event));
    out.push_str("Space pause  Left/Right pack  Up/Down anim  R restart  +/- cast  Tab focus\nL layout  O order  V hide  [ ] alpha  F flip-Y  Esc quit  (drop a .ssab to play it)");
    **text = out;
}

// --- catalogue ------------------------------------------------------------

fn is_ssab(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("ssab")
}

/// Parse one `.ssab` into a [`PackInfo`]. The pack is never renamed — the name
/// inside it is what instance references resolve against.
fn parse_pack(path: &Path) -> Option<PackInfo> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("failed to read {:?}: {e:?}", path);
            return None;
        }
    };
    match SSPack::load(&bytes) {
        Ok(pack) => Some(PackInfo { name: pack.name().to_owned(), path: path.to_string_lossy().into_owned(), anims: pack.animation_names() }),
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

    let packs: Vec<PackInfo> = paths.iter().filter_map(|p| parse_pack(p)).collect();
    let initial = packs.iter().position(|p| p.name == "Basic").unwrap_or(0);
    Catalogue { packs, initial }
}

/// Map a cue's `sound_name` (a bare stem) to an asset path, picking the extension
/// that exists on disk. Returned relative to the `assets/` root the Bevy
/// `AssetServer` loads from.
fn resolve_sound(name: &str) -> Option<String> {
    ["mp3", "wav", "ogg", "flac"].into_iter().find(|ext| Path::new(&format!("{ASSET_ROOT}/overall/sound/{name}.{ext}")).exists()).map(|ext| format!("overall/sound/{name}.{ext}"))
}
