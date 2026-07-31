//! Timeline events, decoded into owned Rust values.
//!
//! **Everything here is owned, not borrowed.** The rest of the frame path hands
//! out views into runtime memory that die with the frame (SPEC §1.1), but an
//! event is something a host reacts to — it fires a sound, spawns a hit box,
//! advances a script — and that reaction routinely outlives the tick that
//! raised it. Copying a handful of small values per event is the price of the
//! host not having to care.
//!
//! ## Where the values come from
//!
//! The runtime reports *which* frames of the timeline the last update crossed;
//! the payloads themselves live in the `.ssab`, addressed by the same index.
//! [`decode_events`] is the join between the two.
//!
//! ## Why the update is what collects them
//!
//! Playback does not visit every frame. Under frame skipping a slow tick jumps
//! several frames at once, and a loop can wrap mid-tick — but the runtime
//! records **every frame it stepped through** (`passed_frames`, which the SDK's
//! own tests pin as `[4, 5, 6, 7, 8]` across a long tick and `[3, 4, 5, 0, 1]`
//! across a loop), so polling it right after the update is what makes a
//! footstep on a skipped frame still fire. Walking frames yourself, or reading
//! only the frame that was landed on, loses them.

use ssab::ssab::ss::format::{AnimationData, SignalCommandParamValue};

/// One value carried by a signal parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum SignalValue {
    Bool(bool),
    /// A choice from a list, as its index.
    Index(i32),
    Integer(i32),
    Floating(f32),
    Text(String),
    /// The parameter exists but was left unset.
    None,
}

/// One `param_id` → value pair on a signal.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalParam {
    pub id: String,
    pub value: SignalValue,
}

/// A "User Data" marker: up to four optional values, in whatever combination
/// the animator filled in. A field is `None` when it was left unset — which is
/// not the same as zero, and is why these are options rather than defaults.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserDataEvent {
    pub part_index: u32,
    pub integer: Option<i32>,
    /// `[x1, y1, x2, y2]` as authored — two corners, not origin+size.
    pub rect: Option<[f32; 4]>,
    pub point: Option<[f32; 2]>,
    pub text: Option<String>,
}

/// A "Signal": a named command plus its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalEvent {
    pub part_index: u32,
    pub command_id: String,
    pub params: Vec<SignalParam>,
}

/// An audio trigger. **The player never produces sound**: this says what to
/// play and how many times, and the host feeds its own audio stack.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioEvent {
    pub part_index: u32,
    /// The alias as authored, kept for diagnostics and for hosts that key their
    /// own audio by name.
    pub sound_name: String,
    /// Addresses the `SoundList`, and the `SoundFile` within it — the pair a
    /// host resolves against the pack when it wants the file rather than the
    /// name.
    pub sound_list_name_hash: u32,
    pub sound_name_hash: u32,
    /// How many times to play it. SpriteStudio has no infinite audio loop, so
    /// this is a count rather than a flag.
    pub loop_num: i32,
}

/// Something that happened on a frame the last update crossed.
#[derive(Clone, Debug, PartialEq)]
pub enum SsEvent {
    UserData(UserDataEvent),
    Signal(SignalEvent),
    Audio(AudioEvent),
}

impl SsEvent {
    /// Which part raised it.
    pub fn part_index(&self) -> u32 {
        match self {
            Self::UserData(e) => e.part_index,
            Self::Signal(e) => e.part_index,
            Self::Audio(e) => e.part_index,
        }
    }
}

/// Append every event on the frames `indices` names to `out`.
///
/// `events` is the current animation's `EventsPerFrame` vector and `indices`
/// the runtime's passed-event indices. **Indices are bounds-checked**: they are
/// only meaningful against the animation currently bound to the context, and a
/// stale one — or the runtime's not-found `-1` — would otherwise read past the
/// FlatBuffer. The Godot Player hit exactly that.
///
/// Signals that are switched off are skipped, matching the sibling Players:
/// an inactive signal is authoring state, not something that happened.
pub(crate) fn decode_events(anim: &AnimationData<'_>, indices: &[i32], out: &mut Vec<SsEvent>) {
    let Some(events) = anim.events() else { return };
    let len = events.len();
    for &index in indices {
        if index < 0 || index as usize >= len {
            continue;
        }
        let per_frame = events.get(index as usize);

        if let Some(users) = per_frame.users() {
            for user in users {
                let value = user.value();
                out.push(SsEvent::UserData(UserDataEvent {
                    part_index: user.part_index() as u32,
                    integer: value.integer().map(|v| v.value()),
                    rect: value.rect().map(|r| [r.x1(), r.y1(), r.x2(), r.y2()]),
                    point: value.point().map(|p| [p.v1(), p.v2()]),
                    text: value.str().map(str::to_owned),
                }));
            }
        }

        if let Some(signals) = per_frame.signals() {
            for signal in signals {
                let value = signal.value();
                if !value.active() {
                    continue;
                }
                let params = value.params().iter().map(|param| SignalParam { id: param.param_id().to_owned(), value: decode_signal_value(&param) }).collect();
                out.push(SsEvent::Signal(SignalEvent { part_index: signal.part_index() as u32, command_id: value.command_id().to_owned(), params }));
            }
        }

        if let Some(audios) = per_frame.audios() {
            for audio in audios {
                let value = audio.value();
                out.push(SsEvent::Audio(AudioEvent {
                    part_index: audio.part_index() as u32,
                    sound_name: value.sound_name().to_owned(),
                    sound_list_name_hash: value.sound_list_name_hash(),
                    sound_name_hash: value.sound_name_hash(),
                    loop_num: value.loop_num(),
                }));
            }
        }
    }
}

/// Unwrap one parameter's union. An unset or unrecognised union reads as
/// [`SignalValue::None`] rather than being dropped, so a host iterating params
/// still sees that the parameter is there.
fn decode_signal_value(param: &ssab::ssab::ss::format::SignalCommandParam<'_>) -> SignalValue {
    match param.value_type() {
        SignalCommandParamValue::Bool => param.value_as_bool().map(|v| SignalValue::Bool(v.value())),
        SignalCommandParamValue::Index => param.value_as_index().map(|v| SignalValue::Index(v.value())),
        SignalCommandParamValue::Integer => param.value_as_integer().map(|v| SignalValue::Integer(v.value())),
        SignalCommandParamValue::Floating => param.value_as_floating().map(|v| SignalValue::Floating(v.value())),
        SignalCommandParamValue::Text => param.value_as_text().map(|v| SignalValue::Text(v.value().to_owned())),
        _ => None,
    }
    .unwrap_or(SignalValue::None)
}
