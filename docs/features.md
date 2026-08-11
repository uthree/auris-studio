# Features

What the application does, panel by panel. The overview is in [the README](../README.md);
how a piece writes itself is in [Automatic composition](composition.md).

## Tracks

* **Instrument tracks** — notes on a timeline, played by a software instrument.
* **Audio tracks** — imported audio, arranged as clips with trim, gain and fades.
* **Buses** — mixing points with no clips of their own; see [Buses and sends](#buses-and-sends).
* Every track carries a **colour**, tinting its header and its clips. A new one takes the next
  palette entry; the track's right-click menu offers the other seven as swatches, since the order
  tracks were made in has nothing to do with which of them are drums.

**Drag a track header up or down** to move it in the list. The arrangement reorders as the pointer
moves rather than showing a line and jumping on release, so what follows the hand is the thing
itself; the whole drag is one undo step. A press that does not travel is a selection, as before, and
a press that lands on the header's fader, pan or mute keeps its own gesture.

Only the list moves. Everything the document holds names a track by id — automation lanes, a
routing output, a send — so a bus can end up above the tracks feeding it without changing a note of
what is heard.

## Dragging files in

Drop an audio file on the window and it arrives on a **new audio track**. Drop an `.sf2` and it
goes on the library's shelf, with the font opened where its sounds are chosen. Drop a handful and
each is read in turn, in the order they were dragged, with the status line naming the one being
read — decoding is slow enough that a folder of takes would otherwise be several seconds of a
window with nothing to say.

Drop a **`.mid`** and it opens as a new piece, tempo and meter and all — see
[MIDI files](#midi-files).

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

## Panels, and where you put them

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

## Editing

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
selection moves, duplicates and deletes as one. A press on empty arrangement that never travelled
was a *click* rather than a sweep, and moves the playhead there — the ruler is no longer the only
place that will take one.

**Create can be a plain click**, for anyone who would rather not hold a modifier to write a note.
Choosing it moves the rubber band to ⇧-drag, which is said on the settings page as you choose it —
⇧ already means *extend the selection* on every other press, so the gesture was there rather than
invented for the occasion. It applies to both surfaces, so a click on an empty lane then makes a
clip instead of moving the playhead; that is the other half of what a bare click costs, and the
reason the modifier remains the default. Deleting cannot be a plain click and is not offered:
creating on a bare click leaves something you can see and undo, and deleting on one would remove
every note you tried to pick up.

The wheel over the arrangement moves down the tracks; ⇧ turns it sideways along the song, and
**Ctrl or ⌥** zooms the time axis about the pointer, so the bar being looked at stays where it is.

**No bar takes the wheel.** A fader, a plugin parameter, a dial on the song sheet — every one of
them is swept with the pointer and none of them answers a scroll. They all sit inside panels that
scroll, and rolling down a column of tracks used to change the level of whichever fader the pointer
crossed on the way: silently, with no drag to remember having started, and nothing on screen saying
which one moved. The wheel belongs to the thing being scrolled.

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

**Cut, copy and paste** are ⌘X, ⌘C and ⌘V, and they mean notes in the piano roll and clips in
the arrangement — the same three keys, scoped to wherever the keyboard is, exactly as ⌘A and ⌘D
already are. Both are on the Edit menu in pairs so it is possible to find out which one you just
got. A paste lands at the playhead; a right-click on an empty lane offers *Paste Here* instead,
because that is the one place a paste has a position of its own behind it.

What is on the clipboard is a *shape* rather than a place. Notes come back with the gaps between
them intact; a block of clips copied off four tracks lands on four consecutive tracks starting
wherever you aim it, and goes on doing so after the tracks it came from have been renamed,
reordered or deleted. What arrives becomes the selection, so it can be dragged straight away
without hunting for it. A paste that fits nowhere — a MIDI clip aimed at an audio track, or a
block whose lower rows run off the bottom of the track list — lands what it can and says so. This
is Auris Studio's own clipboard and not the system's: nothing here goes to another application,
and nothing copied in one arrives here.

Everything placed snaps to the grid button's division, which cycles down to *free* — one tick,
which is as fine as the document gets. Holding ⌘ (Ctrl on Windows) suspends snapping for the
length of a drag, for when one thing has to sit off the beat. A double-click on any fader or knob
puts it back to its default: 0 dB on a volume, centre on a pan.

**Quantise** puts notes that were played back onto that same grid, after the fact. It comes in
three, because the two numbers a note has are separately wrong: *Quantise Starts* (**Q**) tidies
where the notes begin, *Quantise Lengths* tidies how long they are held, and *Quantise Both* does
the two together. A part played a shade ahead of the beat wants its lengths evened out and its feel
left alone; doing both to a take that only needed one is how it stops sounding like anybody played
it. All three are on the roll's right-click menu and on the Edit menu, they act on the selected
notes, and they snap to the division the grid button is showing — quantising to something you
cannot see is a jump with no explanation. A length never rounds down to nothing: on a sixteenth
grid a clipped grace note becomes a sixteenth rather than disappearing. The status line says how
many notes actually moved, which tells you how straight the rest of them already were.

Either edge of a clip is a handle, and the pointer turns into a ↔ over one so you can see that
before you press. Dragging one changes the clip's length, and what that means depends on what the
clip is. An **audio** clip is trimmed, and the trim stops where the material
does — the front walks the clip's window into the source rather than sliding the take along the
timeline, so dragging it back out uncovers what was hidden instead of repeating what is left. A
clip somebody **played** keeps every note exactly where it is. A clip that **wrote itself** is
written again over the stretch it now covers, since it is its recipe rather than its notes: pull
it out and it fills the bars it gained, pull it in and it stops where it stops. Nothing is lost
by that — the recipe still says what the clip is, so dragging back out writes the material back.

**A clip can loop.** The right edge of a clip's *name bar* is a second handle, sitting on top of
the resize handle until it is used: drag it out and the clip goes on saying itself for as long as
you pull, in faded repeats divided by a hairline, and drag it back over the clip's own end to stop.
The clip itself is untouched by any of it — one block, one name, one selection, however many times
it is heard — and the edge below the name bar still resizes, so the phrase and the number of times
it is played are two different things you can change. *Loop Clip* on the right-click menu, on the
Edit menu, and on **L**, does the same thing without the mouse: on, it reaches out to the next clip
on the lane, or doubles where there is nothing in front of it.

Both kinds loop. The last repeat is cut off wherever the loop ends rather than being rounded to a
whole pass, so a loop can stop half way through a bar; on an audio clip the fades stay on the
clip's own two edges and the joins between repeats run flat, because a fade-out at the end of every
pass would pump once a bar. Splitting a looped clip stops both halves repeating — the repeats were
of a block that no longer exists — and duplicating one lands the copy past the repeats rather than
on top of them. An exported file, WAV or MIDI, contains the repeats: nothing about a loop stops at
playback.

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

## MIDI files

**File → Import MIDI File…**, or dropping a `.mid` on the window, reads it as a **new piece**: its
tempo map, its meter, and one track per part with the notes where the file put them. A new document
rather than tracks added to the open one, because a MIDI file brings its own clock — its notes
dropped into a piece running at a different speed would be the right notes at the wrong lengths,
with nothing on screen to say why. Unsaved work is asked about first, exactly as it is for **Open**.

A part that played on **channel 10** gets the noise-drum instrument. That is the only thing a bare
MIDI file says about what a track is *for*, and a General MIDI drum part played on a lead synth is
not something anyone would keep.

**File → Export MIDI File…** writes the other direction, at the application's own resolution — 960
ticks to the quarter note, which an SMF header holds — so a piece that leaves and comes back has
every note in the same place. Tempo is the one thing that does not survive exactly: a MIDI file
stores whole microseconds per quarter note, so 144 bpm goes out as 416 667 and comes back as
143.999 88. A thousandth of a beat per minute, and 96 or 120 divide evenly and return exact.

**Pitch bend and modulation travel both ways.** A clip carries both curves itself — on the clip
rather than in an automation lane, because they belong to the phrase and a clip dragged four bars
later takes them along. **View → Pitch Bend** (`⌘⌥B`) and **View → Modulation** (`⌘⌥W`) put a strip
under the piano roll for each: a press on empty strip writes the point it is about to drag, so
placing a curve and shaping it is one gesture; ⌥-click takes one off; right-click straightens the
whole clip. They span the same timeline as the notes above them, because both happen at a moment in
the phrase and the only useful way to look at one is with the note it is shaping directly overhead.

One set of gestures and one painter for both, differing in exactly two things: the bend goes both
ways from a line across the middle, and the wheel goes up from a floor, because there is nothing
below the bottom of its travel.

Neither is exact through a file, and for opposite reasons. A bend is fourteen bits across the range
a receiver assumes, so a semitone is quantised to about a fiftieth of a cent; the wheel is
controller 1, seven bits, which is all the resolution the wire has — a receiver reading this file
hears exactly what it would hear reading anybody else's.

A curve that does not end at zero is **let go before the clip ends**. Both are channel state an
instrument holds until it is told otherwise: a clip finishing two semitones sharp would detune
everything after it, and one finishing with the wheel up would leave it wobbling.

What a `.mid` has nowhere to put, in either direction: audio tracks, every mixer setting including
mute and solo, which instrument each track plays, and the automation. A MIDI file is the notes and
the clock.

A file counted in **SMPTE frames** rather than beats is refused rather than guessed at. It has no
beats, so it has no bars, and laying it on a musical timeline would mean choosing a tempo on the
file's behalf and writing it down as though the file had said so.

## Automation

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

## Buses and sends

**Bus** on the arrangement's header, or **New Bus** in any track menu, adds a mixing point: a track
with no clips, whose material is whatever is routed into it. It has a fader, a pan, a mute, an
effect chain and an automation lane, because it is a track.

Every strip in the mixer says where it goes. Clicking that name offers the master and every bus,
and the **+** beside it adds a **send** — a copy of the track fed to a bus at a level of its own,
*as well as* wherever the track's own output goes. Six tracks sending to one reverb is six sends;
one fader over a whole drum kit is six outputs. A send row's slider is a mixer control like any
other, so it drags, resets on a double click and can be automated. Right-clicking
one moves its tap before the fader — where a reverb wants to follow the fader down, a headphone mix
does not — or takes it away.

Solo travels both ways along the routing. Soloing a drum track leaves the drum bus open, because
its audio has nowhere else to go; soloing the drum *bus* leaves the drum tracks open, because a bus
has nothing of its own to play. A track is heard exactly when it lies on a path through something
soloed.

A route that would send a signal back into itself is refused, in the mixer and in the picker both:
the list of destinations only ever holds the legal ones. A project file that somehow holds a loop —
nothing here can write one — is repaired on open rather than refused, with a line in the log.

Plugin delay compensation follows the routing rather than the track list. A limiter on a bus holds
back the tracks that *do not* pass through it, so the mix stays in step; and a track feeding the
master dry while sending to that same bus has each copy delayed on its own, so the dry and the wet
still arrive together instead of comb-filtering each other.

## The time signature

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

## The metronome

The button beside the cycle button in the transport, **Transport → Metronome**, or **K** — Logic's
key for it. It clicks on every beat while the transport is rolling, an octave higher on the bar
line so the downbeat is findable without counting.

The beat it clicks is the one you *feel*, not the one the meter is written in: a bar of 6/8 gets
two clicks rather than six, because 6/8 is counted in two dotted quarters. Meter changes and tempo
changes are both followed, so a piece that moves from 4/4 into 7/8 at bar nine has its accents move
with it.

The click is laid over the mix rather than mixed into it. It is past the master fader, past the
master mute and past the meters — so it cannot be turned down by accident, it is audible with every
strip in the project muted, and switching it on does not move a single number on a level meter. It
**never appears in an export**: an offline render takes the same code path as playback in every
other respect, and this is the one line that differs, which is what guarantees a bounce cannot
contain it.

Whether it is on is stored with the project, like the cycle region, because whether a piece wants
counting in is a fact about the piece. It is not an undo step: a practice pass is a run of toggles,
and putting those on the stack would push the edits the pass was checking off the end of it.

## Languages

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

## The harmony lane

A strip under the bar ruler carries the key and the chords, spanning every track because that is
what harmony is: one thing the whole arrangement obeys at any one moment, belonging to no track.
Like the tempo map, it changes as the song goes on.

It, the structure strip above it and the tempo marks along the ruler are each shown or hidden from
the **View** menu, and where you leave them is where they are next launch. A piece with no chords
written pays fifty pixels a row for two empty strips; a piece being arranged around a chorus wants
all three. Hiding a lane hides the drawing of something and never the thing — the document keeps
its chords whether or not they are on screen.

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

The lane can also be **filled in from a melody you played**. Right-click a clip holding a tune and
choose *Accompany This Melody*: its key and one chord per bar are written here, and a bass, a comp
and a kit are added as tracks beside it. The melody itself is not touched, everything it guessed is
here to be corrected, and the parts regenerate around a correction — see
[composition.md](composition.md) for what it reads and what it cannot know.

## The structure lane

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

## Built-in instruments

Deliberately simple chiptune voices, enough to hear the engine working:

| Id | Name | What it is |
| --- | --- | --- |
| `auris.synth.chiptune` | Chiptune | Sine / square / saw / triangle / LFSR noise with pulse width, ADSR, glide, vibrato, unison and bit-crush |
| `auris.synth.fm2` | FM 2-Op | A two-operator FM voice, included to show a different synthesis method dropping in unchanged |
| `auris.synth.noisedrum` | Noise Drum | Pitch-swept noise through a band-pass, for percussion |

Square and saw are PolyBLEP band-limited, so high notes stay clean instead of aliasing.

Both pitched voices have a **vibrato**, in three controls: **Vibrato Rate** in hertz, **Vibrato**
— how far this *sound* wobbles, whatever anybody is doing with a controller — and **Mod Depth**,
how much further the modulation wheel can push it. `Vibrato` starts at zero, so a patch nobody has
touched sounds exactly as it did before any of this existed; `Mod Depth` starts at half a semitone,
because a wheel that does nothing until a parameter is found is a wheel nobody discovers.

The LFO is per voice and restarted at each note on, so a chord struck together wobbles together —
a single instrument-wide one would have every note somewhere different in its cycle, and the chord
would arrive detuned by however far the wheel happened to be up. It keeps running when the depth is
zero, so turning the wheel up mid-note picks the cycle up where it would have been instead of
jumping.

A plugin carrying all four of attack, decay, sustain and release draws them as **the shape they
are**, above its sliders: a polyline with a handle on each corner, dragged the way Logic drags one.
The attack and release corners move along the time axis and the middle one moves in both — sideways
is how long the fall takes, upwards is how far it falls to, which is the pair a hand is reaching for
anyway. The time axis is cube-rooted rather than proportional, because a default five-millisecond
attack against a two-second range is a quarter of one per cent, and a handle a pixel wide is a
control that does not exist. The sliders stay: a graph is how a shape is found and a number is how
it is said, and neither answers for the other. All four or none — the drum synth has a decay and
nothing else, and inventing two corners for it would be a picture of something that is not
happening.

## The log

**View → Log** (`⌘⌥L` / `Ctrl+Alt+L`) opens a panel holding the last five hundred records the
application wrote. It is off by default and remembered in `layout.json` with the rest of the
furniture.

It exists because a DAW is *meant* to fail quietly: a SoundFont whose file has moved costs one
track its sound rather than the session, an unknown plugin is substituted, an engine command
dropped under load is dropped. All of that is logged — and until this panel the log went to a
terminal, which a release build does not open and which nobody launching from an icon has ever
looked at. The result was a track that went silent and said nothing about why.

Newest line first, because the reason anybody opened it is the thing that just happened. The
log's icon in the status bar turns amber while there is a warning or an error nobody has looked
at, which is the only part of this that a person who has never opened the panel will see.

A **release build has no console at all** — `windows_subsystem = "windows"`, so double-clicking
`auris-studio.exe` opens the window and nothing else. A debug build keeps its terminal, because
`cargo run` and `RUST_LOG=debug` are how this is worked on, and the records go to both.

The library panel is a tree: instruments, SoundFonts and effects, each opening into
groups rather than a flat list — the plugins by category, a font by the banks it declares. Every
branch remembers whether it was left open. Clicking an instrument sets it on the selected track,
clicking an effect appends it to that track's chain.

**Every row carries a colour mark**, and the marks line up into a column down the left of the
panel. A plugin wears its category's colour and the category heading wears it too, so where one
group ends and the next begins is answerable without reading a word. A font's sounds are banded
eight at a time, which is General MIDI's own division into sixteen families — Piano, Organ,
Guitar, Bass — and is what makes a hundred and twenty-eight rows of small grey text into something
an eye can find a place in. The percussion bank is one band rather than sixteen: its patches are
kits, not programs.

Nothing in the panel *depends* on the colour. Every coloured row has its name and its number
beside the mark, the mark is never the text, and the hues are placed as far apart on the wheel as
their count allows and then walked outwards until each one clears 3:1 against the surface it is
drawn on — in all four colour schemes, which is checked rather than eyeballed.

## The SoundFont that comes with it

Two oscillators and a noise drum are enough to hear the engine working and nowhere near enough to
write anything, so a build ships with **MuseScore General** — a General MIDI set of 128
instruments and a percussion bank, under the MIT licence. It is FluidR3, Frank Wen's set that half
the free software world quotes, remastered by S. Christian Collins; choosing between the two is
choosing between an original and a curated version of itself. Every release archive carries it,
and it is in the library panel from the moment the window opens, with no import step.

The bytes are **not in this repository**. The file is two hundred megabytes, which is more than
GitHub accepts in one piece and far more than every clone of a source tree should have to carry.
What is version-controlled is the manifest — the URL, the size, the SHA-256 and the licence, in
`auris_session::library` — and the file is fetched:

```bash
tools/fetch-soundfonts.sh
```

Run it once after cloning. It puts the font in `SoundFonts/` at the top of the checkout, where a
`cargo run` build finds it; the release workflow runs the same script before assembling each
archive. `auris soundfonts` says whether it is installed and where. The script asks
`auris soundfonts --manifest` what to fetch rather than carrying its own copy of the list, so a
digest cannot be changed in one place and left stale in the other.

Where the application looks, in order: `$AURIS_SOUNDFONTS`, a `SoundFonts` directory beside the
executable, a macOS bundle's `Contents/Resources/SoundFonts`, up to five directories above the
executable — which is what reaches the checkout from `target/debug` — and finally
`~/.config/auris-studio/SoundFonts`. A build with none of them installed starts perfectly well and
has the built-in instruments, which is exactly what a CI runner does.

The shipped font is put into the document rather than left beside it, because a document is what
holds a reference. It is not an edit: no undo step, no dirty flag, and a new project that has only
been looked at is still unmodified. And because a project saved on one machine names the font at
*that* machine's path, the search for a moved asset looks in the library directories too — the
reference most likely to break when a project is sent to somebody else is also the one that always
has an answer.

## Importing a SoundFont of your own

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

### Shaping a font

The sampler carries the same four controls the built-in instruments do — attack, decay, sustain,
release — and the same draggable envelope above them. They sit *on top of* the font's own shape
rather than replacing it: a piano's hammer is still a hammer, but you can fade it in, hold it
under its own decay, or let it ring long after the key has gone.

**It is off until you switch it on.** The `Envelope` toggle in the window is the whole of it, and
while it is off the four sliders do nothing at all — the sampler is not multiplying every note by
one, it is not running the mechanism. That matters because switching it on **costs** something:

* **Polyphony drops to fifteen notes.** Channel expression is the only per-note gain the
  synthesiser exposes and it applies to a whole MIDI channel, so a note that is to be faded on its
  own needs a channel to itself. There are sixteen and one is reserved.
* **A drum kit's choke groups stop working** — an open hi-hat is no longer cut off by a closed one.
  The choke is only checked between notes sharing a channel, and now they do not.

The window says so, in a strip under the graph, for as long as the switch is on. Turning it back
off restores everything immediately, mid-note if need be.

Once it is on, the envelope owns the note. A release of zero means the note stops when the key
does, and the graph shows exactly that — a vertical drop with no tail. The default is a fifth of a
second, which is close to how a General MIDI patch already behaves.

## Built-in effects

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

Clicking an effect opens its controls in a floating window. Every control is built from the
parameters the plugin declares, so a plugin you have never seen has an editor; two of them draw a
picture as well.

**The equalizer draws its curve.** The spectrum going into it sits behind, the response it is
making is drawn over that, and each band that is switched in has a node on the curve:

* **Drag a node.** Sideways is the frequency, up and down is the gain.
* **The wheel over a node** narrows the band — forwards for tighter, back for wider.
* A high-pass or low-pass node moves **sideways only**, and sits on the centre line. Those shapes
  have a corner, not a level; the gain the slider offers is a number the audio never reads.
* A band that is switched **off** has no node and is not on the curve. Its **On** button in the
  list below is what puts it there.

The sliders under the graph do everything the nodes do and reach a little further — a band can be
set below the 30 Hz the display starts at, and Q is a number as well as a gesture. A graph is how a
shape is found; a number is how it is said.

**An instrument with an amplitude envelope draws that**, on the same principle: attack, decay,
sustain and release are one shape with three corners to drag, not four numbers to imagine.

## Recording

Click an audio track, press **Record** in the transport — or `R` — and play. Pressing it again
stops the take and puts it on the timeline as a clip, at the position the playhead was at when the
first sample arrived. The transport rolls when you start, so a take recorded against the rest of
the song lines up with it.

**The selected audio track is where the take lands**, and its **R** button is outlined to say so
before you press anything. Selecting another audio track moves the take with it; selecting a track
that could not hold one — an instrument track, a bus — leaves nowhere to record, and Record says
that rather than reaching past it for the nearest audio track.

**Filling that R button in aims a take somewhere other than where you are looking**, which is the
only thing the selection cannot say: record the vocal while you read the drum part. It stays where
you put it until you click it off, and clicking it off hands the aim back to the selection. One
track at a time either way — there is one input stream, and arming a second track moves the arm
rather than adding one. Only audio tracks show the button at all.

**Recording needs a saved project**, and pressing Record on one that has never been saved opens
the save dialog rather than refusing. The take is written straight to disk while it happens, into
the project folder's `Audio/`, so a project with no folder has nowhere to put it — and inventing a
temporary directory would mean leaving an hour of playing somewhere the machine tidies up. Files
are named after the track and numbered from the first free number: `Vocals 1.wav`, `Vocals 2.wav`.

After that first save you are not asked again, because the project is saved for you as you go —
see [Autosave](#autosave).

Takes are **32-bit float**, and there is no bit depth to choose. Every integer depth is a decision
about how much of a performance to throw away before anyone has heard it, and float cannot clip —
a singer who leant in on the last chorus is recoverable rather than square.

The input device is chosen in **Settings → Audio**, separately from the output. Choosing one does
not interrupt playback: it is opened only while a take is running.

Two things worth knowing:

* **The input and output run on separate clocks.** The take is pinned to the timeline at its first
  sample, and after that its length is counted at the input device's own rate. Between two
  different pieces of hardware those rates differ very slightly, so an hour-long take can end up a
  few frames long. Nothing corrects for it, deliberately: a take that is visibly a few frames out
  can be nudged, and one that has been quietly resampled cannot.
* **A disk that stalls costs frames**, and you are told how many. There is over a second of slack
  before that can happen. If it does, the take is still usable — but everything after the gap has
  moved earlier by that much.

## Hearing yourself

The **I** button on an audio track's header — or `U` — plays the live input through that track, so
you hear what you are playing. It goes in where the track's own material does: through its effects,
its fader, its pan and wherever it is routed. A singer hears themselves through the reverb they are
about to be recorded into, at the level the fader is set to, and a muted track stays silent.

It works with the transport stopped, which is when you set a level in the first place, and it does
not need a take running. Recording and monitoring are independent switches on the same device.

**Software monitoring costs latency and an interface's own does not.** The signal has to travel
input device → Auris → output device, and Auris holds three blocks of buffer in the middle so the
two devices' unsynchronised clocks cannot run it dry. At a 512-frame block that is around 32 ms on
top of what the hardware costs. If you are recording through an interface with direct monitoring,
use that instead — and use only one of the two, because both at once is hearing yourself twice, a
few milliseconds apart.

That is why it is a switch rather than something that happens whenever a track is armed, and why
the status line names the cost every time you turn it on.

**If the monitor breaks up, the status line says how many times.** The two clocks drift, and once
the gap between them stops being usable Auris jumps back to the live edge rather than replaying
what you have already heard. A handful over a long session is normal; a steady stream means the
machine is not keeping up with the block size, which **Settings → Audio** can change.

## Autosave

Once a project has a folder, it is written back over itself about every thirty seconds — but only
when something has actually changed, and never part way through a drag. Nothing is announced: the
unsaved mark in the title bar going out is the whole of the feedback, because a message every half
minute is a status line that never holds anything else. A save that *fails* is reported every time.

It never invents a place to save. A document that has never been saved has no folder, and choosing
one on your behalf would put your song somewhere you did not put it — so that first save is still
a question, asked once.

**What this costs is worth knowing:** it writes the real file, not a recovery copy beside it, so
**closing without saving stops being a way to undo an afternoon**. Undo still is, for as long as
the window is open. The alternative — a recovery file adopted through a dialog on the next launch
— keeps both at the price of two files that can disagree and a prompt people click through without
reading. One file that is always current is easier to reason about, so that is what this is, and
**Settings → General → Autosave** turns it off for anyone who wants the old bargain back.

## The project folder

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

## Key bindings

Every command is rebindable, and the settings window is where. Press a key onto a row and it takes
that key; ＋ gives the same command a second one; — leaves it with no key at all, which is a
different answer from putting the default back and used to be one nothing could say. Only what you
change is written to `keymap.json`, so a later change to a default still reaches you.

**Some rows start with no key.** Muting a track, soloing it, duplicating it, muting a clip: things
a right-click already reaches, common enough to want under a finger and not common enough to be
worth taking a chord away from everyone who wanted it for something else. The row is there, and
one press puts your key on it. It is the same "no key at all" that — produces, so a command you
gave a key and then took it back from settles where it started rather than in a third state.

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

## The command palette

**⌘⇧P** — Ctrl+Shift+P on Windows — opens a box that finds any command by typing a few letters of
it. `nit` reaches New Instrument Track; a match at the start of a word counts for more than one in
the middle, so `save` puts Save above Add Audio Track. Every rebindable command is there with its
keystroke beside it, which is also how you find out a command *has* one.

It also sets values, which is the part that would otherwise mean a trip to a window or to a corner
of the transport bar: type `1/16` for the editing grid, `6/8` for the meter of the stretch the
playhead is in, a colour scheme's name to repaint the window, or `日本語` to switch language — the
languages are listed in themselves, since the person opening that list is the one who cannot read
what is currently on screen.

## Settings, where dotfiles can reach them

Preferences live in `~/.config/auris-studio/` on every platform — macOS and Windows included,
rather than `~/Library/Application Support` and `%APPDATA%`:

```
~/.config/auris-studio/
  settings.json       audio device, sample rate, buffer size, interface language
  progressions.json   the chord progressions you have kept
  keymap.json         key bindings you have changed from the defaults
  appearance.json     the chosen colour scheme
  layout.json         where each panel is docked, and how large each dock is
```

Five small JSON files, readable and hand-editable, in the directory a dotfiles repository is
already checked out over. Set `AURIS_CONFIG_DIR` to name a directory outright, or `XDG_CONFIG_HOME`
to move the parent. An installation predating the move keeps its settings: the old directory's
files are copied across on the first run, never over a file already in the new place.

## Export

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

## GPU acceleration

`auris-gpu` runs the large, embarrassingly parallel offline reductions on the GPU through
[wgpu](https://wgpu.rs): waveform min/max/RMS extraction for clip drawing, and whole-file
loudness analysis. Every kernel has a CPU fallback, and the application runs correctly with no
GPU present.

Realtime per-block DSP deliberately stays on the CPU — a round trip to the GPU costs more
latency than an entire audio block is allowed to take.

## Frontends

The backend is a set of crates that know nothing about any UI, and each frontend is a thin
layer on top of the same [`Session`](../crates/auris-session) — one document, one engine, one
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
auris presets                                  # the whole songs a piece can start from
auris compose --preset city-pop                # …and one written without a file
auris soundfonts                               # what this build ships with, and whether it is here
auris new song.auris --bpm 128
auris info song.auris                          # tracks, clips, duration
auris render song.auris -o song.wav --bit-depth 24
auris collect song.auris                       # gather every file it uses into its folder
```

An MCP server is the next frontend and needs no new backend work — it is the same `Session`
API with a different transport in front of it.

