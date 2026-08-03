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

**File → Compose from Specification…**, or `auris compose song.asong`, writes a piece from a text
specification: key, scale, tempo, mood, chord progression, form and parts, in a line-oriented
document that an agent can write and a person can edit one line of. The whole piece arrives as a
single undo step, so a composition that is not what was wanted is one press away from the document
that was there before it.

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

Each part is built from one short figure invented per section and then restated bar after bar,
which is what gives a section something an ear can hold on to; the fourth bar of every phrase
answers it rather than repeating it again. A section played twice is the same section both times —
`variation: 0.4` buys back as much departure as you want, and `variation: 0` makes a second chorus
note for note the first. A section that another follows runs a fill into it, and every part leans
gently across a phrase rather than sitting at one level throughout.

Everything is a pure function of the specification and its seed, so the same document always
writes the same piece and `--seed 7` writes a different one. Every decision draws from a stream
addressed by name rather than by call order, so changing the drum density does not silently
rewrite the melody. `auris progressions` lists the catalogue; `--set "field: value"` overrides any
field from the command line.

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

Effects can be chained on any track and on the master bus. A chain that looks ahead — the limiter
does — hands its audio back late, so every other track is held back to match it and the parts stay
in step with each other. An export renders the resulting lead-in and drops it, so the file still
lines up with the timeline.

### Export

Render the whole project to a WAV file at 16-bit, 24-bit or 32-bit float, faster than
realtime. The renderer keeps going past the last clip for as long as the effects take to fall
silent — the tails along a chain add up rather than overlap, because a delay feeding a reverb
keeps feeding it for the whole of its own decay — so nothing is cut off.

An export can be written at any sample rate; the sources are converted to it first, so a project
exported at 96 kHz is the same piece rather than the same samples played faster.

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

macOS and Windows both build and run the desktop application; the backend and the command line
tool build anywhere Rust does. CI covers all three.

### Windows

Nothing to install beyond the Rust toolchain — audio goes through WASAPI and the window is
drawn by gpui's DirectX renderer, both of which come with the system.

Two differences are worth knowing about. Commands bound to ⌘ on macOS are bound to Ctrl here,
including the ⌘-click that places a note; the settings window shows whichever the keyboard in
front of it has. And the menu bar is drawn inside the window rather than by the system, because
Windows has no system menu bar to draw it.

wgpu's Direct3D 12 backend is switched off, so `auris-gpu` runs on Vulkan. It is optional
offline analysis and steps aside when no backend is present, so a machine with neither still
works — everything simply runs on the CPU. The backend does not compile at these versions:
`gpu-allocator` asks for `windows = ">=0.53, <=0.62"`, gpui pins `^0.61`, so the range resolves
to 0.61 while `wgpu-hal` itself uses 0.62 and the two disagree about what an `ID3D12Device` is.
Worth revisiting when either crate moves.

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

* **No recording.** Audio tracks hold imported material only.
* **Muting a track stops its effects.** The master bus keeps processing while muted, so its
  reverb rings out and un-muting does not pop; a track is skipped once its mute has faded, which
  is cheaper but cuts its tail off at the fade rather than letting it decay.
* **Delay compensation is measured, not predicted.** A parameter that changes a plugin's latency
  — the limiter's lookahead is the only one — is noticed after the fact rather than in advance, so
  the tracks are out of step from the moment it moves until the graph is rebuilt: the next frame
  for a single change, the end of the gesture for a drag.
* **Latency compensation is per track, not per clip.** The whole mix plays back as far behind the
  playhead as the longest chain needs, which is unavoidable, but it means turning on a
  look-ahead effect anywhere raises the monitoring latency everywhere.

## Development

```bash
cargo test --workspace                    # unit tests
cargo clippy --workspace --all-targets    # lints
cargo fmt --all                           # formatting
cargo doc --workspace --no-deps --open    # the API documentation
```

Every crate carries `#![warn(missing_docs)]` and CI builds the documentation with warnings denied,
so a public item without a doc comment and a link that does not resolve are both build failures.

The workspace has no root crate, so the account of how the eleven of them fit together lives in
`auris_session::guide` — it is the only crate that depends on every other, and so the only one
whose links to them all resolve. It covers the architecture and the layering rules, the realtime
contract, writing a plugin (with a worked example that is compiled as a test), the composition
format, and where the two platforms differ.

## Licence

MIT OR Apache-2.0.
