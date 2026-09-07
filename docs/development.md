# Development

How the workspace is laid out, how to build it on each platform, and how to add an
instrument or an effect.

## Where things are

```
BACKEND — no UI dependency of any kind
  crates/auris-core     types, music theory, plugin traits, project model — no local dependencies
  crates/auris-dsp      effects and DSP primitives
  crates/auris-synth    built-in instruments
  crates/auris-sampler  SoundFont playback: the font bank and the sampler instrument
  crates/auris-clap     hosting of third-party CLAP plugins — depends on auris-core only
  crates/auris-vst3     hosting of third-party VST3 plugins — depends on auris-core only
  crates/auris-engine   render graph, transport, cpal in and out, offline renderer
  crates/auris-io       audio file import/export, project save/load
  crates/auris-gpu      optional wgpu compute for offline analysis
  crates/auris-compose  score-based automatic composition: a text spec in, notes out
  crates/auris-vocal    singing: lyrics to IPA phonemes, notes to voice-model frames
  crates/auris-singer   singing: those frames through an ONNX voice model, offline
  crates/auris-i18n     interface text in every language, and nothing else
  crates/auris-session  the document, the engine and every command a frontend needs
  crates/auris-toolbox  the commands as tools for a language model, shared by both model doors

FRONTEND
  crates/auris-gpui     the desktop application  (binary: auris-studio)
  crates/auris-cli      the command line tool    (binary: auris)
  crates/auris-mcp      the Model Context Protocol server (binary: auris-mcp)
  crates/auris-agent    the model client: Ollama / OpenAI-compatible (binary: auris-agent)

NOT THE WORKSPACE
  vendor/rustysynth     somebody else's crate, forked — see its own README
  vendor/midir          MIDI library fork — see README-AURIS.md
  training/             Python: what trains the voice models auris-singer plays
```

Dependencies run strictly downhill, and the boundary is enforced by what each crate is *allowed
to name* rather than by convention.

The tree above is a map of the repository. The account of how the workspace actually fits together
is **`auris_session::guide`** — the layering rules and why each boundary is where it is, the two
threads and how they hand work to each other, the realtime contract, writing a plugin, the
composition format, and where the two platforms differ.

```bash
cargo doc -p auris-session --no-deps --open
```

`auris-session` depends on every backend crate, so the guide can link directly to their API
documentation. Keep architectural explanations there and build instructions here.

## The voice models

`training/` is a Python project — PyTorch and Lightning — that trains the singing voices
`auris-singer` plays, and exports each one as a self-contained `.onnx`. It has its own `uv`
environment, its own `pyproject.toml` and its own `doc/`; `cargo` never sees it and it never
imports a crate.

It was a separate repository until it was not, and the reason for moving it is a test. A voice
file is a contract between two languages, and several halves of that contract were written down
twice — the metadata key, the format version, the reserved symbols, the phoneme table down to
which symbols are voiceless. Across two checkouts nothing could compare them, to the point where
`crates/auris-vocal/src/phoneme.rs` carried a comment asserting that its voiceless list matched
the other repository's symbol for symbol: true when written and unverified ever after.
`training/tests/test_host_contract.py` is that comment executable. It reads the Rust sources as
text — a pytest run must not need a Rust toolchain, nor `cargo test` a Python one — and fails
when the two sides drift.

```bash
cd training
uv venv --python 3.11
uv pip install -e '.[dev,export]' --torch-backend=auto
uv run pytest -m "not slow"
```

`--torch-backend=auto` reads the machine's NVIDIA driver and takes the matching PyTorch wheels,
so the one command serves the machine that trains and the machine that only runs the tests.
Training itself wants a card; everything else here is content on the CPU.
`training/doc/development.md` is the rest.

## Building on each platform

