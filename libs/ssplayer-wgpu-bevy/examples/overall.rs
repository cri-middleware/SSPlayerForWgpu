//! An interactive tour of the wgpu SpriteStudio player inside a Bevy app — the
//! Bevy counterpart of the sibling players' `overall` / `webgl-example` demos.
//!
//! It draws one character and steps through the shared sample set (the `Ringo`
//! character and the `overall/*` feature packs — Basic, Shape, PartsColor, Mesh,
//! BlendMode, Effect, Mask, Instance, TextureChange, Sound, UserData, Signal —
//! plus a masked-instance scene), all driven through the three components the
//! adapter exposes: `SsPlayer`, `SsTransform`, `SsPlayback`.
//!
//! Run from the repository root, which is where `assets/` is:
//!
//! ```text
//! cargo run -p ssplayer-wgpu-bevy --example overall
//! ```
//!
//! `scripts/deploy-examples.sh` (or `.ps1`) generates `assets/` first.
//!
//! Keys: `[` / `]` previous / next scene, `Space` play / pause, `Up` / `Down`
//! speed, `R` restart. Timeline frame and the last timeline event show in the
//! on-screen HUD; audio cues are played by this host, since the player only names
//! them.

use bevy::asset::AssetPlugin;
use bevy::audio::{AudioPlayer, PlaybackSettings};
use bevy::camera::CompositingSpace;
use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::render::view::Msaa;
use ssplayer_wgpu_bevy::{SsEvent, SsEventMessage, SsFinished, SsLooped, SsPlayback, SsPlaybackState, SsPlayer, SsPlayerPlugin, SsTransform};

/// The repo's `assets/`, absolute so the example is launchable from anywhere and
/// so Bevy's asset root (for the audio files) points at the same tree the player
/// loads packs from. See the colour note in `bevy_viewer.rs`.
const ASSET_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

/// The tour: `(label, pack, animation)`. The animation names are what the packs
/// actually declare — the sibling examples draw from this same shared set.
const SCENES: &[(&str, &str, &str)] = &[
    ("Ringo — wait", "assets/Ringo/Ringo.ssab", "wait1"),
    ("Ringo — run", "assets/Ringo/Ringo.ssab", "run"),
    ("Ringo — dance", "assets/Ringo/Ringo.ssab", "dance"),
    ("Basic", "assets/overall/Basic.ssab", "anime_1"),
    ("Shape", "assets/overall/Shape.ssab", "anime_1"),
    ("Parts colour", "assets/overall/PartsColor.ssab", "PartsColor_only"),
    ("Mesh", "assets/overall/Mesh.ssab", "anime_1"),
    ("Blend modes", "assets/overall/BlendMode.ssab", "anime_1"),
    ("Effect (particles)", "assets/overall/Effect.ssab", "anime_1"),
    ("Mask", "assets/overall/Mask.ssab", "anime_1"),
    ("Instance (sub-animation)", "assets/overall/Instance.ssab", "anime_1"),
    ("Texture change", "assets/overall/TextureChange.ssab", "anime_1"),
    ("Audio cues", "assets/overall/Sound.ssab", "anime_1"),
    ("User data", "assets/overall/UserData.ssab", "anime_1"),
    ("Signals", "assets/overall/Signal.ssab", "anime_1"),
    ("Masked instances", "assets/Mask/True_Light.ssab", "True_light"),
];

/// Where the tour is, the character it drives, and the last event to show.
#[derive(Resource)]
struct Tour {
    index: usize,
    character: Entity,
    last_event: String,
}

/// Marks the HUD text so a system can find it to update.
#[derive(Component)]
struct Hud;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(Window { title: "SpriteStudio in Bevy".into(), ..default() }), ..default() }).set(AssetPlugin { file_path: ASSET_ROOT.to_string(), ..default() }))
        .add_plugins(SsPlayerPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, (controls, playback_events, refresh_hud))
        .run();
}

fn setup(mut commands: Commands) {
    // Msaa::Off: the player's pipelines are single-sampled. CompositingSpace::Srgb:
    // SpriteStudio art is authored in sRGB and the player writes it verbatim, so a
    // default (sRGB-target, linear-compositing) camera makes it too bright.
    commands.spawn((Camera2d, Msaa::Off, CompositingSpace::Srgb));

    // An opaque backdrop: additive blend modes read the target, so a transparent
    // clear would let them blow out.
    commands.spawn(Sprite { color: Color::srgb(0.10, 0.11, 0.15), custom_size: Some(Vec2::splat(100_000.0)), ..default() });

    let (_, src, anim) = SCENES[0];
    let character = commands.spawn((SsPlayer { src: src.into(), animation: anim.into() }, SsTransform { scale_x: 0.7, scale_y: 0.7, ..default() }, SsPlayback::default())).id();

    commands.spawn((Text::new(""), TextFont { font_size: bevy::text::FontSize::Px(15.0), ..default() }, TextColor(Color::WHITE), Node { position_type: PositionType::Absolute, top: Val::Px(8.0), left: Val::Px(10.0), ..default() }, Hud));

    commands.insert_resource(Tour { index: 0, character, last_event: "—".into() });
}

