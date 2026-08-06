# Changelog

Auris Studio is at `0.x`. **Nothing is stable there** — the project file format, the
configuration files, the key binding ids and every public API may change in any release, without
a migration path. The version number is the promise, and `0` is the promise that there is none.

The release workflow reads the section whose heading matches the tag, so the headings are the
format rather than a convention: `## <version> — <date>`.

## Unreleased

### A General MIDI SoundFont comes with it

* **Auris Studio now ships with MuseScore General**, 128 instruments and a percussion bank under
  the MIT licence. It is in the library panel from the moment the window opens, with nothing to
  import. Five oscillators were enough to hear the engine working and never enough to write
  anything, and "install a SoundFont from somewhere" is not a first five minutes anybody enjoys.
* Not in this repository, because the file is two hundred megabytes — more than GitHub accepts in
  one piece and far more than every clone of a source tree should carry. `tools/fetch-soundfonts.sh`
  downloads it, checks it against a SHA-256, and installs it where the application looks; the
  release workflow runs the same script before it assembles each archive. What is version
  controlled is the manifest, in `auris_session::library`, which is the part worth reviewing.
* The script asks `auris soundfonts --manifest` what to fetch rather than keeping its own copy of
  the list. A URL recorded twice is a URL that goes stale in one of the two places.
* Putting the font in the document is deliberately *not* an edit: no undo step, no dirty flag, and
  a new project nobody has touched is still unmodified. It is what this installation has, the same
  way the built-in instruments are, and neither belongs in a history of what somebody did.
* The search for an asset that has moved now covers the library directories, so a project saved on
  one machine and opened on another finds that machine's copy of the shipped font and writes the
  new path back. The reference most likely to break when a project is sent to somebody else is
  also the only one that always has an answer.
* A build with nothing installed starts, runs and composes on the built-in instruments — which is
  what CI does on every commit.

### The built-in instruments have a vibrato

* **Vibrato Rate**, **Vibrato** and **Mod Depth** on the chiptune and the FM voice, and
  `NoteEvent::Modulation` — MIDI controller 1 — for a wheel to reach them by. The sampler passes
  it to the font, which is where a General MIDI set already has it wired to a vibrato of its own.
* `Vibrato` is zero by default, so a patch nobody has touched sounds exactly as it did before this
  existed and every piece already written is unchanged. `Mod Depth` is *not* zero — half a
  semitone, what a mod wheel does on almost every synthesiser ever sold — because a wheel that
  does nothing until a parameter is found is a wheel nobody discovers.
* One LFO per voice, restarted at each note on, so a chord struck together wobbles together. A
  single instrument-wide one would have every note somewhere different in its cycle, and the chord
  would arrive detuned by however far the wheel happened to be up. It keeps running while the
  depth is zero, so turning the wheel up mid-note picks the cycle up rather than jumping.
* A modulation rate now reads as `5.5 Hz` rather than `6 Hz`: below hearing the useful range is one
  decade wide, and rounded to a whole number half of it reads the same.

### The log has somewhere to go, and the release build has no terminal

* **View → Log** (`⌘⌥L`) opens a panel holding the last five hundred records the application
  wrote. Off by default, remembered in `layout.json`. A DAW is meant to fail quietly — a moved
  SoundFont costs one track its sound rather than the session — and every one of those quiet
  failures was logged to a terminal nobody was looking at. A track went silent and said nothing.
* Newest first, because the reason anybody opened it is the thing that just happened. **The log's
  status-bar icon turns amber** while there is a warning or an error nobody has read, which is the
  only part of this a person who never opens the panel will see.
* **A release build no longer opens a console window.** `windows_subsystem = "windows"`, so
  double-clicking `auris-studio.exe` gives the window and nothing else — where before it gave a
  black terminal beside the application, and closing that terminal closed the application. A debug
  build keeps its console, because `cargo run` and `RUST_LOG` are how this is worked on.
* The recorder sits in *front* of `env_logger` rather than instead of it, so the terminal and the
  panel can never disagree about what was logged.

### Eight whole songs to start from