`cargo run` and `cargo run --release` start the desktop, which downloads a missing standard
SoundFont in the background and displays progress. The cached copy is shared by checkouts.
See [the sound library](features.md#the-soundfont-that-comes-with-it) for its location, manual
installation and disabling automatic downloads.

Release archives include the NAIST Japanese dictionary; a source checkout or new Git worktree
does not. Cargo builds code only. Fetch the dictionary once before trying kanji lyrics:

```powershell
# Windows / PowerShell
.\tools\fetch-dictionary.ps1
```

```bash
# macOS / Linux / Git Bash
tools/fetch-dictionary.sh
```

Both scripts read the pinned manifest from `auris dictionary --manifest`, verify the archive,
and install it with its license under `Dictionary/naist-jdic`. Restart the desktop after fetching;
it loads the dictionary automatically, without a path override in Settings. Use
`cargo run -p auris-cli -- dictionary` to check discovery. PowerShell also accepts
`-AurisPath .\target\debug\auris.exe` to use an already built CLI.

For a copy shared by worktrees, pass `-Destination "$HOME/.config/auris-studio/Dictionary"`
in PowerShell, or `"$HOME/.config/auris-studio/Dictionary"` to the Bash script. If
`AURIS_CONFIG_DIR` is set, use its `Dictionary` subdirectory instead. An explicit
`AURIS_DICTIONARY` points to the parent of `naist-jdic` and replaces the automatic search.

### Windows

Build with the Rust MSVC toolchain, Visual Studio C++ Build Tools, and LLVM/Clang.
From a Visual Studio developer PowerShell terminal, prepare LLVM and then build:

```powershell
.\tools\setup-windows.ps1 -InstallLlvm
cargo build --locked
```

The setup script uses an existing LLVM installation or installs it with Chocolatey or WinGet.
It sets `LIBCLANG_PATH` for the current PowerShell session; run it again when opening a new
terminal. For a custom installation, use `-LlvmBin 'D:\LLVM\bin'`. CI and release builds use
the same script.

An `asio-sys` build failure reporting `Unable to find libclang` means this build dependency
is missing or `LIBCLANG_PATH` points to the wrong directory. The Visual Studio C++ workload
alone does not provide the `libclang.dll` required by [bindgen](https://rust-lang.github.io/rust-bindgen/requirements.html),
and Rust's bundled LLVM does not replace this dependency.

Windows builds include WASAPI and ASIO. CPAL's ASIO dependency downloads the ASIO SDK on
the first build; set `CPAL_ASIO_DIR` to an extracted SDK directory to use a local copy.

In Settings → Audio, select the audio driver, device, sample rate and requested buffer size.
WASAPI uses shared mode and requests elevated audio thread priority. ASIO requires an installed
device driver and uses the same interface for input and output. Both directions share the
output's rate and buffer size. Switching drivers stops playback and reopens monitoring;
settings cannot change during a recording take.

The applied buffer readout comes from the running backend, and may differ from the requested
size when the driver clamps it or rejects a fixed size. Its duration is for one buffer, not
the time from a key press to sound or the input-to-output round trip. Start with 128 or 256
frames and increase the size if audio drops out. Measure end-to-end latency with a hardware
loopback when evaluating a particular interface.

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

## Adding a sound source

Implement two traits and register a factory. That is the whole procedure: `Parameterized`, which
declares the parameters and reads and writes them by id, and then `Instrument` or `Effect`. The
parameters you declare *become the editor* — the UI is generated from the descriptors, with the
right widget, range, unit and scaling, rather than hand-written per plugin.

The worked example is in **`auris_session::guide::plugins`**, along with what belongs in `prepare`
against what may happen in `process`, how to register a whole pack of them, and what to do when a
plugin needs something a factory closure cannot carry.

```bash
cargo doc -p auris-session --no-deps --open
```

It is there rather than here because it is **compiled**: the guide's example is a doctest and the
test suite runs it, so a plugin snippet that stopped matching the traits would fail the build
instead of quietly misleading whoever copied it.

## The commands

```bash
cargo test --workspace                    # unit tests
cargo clippy --workspace --all-targets    # lints
cargo fmt --all                           # formatting
cargo doc --workspace --no-deps --open    # the API documentation
```

The desktop application's agent panel runs `auris-agent` as a child process and looks for it
beside its own executable, which is where the release archive puts it. `cargo run` builds only
the desktop app (`default-members`), so after a fresh checkout the panel reports the binary as
missing until a `cargo build -p auris-agent` in the same profile puts it there.

Every crate carries `#![warn(missing_docs)]` and CI builds the documentation with warnings denied,
so a public item without a doc comment and a link that does not resolve are both build failures.
That is also what keeps `auris_session::guide` honest: its examples are doctests, so the account
of the system cannot drift away from the system without the build saying so.


## Testing the window

The project, Settings and voice setup windows share GPUI title-bar chrome in
`crates/auris-gpui/src/titlebar.rs`. Window options keep the native frame transparent, and each
view supplies content from its current theme. Interactive controls must remain beside dedicated
drag regions: on Windows, a parent drag hitbox would intercept its children's input. Native
window-control hitboxes retain Snap and maximize/restore behavior. The close button invokes the
main window's unsaved-document guard. Drag regions occlude ancestor focus handlers so those
handlers cannot prevent the native window-move action.

The layout takes its project identity from [Zed's title bar](https://github.com/zed-industries/zed/blob/main/crates/title_bar/src/title_bar.rs)
and its centered transport readouts from [Logic Pro's control bar](https://support.apple.com/en-euro/guide/logicpro/lgcp5bdd6d9d/mac).
When changing the title-bar height, include it in `AurisApp::chrome_height` so dock sizing and
initial timeline coordinates agree with the view tree. Check native dragging, resizing and
window controls on both platforms; the headless harness exercises the application controls.

`crates/auris-gpui/src/harness.rs` opens the whole application in a window with no display, no
GPU and no audio device behind it, and drives it from `cargo test`. gpui ships the platform that
makes this possible; this crate's dev-dependency on `gpui/test-support` is what switches it on.

```bash
cargo test -p auris-gpui --bins        # this crate is a binary, so `--lib` finds nothing
```

Everything except the pixels and the hardware is real — the real keymap, the real view tree, the
real session, the real commands — so a test presses keys, clicks controls by name and drags the
pointer across the lanes, then asks the document what happened. The helpers are `open`,
`with_a_clip`, `paint`, `click`, `choose`, `drag`, `lane_point` and `roll_point`; each carries a
doc comment saying what it is for and what it costs.

Two things it cannot check, both worth knowing before writing a test against it:

* **Nothing may assert on a pixel.** Text is laid out through `NoopTextSystem`, which gives every
  glyph the same metrics, and the test window throws the scene away instead of rasterising it.
  Colour, spacing and legibility stay a human's job; *behaviour* stops being one.
* **The transport is not observable.** `Session::is_playing` and the playhead are atomics the
  *audio thread* writes, and a session with no device has no audio thread — so Play and Seek are
  sent and nothing comes back. Assert on the document and on the view state, which are written
  where the command runs.

That second point is why the house rule about free functions matters as much as it did: a
decision the window cannot reach still belongs somewhere a test can. See
`crate::ui::context_menu::clips::splittable` for one that ended up there for exactly this reason.

Every `icon_button`, `button`, context-menu row and menu-bar row carries a name the harness can
find it by, from one line each. `debug_selector` compiles to nothing unless gpui is built with
`test-support`, which only `cargo test` does.
