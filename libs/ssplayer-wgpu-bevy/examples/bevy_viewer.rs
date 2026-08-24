//! A Bevy app with a SpriteStudio character in it, for eyeballing masks.
//!
//! Run from the repository root, which is where `assets/` is:
//!
//! ```text
//! cargo run -p ssplayer-wgpu-bevy --example bevy_viewer            # true_light
//! cargo run -p ssplayer-wgpu-bevy --example bevy_viewer -- walk_gate
//! cargo run -p ssplayer-wgpu-bevy --example bevy_viewer -- mask
//! ```
//!
//! `scripts/deploy-examples.sh` (or `.ps1`) generates `assets/` first.
//!
//! The pack is chosen on the command line so both mask fixtures can be viewed
//! without a rebuild — the binary is the same, only the argument changes. Each is
//! a masking animation, drawn over a plain Bevy sprite to show the player
//! compositing onto Bevy's own pass. The mask has no stencil to clip with —
//! Bevy's 2d depth texture carries none — so the player rasterises its own
//! coverage bitmap instead.

use bevy::prelude::*;
use bevy::render::view::Msaa;
use ssplayer_wgpu_bevy::{SsPlayback, SsPlayer, SsPlayerPlugin, SsTransform};

/// Selectable masking fixtures: `(key, pack, animation)`. The animation names are
/// what the packs actually declare (`SSPlayer::animation_names`), not the file
/// stems — `True_Light.ssab` plays `True_light`, note the case.
const MASK_PACKS: &[(&str, &str, &str)] = &[("true_light", "assets/Mask/True_Light.ssab", "True_light"), ("walk_gate", "assets/Mask/Walk_Gate.ssab", "Walk_gate"), ("mask", "assets/overall/Mask.ssab", "anime_1")];

/// The pack named on the command line, defaulting to the first entry.
fn selected_mask() -> (&'static str, &'static str) {
    let key = std::env::args().nth(1).unwrap_or_default();
    let entry = MASK_PACKS.iter().find(|(k, ..)| *k == key).unwrap_or(&MASK_PACKS[0]);
    (entry.1, entry.2)
}

fn main() {
    App::new().add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(Window { title: "SpriteStudio in Bevy — mask check".into(), ..default() }), ..default() })).add_plugins(SsPlayerPlugin).add_systems(Startup, setup).run();
}

fn setup(mut commands: Commands) {
    // Msaa::Off is required: the player's pipelines are single-sampled, and a
    // multisampled target would not match them.
    commands.spawn((Camera2d, Msaa::Off));

    // A Bevy sprite, to show what the player composites over.
    commands.spawn(Sprite { color: Color::srgb(0.15, 0.18, 0.28), custom_size: Some(Vec2::new(520.0, 520.0)), ..default() });

    let (src, animation) = selected_mask();
    info!("ssplayer-wgpu-bevy example: mask pack {src} / animation {animation}");
    commands.spawn((SsPlayer { src: src.into(), animation: animation.into() }, SsTransform { scale_x: 0.5, scale_y: 0.5, ..default() }, SsPlayback { order: 1, ..default() }));
}
