# Events and user data

An animator can put markers on the timeline, and the player hands them to you as the playhead
crosses them. Three kinds:

| Kind | What it carries | Typical use |
|---|---|---|
| **User data** | up to four optional values — an integer, a rect, a point, a string | a hit box on frame 12, a spawn point, a state name |
| **Signal** | a named command plus typed parameters | "the animation says: open the door", with arguments |
| **Audio** | a sound name, its hashes, and a play count | a footstep, a voice cue |

!!! warning "The player never produces sound"
    An audio event says *what* to play and *how many times*. Feed the cue to your own audio stack.

## Reading them

Read after the update that produced them. What you get is **the events the last tick crossed**.

=== "Rust"

    ```rust
    scene.update(&mut renderer, &device, &queue, dt, &mut [&mut player]);

    for event in player.events() {
        match event {
            SsEvent::UserData(u) => println!("user data on frame {}: {:?}", u.frame_no, u.text),
            SsEvent::Signal(s)   => println!("signal {} with {} params", s.command_id, s.params.len()),
            SsEvent::Audio(a)    => if player.is_playing_forward() { audio.play(&a.sound_name, a.loop_num) },
        }
    }
    ```

=== "C"

    ```c
    ss_scene_update(scene, dt);

    uint32_t n = ss_player_event_count(player);
    for (uint32_t i = 0; i < n; i++) {
        SsEventData e;
        if (ss_player_event_at(player, i, &e) != SS_OK) continue;

        switch (e.kind) {
        case SS_EVENT_USER_DATA:
            if (e.has_integer) { /* e.integer */ }
            if (e.has_text)    { /* e.text -- copy it if you keep it */ }
            break;
        case SS_EVENT_SIGNAL:
            for (uint32_t p = 0; p < e.param_count; p++) {
                SsSignalParamData param;
                ss_player_event_signal_param_at(player, i, p, &param);
            }
            break;
        case SS_EVENT_AUDIO:
            if (ss_player_is_playing_forward(player)) play_sound(e.text, e.loop_num);
            break;
        }
    }
    ```

=== "JavaScript"

    ```js
    surface.frame(dt);

    for (const e of player.events()) {
      switch (e.kind) {
        case 'userData': console.log(e.integer, e.rect, e.point, e.text); break;
        case 'signal':   console.log(e.commandId, e.params); break;
        case 'audio':    if (player.isPlayingForward()) audio.play(e.soundName, e.loopNum); break;
      }
    }
    ```

=== "Bevy"

    ```rust
    fn react(mut events: MessageReader<SsEventMessage>) {
        for message in events.read() {
            match &message.event {
                SsEvent::UserData(u) => { /* ... */ }
                SsEvent::Signal(s)   => { /* ... */ }
                SsEvent::Audio(a)    => { /* ... */ }
            }
        }
    }
    ```

    Messages are **one frame behind**: playback runs in the render world, after that frame's
    `Update`. For a sound cue that is imperceptible.

## Every frame stepped through raises its events

This is what makes the list complete.

A slow tick under frame skipping crosses several authored frames at once, and a loop can wrap in
the middle of one. **Every frame stepped over reports its events**, not just the frame the playhead
landed on — so a footstep on frame 7 is not lost because the tick went from 5 to 9.

That is also why each event carries the frame it was **authored** on, which is not necessarily the
frame it was noticed on.

## With motion blending

Only track 0's animation fires events, and from the moment a `crossfade` on track 0 begins that is
the incoming animation: its events come through from the start of the fade, and the outgoing
animation's stop — unless `set_track_outgoing_event_threshold` keeps them going for part of it. Each event also says which
**source** fired it — `source()`, the handle `track_main_source` and `add_track_source` use — which
tells events apart once another source has opted in with `set_source_event_threshold`
([Motion blending](./motion-blending.md)). The C face carries it as `SsEventData.source`, and each
JavaScript event object as `source`.

## Empty is meaningful