/// `[` / `]` switch scene, `Space` play/pause, `Up` / `Down` speed, `R` restart —
/// each a plain edit of the character's components.
fn controls(keys: Res<ButtonInput<KeyCode>>, mut tour: ResMut<Tour>, mut players: Query<(&mut SsPlayer, &mut SsPlayback)>) {
    let Ok((mut player, mut playback)) = players.get_mut(tour.character) else {
        return;
    };

    let step = keys.just_pressed(KeyCode::BracketRight) as i32 - keys.just_pressed(KeyCode::BracketLeft) as i32;
    if step != 0 {
        let n = SCENES.len() as i32;
        tour.index = (((tour.index as i32 + step) % n + n) % n) as usize;
        let (_, src, anim) = SCENES[tour.index];
        // Changing `src` rebuilds from the new pack; changing `animation` selects
        // within it. Both are what the adapter watches for.
        player.src = src.into();
        player.animation = anim.into();
        tour.last_event = "—".into();
    }

    if keys.just_pressed(KeyCode::Space) {
        playback.playing = !playback.playing;
    }
    if keys.just_pressed(KeyCode::KeyR) {
        // A generation counter: bumping it replays from the head, which pausing
        // and resuming cannot do.
        playback.restart = playback.restart.wrapping_add(1);
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        playback.speed = (playback.speed + 0.1).min(3.0);
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        playback.speed = (playback.speed - 0.1).max(0.1);
    }
}

/// The return trip: timeline events come back as messages. This logs them, keeps
/// the last one for the HUD, and — for an audio cue — actually plays the sound,
/// because the player only says *what* to play.
fn playback_events(mut events: MessageReader<SsEventMessage>, mut finished: MessageReader<SsFinished>, mut looped: MessageReader<SsLooped>, mut tour: ResMut<Tour>, asset_server: Res<AssetServer>, mut commands: Commands) {
    for message in events.read() {
        match &message.event {
            SsEvent::Audio(audio) => {
                tour.last_event = format!("audio {:?} x{}", audio.sound_name, audio.loop_num);
                if let Some(path) = resolve_sound(&audio.sound_name) {
                    commands.spawn((AudioPlayer::new(asset_server.load(path)), PlaybackSettings::DESPAWN));
                }
            }
            SsEvent::UserData(data) => tour.last_event = format!("user data on part {}", data.part_index),
            SsEvent::Signal(signal) => tour.last_event = format!("signal {:?}", signal.command_id),
        }
    }
    for _ in finished.read() {
        tour.last_event = "finished".into();
    }
    for _ in looped.read() {
        tour.last_event = "looped".into();
    }
}

/// Draw the HUD from the character's read-back state.
fn refresh_hud(tour: Res<Tour>, states: Query<&SsPlaybackState>, playbacks: Query<&SsPlayback>, mut hud: Query<&mut Text, With<Hud>>) {
    let Ok(mut text) = hud.single_mut() else {
        return;
    };
    let (label, _, anim) = SCENES[tour.index];
    let frame = states.get(tour.character).map(|state| state.frame_no).unwrap_or(0.0);
    let (playing, speed) = playbacks.get(tour.character).map(|p| (p.playing, p.speed)).unwrap_or((true, 1.0));
    **text = format!("scene {}/{}: {label}  ({anim})\n[ ] switch   Space {}   Up/Down speed {:.1}   R restart\nframe {frame:.1}   last event: {}", tour.index + 1, SCENES.len(), if playing { "pause" } else { "resume" }, speed, tour.last_event,);
}

/// Map a cue's `sound_name` to an asset path (relative to Bevy's asset root),
/// picking the extension that exists — the pack names the sound, the host owns
/// where the file lives.
fn resolve_sound(name: &str) -> Option<String> {
    ["mp3", "wav", "ogg", "flac"].into_iter().find(|ext| std::path::Path::new(&format!("{ASSET_ROOT}/overall/sound/{name}.{ext}")).exists()).map(|ext| format!("overall/sound/{name}.{ext}"))
}
