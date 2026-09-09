# Composing from an idea

In the song sheet, choose a mood, major or minor, and a speed, then create the song.
For example, **Dark**, **Minor**, and **Slow** asks for a dark minor-key song at 80 BPM.
**Follow the mood** chooses minor for darker moods and major for brighter ones, and picks
a slow, moderate, or fast tempo from the mood's energy. An explicit mode stays selected when
the mood changes. The style chooses the instruments and form, using newly generated harmony.

The same request works in a `.asong` file through the CLI or composition tools:

```toml
style = "pop-band"
mood = "dark"
tonality = "minor"
pace = "slow"
```

`tonality` accepts `auto`, `major`, or `minor`. `pace` accepts `auto`, `slow`, `moderate`, or
`fast` (80, 120, or 160 BPM). Omitting them when naming a mood uses `auto`.
An explicit `key` or `scale` wins over `tonality`; an explicit `tempo` wins over `pace`.
`chords` supplies an explicit progression if desired.

The advanced view exposes exact key, chord progressions, arrangement, and performance controls.
Saved specifications contain the resolved settings, so reopening a song retains the key and
tempo it was saved with. Choose **Follow the mood** again to make either automatic.

## How harmony is generated

The generator in `auris-compose/src/progression.rs` has three steps:

1. Split a section into balanced phrases of at most four bars.
2. Choose a complete phrase from six major or six minor phrases. Repeat the opening phrase
   every other phrase, with independently chosen phrases between those repetitions.
3. In busier moods, optionally prepare V with ii in major or iv in minor within the same bar.

The frame planner adds chord colour and decides whether a section needs a dominant arrival.
All instruments read that completed plan. Short sections retain each phrase's opening and
destination; odd lengths remain fully covered. The seed chooses repeatable takes within a build.