* **Style** is the first row of the song sheet, and choosing one fills the rest of it: `chiptune`,
  `pop-band`, `city-pop`, `rock`, `jazz-trio`, `orchestral`, `synthwave`, `ambient`. Around thirty
  dials was a lot to be asked for before anything had made a sound, and knowing which of them
  matter is exactly what somebody opening a composer for the first time does not know.
* A style replaces the *whole* sheet — tempo, key, groove, progression, form and roster — because
  half a style is the arrangement of one at the tempo of another, which is not a style at all.
* `auris presets` lists them and **`auris compose --preset city-pop`** writes one with no file at
  all. Every other option means the same thing either way, because a named style and a file both
  arrive as the same text.
* Each preset is a `.asong` document embedded in the build rather than a structure assembled in
  code. A preset is meant to be read, the format was designed to be the readable one, and it makes
  the presets parser tests that fail loudly rather than silently.
* The part row's instrument picker now offers the General MIDI sounds, grouped into the sixteen
  families the standard already divides them into — a hundred and twenty-eight names in one menu
  is a menu nobody can read, and it would be taller than the screen. A drum part is offered the
  eight kits instead. Choosing a plugin clears the program, so the row never says one thing while
  the piece plays another.

### A composed part can ask for a real instrument

* **`program = "String Ensemble 1"` in a `.asong`** puts that part on the shipped SoundFont. By
  name — read case-, space- and punctuation-insensitively — or by number, for anybody working
  from a font's own listing. The composer had no way to name a SoundFont sound at all: an
  instrument was a plugin id, and a SoundFont's sounds do not have those.
* A part may carry `program` *and* `instrument`, and that is deliberate. The program is played
  where there is a font to play it from and the plugin is the fallback where there is not, so a
  specification asking for strings on a build with no library comes out as an oscillator rather
  than as silence — and the compose report names the missing library, so it is clear why.
* **On a drum part the same field is a kit**, because in General MIDI it is: the patch selects the
  whole kit and the note number picks the drum. Which of the two readings a number gets is never
  guessed at, because the role has already said — and a kit writes itself back out as
  `"TR-808 Kit"` rather than as whatever guitar shares its number.
* `auris compose --print` and the composed-track listing now name the *sound*, not the plugin the
  part would have fallen back to.

### A composed song arrives as a whole document

* **A composed piece now carries its own harmony and its own structure.** Both were computed and
  then dropped: the composer resolves a key and a full chord progression per section and names
  every stretch, and a composed song opened with an empty harmony lane and an empty structure lane
  over a piece that plainly has chords and sections. Worse than cosmetic, because a clip generated
  afterwards *reads* both — a part added by hand to a finished song had nothing to agree with.
* A key change is written only where the key changes, so a song in one key throughout has one
  point rather than one per section. Past the last bar the harmony and the structure both end,
  rather than the final chord and the outro running on for ever.
* **The drums can be asked for one voice at a time.** `Kick`, `Snare` and `Hat` join `Drums` in
  the part picker, so a hi-hat can be written onto a track of its own. A kit on one track is three
  voices no fader can separate; three tracks is a mix. `Project::FORMAT_VERSION` is 7.
* **A composed piece arrives with a rough mix.** The kit goes under one drum fader, the pitched
  parts share a room fed by sends, and the parts are spread across the stereo image instead of
  stacked in the middle. What stays centred is what a listener localises the song by — the tune,
  the bass and the kick — and nothing goes hard over, because a part at the edge of the image
  disappears on a phone. It is not a substitute for mixing; it is the ten minutes a person would
  have spent setting up before they could hear whether the piece was any good.
* More room means further away, which is the whole of the send ordering: the pad is furthest back
  because being a wash is what makes it a bed, and the tune is nearest. The bass and the kick get
  none at all — low frequencies in a reverb are mud. The reverb on the bus is set fully wet, which
  is the one setting a send/return reverb cannot be left at its default for.
* Not a note of what the composer writes moved: its tests compare whole pieces chord by chord and
  note by note, and they pass unchanged.

