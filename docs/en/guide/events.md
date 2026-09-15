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
            SsEvent::Audio(a)    => audio.play(&a.sound_name, a.loop_num),
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
            play_sound(e.text, e.loop_num);
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
        case 'audio':    audio.play(e.soundName, e.loopNum); break;
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

## Empty is meaningful

| Situation | What `events()` gives you |
|---|---|
| A tick that crossed no marker | empty |
| A hidden player (`set_visible(false)`) | **empty** — the timeline did not advance, so the player reports nothing rather than repeating its last visible tick |
| A paused player | empty, for the same reason |

A host that plays a sound per event therefore cannot hear it twice because a character was culled.

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

Every event also carries `part_index` and the part's authored name, so "which part said this" needs
no bookkeeping of your own.

## Signal parameters

A signal is a command id plus a list of `(id, value)` pairs, where the value is one of: boolean, a
choice **index**, an integer, a float, text — or **none**, meaning the parameter exists but was left
unset.

On the C face the values are flattened: `value_type` says which field of `SsSignalParamData` to
read. In Rust it is the `SignalValue` enum; in JavaScript, a plain value.

## Completion and loop edges

Events tell you what the timeline said. Two other signals tell you what playback did:

| | Rust | C | JavaScript | Bevy |
|---|---|---|---|---|
| Completed on this tick | `just_finished()` | `ss_player_just_finished` | `justFinished()` | `SsFinished` message |
| Crossed a loop boundary | `just_looped()` | `ss_player_just_looped` | `justLooped()` | `SsLooped` message |

Both are **edges**, true for exactly the tick that did it, and the next `update()` moves them on — so
a completed one-shot cannot fire twice, including after the character is hidden and shown again.
Reading an edge does not consume it; the tick does.

## Next

* [Part overrides](./part-overrides.md) — changing what a part draws, often in response to an event
* [Playback and transport](./playback.md) — the transport verbs an event handler usually calls
