# Playback and transport

Four verbs, and they are not interchangeable. If an animation jumps back to its start when you did
not ask it to, this page is why.

## The four verbs

| Verb | What it does |
|---|---|
| **play** | **Head-out.** Rewinds to the start of the section and re-arms the loop count |
| **pause** | Holds on the current frame. Idempotent, and the player still counts as playing — a hold is not a stop |
| **resume** | Lifts a pause, carrying on from the same frame. Idempotent, and a **no-op on a stopped animation** |
| **stop** | Ends playback. The playhead stays where it stands, so you keep drawing the frame it stopped on |

!!! danger "`play()` is not how you lift a `pause()`"
    It rewinds. Using it to un-pause silently sends the playhead back to the start of the section
    and resets the loop counter — a bug that looks like a stutter and reads like a runtime fault.
    **`resume()` is what carries on in place.**

    The mirror of that: `resume()` cannot *start* anything. On a stopped animation it does nothing,
    because starting one is `play`'s job and `play` rewinds. This split is the SDK's transport
    contract and is the same on every player in the family.

=== "Rust"

    ```rust
    player.set_animation("attack")?;   // select — the allocating step
    player.play();                     // start from the head
    player.pause();                    // hold
    player.resume();                   // carry on from the same frame
    player.stop();                     // end, leaving the playhead where it is
    player.play_from(12.0);            // start from a frame, re-arming the loop count
    ```

=== "C"

    ```c
    ss_player_set_animation(player, "attack");
    ss_player_play(player);
    ss_player_pause(player);
    ss_player_resume(player);
    ss_player_stop(player);
    ss_player_play_from(player, 12.0f);
    ```

=== "JavaScript"

    ```js
    player.setAnimation('attack');
    player.play();
    player.pause();
    player.resume();
    player.stop();
    player.playFrom(12);
    ```

=== "Bevy"

    ```rust
    // SsPlayback::playing is pause/resume: it holds and carries on, and cannot
    // replay a finished animation.
    playback.playing = false;   // pause
    playback.playing = true;    // resume

    // To play from the head, bump the generation counter — any change of value.
    playback.restart += 1;

    // The rest are messages, since each happens once.
    plays.write(SsPlay { entity, start_frame: Some(12.0) });   // play from a frame
    stops.write(SsStop { entity });                             // stop
    ```

## Speed, direction and ping-pong

**Speed is a scale, not a direction.** `0.0` and negative values are a *stop*, and read back as the
`0.0` they became — reversing is a separate setting.

=== "Rust"

    ```rust
    use ssplayer_wgpu::{PlaybackDirection, PlaybackStyle};

    player.set_speed_scale(2.0);                                    // twice the authored rate
    player.set_playback_direction(PlaybackDirection::Backward, PlaybackStyle::Normal);
    player.set_playback_direction(PlaybackDirection::Forward, PlaybackStyle::PingPong);
    player.set_frame_rate(30);                                      // override the authored rate
    ```

=== "C"

    ```c
    ss_player_set_speed_scale(player, 2.0f);
    ss_player_set_playback_direction(player, 1 /* backward */, 0 /* normal */);
    ss_player_set_playback_direction(player, 0 /* forward  */, 1 /* ping-pong */);
    ss_player_set_frame_rate(player, 30);
    ```

=== "JavaScript"

    ```js
    player.setSpeedScale(2.0);
    player.setPlaybackDirection(1, 0);   // backward, normal
    player.setPlaybackDirection(0, 1);   // forward, ping-pong
    player.setFrameRate(30);
    ```

=== "Bevy"

    ```rust
    playback.speed_scale = 2.0;
    playback.direction = PlaybackDirection::Backward;
    playback.style = PlaybackStyle::PingPong;
    playback.frame_rate = 30;
    ```

| Wire value | Direction | Style |
|---|---|---|
| `0` | Forward | Normal — jump back to the start, or stop, per the loop count |
| `1` | Backward | PingPong — turn around and walk back, so one loop is a full round trip |

The `0` / `1` encoding is shared with every sibling player, which is why the C and JavaScript faces
take integers where Rust takes an enum.

Prefer **speed** over **frame rate**: speed scales the authored timing, while the frame rate
replaces it.

## Looping

`count` cycles, then stop. `-1` **or `0`** loops forever. Under ping-pong, one cycle is a full round
trip.

=== "Rust"

    ```rust
    player.set_loop_count(3);        // three cycles then stop
    player.set_loop_count(-1);       // forever
    let left = player.remaining_loops();
    ```