### Tracks can be dragged into order

* **Drag a track header up or down** to move it in the list. The arrangement reorders as the
  pointer moves rather than drawing a line and jumping on release, so what follows the hand is the
  arrangement itself. The whole drag is one undo step and one graph rebuild — a reorder is
  structural, and rebuilding on every pointer move would instantiate every plugin in the project a
  hundred times across one gesture.
* A press that does not travel is still a selection, and a press that lands on the header's fader,
  pan or mute keeps its own gesture: the header is the fallback grab, not the first claim.
* Only the list moves. Automation lanes, a routing output and a send all name a track by id, so a
  bus can end up above the tracks feeding it and nothing about the mix changes.
* **Fixed: an open automation lane pushed every header below it out of register with its track.**
  The lane column grew a row and the header column did not. The headers are now built from the same
  row walk the canvas uses, so the two cannot disagree, and the band beside an open lane carries
  the automated parameter's name.

### Buses and sends

* **A track no longer has to go to the master.** Its output is the master or a **bus**, and it can
  carry any number of **sends** — taps that feed a bus *as well as* wherever it goes itself. One
  reverb shared by six tracks is six sends; one fader over a whole drum kit is six outputs.
* A bus is a track kind rather than a thing of its own, so it has a fader, a pan, a mute, an effect
  chain, a colour and an automation lane without any of them being written twice, and every command
  that addresses a strip by track id addresses it too. What it has instead of clips is whatever is
  routed into it.
* Every mixer strip says where it goes; clicking that name offers the legal destinations, and the
  **+** beside it adds a send. A send's level is a mixer control like a fader — it drags, takes the
  wheel, resets on a double click and can be automated. Right-clicking one moves its tap before the
  fader or takes it away.
* **Solo travels both ways along the routing.** Soloing a drum track leaves the drum bus open, or
  its audio has nowhere to go; soloing the drum bus leaves the drum tracks open, or a thing with no
  material of its own plays silence.
* A route that would loop back on itself is refused, and the picker only ever offers destinations
  that would not. A file that holds one — nothing here can write one — is repaired on open with a
  line in the log, rather than refused.
* **Plugin delay compensation now follows the routing rather than the track list.** A limiter on a
  bus holds back the tracks that do *not* pass through it. Each outgoing copy of a track gets a
  delay of its own on top, so a track feeding the master dry while sending to that same bus has the
  dry and the wet arrive together instead of comb-filtering each other. Effect tails add up along a
  path the same way, so an export of a track ringing into a bus ringing into the master keeps going
  for all three.
* `Project::FORMAT_VERSION` is 6. A version 5 file opens; a version 6 file does not open in an
  older build, which is the point — the fields would be *ignored* rather than rejected, and a mix
  where six tracks feed one reverb would come up with all six routed dry and be saved back that way.

### MIDI files go in and out

* **File → Import MIDI File…**, or dropping a `.mid` on the window, reads it as a new piece: its
  tempo map, its meter, and one track per part. A new document rather than tracks added to the open
  one, because a MIDI file brings its own clock — its notes in a piece running at a different speed
  would be the right notes at the wrong lengths, with nothing on screen to say why. Unsaved work is
  asked about first, exactly as it is for Open.
* **File → Export MIDI File…** writes the other direction, at 960 ticks to the quarter note, so a
  piece that leaves and comes back has every note in the same place. A tempo does not survive
  exactly: MIDI stores whole microseconds per quarter, so 144 bpm returns as 143.999 88, while 96
  and 120 divide evenly and return exact.
* Four things real files do that a naive reader gets wrong are handled and tested: a note-on at
  zero velocity is a note-off; the same pitch struck twice before either release is two notes; a
  note nobody released is closed where its track ends; and two channels in one track are two
  instruments, which is what a format 0 file always is. A part on **channel 10** gets the
  noise-drum instrument.
* A file counted in **SMPTE frames** is refused rather than guessed at. It has no beats, so it has
  no bars, and putting it on a musical timeline would mean choosing a tempo on its behalf.