| Situation | What `events()` gives you |
|---|---|
| A tick that crossed no marker | empty |
| A culled player (`set_culled(true)`) | **empty** — the timeline did not advance, so the player reports nothing rather than repeating the last tick that ran |
| A hidden player (`set_visible(false)`) | reported as usual — hiding skips the draw, not the timeline |
| A paused or stopped player | empty, since its timeline does not advance — except on the update after a `seek`, which reports the frame it landed on |
| A player with no animation selected — before the first `set_animation`, or after a `load` | **empty** — there is no tree to tick, so the old pack's last tick is not repeated either |

A host that plays a sound per event therefore cannot hear it twice because a character was culled.

## Playing an audio event

The player reports the key it crossed; whether it sounds is yours to decide. Every SpriteStudio
Player that plays audio itself decides by two rules, and on this one they are the host's:

* **Not while playing backward.** SpriteStudio has no reverse audio. Check `is_playing_forward()`
  (`ss_player_is_playing_forward`, `isPlayingForward()`, and on Bevy the message's entity's
  `SsPlaybackState::playing_forward`) — it is false on a ping-pong's return leg as well as under a
  backward direction. Hold back the sound, not the event: a subtitle or a rumble keyed to the same
  marker still wants it.
* **Not for where a seek landed.** A `seek` makes the next update report the events of the frame it
  landed on, whether the player is playing, paused or stopped. To keep a seek silent, skip the audio
  of the first update after it. The first frame after `play` is meant to sound.

A sound plays to its end. `pause`, `stop` and a new animation stop the *animation*, not the sounds it
already started; stop those only when the character itself goes away.

## What is in a user-data event

Four optional fields, and **unset is not zero**. That distinction is the whole reason they are
optional: an animator who filled in only the text left the integer *unset*, and a `0` would be a
different statement.

| Field | Rust | C | JavaScript |
|---|---|---|---|
| Integer | `Option<i32>` | `has_integer` + `integer` | `e.integer` or absent |
| Rect | `Option<[f32; 4]>` — `[x1, y1, x2, y2]`, **two corners, not origin + size** | `has_rect` + `rect[4]` | `e.rect` |
| Point | `Option<[f32; 2]>` | `has_point` + `point[2]` | `e.point` |
| Text | `Option<String>` | `has_text` + `text` | `e.text` |

Every event also carries `part_index`, and on Rust, JavaScript and Bevy the part's authored name, so
"which part said this" needs no bookkeeping of your own. The C event carries the index alone; turn
it into the name with `ss_player_part_name(player, part_index)`.

## Signal parameters

A signal is a command id plus a list of `(id, value)` pairs, where the value is one of: boolean, a
choice **index**, an integer, a float, text — or **none**, meaning the parameter exists but was left
unset.

On the C face the values are flattened: `value_type` says which field of `SsSignalParamData` to
read. In Rust it is the `SignalValue` enum; in JavaScript, a plain value.

## Start, completion and loop edges

Events tell you what the timeline said. Three other signals tell you what playback did:

| | Rust | C | JavaScript | Bevy |
|---|---|---|---|---|
| Started on this tick | `just_started()` | `ss_player_just_started` | `justStarted()` | `SsStarted` message |
| Completed on this tick | `just_finished()` | `ss_player_just_finished` | `justFinished()` | `SsFinished` message |
| Crossed a loop boundary | `just_looped()` | `ss_player_just_looped` | `justLooped()` | `SsLooped` message |

All three are **edges**, true for exactly the tick that did it, and the next `update()` moves them
on — so a completed one-shot cannot fire twice, including after the character is culled and brought
back. Reading an edge does not consume it; the tick does.

The start edge is raised by the first tick after `play` or `play_from`, and **not by `resume`**,
which carries on rather than starting. After a `play` of your own you already know; it earns its
place when something else started playback — the Bevy adapter plays whatever animation you hand
`SsPlayer`. A `play` on a culled character is reported by the first tick that runs once it is
unculled, and one that `stop` or `set_animation` overtook before any tick ran is not reported.

## Next

* [Part overrides](./part-overrides.md) — changing what a part draws, often in response to an event
* [Playback and transport](./playback.md) — the transport verbs an event handler usually calls