=== "C"

    ```c
    ss_player_set_loop_count(player, 3);
    int32_t left = ss_player_remaining_loops(player);
    ```

=== "JavaScript"

    ```js
    player.setLoopCount(3);
    const left = player.remainingLoops();
    ```

=== "Bevy"

    ```rust
    playback.loop_count = 3;   // a change re-arms the count, as a restart does
    ```

## Playing part of an animation

A **section** restricts the playhead to a frame range of the current animation; looping and
ping-pong turn around inside it.

Set it **after** `set_animation` — frame numbers only mean something inside the animation they were
measured in, and selecting an animation rebuilds the tree and takes the section with it.

=== "Rust"

    ```rust
    player.set_animation("walk")?;
    player.set_animation_section(10, 24);
    player.play();                          // starts at frame 10
    player.seek(18.0);                      // jump inside the section
    ```

=== "C"

    ```c
    ss_player_set_animation(player, "walk");
    ss_player_set_animation_section(player, 10, 24);
    ss_player_play(player);
    ss_player_seek(player, 18.0f);
    ```

=== "JavaScript"

    ```js
    player.setAnimation('walk');
    player.setAnimationSection(10, 24);
    player.play();
    player.seek(18);
    ```

=== "Bevy"

    ```rust
    // The section applies to whichever animation is set up, so change the two
    // together; setting it puts the playhead at its start.
    character.animation = "walk".into();
    playback.animation_section = Some((10, 24));

    // A seek is a message, since it happens once. `seeks` is a MessageWriter<SsSeek>.
    seeks.write(SsSeek { entity, frame: 18.0 });
    ```

## What survives switching animation or pack

The transport settings belong to the player, not to the animation. They hold across
`set_animation` and across a new pack (`load`, `set_pack`), and they can be set before the first
animation. This is the family's behaviour, not this player's invention.

| Survives `set_animation` and a new pack | Does **not** survive |
|---|---|
| speed scale, frame-rate override, playback direction, frame skip | the **section** — frame numbers mean nothing against a different clip |
| the loop count | |

## Asking where playback is

| | Rust | C | JavaScript |
|---|---|---|---|
| Current frame | `frame_no()` | `ss_player_frame_no` | `frameNo()` |
| Range | `start_frame()` / `end_frame()` / `total_frames()` | `ss_player_start_frame` / `_end_frame` / `_total_frames` | `startFrame()` / `endFrame()` / `totalFrames()` |
| Is it running | `is_playing()` / `is_pausing()` | `ss_player_is_playing` / `_is_pausing` | `isPlaying()` / `isPausing()` |
| Did it complete | `is_finished()` | `ss_player_is_finished` | `isFinished()` |
| **Edges** from the last tick | `just_started()` / `just_looped()` / `just_finished()` | `ss_player_just_started` / `_just_looped` / `_just_finished` | `justStarted()` / `justLooped()` / `justFinished()` |

The last row is the one to reach for. `just_finished()` is an **edge**: true for exactly the tick
that completed playback, and the next `update()` moves it on — so a completion cannot fire twice, and
a player that was culled and comes back does not re-announce a completion you already handled. Reading
it does not consume it; two reads in the same frame both see it. It is the *tick* that clears the edge,
not the *read*.

!!! note "A tick that did not run reports nothing"
    Culling a character (`set_culled(true)`) skips the timeline advance, and the player clears its
    events and its start, loop and completion edges for that frame rather than repeating the last
    answers of a tick that ran. A host that plays a sound per event will not hear it twice. A `play`
    made while culled is not lost: the first tick that runs reports its start.

    Hiding a character (`set_visible(false)`) is different: only the draw is skipped. The timeline
    keeps running and every tick reports as usual, so a character blinked by toggling it keeps its
    timing.

## Smoothness: sub-frame and frame skip

| Setting | Default | What it does |
|---|---|---|
| Sub-frame | on | Interpolates between authored frames, so playback follows the display rate rather than a fixed fps — smooth even in slow motion |
| Frame skip | on | A long tick crosses several frames at once and lands on the right one, instead of drifting behind. **Every frame stepped through still raises its events** |

Turn sub-frame off for a deliberately stepped, "on-twos" look; turn frame skip off when you need
exactly one frame advanced per update, whatever the elapsed time.

## Next

* [Events and user data](./events.md) — reacting to the timeline
* [Motion blending](./motion-blending.md) — playing two animations at once and crossfading
* [Part overrides](./part-overrides.md) — changing what a part draws
