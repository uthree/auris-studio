# Changelog

Auris Studio is at `0.x`. **Nothing is stable there** — the project file format, the
configuration files, the key binding ids and every public API may change in any release, without
a migration path. The version number is the promise, and `0` is the promise that there is none.

The release workflow reads the section whose heading matches the tag, so the headings are the
format rather than a convention: `## <version> — <date>`.

## Unreleased

### Both edges of a clip are handles

* The pointer becomes a ↔ over one, so the grab can be seen before it is tried. Nothing on screen
  said the edges could be taken hold of, which made the whole gesture something you had to already
  know about. The zone the arrow lights up is the zone the press acts on, tested rather than
  trusted — including the band an audio clip gives to its fade handles, which the arrow stays out
  of because a press there takes a fade instead.
* A note's end in the piano roll gets the same arrow, and holds it back while the velocity tool is
  in hand, since that tool drags a note's velocity rather than its length.

* Dragging a clip's **left** edge trims its front instead of moving it. An audio clip's window
  walks into its source, so the material stays where it sounds and dragging back out uncovers what
  was hidden; a played clip's notes are rebased, keeping the sounding half of anything the cut runs
  through.
* A clip that **wrote itself** is written again at its new length, from either edge. It used to
  gain a tail of silence when pulled out and keep notes hanging past its own end when pulled in.
* An audio clip's edge now stops where its material does. Past the last frame it drew — and
  saved — a stretch of silence with the waveform ending part way, which the renderer clamped
  anyway: the picture and the sound disagreed.

### The time signature changes along the song

* The document's one time signature is now a map over the timeline, beside the tempo map, the
  harmony and the structure. A change is written from the ruler's right-click menu and lands on a
  bar line — a meter beginning mid-bar would leave that bar with no length and every bar number
  after it uncountable — and the ruler, the grid, the piano roll, the position readout and every
  command that counts bars follow it.
* The transport bar's centre now holds three readouts rather than two: position, tempo and
  signature. The signature shows the meter the playhead is in; clicking it drops the common
  meters, with *Other…* for anything else the notation holds.
* Editing the meter moves the bar lines and not one sample. Notes, clips, chords and sections are
  stored in ticks, so nothing under the ruler moves when the ruler is renumbered.

### The command palette does more

* Four commands that only the mouse could reach are now bindable, on the menus and in the palette:
  Tempo…, Time Signature…, Next Grid Division and Go to Position…
* The palette can set a value and not only fire a command. Type `1/16` for the editing grid, `6/8`
  for the meter, a colour scheme's name, or `日本語` to switch language — the languages listed in
  themselves, since the person opening that list is the one who cannot read the current one.

### Compatibility

* `Project::FORMAT_VERSION` is 4. A version 3 document opens with every note, clip and chord
  intact and comes up in 4/4, because the field changed shape rather than gaining a sibling. A
  document written in 3/4 by `auris compose` under 0.1.0 opens in 4/4 and wants its meter set
  again.
* `TempoMap::bar_beat_at` is gone; the arithmetic lives on `SignatureMap`, which is where bars
  were always decided. `Project::time_signature` is now `Project::signatures`, and
  `Session::harmony_grid` is now `Session::harmony_grid_at`, since which note takes the beat
  depends on where you are.

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
