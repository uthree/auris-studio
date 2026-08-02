# Auris Studio

A DAW for the AI Era — a digital audio workstation written in Rust, with a GPU-rendered
interface built on [gpui](https://crates.io/crates/gpui), the UI framework behind the Zed
editor.

Auris Studio is designed around one idea: **a sound source is just a trait implementation.**
Instruments and effects describe their own parameters, register themselves by id, and the
engine and UI pick them up without knowing anything else about them.

## Status

Early but functional. The pieces below work end to end: write notes in the piano roll, play
them through a built-in synth, shape them with effects, and render the result to a WAV file.

## Features

### Tracks

* **Instrument tracks** — notes on a timeline, played by a software instrument.
* **Audio tracks** — imported audio, arranged as clips with trim, gain and fades.

### Editing

Right-clicking any component opens its menu: tracks and clips offer duplicate, rename, delete
and mute; a clip adds split-at-playhead and cycle-over-clip; the piano roll offers duplicate,
delete and transposition of the selected notes; effect slots offer bypass, reorder and remove.
Renaming goes through the platform's input handler, so an IME composes into the field the way
it does anywhere else.

Creating and deleting are pointer gestures, and which gesture does which is a setting. The
defaults are Logic's — ⌘-click creates a note or a clip, a double-click deletes what is under
the pointer — with ⌥-click available for either. Dragging across empty space in the piano roll
or the arrangement sweeps a selection rectangle; ⇧-drag adds to what is already selected, and a
multiple selection moves, duplicates and deletes as one.

### Languages

The interface is available in English and Japanese, chosen under Settings → General or followed
from the system locale. Both frontends read the same preference, so the desktop application and
the command line tool answer in the same language.

Plugin names and parameters are translated where the term is known and left in the plugin
author's own wording where it is not, so a third-party plugin degrades to English rather than to
a missing-string marker.

### Automatic composition

`auris compose song.asong` writes a piece from a text specification: key, scale, tempo, mood,
chord progression, form and parts, in a line-oriented document that an agent can write and a
person can edit one line of.

```
title:  Neon Drive
key:    C minor
tempo:  128
mood:   driving
chords: @marusa
form:   intro verse chorus verse chorus outro

[section chorus]
bars: 8
intensity: 0.95
```

Progressions can be quoted from a catalogue by name — `@marusa` (丸サ進行), `@royal-road`
(王道進行), `@koakuma`, `@komuro`, `@canon`, `@junjo`, `@blues`, `@andalusian` and the rest — or
written out in roman numerals (`| IVmaj7 | III7 | vi7 | I7 |`) in any key. A quoted progression is
never recoloured, because the whole point of naming one is that it comes out sounding like
itself.

Everything is a pure function of the specification and its seed, so the same document always
writes the same piece and `--seed 7` writes a different one. `auris progressions` lists the
catalogue; `--set "field: value"` overrides any field from the command line.

### Built-in instruments

Deliberately simple chiptune voices, enough to hear the engine working:

| Id | Name | What it is |
| --- | --- | --- |
| `auris.synth.chiptune` | Chiptune | Sine / square / saw / triangle / LFSR noise with pulse width, ADSR, glide, unison and bit-crush |
| `auris.synth.fm2` | FM 2-Op | A two-operator FM voice, included to show a different synthesis method dropping in unchanged |
| `auris.synth.noisedrum` | Noise Drum | Pitch-swept noise through a band-pass, for percussion |

Square and saw are PolyBLEP band-limited, so high notes stay clean instead of aliasing.

### Built-in effects

| Id | Name | Notes |
| --- | --- | --- |
| `auris.fx.gain` | Gain & Pan | Constant-power pan law, stereo width, phase invert |
| `auris.fx.eq` | Equalizer | Six bands (HP, low shelf, two peaks, high shelf, LP) with a live response curve |
| `auris.fx.compressor` | Compressor | Soft knee, gain-reduction metering |
| `auris.fx.delay` | Delay | Ping-pong, damped feedback |
| `auris.fx.reverb` | Reverb | Freeverb-style comb/all-pass network |
| `auris.fx.distortion` | Distortion | Soft clip, hard clip, wavefolder, bitcrusher |
| `auris.fx.limiter` | Limiter | Lookahead, so the ceiling is actually a ceiling |

Effects can be chained on any track and on the master bus.

### Export

Render the whole project to a WAV file at 16-bit, 24-bit or 32-bit float, faster than
realtime. The renderer keeps going past the last clip for as long as the longest effect tail,
so reverb and delay decay naturally instead of being cut off.

### GPU acceleration

`auris-gpu` runs the large, embarrassingly parallel offline reductions on the GPU through
[wgpu](https://wgpu.rs): waveform min/max/RMS extraction for clip drawing, and whole-file
loudness analysis. Every kernel has a CPU fallback, and the application runs correctly with no
GPU present.

Realtime per-block DSP deliberately stays on the CPU — a round trip to the GPU costs more
latency than an entire audio block is allowed to take.

## Frontends

The backend is a set of crates that know nothing about any UI, and each frontend is a thin
layer on top of the same [`Session`](crates/auris-session) — one document, one engine, one
method per user command.

```bash
cargo run --release                 # the desktop application
cargo run --release -p auris-cli -- help
```

The command line tool exists as much to keep the split honest as to be useful: it drives the
identical session with no window and no audio device, so anything that leaks into the UI stops
compiling here.

```bash
auris compose song.asong -o song.auris         # write a piece from a specification
auris progressions                             # every chord progression known by name
auris plugins                                  # every registered instrument and effect
auris new song.auris --bpm 128
auris info song.auris                          # tracks, clips, duration
auris render song.auris -o song.wav --bit-depth 24
```

An MCP server is the next frontend and needs no new backend work — it is the same `Session`
API with a different transport in front of it.

## Building

```bash
cargo run --release
```

### macOS without a full Xcode install

gpui normally compiles its Metal shaders at build time by shelling out to `xcrun metal`, which
lives inside Xcode and is unreachable while `xcode-select -p` points at the Command Line
Tools. This project enables gpui's `runtime_shaders` feature instead, which compiles them
through the Metal framework at start-up — so the Command Line Tools are enough.

Nothing to configure; it is already set in `Cargo.toml`, and it is worth keeping even with
Xcode installed. The cost is one shader compile at launch; the benefit is that the project
builds on a machine without a 15 GB download.

## Layout

```
BACKEND — no UI dependency of any kind
  crates/auris-core     types, plugin traits, project model — no local dependencies at all
  crates/auris-dsp      effects and DSP primitives
  crates/auris-synth    built-in instruments
  crates/auris-engine   render graph, transport, cpal output, offline renderer
  crates/auris-io       audio file import/export, project save/load
  crates/auris-gpu      wgpu compute for offline analysis
  crates/auris-compose  score-based automatic composition: a text spec in, notes out
  crates/auris-i18n     interface text in every language, and nothing else
  crates/auris-session  the document, the engine and every command a frontend needs

FRONTEND
  crates/auris-gpui     the desktop application  (binary: auris-studio)
  crates/auris-cli      the command line tool    (binary: auris)
```

Dependencies run strictly downhill and the boundary is enforced by what each crate is allowed
to name. `auris-core` depends on nothing local. `auris-engine` does *not* depend on
`auris-dsp` or `auris-synth` — it drives plugins purely through the `auris-core` traits, which
is what keeps the plugin system honest. And `auris-gpui` depends on `auris-session` and gpui
and nothing else in the workspace: if it ever needs `auris-engine` directly, something that
belongs in the session layer has leaked into the UI.

### What lives where

Keeping the *rendering* backend UI-free is the easy part. The piece that usually leaks is the
orchestration around it — building the registry, rebuilding the render graph after an edit,
deciding which changes need a whole new graph and which fit in a command, tracking undo,
resolving a parameter to the plugin that owns it. All of that is in `auris-session`, so a
second frontend reuses it instead of reimplementing it slightly differently.

Gestures are handled with transactions: a pointer drag opens one, makes as many edits as it
likes, and closes it. The result is one undo step and one graph rebuild — and none of either
when the drag changed nothing.

## Adding a sound source

Implement two traits and register a factory. That is the whole procedure.

```rust
use auris_core::prelude::*;

struct MySynth { /* ... */ }

impl Parameterized for MySynth {
    fn parameters(&self) -> &[ParamDescriptor] { /* ... */ }
    fn param(&self, id: ParamId) -> f32 { /* ... */ }
    fn set_param(&mut self, id: ParamId, value: f32) { /* ... */ }
}

impl Instrument for MySynth {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            "mine.synth.example",
            "Example",
            "A worked example",
            PluginCategory::Synth,
        )
    }
    fn prepare(&mut self, ctx: &PrepareContext) { /* allocate here */ }
    fn reset(&mut self) { /* ... */ }
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        /* never allocate here */
    }
}

registry.register_instrument(|| Box::new(MySynth::new()));
```

The parameters you declare become UI controls automatically, with the right widget, range,
unit and scaling — the editor is generated from `ParamDescriptor`, not hand-written per plugin.
Effects work identically through the `Effect` trait.

## Known limitations

Honest list of what is not there yet, so nobody discovers these the hard way:

* **No plugin delay compensation.** `Effect::latency_frames` is reported but not acted on, so
  a latency-introducing effect shifts its track against the others. Every built-in effect is
  zero-latency except the limiter.
* **Effect tails take the maximum, not the sum.** A delay feeding a reverb exports a shorter
  tail than it should.
* **Audio sources are assumed to be at the render rate.** Import resamples to the project rate,
  but if the output device runs at a different rate the engine does not resample again.
* **Mute is a hard gate.** Toggling it during playback steps to zero within one sample rather
  than ramping, which can click.
* **Seeking into overlapping notes of the same pitch** re-triggers one note, so the first
  following note-off cuts it short.
* **No recording.** Audio tracks hold imported material only.

## Development

```bash
cargo test --workspace                    # unit tests
cargo clippy --workspace --all-targets    # lints
cargo fmt --all                           # formatting
```

## Licence

MIT OR Apache-2.0.