* What a `.mid` has nowhere to put, in either direction: audio tracks, the mixer, which instrument
  each track plays, and the automation.
* `MidiClip::playable_notes` is new, and the renderer now asks it too. Which notes a clip actually
  plays was written inline in the scheduler; a second copy in the exporter would have drifted into
  a file that is not the piece you can hear.

### Two things the backend could already do and nothing could ask for

* **A track's colour can be chosen.** It was picked from a palette by the track's position and
  then fixed there for good — and the order tracks were made in has nothing to do with which of
  them are drums. The track's right-click menu now offers the palette as swatches. Numbered rather
  than named, because the set holds two entries a reasonable person would call orange.
* **A whole track can be frozen.** *Keep Every Take Here* drops every recipe on it, so nothing on
  that track is written again when the chords underneath change. `Session::freeze_track` had been
  implemented and tested for some time with no way to reach it; the clip-level command was the only
  one on a menu. The status line reports how many clips it acted on, because a track reaches
  further down than the panel shows.

### Parameters move along the timeline

* The document holds **automation**: a curve per parameter, beside the tempo, the meter, the key
  and the chords. Right-click a track header for *Automate Volume* or *Automate Pan* and a lane
  opens under it; a press on empty lane writes a point and starts dragging it, the delete gesture
  takes one off, and a drag is one undo step.
* A parameter with no lane is **not automated at all** and keeps its stored value. Only an existing
  lane takes over, which is what lets a mix be automated one control at a time — and taking the
  last point off gives the parameter back.
* A lane is **not anchored at the start of the song**. It holds its nearest value flat outside the
  stretch it was written over, because it makes a claim about that stretch and none about the rest.
  A tempo has to be defined from the first sample; a filter cutoff does not.
* A lane carries how to get between its points. A fader runs in a straight line; a parameter with
  discrete positions **holds**, because interpolating a waveform chooser would sweep through every
  option between two settings and sound all of them on the way.
* Playback and export take the same path. Seeking or looping arrives at the values under the
  playhead rather than sliding to them — landing in the middle of a fade used to swell up to it
  from wherever the fader had been left.
* `Project::FORMAT_VERSION` is 5. This is a new field with a default, which normally does not move
  the version, but the direction that matters is the other one: an older build ignores a field it
  does not know, so it would open an automated mix, play it at the wrong levels, and write those
  levels back on the next save. Refusing to open is the only honest answer.
* `ParamTarget` moved from `auris-session` to `auris_core::param`, because a lane is a target and
  a shape and the document may not name a crate above it. `auris_session::param` re-exports it, so
  the old path still resolves.

### Files can be dragged into the window

* An audio file dropped on the window arrives on a **new audio track**; an `.sf2` goes on the
  library's shelf with the font opened where its sounds are chosen; a `.auris` project **opens**.
  All three were reachable only through a File menu that a person has to already know is there.
* A dropped project goes through the same unsaved-work guard the Open command does, and the guard
  carries the dropped path — answering *Save* saves what is open and then opens the one that was
  dropped, rather than saving and then asking again which file was meant.
* A project has to be dropped on its own. It is a document rather than something that goes into
  one, so a drop holding a project and three takes has no reading that does not risk the work on
  screen — import into a document about to be replaced, or replace the document the takes were
  meant for. Two projects have the same problem and no tie-break at all, so the whole drop is
  refused with a line saying why, and the border stays dark while it is still in the air.
* A drop is understood by what the file is rather than by where it was let go, so there is no
  target to aim at — the window takes it over the lanes, the mixer or the library alike. What the
  position decides is when: audio dropped on the lanes starts there, snapped to the grid the way a
  dragged clip is, and audio dropped anywhere else starts at the playhead.
* Several files at once are read in the order they were dragged, one at a time with the status
  line naming each, and a drop that only partly arrives says how many did and how many did not. A
  border lights up while a drag holding something readable is over the window, so a folder or a
  PDF says beforehand that it will not be understood.
* Importing audio now scrolls to the track it made, from the File menu as well as from a drop. On
  an arrangement taller than the window it was landing out of sight.

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
