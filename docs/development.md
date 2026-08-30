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
  crates/auris-engine   render graph, transport, cpal in and out, offline renderer
  crates/auris-io       audio file import/export, project save/load
  crates/auris-gpu      optional wgpu compute for offline analysis
  crates/auris-compose  score-based automatic composition: a text spec in, notes out
  crates/auris-i18n     interface text in every language, and nothing else
  crates/auris-session  the document, the engine and every command a frontend needs
  crates/auris-toolbox  the commands as tools for a language model, shared by both model doors

FRONTEND
  crates/auris-gpui     the desktop application  (binary: auris-studio)
  crates/auris-cli      the command line tool    (binary: auris)
  crates/auris-mcp      the Model Context Protocol server (binary: auris-mcp)
  crates/auris-agent    the model client: Ollama / OpenAI-compatible (binary: auris-agent)
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

It lives there because `auris-session` is the only crate depending on every other, and so the only
place a link to each of them resolves — which is also why it does not live here. A second copy of
it in this file would be wrong by the next crate anybody adds; the one line of this tree that had
already gone stale while it sat in the README is how that goes.

## Building on each platform

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
