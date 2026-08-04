# Changelog

Auris Studio is at `0.x`. **Nothing is stable there** — the project file format, the
configuration files, the key binding ids and every public API may change in any release, without
a migration path. The version number is the promise, and `0` is the promise that there is none.

The release workflow reads the section whose heading matches the tag, so the headings are the
format rather than a convention: `## <version> — <date>`.

## 0.1.0 — 2026-08-05

The first release. What is here works end to end: write notes, play them through a built-in
instrument, shape them with effects, and render the result to a WAV file.

### The window

* An arrangement of instrument and audio tracks, with a bar ruler, a cycle region and a playhead
  that scrolls itself into view while the transport rolls.
* A piano roll with two tools — pointer and velocity — a mixer with per-track strips and a
  master bus, an inspector, and a library that browses instruments, effects and SoundFonts as a
  tree.
* Every panel is docked to the left, the right or the bottom, and can be moved between them from
  its icon in the status bar. Where you leave them is where they are next launch.
* A command palette, a right-click menu on every component, and a menu bar — drawn by the
  application on Windows and Linux, the system's own on macOS. Both answer to the keyboard.
* Colour schemes, chosen in the settings window and checked for contrast by a test.
* English and Japanese throughout, following the system locale unless told otherwise.

### Sound

* Chiptune, two-operator FM and noise-drum instruments, all band-limited where it matters.
* Effects: gain, pan, delay, reverb, chorus, distortion, compressor, limiter and a parametric
  equalizer with a spectrum analyser.
* SoundFont playback: `.sf2` files are imported once and referenced by font, bank and patch, so a
  project stays small and opens playing the same sound.
* A realtime engine on cpal with a bounded command channel, plugin latency compensation, effect
  tails summed along the chain, and pre-built graphs handed over so nothing is dropped on the
  audio thread.
* Audio import through Symphonia, resampled when the device disagrees with the project.

### Writing music

* A harmony lane holding a key and a chord progression, editable on the timeline and audible by
  pressing or sweeping it.
* A structure lane naming the song's sections.
* Clips that write themselves from a preset and the harmony under them, and remember the recipe
  so they can be written again after the chords change.
* Whole-song composition from a text specification, from the desktop application or the command
  line.

### Files

* A project is a folder holding `<name>.auris` and an `Audio/` directory. Imported audio is
  copied in; assets are found again by name and size when they move, and a missing one costs that
  track's sound rather than the whole document.
* Configuration lives in `~/.config/auris-studio/` on every platform, in four small JSON files a
  dotfiles repository can carry.
* WAV export at 16-bit, 24-bit or 32-bit float, for the whole project or for the cycle region.

### Frontends

* `auris-studio`, the desktop application, on macOS and Windows.
* `auris`, the command line tool, on macOS, Windows and Linux.
