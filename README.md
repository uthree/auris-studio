# Auris Studio

A DAW for the AI Era — a digital audio workstation written in Rust, with a GPU-rendered
interface built on [gpui](https://crates.io/crates/gpui), the UI framework behind the Zed
editor.

Auris Studio is designed around one idea: **a sound source is just a trait implementation.**
Instruments and effects describe their own parameters, register themselves by id, and the
engine and UI pick them up without knowing anything else about them.

## Status

Early but functional. It works end to end: write notes in the piano roll, play them through a
built-in instrument or the shipped SoundFont, shape them with effects, arrange the result across
buses, and render it to a WAV file. A whole piece can also be written for you from a text
specification, and edited afterwards like anything else.

macOS and Windows both run the desktop application. The backend and the command line tool build
anywhere Rust does, and CI covers all three.

## What is here

* **Tracks** for instruments, for imported audio, and buses to mix them through, with sends,
  routing, and delay compensation that follows the routing rather than the track list.
* **A piano roll** with a velocity tool, pitch-bend and modulation strips, and clips whose edges
  trim rather than move.
* **Automation** for any parameter, on a lane beside the tempo, the meter, the key and the chords.
* **A harmony lane and a structure lane** — the piece knows its own key, its chord progression and
  the name of every section, and clips can be written from them.
* **Automatic composition** from a text specification or from one of eight whole-song presets,
  on the desktop or from the command line.
* **A General MIDI SoundFont in the box**, so there is something to play from the first launch.
* **Recording onto audio tracks**, with input monitoring, an arm that overrides the selection, and
  punch recording that replaces only the bars it was asked for.
* **MIDI files in and out**, audio import through Symphonia, WAV export at 16, 24 or 32-bit.
* **English and Japanese** throughout, following the system locale.

## Documentation

* [Features](docs/features.md) — what the application does, panel by panel.
* [Automatic composition](docs/composition.md) — the song specification, and clips that rewrite
  themselves when the chords under them change.
* [Development](docs/development.md) — the workspace layout, building on each platform, and
  adding an instrument or an effect.
* [Training a voice](https://github.com/uthree/auris-studio/tree/main/training) — `training/`
  is the Python project that trains the singing voices the DAW plays, and exports each one as a
  self-contained `.onnx`. It is not part of the Rust workspace and not part of a release.
* [CHANGELOG](CHANGELOG.md) — what changed, and what it broke.

The API documentation is `cargo doc --workspace --no-deps --open`. The workspace has no root
crate, so the account of how the eighteen of them fit together lives in `auris_session::guide`,
which is the only crate that depends on every other and so the only one whose links to them all
resolve.

## Downloads

Built binaries are on the [releases page](https://github.com/uthree/auris-studio/releases):
`Auris Studio.app`, `auris`, `auris-mcp` and `auris-agent` for macOS, as universal binaries for
Apple Silicon and Intel; the corresponding four `.exe`s for Windows; the `auris` command line
tool alone for Linux.

Every archive carries the shipped SoundFont, which is most of its size and all of the instruments
past the built-in four. On macOS it is inside the bundle, so dragging `Auris Studio.app` to
/Applications takes the sounds with it.

None of it is code-signed, so the first launch needs a word with the operating system. On macOS,
open the app from its right-click menu once rather than by double-clicking it, or run `xattr -dr
com.apple.quarantine "Auris Studio.app"`. On Windows, SmartScreen wants *More info → Run anyway*.

Auris Studio is at `0.x`, and [nothing is stable there](CHANGELOG.md): the project format, the
configuration files and every public API may change in any release, with no migration path.

## Building

```bash
tools/fetch-soundfonts.sh
cargo run --release
```

The first line is once per checkout and downloads the shipped SoundFont, which is not in the
repository — see [The SoundFont that comes with it](docs/features.md#the-soundfont-that-comes-with-it).
Skip it and everything still builds and runs; there are simply four instruments instead of a
hundred and thirty.

Platform notes — Windows needs nothing beyond the Rust toolchain, and macOS does not need a full
Xcode install — are in [Development](docs/development.md#building-on-each-platform).

## Known limitations

An honest list, so nobody discovers these the hard way:

* **No MIDI hardware input.** A keyboard plugged into the machine cannot play a track or record
  one; notes are written in the piano roll, imported from a `.mid`, or composed. Audio recording is
  a different thing and it is here.
* **What writes itself writes in one meter.** The timeline holds as many signature changes as you
  like, but a specification says `meter:` once, and a generated clip is built on the grid of the
  meter it starts in.
* **Muting a track stops its effects**, so its tail is cut off at the fade rather than left to
  decay. The master bus keeps processing while muted, so un-muting does not pop.
* **Delay compensation is measured, not predicted.** A parameter that changes a plugin's latency
  — the limiter's lookahead is the only one — is noticed after the fact, so the tracks are out of
  step from the moment it moves until the graph is rebuilt.
* **Latency compensation is per track, not per clip.** Turning on a look-ahead effect anywhere
  raises the monitoring latency everywhere.

## Licence

Apache-2.0. The full text is in [LICENSE](LICENSE), and it is the licence every release archive
carries.
