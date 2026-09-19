//! Timeline events, decoded into owned Rust values.
//!
//! **Everything here is owned, not borrowed**, so a host's reaction to an event
//! may outlive the tick that raised it.
//!
//! The runtime reports which frames of the timeline the last update crossed; the
//! payloads live in the `.ssab`, addressed by the same index. [`decode_events`]
//! joins the two.
//!
//! Events are collected during the update, which is what makes them complete: a
//! slow tick under frame skipping crosses several frames at once and a loop can
//! wrap mid-tick, and every frame stepped through raises its events. Reading
//! only the frame that was landed on loses the rest.

use ssab::ssab::ss::format::{AnimationData, SignalCommandParamValue, SsAnimeBinary};

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
    /// The part's name as authored, or empty when the pack has none for it.
    pub part_name: String,
    /// The frame the key is **authored** on, which is not necessarily the frame
    /// it was noticed on: one update can step across several and reports every
    /// key in between.
    pub frame_no: u32,
    pub integer: Option<i32>,
    /// `[left, top, right, bottom]` as authored — four edges, not origin+size.
    pub rect: Option<[f32; 4]>,
    pub point: Option<[f32; 2]>,
    pub text: Option<String>,
}

/// A "Signal": a named command plus its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct SignalEvent {
    pub part_index: u32,
    /// The part's name as authored, or empty when the pack has none for it.
    pub part_name: String,
    /// The frame the key is **authored** on, which is not necessarily the frame
    /// it was noticed on: one update can step across several and reports every
    /// key in between.
    pub frame_no: u32,
    pub command_id: String,
    pub params: Vec<SignalParam>,
}

/// An audio trigger. **The player never produces sound**: this says what to
/// play and how many times, and the host feeds its own audio stack.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioEvent {
    pub part_index: u32,
    /// The part's name as authored, or empty when the pack has none for it.
    pub part_name: String,
    /// The frame the key is **authored** on, which is not necessarily the frame
    /// it was noticed on: one update can step across several and reports every
    /// key in between.
    pub frame_no: u32,
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
pub(crate) fn decode_events(ab: &SsAnimeBinary<'_>, anim: &AnimationData<'_>, indices: &[i32], out: &mut Vec<SsEvent>) {
    let Some(events) = anim.events() else { return };
    let len = events.len();
    // The pack's part list is shared across its animations, so one lookup per
    // event resolves the name a host would otherwise carry the pack around for.
    let parts = ab.parts();
    let part_name = |index: u32| -> String { parts.get(index as usize).name().to_owned() };
    for &index in indices {
        if index < 0 || index as usize >= len {
            continue;
        }
        let per_frame = events.get(index as usize);
        let frame_no = per_frame.frame_index() as u32;

        if let Some(users) = per_frame.users() {
            for user in users {
                let value = user.value();
                out.push(SsEvent::UserData(UserDataEvent {
                    part_index: user.part_index() as u32,
                    part_name: part_name(user.part_index() as u32),
                    frame_no,
                    integer: value.integer().map(|v| v.value()),
                    rect: value.rect().map(|r| [r.left(), r.top(), r.right(), r.bottom()]),
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
                out.push(SsEvent::Signal(SignalEvent { part_index: signal.part_index() as u32, part_name: part_name(signal.part_index() as u32), frame_no, command_id: value.command_id().to_owned(), params }));
            }
        }

        if let Some(audios) = per_frame.audios() {
            for audio in audios {
                let value = audio.value();
                out.push(SsEvent::Audio(AudioEvent {
                    part_index: audio.part_index() as u32,
                    part_name: part_name(audio.part_index() as u32),
                    frame_no,
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
