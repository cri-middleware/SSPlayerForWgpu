//! A Bevy app with a SpriteStudio character in it, for eyeballing masks.
//!
//! Run from the repository root, which is where `assets/` is:
//!
//! ```text
//! cargo run -p ssplayer-wgpu-bevy --example bevy_viewer            # true_light
//! cargo run -p ssplayer-wgpu-bevy --example bevy_viewer -- walk_gate
//! cargo run -p ssplayer-wgpu-bevy --example bevy_viewer -- mask
//! cargo run -p ssplayer-wgpu-bevy --example bevy_viewer -- sound     # watch the console
//! ```
//!
//! `scripts/deploy-examples.sh` (or `.ps1`) generates `assets/` first.
//!
//! The pack is chosen on the command line so any fixture can be viewed without a
//! rebuild — the binary is the same, only the argument changes. The `mask` /
//! `true_light` / `walk_gate` packs show masking (with no stencil to clip with —
//! Bevy's 2d depth carries none — so the player rasterises its own coverage
//! bitmap); the `sound` / `userdata` / `signal` packs raise timeline events,
//! which `log_playback` prints, showing the render->main round trip.

use bevy::asset::AssetPlugin;
use bevy::audio::{AudioPlayer, PlaybackSettings};
use bevy::prelude::*;
use bevy::render::view::Msaa;
use ssplayer_wgpu_bevy::{SsEvent, SsEventMessage, SsFinished, SsLooped, SsPlayback, SsPlayer, SsPlayerPlugin, SsTransform};

/// Selectable fixtures: `(key, pack, animation)`. The animation names are what
/// the packs actually declare (`SSPlayer::animation_names`), not the file stems —
/// `True_Light.ssab` plays `True_light`, note the case. The first three mask; the
/// last three raise events.
const PACKS: &[(&str, &str, &str)] = &[
    ("true_light", "assets/Mask/True_Light.ssab", "True_light"),
    ("walk_gate", "assets/Mask/Walk_Gate.ssab", "Walk_gate"),
    ("mask", "assets/overall/Mask.ssab", "anime_1"),
    ("sound", "assets/overall/Sound.ssab", "anime_1"),
    ("userdata", "assets/overall/UserData.ssab", "anime_1"),
    ("signal", "assets/overall/Signal.ssab", "anime_1"),
];

/// The pack named on the command line, defaulting to the first entry.
fn selected_pack() -> (&'static str, &'static str) {
    let key = std::env::args().nth(1).unwrap_or_default();
    let entry = PACKS.iter().find(|(k, ..)| *k == key).unwrap_or(&PACKS[0]);
    (entry.1, entry.2)
}

/// The repo's `assets/`, absolute so it does not depend on the working directory.
/// Bevy's asset root otherwise defaults to *this crate's* folder, not the repo's,
/// so the sounds — which live beside the packs under the repo `assets/` — would
/// 404. The player's own loader roots on the `src` string instead, which is why
/// the `.ssab` loads regardless.
const ASSET_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(Window { title: "SpriteStudio in Bevy — mask check".into(), ..default() }), ..default() }).set(AssetPlugin { file_path: ASSET_ROOT.to_string(), ..default() }))
        .add_plugins(SsPlayerPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, (log_playback, play_sounds))
        .run();
}

fn setup(mut commands: Commands) {
    // Msaa::Off is required: the player's pipelines are single-sampled, and a
    // multisampled target would not match them.
    commands.spawn((Camera2d, Msaa::Off));

    // A Bevy sprite, to show what the player composites over.
    commands.spawn(Sprite { color: Color::srgb(0.15, 0.18, 0.28), custom_size: Some(Vec2::new(520.0, 520.0)), ..default() });

    let (src, animation) = selected_pack();
    info!("ssplayer-wgpu-bevy example: pack {src} / animation {animation}");
    commands.spawn((SsPlayer { src: src.into(), animation: animation.into() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback { order: 1, ..default() }));
}

/// The other half of the round-trip: playback runs in the render world and its
/// events come back as messages. This just logs them — an audio cue is where a
/// real host would feed its own sound stack, since the player plays none itself.
fn log_playback(mut events: MessageReader<SsEventMessage>, mut finished: MessageReader<SsFinished>, mut looped: MessageReader<SsLooped>) {
    for message in events.read() {
        match &message.event {
            SsEvent::Audio(audio) => info!("audio cue: {:?} x{}", audio.sound_name, audio.loop_num),
            SsEvent::UserData(data) => info!("user data on part {}", data.part_index),
            SsEvent::Signal(signal) => info!("signal {:?} on part {}", signal.command_id, signal.part_index),
        }
    }
    for _ in finished.read() {
        info!("animation finished");
    }
    for _ in looped.read() {
        info!("animation looped");
    }
}

/// The host side of an audio cue: the player says *what* to play, this actually
/// plays it. Shows the division of labour — nothing above this line makes a
/// sound. Not part of the adapter; a host wires its own audio like this.
fn play_sounds(mut events: MessageReader<SsEventMessage>, asset_server: Res<AssetServer>, mut commands: Commands) {
    for message in events.read() {
        if let SsEvent::Audio(audio) = &message.event {
            let Some(path) = resolve_sound(&audio.sound_name) else {
                warn!("no file for sound {:?} under assets/overall/sound/", audio.sound_name);
                continue;
            };
            // `DESPAWN` plays once and cleans up. SpriteStudio's `loop_num` is a
            // repeat count Bevy has no single-shot equivalent for, so this demo
            // honours the trigger rather than the count.
            commands.spawn((AudioPlayer::new(asset_server.load(path)), PlaybackSettings::DESPAWN));
        }
    }
}

/// Map a cue's `sound_name` (a bare stem) to an asset path, picking the extension
/// that exists on disk: the pack names the sound, the host owns where the file
/// lives and in what format. Returned relative to the `assets/` root the Bevy
/// `AssetServer` loads from.
fn resolve_sound(name: &str) -> Option<String> {
    ["mp3", "wav", "ogg", "flac"].into_iter().find(|ext| std::path::Path::new(&format!("{ASSET_ROOT}/overall/sound/{name}.{ext}")).exists()).map(|ext| format!("overall/sound/{name}.{ext}"))
}
