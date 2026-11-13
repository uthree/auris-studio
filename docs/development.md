# Development

[日本語](development.ja.md)

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
  crates/auris-analysis offline music recognition with DSP and optional local models
  crates/auris-compose  score-based automatic composition: a text spec in, notes out
  crates/auris-vocal    singing: lyrics to IPA phonemes, notes to voice-model frames
  crates/auris-singer   singing: DiffSinger, LeapSinger and VOICEVOX, offline
  crates/auris-i18n     interface text in every language, and nothing else
  crates/auris-session  the document, the engine and every command a frontend needs
  crates/auris-toolbox  the commands as tools for a language model, shared by both model doors
  crates/auris-video    note-roll videos: frames in software, encoded by an installed ffmpeg

FRONTEND
  crates/auris-gpui     the desktop application  (binary: auris-studio)
  crates/auris-cli      the command line tool    (binary: auris)
  crates/auris-mcp      the Model Context Protocol server (binary: auris-mcp)
  crates/auris-agent    the model client: Ollama / OpenAI-compatible (library: background model worker)

ALONGSIDE THE APPLICATION
  crates/auris-update-helper  replaces the installed binary once the app has exited
                              (binary: auris-update-helper)

NOT THE WORKSPACE
  vendor/rustysynth     somebody else's crate, forked — see its own README
  vendor/vst3-host      VST3 host fork — see README-AURIS.md
  vendor/gpui-flow      mixer graph UI fork — see README-AURIS.md
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

## Building on each platform

`cargo run` and `cargo run --release` start the desktop with the native physical instruments
ready to play. See [physical instruments](physical-instruments.md) for their models and controls.

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

Build with the Rust MSVC toolchain and Visual Studio C++ Build Tools.
From a Visual Studio developer PowerShell terminal:

```powershell
cargo run
```

Windows builds use WASAPI by default. To include ASIO, enable the `asio` Cargo feature;
this also requires LLVM/Clang and the ASIO SDK:

```powershell
.\tools\setup-windows.ps1 -InstallLlvm
cargo run --features asio
```

All four frontends accept this feature, including `cargo run -p auris-cli --features asio`.
The Windows release workflow builds all four with `--release --features asio`. For a local
release build with the same audio backends, use `cargo build --release --locked --features asio`.
`--release` alone changes optimization settings and does not enable ASIO.

The setup script uses an existing LLVM installation or installs it with Chocolatey or WinGet.
It sets `LIBCLANG_PATH` for the current PowerShell session; run it again when opening a new
terminal. For a custom installation, use `-LlvmBin 'D:\LLVM\bin'`. CI's ASIO checks and
Windows release builds use the same script. CI tests both the default and ASIO configurations.

