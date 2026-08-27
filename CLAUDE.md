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

## The vendored synthesiser

`rustysynth` is a **fork**, kept in `vendor/rustysynth` and excluded from the workspace so that
`--workspace` does not hold somebody else's code to this project's lints and doc rules. The
published crate discards a SoundFont's modulator lists, which left the shipped font's pianos
playing through a filter nothing ever opened — twenty decibels down, and *falling* as the note was
struck harder. `vendor/rustysynth/README.md` is the account: what was added, what was deliberately
left out, and the measurement.

Its own tests run from its own directory. Two of the upstream ones fail there, because they want
SoundFont files the published crate does not ship.

## Layout

The rules below are the short form, kept here because they are needed on every task and a page
that has to be opened is a page that gets guessed at instead. The *account* — why each boundary is
where it is, the two threads, the realtime contract — is `auris_session::guide`, and that is where
it gets edited first: when the two disagree, the guide is right and this is stale.

```
Cargo.toml               virtual manifest; `default-members` points at the desktop app
vendor/rustysynth        somebody else's crate, forked — see its README; excluded from the workspace
crates/auris-core        types, music theory, plugin traits, project model — no local dependencies
crates/auris-dsp         effects and DSP primitives
crates/auris-synth       built-in chiptune instruments; depends on auris-dsp
crates/auris-sampler     SoundFont playback: the font bank and the sampler instrument;
                         depends on auris-dsp
crates/auris-clap        hosting of third-party CLAP plugins; depends on auris-core only
crates/auris-engine      render graph, transport, cpal in and out, offline renderer
crates/auris-io          audio file import/export, project save/load
crates/auris-gpu         optional wgpu compute for offline analysis
crates/auris-compose     score-based automatic composition; depends on auris-core only
crates/auris-vocal       singing: lyrics to IPA phonemes, notes to voice-model frames;
                         depends on auris-core only
crates/auris-i18n        interface text in every language; no local dependencies
crates/auris-session     headless session: the document, the engine, every command
crates/auris-gpui        desktop frontend (binary `auris-studio`)
crates/auris-cli         command line frontend (binary `auris`)
```

Dependency direction is strictly downhill and the frontend boundary matters:

* Nothing at or below `auris-session` may name a UI toolkit.
* `auris-engine` may not name `auris-dsp`, `auris-synth` or `auris-sampler`; it drives plugins
  through the `auris-core` traits only.
* Music theory (`auris_core::theory` — keys, scales, chords, roman numerals) lives in `auris-core`
  because the document holds a key and a chord progression, and the document model may not name a
  crate above it. `auris-compose` re-exports the module, so `crate::theory::…` still resolves there.
  It is the composer's vocabulary, not the composer's property.
* Both instrument crates take their primitives from `auris-dsp`, not their effects. `auris_dsp::Adsr`
  is the one that matters: the built-in voices and the sampler's per-note fade are the same
  generator, so an attack of five milliseconds means the same thing on both.
* A hosted CLAP plugin is **two objects**, because CLAP's main-thread/audio-thread split is the
  same one Auris already has. `auris_clap::ClapPlugin` is not `Send` and answers questions; the
  `ClapEffect` it hands out is `Send`, implements `auris_core::Effect`, and is the only half the
  graph ever sees. `auris-engine` therefore needs no CLAP dependency and does not have one. A
  hosted plugin cannot go through `PluginRegistry`, whose factory is `Fn() -> Box<dyn Effect>`
  and so cannot produce the main-thread half as well: the session places it instead.
* Sample data cannot travel through a `PluginState`, which is a map of `f32`. `auris-sampler`
  therefore keeps a `SoundFontBank` that the session owns and the registry's factory closure
  captures; a track names a sound by font id, bank and patch, never by position.
* `auris-gpui` and `auris-cli` depend on `auris-session`, `auris-i18n` and their own toolkit —
  nothing else in the workspace. `auris-i18n` is there because interface text is presentation and
  both frontends need it; it depends on nothing itself, so it adds no reach. A frontend naming
  `auris-engine`, `auris-core` or `auris-io` means logic that belongs in the session layer has
  leaked upward — move it down instead of adding the dependency.

New work that is a *command* (anything a user could ask for) goes in `auris-session` so every
frontend gets it. New work that is *presentation* stays in the frontend.

## The project folder

A project is a folder holding `MySong.auris` and an `Audio/` directory, and `Session::save_as`
creates it — choosing `MySong.auris` writes `MySong/MySong.auris`. The invariant that makes the
whole thing work is **one folder, one project**: two documents in one folder would share its
`Audio/`, and Save As would leave both pointing at the same files.

* An asset reference is an `AssetPath`, not a path. `Inside` is relative to the folder so the
  folder can be moved; `External` is absolute because nothing else would find it. `Inside` paths
  are stored with `/` separators on every platform — `Audio\kick.wav` is a *file called that* on
  a Mac, and a Windows-saved project would open there with silent tracks.
* Which one an asset gets is **policy**. Imported audio is copied in; a SoundFont is a library
  shared by every project and is left where it lies. `Session::collect_assets` is the opt-in that
  copies the fonts too, for archiving.
* Never `Path::join` a stored relative path — an absolute one would discard the folder.
  `AssetPath::resolve` rebuilds it component by component for exactly that reason.
* A file that has moved is searched for by name and confirmed by size, and what is found is
  written back into the document. Missing assets are reported, never fatal: the project opens
  with that one track silent.

Bump `Project::FORMAT_VERSION` whenever an older build could *misread* a newer file, and carry the
other direction with `serde(default)`. A new field carries backwards on a default; a new *variant*
of a stored enum does not, and is a bump. What the number is today, and why each bump happened, is
the constant's own doc comment — read it there rather than copying it here, where it would be wrong
by the next one.

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
* **The window is testable.** `auris_gpui::harness` opens the whole application with no display,
  no GPU and no audio device behind it, and drives it from `cargo test` — real keymap, real view
  tree, real session. A gesture is made as a gesture (press, move, release) and the document is
  asked what happened. `docs/development.md` has the account; the two limits are that nothing may
  assert on a pixel (`NoopTextSystem` gives every glyph the same metrics and the scene is thrown
  away) and that the transport is invisible (`is_playing` and the playhead are atomics the audio
  thread writes, and there is no audio thread). The second is why a decision still belongs in a
  free function even when a window test could nearly reach it.
* `cargo test -p auris-gpui --bins`. That crate is a binary, so `--lib` finds no target.

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
