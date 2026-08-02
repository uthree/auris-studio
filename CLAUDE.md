# Auris Studio — working notes

A digital audio workstation written in Rust, with a [gpui](https://crates.io/crates/gpui) UI.

## Build environment

gpui is depended on with its **`runtime_shaders`** feature, which compiles the Metal shaders
through the Metal framework at start-up instead of at build time. The build-time path shells
out to `xcrun metal`, which lives inside Xcode
(`Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/metal`) and is
therefore unreachable when `xcode-select -p` points at `/Library/Developer/CommandLineTools`.

Keep the feature even on a machine where Xcode *is* selected. It costs a one-off shader
compile at start-up and in exchange the project builds with nothing but the Command Line
Tools, which is worth far more than those milliseconds.

## Layout

```
Cargo.toml            workspace root; also the `auris-studio` binary (the gpui app)
src/                  UI code for the binary
crates/auris-core     types, plugin traits, project model — no backend dependencies
crates/auris-dsp      effects and DSP primitives
crates/auris-synth    built-in chiptune instruments
crates/auris-engine   render graph, transport, cpal output, offline renderer
crates/auris-io       audio file import/export, project save/load
crates/auris-gpu      optional wgpu compute for offline analysis
```

Dependency direction is strictly downhill: `core` depends on nothing local, everything else
depends on `core`, and only the binary depends on all of them.

## Conventions

* Comments, documentation and the README are written in English.
* Every public item carries a doc comment (`#![warn(missing_docs)]` is on in each crate).
* Run `cargo fmt --all` and `cargo clippy --workspace --all-targets` before committing.
* DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths)
  rather than on "it runs".

## Realtime rules

`Instrument::process` and `Effect::process` run on the CoreAudio callback thread. No
allocation, locking, blocking or I/O in those paths. Anything expensive goes in `prepare`,
which the engine calls from a normal thread. The engine communicates with the audio thread
through a bounded command channel and hands over ownership of pre-built graphs; the audio
thread returns replaced graphs down a second channel so they are dropped off the RT thread.

## Commands

```bash
cargo run                                   # launch the DAW
cargo test --workspace                      # all tests
cargo clippy --workspace --all-targets      # lints
```