An `asio-sys` build failure reporting `Unable to find libclang` means this build dependency
is missing or `LIBCLANG_PATH` points to the wrong directory. The Visual Studio C++ workload
alone does not provide the `libclang.dll` required by [bindgen](https://rust-lang.github.io/rust-bindgen/requirements.html),
and Rust's bundled LLVM does not replace this dependency.

With `asio` enabled, CPAL downloads the ASIO SDK on the first build; set `CPAL_ASIO_DIR`
to an extracted SDK directory to use a local copy. The default build needs neither the
ASIO SDK nor LLVM for ASIO bindings.

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

`auris-gpu` runs on Vulkan on Windows. The plugin hosts use Windows 0.62 bindings;
released GPUI uses Windows 0.61. GPU analysis is optional and falls back to the CPU
when no adapter is available.

### macOS without a full Xcode install

macOS builds require 13.4 or later, matching the shipped Core ML ONNX Runtime. The workspace's
Cargo configuration sets that deployment target and the app bundle uses the same minimum
version. Releases are built for Apple Silicon only; Intel Macs are not supported.

gpui normally compiles its Metal shaders at build time by shelling out to `xcrun metal`, which
lives inside Xcode and is unreachable while `xcode-select -p` points at the Command Line
Tools. This project enables `gpui/runtime_shaders` instead, which compiles them
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

The development and test profiles trade some optimization for shorter build times. See
[build performance](build-performance.md) for the profile settings, CI caches and timing reports.

```bash
cargo test --workspace                    # unit tests
cargo clippy --workspace --all-targets    # lints
cargo fmt --all                           # formatting
cargo doc --workspace --no-deps --open    # the API documentation
```

The desktop links the `auris-agent` library and starts a cancellable background thread for
its Agent Panel. `cargo run` builds everything the panel needs. The worker owns an async
runtime and exchanges messages with the UI through channels; permissions and document edits
are handled on the UI thread. Stopping or replacing a conversation cancels pending model and
approval waits without blocking the UI. Provider model discovery also runs in a background
thread with a timeout. Neither path starts a process or changes the process working directory.

Every crate carries `#![warn(missing_docs)]` and CI builds the documentation with warnings denied,
so a public item without a doc comment and a link that does not resolve are both build failures.
That is also what keeps `auris_session::guide` honest: its examples are doctests, so the account
of the system cannot drift away from the system without the build saying so.


## Testing the window

Project tabs retain independent `AurisApp` entities inside `ProjectTabs`, the native window's
root view. Each document keeps its session, Undo history, selection, chat and background jobs.
Only the selected document is painted. The tab strip uses the shared themed buttons and scrolls
horizontally; Ctrl+Tab and Ctrl+Shift+Tab cycle tabs, and Command+W on macOS or Ctrl+W elsewhere
closes the current tab. New Project opens another start screen in the same window. Opening an
already-open project selects its existing tab. Window close checks every tab's unsaved changes;
application quit checks every document across all windows. Approval is tied to a document's
revision and is rechecked after asynchronous saves. Tab selection releases held preview notes
but does not transfer realtime audio; explicit audio commands do that, and recording blocks
transfer. `harness::open_tabs` exercises the complete container with isolated preferences.

The Window menu switches each dock panel between the main window and a separate native window.
The layout file preserves that choice, visibility and the return dock. Closing a detached panel
hides it; the View menu reopens it, and clearing its Window-menu check returns it to its dock.
Plugin controls, musical typing, analysis results and the timbre map also use native windows.
All of these views share one `AurisApp` and session, including selection, edits and Undo.

`auxiliary_window.rs` defers native window creation and removal until the current draw finishes.
GPUI draws a new window synchronously, so opening it while the document entity is borrowed would
re-enter that entity. Input routing is shared by every document window; transient menus and
text editors belong to the window that opened them. Each panel is rendered in only one window,
which also keeps its recorded gesture coordinates and text-input handlers unambiguous.

The project, Settings and voice setup windows share GPUI title-bar chrome in
`crates/auris-gpui/src/titlebar.rs`. Its shared window options keep the native frame transparent
on macOS and Windows, and request client decorations on Linux; `appears_transparent` alone has
no effect on Linux. Each view supplies content from its current theme. Linux window roots add
resize targets at their edges and corners when the compositor accepts client decorations,
excluding tiled edges and maximized/fullscreen windows. Interactive controls must remain beside
dedicated drag regions: on Windows, a parent drag hitbox would intercept its children's input. Native
window-control hitboxes retain Snap and maximize/restore behavior. The close button invokes the
main window's unsaved-document guard. Drag regions occlude ancestor focus handlers so those
handlers cannot prevent the native window-move action.

The layout takes its project identity from [Zed's title bar](https://github.com/zed-industries/zed/blob/main/crates/title_bar/src/title_bar.rs)
and its centered transport readouts from [Logic Pro's control bar](https://support.apple.com/en-euro/guide/logicpro/lgcp5bdd6d9d/mac).
The title bar holds project identity and the application-menu trigger. Playback and its
readouts share one inset surface in the control bar; editing controls and meters use a second
row in narrow windows.
Panel switches stay at the bottom edge, with labels when space permits. On Windows and Linux,
the application menu opens as an overlay from its trigger or F10. Arrow keys navigate it and
Escape returns focus to the invoking control. macOS retains the system menu bar.
Keep both title-bar and responsive transport heights in `AurisApp::chrome_height` so dock sizing
and initial timeline coordinates agree with the view tree. Check native dragging, resizing and
window controls on each platform; the headless harness exercises the application controls.

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

<a id="real-agent-window-tests"></a>
### Real-agent window tests

The ignored window tests that drive an installed agent read their configuration from
`AURIS_TOOL_SURFACE_EVAL`, a path to JSON containing `backend`, `model`, an absolute fresh
`output` folder and `timeout_seconds`. Optional `provider`, `url`, `api_key_env`,
`context_tokens` and `thinking` configure the selected transport. The base test is
`ui::agent_chat::tool_surface_eval::installed_agent_edits_and_exports_the_unsaved_document`;
the arrangement and breathing harnesses add their own tests on the same configuration.

## Live audio visualizer

Open **Window > Audio Visualizer** to inspect a strip. The window is the Atelier analyser: a
spectrum beside a loudness meter, a spectrogram beside a goniometer, and an oscilloscope under
them, each a card drawn with `atelier::viz`. The toolbar chooses **Master** or the **Selected
track**, and **Pre-fader** (after the chain, before fader, pan and mute) or **Post-fader** (what the
strip sends on). The EQ editor has an independent tap, so both windows can remain open on
different strips. Every control is also in the command palette. Below 900 px the cards stack in
one column.

The spectrum is sixty sixth-octave bars from 20 Hz to 20.48 kHz, exactly ten octaves so each grid
division is one, over 72 dB. It is taken over 4096 samples (85 ms at 48 kHz), four times the EQ
editor's window, and a band too narrow to hold a bin of its own shows the nearest bin
(`auris_dsp::bands_from_bins_nearest`) rather than a hole. It can show the equal-power stereo
average, either channel, the larger L/R value in each bin, or the mono sum. **Average** smooths
power with a time-based 200 ms response, so delayed repaints do not change its meaning. **Peak
hold** keeps the largest observed value per band. **Save comparison** draws the displayed spectrum
as a line over the bars, and **Clear comparison / peaks** starts again.

The loudness card is an EBU R128 meter (`auris_dsp::LoudnessMeter`): momentary (400 ms),
short-term (3 s), integrated and loudness range since the last reset, and the four-times
oversampled true peak, all with the same K-weighting and gates as the offline `integrated_lufs`.
It measures every sample: the session reads what the scope has published since its last read
(`Scope::read_new`) on every repaint, frozen or not, from a ring of 32768 frames, so only a window
that goes two thirds of a second without repainting loses any. Changing source or tap starts it
again. The badge compares the integrated loudness with the −14 LUFS the streaming services
normalise to (±1 LU) and the true peak with their −1 dBTP ceiling. The two bars beside it are
each channel's sample peak, held for two seconds.

The spectrogram keeps the last three seconds in 25 ms columns, high frequencies on top, on the
theme's heat ramp.

The oscilloscope overlays the left and right channels. Its automatic trigger aligns a rising zero
crossing from the louder channel, keeping periodic signals stable without delaying or modifying
playback. **Time span** chooses one quarter, one half, or the full 1024-sample window; **Display
gain** 1×, 2×, 4× or automatic. Both change only the display.

The goniometer plots side horizontally and mid vertically: mono is a vertical trace, opposite
polarity a horizontal one. Optional automatic gain affects only the display. The correlation bar
grows from the centre towards −1 or +1; the lowest value is held until **Reset correlation**, and an
undefined value (one or both channels silent) is shown as a dash. Negative correlation is a reason
to listen in mono, not an automatic assessment of musical quality.

**Freeze** holds every card, the loudness numbers included, while the loudness meter goes on
measuring underneath.

For CLAP playback stalls, parameter-display snapshots and the reproducible concurrent
playback probe, see [Hosted plugin playback performance](hosted-plugin-performance.md).
