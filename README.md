# Auris Studio

A DAW for the AI Era — a digital audio workstation written in Rust, with a GPU-rendered
interface built on [gpui](https://crates.io/crates/gpui), the UI framework behind the Zed
editor.

Auris Studio is designed around one idea: **a sound source is just a trait implementation.**
Instruments and effects describe their own parameters, register themselves by id, and the
engine and UI pick them up without knowing anything else about them.

## Status

Early but functional. The pieces below work end to end: write notes in the piano roll, play
them through a built-in synth, shape them with effects, and render the result to a WAV file.

## Features

### Tracks

* **Instrument tracks** — notes on a timeline, played by a software instrument.
* **Audio tracks** — imported audio, arranged as clips with trim, gain and fades.

### Dragging files in

Drop an audio file on the window and it arrives on a **new audio track**. Drop an `.sf2` and it
goes on the library's shelf, with the font opened where its sounds are chosen. Drop a handful and
each is read in turn, in the order they were dragged, with the status line naming the one being
read — decoding is slow enough that a folder of takes would otherwise be several seconds of a
window with nothing to say.

Drop a **`.auris` project** and it opens, the same way **File → Open** would: unsaved work in the
document you have open is asked about first, and answering *Save* saves it and then opens the one
you dropped. A project has to arrive **on its own**, though — it is a document rather than
something that goes into one, so a drop holding a project and three takes has no reading that does
not risk the work on screen, and the whole drop is refused with a line saying why.

A drop is understood by **what the file is, not where it was let go**: the window takes it whether
the pointer is over the lanes, the mixer or the library, so there is no target to aim at and no
rule to learn first. Where it landed decides one thing — audio dropped on the lanes starts *there*,
snapped to the grid the way a dragged clip would be, and audio dropped anywhere else starts at the
playhead. A border lights up while a drag holding something readable is over the window, so a
folder or a PDF says beforehand that it will not be understood.

### Panels, and where you put them

The arrangement is the middle of the window. Everything else — the library, the piano roll, the
mixer, the inspector — is a panel, and every panel lives in one of three docks: a column down the
left, a column down the right, or the strip along the bottom. They start where a DAW puts them,
library left, inspector right, the two editors sharing the bottom, and none of that is fixed.

The status bar carries a small icon for every panel, grouped by the dock it belongs to: the left
dock's at the left-hand end, the bottom dock's and then the right dock's at the other. Clicking one
shows that panel, clicking the one already showing shuts its dock, and right-clicking offers **Dock
Left**, **Dock Bottom** and **Dock Right** — so the mixer can be a right-hand column with the roll
still along the bottom, or both editors on the bottom as tabs. A dock shows one panel at a time,
which is what keeps a 240-pixel column from becoming two half-panels; the panel it is *not* showing
still has its icon, so nothing can be put away and lost.

Each dock's divider drags to resize it, and no drag can squeeze the arrangement out of existence.
Where you leave the panels is where they are next launch — the whole arrangement is written to
`layout.json`.

### Editing

Right-clicking any component opens its menu: tracks and clips offer duplicate, rename, delete
and mute; a clip adds split-at-playhead and cycle-over-clip; the piano roll offers duplicate,
delete, transposition and dynamics — pp through ff — for the selected notes; effect slots offer
bypass, reorder and remove. A menu can be answered from the keyboard as well as the pointer.
Renaming goes through the platform's input handler, so an IME composes into the field the way
it does anywhere else.

Creating and deleting are pointer gestures, and which gesture does which is a setting. The
defaults are Logic's — ⌘-click creates a note or a clip, ⌥-click deletes what is under the
pointer — with the double-click available for either, though nothing destructive is bound there
by default: a double-click opens a clip in every editor that has clips, and the gesture people
arrive with should not destroy their work. Dragging across empty space in the piano roll or the
arrangement sweeps a selection rectangle; ⇧-drag adds to what is already selected, and a multiple
selection moves, duplicates and deletes as one.

The piano roll has two tools, and the strip in its header says which one is in hand — as does the
status line, when the key is used and the roll is not on screen to show it. **T** puts the next
tool in hand, which with two tools is Logic's press-it-twice-to-swap-back; like every other
binding it can be changed in the settings window. The pointer
selects, moves, resizes and creates — a note's end is a handle, and the pointer turns into a ↔
over one, the same way a clip's edges do in the arrangement. The velocity tool does one thing:
drag a note up or down and
it is struck harder or softer, with the value shown beside it as it goes. A selection is dragged
together and keeps its shape — a phrase written soft-loud-soft is still soft-loud-soft once it
has been played harder — and running off either end and coming back restores that shape rather
than leaving the whole chord flat against the limit it was pushed into. One drag is one undo
step, and Escape during it puts the notes back. Every note also carries a bar inside it showing
the same value, because the colour ramp can say roughly where in the range a note sits but not
the difference between 96 and 100, which is the difference the drag is being made to find.

Logic offers that gesture on ⌃⌥-drag as well as on the tool, and that half cannot be carried
across: on macOS a ⌃-click becomes a right-click before the window sees it — ⌃ stripped off on
the way — so it would arrive as a request for the context menu rather than as a drag.

Everything placed snaps to the grid button's division, which cycles down to *free* — one tick,
which is as fine as the document gets. Holding ⌘ (Ctrl on Windows) suspends snapping for the
length of a drag, for when one thing has to sit off the beat. A double-click on any fader or knob
puts it back to its default: 0 dB on a volume, centre on a pan.

Either edge of a clip is a handle, and the pointer turns into a ↔ over one so you can see that
before you press. Dragging one changes the clip's length, and what that means depends on what the
clip is. An **audio** clip is trimmed, and the trim stops where the material
does — the front walks the clip's window into the source rather than sliding the take along the
timeline, so dragging it back out uncovers what was hidden instead of repeating what is left. A
clip somebody **played** keeps every note exactly where it is. A clip that **wrote itself** is
written again over the stretch it now covers, since it is its recipe rather than its notes: pull
it out and it fills the bars it gained, pull it in and it stops where it stops. Nothing is lost
by that — the recipe still says what the clip is, so dragging back out writes the material back.

An audio clip fades in and out by its handles: the pair sits in a band just under the clip's
name bar, one at each end, and dragging one draws the fade as a dimmed wedge over the waveform
with the ramp across it. Fades ignore the grid on purpose — they are shaped by ear against the
waveform, and no grid position has anything to do with where a breath ends. The clip's own gain
is on its right-click menu, in decibels, applied before the track's effects; while it is not
0 dB the clip prints the number beside its name, and *Remove Fades* on the same menu takes both
fades back off.

The middle of the transport bar holds three readouts: the position, the tempo and the time
signature. The first two are typeable — double-click either and the number can be entered
directly. A wheel is for finding a tempo by feel and a drag for nudging it, but neither is any
way to reach 174 from 120, or bar 97 from bar 1. The position takes as much of
`bar.beat.hundredth` as you care to give it — `97` is the top of bar 97, `97.3` is its third
beat. All three are on the command palette too, so none of them needs the mouse.

The tempo can change along the timeline. The readout shows — and edits — the tempo of the
stretch the playhead is in; a change elsewhere is written from the ruler's right-click menu
(*Tempo from Here…*), lands on the beat, and is marked along the ruler's lower edge with its
number. The same menu removes the change in force under the pointer. The song's opening tempo
at the start of the timeline is always there and cannot be removed, exactly like the key.

### Automation

Right-click a track header and choose **Automate Volume** or **Automate Pan**: a lane opens under
that track showing the parameter's curve over the same timeline the clips sit on. Choosing the
other parameter swaps what the lane draws; choosing the same one again closes it.

A press on empty lane writes a point and starts dragging it, so placing a value and shaping it is
one gesture. Dragging a point moves it in both directions at once — along the timeline and up or
down through the parameter's range — snapped to the grid unless the platform's command modifier is
held, exactly like a clip. The delete gesture takes a point off. A drag is one undo step.

**A parameter with no lane is not automated at all**, and keeps whatever its fader or knob is set
to. Only once a point exists does the lane take over, which is what lets a mix be automated one
control at a time. Taking the last point off — or **Clear Automation** — hands the parameter back
to its stored value.

A lane holds its nearest value flat outside the stretch it was written over: it makes a claim
about that stretch and none about the rest of the song. So a single point at bar 40 sets the level
for the whole piece, and a fade written across bars 40 to 44 leaves everything before bar 40 at
where the fade begins.

Playback and export take the same path, so what you hear is what is written. The lane is read once
per processing block rather than once per sample — for a fader that is not an approximation, since
a gain is a target the mixer ramps across the block it is given, and for a plugin parameter there
is nowhere finer to put one. Seeking or looping *arrives* at the values under the playhead rather
than sliding to them: a playhead that jumped is not a fader that moved.

Volume and pan are what the lane offers today. The document, the engine and the commands underneath
already address every parameter a plugin has — see
[`ParamTarget`](https://docs.rs/auris-core) — so widening the menu is a menu change.

### The time signature

So can the meter. The signature readout shows the one the playhead is in, and clicking it drops
the common meters — 4/4, 3/4, 6/8, 7/8 and the rest — with *Other…* for anything else the
notation can hold, up to 32 beats over a whole, half, quarter, eighth or sixteenth note. Choosing
one turns the stretch the playhead is in.

A change further along is written from the ruler's right-click menu (*Time Signature from Here…*).
It **lands on a bar line**, always: a 3/4 beginning half way through a bar of 4/4 would leave that
bar with no length and every bar number after it uncountable. The bar it starts on is numbered on
the ruler whatever the zoom, with the new signature printed beside the number, and the bar lines
and the grid follow it from there — a bar of 7/8 is seven eighths wide and the next bar number is
the next bar number.

Writing a change *before* one already there moves the later ones onto the new bar grid, because
that invariant has to hold across the whole song. Nothing else moves: notes, clips, chords and
sections are stored in ticks, so changing the meter moves the bar lines over them and not one
sample of what you hear. Undo takes it back in one step.

### Languages

The interface is available in English and Japanese, chosen under Settings → General or followed
from the system locale.

That is the window. `auris` prints English whatever the setting says, because a terminal is not a
surface that can promise to render anything else: a Windows console on a code page other than
UTF-8 turns Japanese into mojibake, and a pipe into a tool that assumes ASCII does worse. Names
that came out of a document — a project's, a track's — are still whatever you typed, because
those are your words rather than the program's.

Plugin names and parameters are translated where the term is known and left in the plugin
author's own wording where it is not, so a third-party plugin degrades to English rather than to
a missing-string marker.

### The harmony lane

A strip under the bar ruler carries the key and the chords, spanning every track because that is
what harmony is: one thing the whole arrangement obeys at any one moment, belonging to no track.
Like the tempo map, it changes as the song goes on.

Right-click it to type a chord (`IV`, `vi`, `bVII7`) or a key (`Eb`, `F# minor`), or to write one
of the named progressions — `axis`, `marusa`, `royal-road`, `canon` and a dozen more, the same
catalogue `auris progressions` lists and the composer reads as `@marusa`. A progression is written
across the cycle region when there is one, and across its own length otherwise.

The box says what it wants and offers it: a line of syntax under the field, and a row of the
degrees or keys it would accept, narrowing as you type. Clicking one answers the box; **Tab** walks
the row without leaving the keyboard, marking where it has got to, and wraps at the end. The walk
follows what was *typed* rather than what the last press wrote, so `b` reaches all four borrowings
rather than completing to `bIII` and stopping there. The rule worth stating before it is broken
rather than after is the case of a numeral — `IV` is major and `iv` is minor — which nothing else
on screen was saying.

A chord is stored as a roman numeral, not as `Fmaj7`, so changing the key transposes the whole
progression and a modulation halfway through a section reharmonises the rest of it without a
single chord being rewritten. What the lane shows is both: `IVmaj7 · Fmaj7`.

### The structure lane

Above the harmony sits the song's own shape: a strip of section names — イントロ, Aメロ, サビ,
or Intro, Verse, Chorus — each in force until the next, snapping to bar lines because "the
chorus starts at this bar" is the thing being said. Double-click to name the section under the
pointer (the field offers the usual vocabulary and completes it with **Tab**; the label is free
text, so Cメロ and 落ちサビ are as sayable as anything on the list), drag a boundary's leading
edge to move it, and right-click to rename, remove, or end the structure where an outro stops.
A label that repeats is numbered where it is drawn — サビ 1, サビ 2 — counted from the start of
the song rather than stored, so the numbering can never disagree with the timeline.

The labels are more than a map. A clip generated inside a named section draws its figures from
the *label*: two clips written into stretches both called サビ come out recognisably the same
idea, and a stretch called Bメロ writes something else — the same rule that makes a repeated
section recognisable inside the composer, read off the timeline instead of a specification.
This is the ground the whole-song generator will stand on; today it is already the difference
between clips that happen to coexist and clips that belong to the same song.

### Automatic composition

**File → Compose from Specification…**, or `auris compose song.asong`, writes a piece from a text
specification: key, scale, tempo, mood, chord progression, form and parts, in a line-oriented
document that an agent can write and a person can edit one line of. The whole piece arrives as a
single undo step, so a composition that is not what was wanted is one press away from the document
that was there before it.

```
title:  Neon Drive
key:    C minor
tempo:  128
mood:   driving
chords: @marusa
form:   intro verse chorus verse chorus outro

[section chorus]
bars: 8
intensity: 0.95
```

Progressions can be quoted from a catalogue by name — `@marusa` (丸サ進行), `@royal-road`
(王道進行), `@koakuma`, `@komuro`, `@canon`, `@junjo`, `@blues`, `@andalusian` and the rest — or
written out in roman numerals (`| IVmaj7 | III7 | vi7 | I7 |`) in any key. A quoted progression is
never recoloured, because the whole point of naming one is that it comes out sounding like
itself.

Each part is built from one short figure invented per section and then restated bar after bar,
which is what gives a section something an ear can hold on to; the fourth bar of every phrase
answers it rather than repeating it again. A section played twice is the same section both times —
`variation: 0.4` buys back as much departure as you want, and `variation: 0` makes a second chorus
note for note the first. A section that another follows runs a fill into it, and every part leans
gently across a phrase rather than sitting at one level throughout.

Everything is a pure function of the specification and its seed, so the same document always
writes the same piece and `--seed 7` writes a different one. Every decision draws from a stream
addressed by name rather than by call order, so changing the drum density does not silently
rewrite the melody. `auris progressions` lists the catalogue; `--set "field: value"` overrides any
field from the command line.

### Clips that write themselves

A single clip can be written from the chords underneath it without a specification for a whole
song: right-click an empty stretch of an instrument track and choose a preset — lead, chords, pad,
arpeggio, stab, bass or drums. The clip keeps the recipe that produced it, so **Another Take** is
the next seed, **Write It Again** follows the chords when they move, and **Keep This One** drops
the recipe when a take turns out to be the keeper. The dials are in the inspector:

| | |
|---|---|
| **Subdivision** | How finely the beat divides: 1/8, 1/16, or either of them in triplets |
| **Octave** | Which register, ±2 from where the preset sits |
| **Density** | How busy the part is — for a comp, which figure it reaches for |
| **Syncopation** | How far the figure pulls off the beat, without making it busier |
| **Gate** | How long each note sounds, as a share of the gap to the next |
| **Intensity** | How hard it is played |
| **Dynamics** | How far apart the hardest and softest notes are struck |
| **Fill** | How much of the last bar the snare runs into what follows — drums only |
| **Swing** | How late the offbeats are |
| **Humanize** | How far timing and velocity wander |

Intensity and dynamics are the level and the spread, and they are separate because they are two
questions: a part can be played hard and flat, or softly with everything in it moving. At 0 the
dynamics leave every note struck alike — a sequencer, which is sometimes exactly the point — and
the level stays where the intensity put it rather than sagging with the spread. It reaches every
source of variation at once: the metric hierarchy, the accents, the lean across a phrase and the
crescendo of a drum fill.

A kit reads the density around the middle: below it the groove thins from its weakest hits upward
and never loses a downbeat, and above it the steps the groove left empty start taking ghost notes
— which is how a drummer gets busier without playing something else. *Which* rhythm it plays is
still the groove picker, and that is a choice from a drummer's own vocabulary rather than a
number: `basic-rock`, `eight-beat`, `sixteen-beat`, `four-on-the-floor`, `shuffle`, `breakbeat`,
`bossa-nova`, `half-time` and `sparse`.

Only the rows a preset can actually hear are drawn. A kit has no gate, subdivision, octave or
syncopation — a one-shot ignores its note-off, its pitches are drum numbers rather than notes, and
where it plays is which groove it plays. Nothing but a kit has a fill, because nothing else has a
last bar to announce. A pad has no syncopation, because it sounds each chord where the chord is. A
part on a triplet grid has no swing.

The subdivision is per part, not per song, so a stab hammering triplets over a straight kit is a
setting rather than a fight. Swing disappears on a triplet grid, because a grid already sitting
where swing is trying to push it has nothing left to be pushed — and a drum kit ignores the
subdivision entirely, since a groove is written in sixteenths and read by index.

A chord part picks its figure once for the section and restates it, the way a keyboard player picks
a feel and keeps it; only the fourth bar of a phrase is allowed to turn it over, and only
sometimes. At the top of the density dial the figure it reaches for is a rhythm rolled from the
metric hierarchy — most of the steps, with the holes that make it a rhythm rather than a tremolo.

**Chords** and **pad** read the same harmony through the same writer, and what separates them is
what a pad does at a chord change: it holds whatever the two chords have in common and moves only
the voices that have somewhere to go, where a comp restrikes every one of them. That is the
difference between a chord *changing* and a chord *drifting*, and it is the reason the two presets
are two presets rather than one with the rhythm turned off.

The **stab** preset is the settings that have to be turned up together: fast, short and hammered,
which is what most dance music has underneath it. It arrives with its own dials rather than the
middling defaults, and moving one of them keeps it moved when the preset changes — a dial somebody
set is theirs, a dial still where the last preset left it is not.

`auris` compose specifications reach the same settings, per part:

```
[part chords]
subdivision: 16t
gate: 0.25
```

### Built-in instruments

Deliberately simple chiptune voices, enough to hear the engine working:

| Id | Name | What it is |
| --- | --- | --- |
| `auris.synth.chiptune` | Chiptune | Sine / square / saw / triangle / LFSR noise with pulse width, ADSR, glide, unison and bit-crush |
| `auris.synth.fm2` | FM 2-Op | A two-operator FM voice, included to show a different synthesis method dropping in unchanged |
| `auris.synth.noisedrum` | Noise Drum | Pitch-swept noise through a band-pass, for percussion |

Square and saw are PolyBLEP band-limited, so high notes stay clean instead of aliasing.

The library panel is a tree: instruments, SoundFonts and effects, each opening into
groups rather than a flat list — the plugins by category, a font by the banks it declares. Every
branch remembers whether it was left open. Clicking an instrument sets it on the selected track,
clicking an effect appends it to that track's chain.

### SoundFonts

**File → Import SoundFont…** — or dropping the file on the window — reads an `.sf2` and puts its
sounds on the shelf. The library panel lists every imported font; opening one shows the banks it
declares and opening a bank shows its sounds, and clicking a sound points the selected track at
it — switching that track to the sampler in the same edit, so it is one click and one undo step
rather than two.

A project stores the font's *path* and names each sound by bank and patch, never by position in
the list. That is what makes a piece saved last week open playing the same instrument: a position
moves the moment anyone edits the file. The samples themselves stay out of the document, so a
project referring to a two-hundred-megabyte orchestral set is still a few kilobytes of JSON, and
a font whose file has moved costs one track its sound rather than costing you the session.

A font is *not* copied into the project folder, unlike imported audio — see
[The project folder](#the-project-folder). One font is shared by every project that uses it, and
paying two hundred megabytes per project to shorten a path would be a poor trade. What the
document keeps instead is enough to recognise the file again if it moves: its name and its size.

Playback is `rustysynth`, which both parses the format and renders it. Building the synthesiser
happens off the audio thread; what runs on it allocates nothing.

### Built-in effects

| Id | Name | Notes |
| --- | --- | --- |
| `auris.fx.gain` | Gain & Pan | Constant-power pan law, stereo width, phase invert |
| `auris.fx.eq` | Equalizer | Six bands (HP, low shelf, two peaks, high shelf, LP) with a live response curve |
| `auris.fx.compressor` | Compressor | Soft knee, gain-reduction metering |
| `auris.fx.delay` | Delay | Ping-pong, damped feedback |
| `auris.fx.reverb` | Reverb | Freeverb-style comb/all-pass network |
| `auris.fx.distortion` | Distortion | Soft clip, hard clip, wavefolder, bitcrusher |
| `auris.fx.limiter` | Limiter | Lookahead, so the ceiling is actually a ceiling |

Effects can be chained on any track and on the master bus. A chain that looks ahead — the limiter
does — hands its audio back late, so every other track is held back to match it and the parts stay
in step with each other. An export renders the resulting lead-in and drops it, so the file still
lines up with the timeline.

### The project folder

A project is a folder, not a lone file. **Save As** creates it: choosing `MySong.auris` writes

```
MySong/
  MySong.auris        the document, pretty-printed JSON
  Audio/
    kick.wav          every audio file the song uses
```

Audio is copied in as it is imported, and the document refers to it *relative to the folder*. So
the folder is the thing you move, rename, copy to another machine, zip up or put in version
control, and it opens on the other side with nothing missing. That only holds while one folder
holds one project, which is why Save As creates the folder rather than trusting anyone to: two
documents sharing a folder would share its `Audio/`, and saving one under a new name would
silently leave both pointing at the same files. Relative paths are stored with `/` separators
whatever the platform wrote them, so a project saved on Windows opens on a Mac.

Because the document goes one folder deeper than the name you type, the system save dialog cannot
warn you about replacing one — it is asking about a path nothing is written to. Auris asks
instead, naming the project that would go. It also asks before New Project, Open Project, closing
the window or quitting throw away unsaved changes, since each of those clears the undo history
along with the document.

SoundFonts stay where they are, for the reason given above. **File → Collect Assets into Project**
(or `auris collect`) copies those in too, for archiving a project or handing it to someone else —
explicit, because the bill is measured in hundreds of megabytes.

When a file has moved anyway, opening the project looks for it: in the project folder, and in the
directories the project's other files turned out to be living in. A font is confirmed by its
recorded size, so a different one wearing the same name is not quietly adopted. Anything found is
written back into the document, so the search happens once rather than on every open. Anything
genuinely gone is reported, and the project still opens with that one track silent.

### Key bindings

Every command is rebindable, and the settings window is where. Press a key onto a row and it takes
that key; ＋ gives the same command a second one; — leaves it with no key at all, which is a
different answer from putting the default back and used to be one nothing could say. Only what you
change is written to `keymap.json`, so a later change to a default still reaches you.

A binding is captured the way *this* keyboard reported it and stored the way both keyboards spell
it, so a `keymap.json` kept in a dotfiles repository binds ⌘ on the Mac and Ctrl on the Windows
machine from the same line.

Bindings are scoped to where the keyboard is. Most of them are the window's and fire wherever you
are; some belong to one panel, and the row says which — **T** puts the next tool in the piano
roll's hand and does nothing while you are in the mixer. **Tab** and **⇧Tab** move between panels,
and the one holding the keyboard is outlined. Two commands on one key in *different* panels is not
a conflict and is not reported as one; a clash you could actually reach is shown, and allowed —
you may be halfway through swapping a pair over.

Nothing needs the mouse. **F10** drops open the menu bar this window draws for itself on Windows
and Linux; ← and → walk the menus, ↑ and ↓ the rows, Return runs one and Escape closes it. A
right-click menu answers to the same keys. While either is open every binding is out of reach, so
walking a menu cannot also run the command a letter is bound to.

### The command palette

**⌘⇧P** — Ctrl+Shift+P on Windows — opens a box that finds any command by typing a few letters of
it. `nit` reaches New Instrument Track; a match at the start of a word counts for more than one in
the middle, so `save` puts Save above Add Audio Track. Every rebindable command is there with its
keystroke beside it, which is also how you find out a command *has* one.

It also sets values, which is the part that would otherwise mean a trip to a window or to a corner
of the transport bar: type `1/16` for the editing grid, `6/8` for the meter of the stretch the
playhead is in, a colour scheme's name to repaint the window, or `日本語` to switch language — the
languages are listed in themselves, since the person opening that list is the one who cannot read
what is currently on screen.

### Settings, where dotfiles can reach them

Preferences live in `~/.config/auris-studio/` on every platform — macOS and Windows included,
rather than `~/Library/Application Support` and `%APPDATA%`:

```
~/.config/auris-studio/
  settings.json       audio device, sample rate, buffer size, interface language
  keymap.json         key bindings you have changed from the defaults
  appearance.json     the chosen colour scheme
  layout.json         where each panel is docked, and how large each dock is
```

Four small JSON files, readable and hand-editable, in the directory a dotfiles repository is
already checked out over. Set `AURIS_CONFIG_DIR` to name a directory outright, or `XDG_CONFIG_HOME`
to move the parent. An installation predating the move keeps its settings: the old directory's
files are copied across on the first run, never over a file already in the new place.

### Export

Render the whole project to a WAV file at 16-bit, 24-bit or 32-bit float, faster than
realtime. The renderer keeps going past the last clip for as long as the effects take to fall
silent — the tails along a chain add up rather than overlap, because a delay feeding a reverb
keeps feeding it for the whole of its own decay — so nothing is cut off.

An export can be written at any sample rate; the sources are converted to it first, so a project
exported at 96 kHz is the same piece rather than the same samples played faster.

The cycle region exports on its own through *File → Export Cycle…*, or `auris render --loop`
from the command line. The range ends the way pressing Stop there sounds: the voices are
released at the boundary and the tail holds the ring-out of what was inside the range, never a
performance of the material beyond it.

### GPU acceleration

`auris-gpu` runs the large, embarrassingly parallel offline reductions on the GPU through
[wgpu](https://wgpu.rs): waveform min/max/RMS extraction for clip drawing, and whole-file
loudness analysis. Every kernel has a CPU fallback, and the application runs correctly with no
GPU present.

Realtime per-block DSP deliberately stays on the CPU — a round trip to the GPU costs more
latency than an entire audio block is allowed to take.

## Frontends

The backend is a set of crates that know nothing about any UI, and each frontend is a thin
layer on top of the same [`Session`](crates/auris-session) — one document, one engine, one
method per user command.

```bash
cargo run --release                 # the desktop application
cargo run --release -p auris-cli -- help
```

The command line tool exists as much to keep the split honest as to be useful: it drives the
identical session with no window and no audio device, so anything that leaks into the UI stops
compiling here.

```bash
auris compose song.asong -o song.auris         # write a piece from a specification
auris progressions                             # every chord progression known by name
auris plugins                                  # every registered instrument and effect
auris new song.auris --bpm 128
auris info song.auris                          # tracks, clips, duration
auris render song.auris -o song.wav --bit-depth 24
auris collect song.auris                       # gather every file it uses into its folder
```

An MCP server is the next frontend and needs no new backend work — it is the same `Session`
API with a different transport in front of it.

## Downloads

Built binaries are on the [releases page](https://github.com/uthree/auris-studio/releases):
`Auris Studio.app` and the `auris` command line tool for macOS, as one universal binary for
Apple Silicon and Intel; both `.exe`s for Windows; the command line tool alone for Linux.

None of it is code-signed, so the first launch needs a word with the operating system. On macOS,
open the app from its right-click menu once rather than by double-clicking it, or run `xattr -dr
com.apple.quarantine "Auris Studio.app"`. On Windows, SmartScreen wants *More info → Run anyway*.

Auris Studio is at `0.x`, and [nothing is stable there](CHANGELOG.md): the project format, the
configuration files and every public API may change in any release, with no migration path.

## Building

```bash
cargo run --release
```

macOS and Windows both build and run the desktop application; the backend and the command line
tool build anywhere Rust does. CI covers all three.

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

## Layout

```
BACKEND — no UI dependency of any kind
  crates/auris-core     types, plugin traits, project model — no local dependencies at all
  crates/auris-dsp      effects and DSP primitives
  crates/auris-synth    built-in instruments
  crates/auris-sampler  SoundFont playback: the font bank and the sampler instrument
  crates/auris-engine   render graph, transport, cpal output, offline renderer
  crates/auris-io       audio file import/export, project save/load
  crates/auris-gpu      wgpu compute for offline analysis
  crates/auris-compose  score-based automatic composition: a text spec in, notes out
  crates/auris-i18n     interface text in every language, and nothing else
  crates/auris-session  the document, the engine and every command a frontend needs

FRONTEND
  crates/auris-gpui     the desktop application  (binary: auris-studio)
  crates/auris-cli      the command line tool    (binary: auris)
```

Dependencies run strictly downhill and the boundary is enforced by what each crate is allowed
to name. `auris-core` depends on nothing local. `auris-engine` does *not* depend on
`auris-dsp`, `auris-synth` or `auris-sampler` — it drives plugins purely through the
`auris-core` traits, which is what keeps the plugin system honest. And `auris-gpui` depends on
`auris-session` and gpui
and nothing else in the workspace: if it ever needs `auris-engine` directly, something that
belongs in the session layer has leaked into the UI.

### What lives where

Keeping the *rendering* backend UI-free is the easy part. The piece that usually leaks is the
orchestration around it — building the registry, rebuilding the render graph after an edit,
deciding which changes need a whole new graph and which fit in a command, tracking undo,
resolving a parameter to the plugin that owns it. All of that is in `auris-session`, so a
second frontend reuses it instead of reimplementing it slightly differently.

Gestures are handled with transactions: a pointer drag opens one, makes as many edits as it
likes, and closes it. The result is one undo step and one graph rebuild — and none of either
when the drag changed nothing.

## Adding a sound source

Implement two traits and register a factory. That is the whole procedure.

```rust
use auris_core::prelude::*;

struct MySynth { /* ... */ }

impl Parameterized for MySynth {
    fn parameters(&self) -> &[ParamDescriptor] { /* ... */ }
    fn param(&self, id: ParamId) -> f32 { /* ... */ }
    fn set_param(&mut self, id: ParamId, value: f32) { /* ... */ }
}

impl Instrument for MySynth {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            "mine.synth.example",
            "Example",
            "A worked example",
            PluginCategory::Synth,
        )
    }
    fn prepare(&mut self, ctx: &PrepareContext) { /* allocate here */ }
    fn reset(&mut self) { /* ... */ }
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        /* never allocate here */
    }
}

registry.register_instrument(|| Box::new(MySynth::new()));
```

The parameters you declare become UI controls automatically, with the right widget, range,
unit and scaling — the editor is generated from `ParamDescriptor`, not hand-written per plugin.
Effects work identically through the `Effect` trait.

## Known limitations

Honest list of what is not there yet, so nobody discovers these the hard way:

* **No recording.** Audio tracks hold imported material only.
* **What writes itself writes in one meter.** The timeline holds as many signature changes as you
  like, but a specification says `meter:` once, and a generated clip is built on the grid of the
  meter it starts in. A part written across a change keeps the bars it began with.
* **Muting a track stops its effects.** The master bus keeps processing while muted, so its
  reverb rings out and un-muting does not pop; a track is skipped once its mute has faded, which
  is cheaper but cuts its tail off at the fade rather than letting it decay.
* **Delay compensation is measured, not predicted.** A parameter that changes a plugin's latency
  — the limiter's lookahead is the only one — is noticed after the fact rather than in advance, so
  the tracks are out of step from the moment it moves until the graph is rebuilt: the next frame
  for a single change, the end of the gesture for a drag.
* **Latency compensation is per track, not per clip.** The whole mix plays back as far behind the
  playhead as the longest chain needs, which is unavoidable, but it means turning on a
  look-ahead effect anywhere raises the monitoring latency everywhere.

## Development

```bash
cargo test --workspace                    # unit tests
cargo clippy --workspace --all-targets    # lints
cargo fmt --all                           # formatting
cargo doc --workspace --no-deps --open    # the API documentation
```

Every crate carries `#![warn(missing_docs)]` and CI builds the documentation with warnings denied,
so a public item without a doc comment and a link that does not resolve are both build failures.

The workspace has no root crate, so the account of how the twelve of them fit together lives in
`auris_session::guide` — it is the only crate that depends on every other, and so the only one
whose links to them all resolve. It covers the architecture and the layering rules, the realtime
contract, writing a plugin (with a worked example that is compiled as a test), the composition
format, and where the two platforms differ.

## Licence

Apache-2.0. The full text is in [LICENSE](LICENSE), and it is the licence every release archive
carries.
