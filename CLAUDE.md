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

## Platforms

macOS and Windows both run the desktop application, and CI builds the whole workspace on both.
Development happens on macOS, so the Windows-only paths are the ones that rot; the rules that
keep them alive:

* **Never name a platform key.** gpui's `Modifiers::platform` is ⌘ on macOS and the *Windows
  key* on Windows, which the shell claims first. Read `Modifiers::secondary()`, and write
  `secondary-` in a keystroke, never `cmd-`.
* **Decide with `cfg!`, not `#[cfg]`,** wherever it is a choice rather than an API that only
  exists on one platform. Both arms then compile and their tests run everywhere, which is the
  only reason the Windows menu bar can be checked from a Mac.
* **The keystroke a user sees is not the keystroke that is stored.** `secondary-s` is stored;
  `actions::normalise_keystroke` turns it into what the keyboard reports, for comparing, and
  `actions::menu_keystroke` into ⌘S or Ctrl+S, for reading.
* **Windows sets no locale variables.** `Language::from_system_locale` is what makes a Japanese
  Windows install come up in Japanese.

wgpu's `dx12` backend is off because it does not compile at these versions — `gpu-allocator`
resolves `windows` to 0.61 while `wgpu-hal` uses 0.62. Windows runs `auris-gpu` on Vulkan.

## Layout

```
Cargo.toml               virtual manifest; `default-members` points at the desktop app
crates/auris-core        types, plugin traits, project model — no local dependencies
crates/auris-dsp         effects and DSP primitives
crates/auris-synth       built-in chiptune instruments
crates/auris-sampler     SoundFont playback: the font bank and the sampler instrument
crates/auris-engine      render graph, transport, cpal output, offline renderer
crates/auris-io          audio file import/export, project save/load
crates/auris-gpu         optional wgpu compute for offline analysis
crates/auris-compose     score-based automatic composition; depends on auris-core only
crates/auris-i18n        interface text in every language; no local dependencies
crates/auris-session     headless session: the document, the engine, every command
crates/auris-gpui        desktop frontend (binary `auris-studio`)
crates/auris-cli         command line frontend (binary `auris`)
```

Dependency direction is strictly downhill and the frontend boundary matters:

* Nothing at or below `auris-session` may name a UI toolkit.
* `auris-engine` may not name `auris-dsp`, `auris-synth` or `auris-sampler`; it drives plugins
  through the `auris-core` traits only.
* Sample data cannot travel through a `PluginState`, which is a map of `f32`. `auris-sampler`
  therefore keeps a `SoundFontBank` that the session owns and the registry's factory closure
  captures; a track names a sound by font id, bank and patch, never by position.
* `auris-gpui` and `auris-cli` depend on `auris-session` and their own toolkit, nothing else in
  the workspace. A frontend reaching for `auris-engine` means logic that belongs in the session
  layer has leaked upward — move it down instead of adding the dependency.

New work that is a *command* (anything a user could ask for) goes in `auris-session` so every
frontend gets it. New work that is *presentation* stays in the frontend.

## Conventions

* Comments, documentation and the README are written in English.
* Every public item carries a doc comment (`#![warn(missing_docs)]` is on in each crate). CI
  builds the docs with warnings denied, so a link that does not resolve fails the build too — a
  doc comment naming a private item wants backticks, not brackets.
* The workspace has no root crate, so the account of how the crates fit together is
  `auris_session::guide`. That is the only crate depending on every other, and so the only place
  intra-doc links to them all resolve. Anything about the system as a whole belongs there.
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
cargo run                                   # launch the DAW (default-members)
cargo run -p auris-cli -- help              # the command line frontend
cargo test --workspace                      # all tests
cargo clippy --workspace --all-targets      # lints
cargo doc --workspace --no-deps --open      # the API documentation
```
