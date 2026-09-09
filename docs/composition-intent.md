# Composing from an idea

In the song sheet, choose a mood, major or minor, and a speed, then create the song.
For example, **Dark**, **Minor**, and **Slow** asks for a dark minor-key song at 80 BPM.
**Follow the mood** chooses a scale from the mood and picks a slow, moderate, or fast tempo
from its energy. **Sound** optionally selects a more specific character, with a plain-language
description beside the scale name. A selected sound stays fixed when the mood or style changes.
The style chooses the instruments and form, using newly generated harmony.

| Sound | Scale | Automatic mood example |
| --- | --- | --- |
| Bright | Major | Bright |
| Subdued | Natural minor | Dark |
| Dark with warmth | Dorian | Epic |
| Floating | Lydian | Dreamy |
| Bright and mellow | Mixolydian | Funky |
| Tense and dark | Phrygian | Tense |

These descriptions are starting points: rhythm, register and instruments also shape the mood.
The automatic rules use the mood's continuous values, so the advanced mood pads work too.
An explicit **Major** or **Minor** tonality uses that scale and resets Sound to automatic.
Choosing a specific sound updates the major/minor family to match; choosing **Automatic** in
Sound returns the scale to following the mood. The automatic Sound label shows the resolved key.

The same request works in a `.asong` file through the CLI or composition tools:

```toml
style = "pop-band"
mood = "dark"
tonality = "minor"
pace = "slow"
```

`tonality` accepts `auto`, `major`, or `minor`. `pace` accepts `auto`, `slow`, `moderate`, or
`fast` (80, 120, or 160 BPM). Omitting them when naming a mood uses `auto`.
`sound` accepts `auto`, `major`, `minor`, `dorian`, `lydian`, `mixolydian`, or `phrygian`.
For example, `mood = "dark"` with `sound = "dorian"` keeps dark performance settings and uses
Dorian harmony. See `examples/dorian-song.asong` for a complete request.
An explicit `sound` wins over `tonality`, which wins over mood-based scale selection.
An explicit `key` or `scale` wins over both; an explicit `tempo` wins over `pace`.
`chords` supplies an explicit progression if desired.

The advanced view exposes exact key, chord progressions, arrangement, and performance controls.
Saved specifications contain the resolved settings, so reopening a song retains the key and
tempo it was saved with. Choose **Automatic** in Sound to make the scale follow the mood again.
An exact key entered in the advanced view remains explicit when the mood changes.

## Auditioning chords before composing

The bottom of the song sheet has a section picker and **Preview chords**. It renders the
first eight bars (or fewer for a short section or a thirty-second limit) as simultaneous
keyboard chords, with a fixed timbre, velocity and playback level. The section's tempo and
chord-change timing are retained. Chord names appear in playback order.

**Stop preview** stops sound without discarding the rendered candidate. **Another progression**
invents an alternative for the selected section. **Use these chords** adopts its complete
progression, including chord colours, into the draft; **Create song** then uses those chords.
Adoption does not change other sections or the melody seed. A long section's preview covers
only its opening, while adoption retains the complete section. Saved specifications keep the
adopted progression explicit.

Previewing pauses arrangement playback and bypasses the project's mixer. It requires no track
or SoundFont and makes no document edits. Changing the draft or closing the sheet cancels stale
audio and render results. Previewing is unavailable during recording.

For a headless WAV using the same renderer:

```sh
cargo run -p auris-session --example chord_preview -- examples/dorian-song.asong verse target/chords.wav
```

## How harmony is generated

The generator in `auris-compose/src/progression.rs` has three steps:

1. Split a section into balanced phrases of at most four bars.
2. Choose a complete phrase of scale degrees and build chords by stacking the selected scale's
   thirds. Major and minor have tonal phrases; the four modal sounds share six phrase shapes
   with just two degree choices per mode. Each modal phrase anchors the tonic and visits a
   characteristic chord: IV in Dorian, II in Lydian and Phrygian, VII in Mixolydian.
   Repeat the opening phrase every other phrase, varying the phrases between repetitions.
3. In busier tonal phrases, optionally prepare V with ii in major or iv in minor within the bar.

The frame planner adds chord colour and decides whether a section needs an arrival.
Modal harmony keeps extensions within the selected scale and approaches the tonic with its
characteristic chord, retaining the mode instead of inserting a tonal dominant. An explicit
dominant lead-in for a key change is still honoured. Tonal minor can use a major dominant.
All instruments read that completed plan. Short sections retain each phrase's opening and
destination, with modal phrases retaining the characteristic chord even at two bars. Odd lengths
remain fully covered. The seed chooses repeatable takes within a build.
