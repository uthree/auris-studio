# Features

What the application does, panel by panel. The overview is in [the README](../README.md);
how a piece writes itself is in [Automatic composition](composition.md).

## Tracks

* **Instrument tracks** — notes on a timeline, played by a software instrument.
* **Singer tracks** — notes that carry words, for a singing-voice synthesiser; see
  [Singer tracks](#singer-tracks).
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

**Every panel that scrolls says so.** A bar appears along the edge of the browser, the inspector,
the log, the lane column and the mixer's strips the moment there is more than fits, and takes no
room at all while everything does. The thumb is as long a share of its track as the view is of the
content; dragging it carries the panel, and pressing the track anywhere jumps there, because with
forty channel strips the alternative is dragging the whole way.

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

**To crossfade two takes**, drag one over the other on the same track so they overlap. The join is
shaped when you let go — the earlier clip fades out across the overlap while the later one fades
in across the same stretch — and it is part of the same Undo as the move, because the fade only
exists because the clip landed there. Nothing moves to make room: how long the join is is how far
you dragged, so the way to lengthen a crossfade is to lengthen the overlap.

**A fade you drew is never written over.** The drop shapes a join only where neither of the two
meeting edges already carries a fade; where one does, *Crossfade* on either clip's right-click
menu does it on demand.

A join uses a different fade shape from an edge. A fade **to or from silence** is a straight line
in amplitude, which is what it looks like it should be. A **crossfade** is a quarter of a sine
against a quarter of a cosine, because two straight ramps crossing sum to about three decibels
less in the middle than at either end whenever the two clips are not the same performance — a hole
in the join. The shapes are drawn as they sound: a crossfade's ramp is bowed where an edge fade's
is straight.

Either shape can be chosen by hand — *Fade-In Shape* and *Fade-Out Shape* on the clip's menu,
which appear once that edge has a fade on it. That is for the joins made another way: a fade
dragged out by its handle over a neighbour, or a clip trimmed back until it met one.

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

## Singer tracks

A singer track holds ordinary note clips — everything the piano roll does applies unchanged —
but each note can carry a **lyric** and the **phonemes** it is sung as, written in IPA. Until
a voice is chosen, the track plays through the built-in **Vocal** preview instrument (a
formant filter singing one open vowel), so a melody with words on it can be heard while it is
written.

A voice lives on the **library shelf, with the instruments**: the browser's Voices section
lists every `.onnx` found in a `Voices` folder — beside the binaries, in the configuration
directory (`AURIS_VOICES` overrides the search), or in any folder registered from the
section's own *Add Voice Folder…* row, remembered and never copied, the way plugin folders
are. **One click puts the voice on the selected singer track**, exactly as one click puts a
sound on an instrument track, and the search box finds voices by name like everything else.
**Track → Choose Voice…** remains as the file dialog for a one-off file somewhere unusual.
A voice trained on several corpora carries one **speaker** per source, and **Track → Next
Speaker** moves the track round that list, the status line saying who sings now and where
they stand in it; a single-speaker voice says so and stays. The choice is the track's, saved
with the document, and a take is pinned by the speaker the way it is by the seed. Either way
the track is pointed at a trained
voice model — one self-contained `.onnx` file, left where it lies, trained by the project in the
repository's `training/` directory — and from then on the track sings for real. The render is a **take**: an ordinary audio file in `Audio/` that
plays, exports and reopens with everything else, pinned by a seed so the same notes, lyrics,
voice and seed sing the same performance again — and kept as audio in the project, so what
was frozen is what every machine plays. **The window keeps the take abreast of
the score by itself**: shortly after an edit settles, the voice re-renders in the background
— the header badge reads *… ♪ voice* while it works, and an edit landing mid-render throws
the stale work away and starts over at the next quiet moment. **Track → Sing** remains as
the explicit ask (it shows the export overlay's progress bar and stop button), and is the
road to a *different* take: another seed is another performance. A take is never silently
rewritten to different text — between the edit and the re-render it keeps playing, with the
badge reading *! ♪ voice*, behind the notes.

Auditioning sings too: once a voice is chosen, clicking or dragging a note sounds the model
singing that note's own syllable at the grabbed pitch, rendered in the background and played
the moment it is ready. Renders are cached, so dragging across pitches is instant everywhere
the drag has already been; a track with no voice previews through the formant instrument as
before.

While a singer clip is open, the piano roll draws the **sung pitch curve** over the notes:
the contour the model is fed — pitch plus bend in fractional semitones, consonants riding
their vowel's pitch, the pitch travelling across each boundary where two notes touch, rests
leaving a gap in the line — so a drawn portamento or vibrato reads exactly as it will sound. The **phoneme segmentation** is drawn from the same frames:
a faint divider inside the note at each cut and the IPA symbol above the note where its
frames begin, so the milliseconds a consonant takes are the same milliseconds on screen
however long the note holds. Zoomed far out the symbols step aside and the dividers stay.

How many milliseconds that is belongs to the voice: a newer auris-singer export carries the
**consonant durations it measured from its own training data** (an affricate like つ's `ts`
runs about twice a plain stop), choosing the voice copies that table into the document
beside its name, and the segmentation, the boundary grab, the note preview and the render
all lay phonemes out from it. A voice without the table — or a track without a voice — uses
a fixed sixty milliseconds, the rule as it always was.

Where the model computes is a preference in **Settings → General → Singing Synthesis**:
*Auto* (the default) sings on the platform's own GPU provider — DirectML on Windows, Core ML
on macOS — whenever the runtime offers one, and on the CPU everywhere else, including
mid-render: a GPU that accepts the model and then refuses its shapes hands the render to the
CPU and it finishes. *GPU* insists, and shows the refusal as an error instead; *CPU* opts
out. The choice takes effect from the next render.

The cuts are yours to move: **drag a divider** and the phoneme to its left is pinned to the
length you gave it, stored on the note beside its phonemes so it travels and saves with the
word. The rule lays the unpinned phonemes out around the pins — the last syllabic still
absorbs the rest of the note, and pins that outgrow the note squeeze together proportionally
so every phoneme keeps sounding. Retyping the word takes its pins with it (they belonged to
phonemes that no longer exist), the note's right-click menu offers **Reset Phoneme Timing**
while any pin stands, one drag is one undo step, and the take re-renders itself afterwards
like after any other edit.

A sung note can also carry **pitch ornaments** — a **scoop** (しゃくり) rising into it from
below, a **fall** dropping away at its end, and a **vibrato** swaying around it once settled.
The note's right-click menu puts each one on with a measured default (about a semitone of
scoop over a tenth of a second; a vibrato near six hertz at a third of a semitone, fading in
after a moment) and the same rows take them off again. Each ornament then wears a small
square **handle on the drawn pitch curve**: the scoop's and the fall's sit at the corner of
the gesture — drag horizontally for how long, vertically for how deep — and the vibrato's
rides the crest of its first sway, moving its onset and its depth the same way. A scoop or
fall never takes more than half its note, so the two cannot collide; ornaments are stored on
the note beside its lyric, travel and save with it, and — being pitch, not phonemes — survive
the word being retyped. One drag is one undo step, the curve on screen is the exact contour
the voice model is fed, and the take re-renders itself afterwards like after any other edit.
Anything an ornament cannot say — a slide between notes, an off-template swoop — is still the
bend curve's to draw, and the two add together.

**Double-click a note** to type its word. **Return commits and walks to the next note**, so a
verse is typed word after word without touching the mouse; an empty field takes the word off.
The note's right-click menu offers **Edit Lyric…**, **Edit Phonemes…** (space-separated IPA,
for correcting a reading by hand — the lyric stays as spelt) and **Write Lyrics…**, which lays
a phrase across the selected notes one mora to a note: こんにちは across five notes reads
こ・ん・に・ち・は. A kanji word carries itself on the first note with `+` on the rest.

Lyrics in **kana need nothing installed** — a built-in table reads them directly. **Kanji**
goes through the Japanese dictionary, and **a release ships one**: the prebuilt `naist-jdic`
(jpreprocess's build, BSD-3-Clause) sits in a `Dictionary` directory beside the binaries —
inside the bundle on macOS — and is found the way the SoundFonts are, including from a
checkout after `tools/fetch-dictionary.sh`. The setting (*Settings → General → Japanese
Dictionary*) is an **override** for swapping in a folder of your own; *Clear* returns to the
shipped one, and `AURIS_DICTIONARY` overrides the search the way `AURIS_SOUNDFONTS` does.
`auris dictionary` at the command line says which one is answering.

**File → Export Singer Frames…** writes what a voice model consumes: one phoneme id, one pitch
in Hz and one energy per frame, as JSON, sampled at the track's frame hop (10 ms unless
changed). Pitch is the note plus its bend curve plus its ornaments; energy is the velocity shaped by an envelope
and the expression pedal (controller 11), which the preview instrument also obeys — what you
hear and what the model is told stay one story.

### Composing from lyrics

The words-first direction, modelled on
[Orpheus](https://www.orpheus-music.org/): the `compose_lyrics` tool (over MCP and in the
agent panel) takes Japanese lyrics and writes a song under them. Phrases are cut where a
singer breathes — line breaks and punctuation — each mora becomes one note, and the melody is
*searched* rather than sampled: a dynamic-programming pass over the candidate pitches,
scored so that the tune stays in the voice's range, leaps sensibly, lands its cadences on the
chord, and — the Orpheus constraint — **does not contradict the lyric's spoken pitch
accent**: the line rises where the word rises and falls exactly where its accent falls, so
the sung words stay intelligible. Chords go into the harmony lane first (王道進行 unless the
document already has its own), the standard band comes along behind, and every note lands
carrying its mora and phonemes, ready for **Sing** once the track has a voice.

In the window it is **File → Compose from Lyrics…** (also in the command palette): type or
paste the words into a multi-line field — Return breaks a phrase, and 、and ！？ cut them
too — then press Ctrl+Return (⌘Return on a Mac) and the piano roll opens on what was
written. Every run draws a fresh seed, so the command pressed twice is two takes, and the
status bar names the seed so a take can be asked for again at any of the model doors.

It is also part of **composing a whole song**: the song sheet's third column *is* the
lyrics — one multi-line box per section, in the order the form plays them, standing beside
the form itself (the parts moved to a scrolling strip along the bottom to make the room).
An `.asong` specification says the same thing as `lyrics = "..."` on a `[section]`. Click a
box and it is a real editor in place: Return breaks a phrase, Tab walks to the next
section, a click lands the caret on the character under it, and every keystroke is already
on the song sheet — Escape merely puts the keyboard down, because nothing is left
uncommitted. The margin measures the words as they are typed: each line shows the notes it
would sing (one per mora), and the box's heading tallies the bars the sung rhythm needs
against the bars the section has, turning red once the words would outrun it — computed by
the same reading and the same rhythm Write uses, so the numbers cannot drift from what
happens. Writing the piece then adds a Vocal track beside the band, one clip per
playing of each lyrical section, the melody searched over that section's own harmony; a
chorus sings the same words on every playing, which is what makes it the same chorus. Leave
a section's box empty and it is instrumental, exactly as before.

A composed vocal also arrives **ornamented, by rule**: the first note of each phrase scoops
in, any note held past half a second carries a vibrato that waits out the front of the note
and fades in, and the line's last note falls away. These are the same scoop, fall and
vibrato a hand places in the piano roll — visible on the drawn pitch curve, adjustable and
removable one by one — so the rules are a starting point, never a verdict.

The accent comes from the same Japanese dictionary the lyrics use — shipped with a release,
so it is simply there. Without one (a checkout that has not fetched it), kana lyrics still
compose — the melody is free of the accent, and the tool says so — so the dictionary is what
turns "a tune with words attached" into "a tune the words shaped". The same lyrics and seed
always write the same song, and everything written is ordinary editable material: notes,
chords, recipes, one undo step.

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

Keying an effect from a track is a route as well, and counts as one here: a compressor on the bass
listening to the kick means the kick has to be mixed first. See **Built-in effects** for what that
is for.

A route that would send a signal back into itself is refused, in the mixer and in the picker both:
the list of destinations only ever holds the legal ones. A project file that somehow holds a loop —
nothing here can write one — is repaired on open rather than refused, with a line in the log.

Plugin delay compensation follows the routing rather than the track list. A limiter on a bus holds
back the tracks that *do not* pass through it, so the mix stays in step; and a track feeding the
master dry while sending to that same bus has each copy delayed on its own, so the dry and the wet
still arrive together instead of comb-filtering each other.

## Setting the levels by listening

**Compose → Balance the Mix** renders every track on its own, measures it, and moves its fader
until the part sits where it is supposed to sit. Then it renders the whole mix and lifts it onto
−14 LUFS, which is where streaming services normalise to. Composing a piece ends by doing this, so
the command is for a piece written before it existed, or one whose instruments have changed since.

The measurement is programme loudness to ITU-R BS.1770 — the same one every broadcaster and
streaming service uses — and not a peak or an RMS. A peak is one sample and hears nothing about the
rest; an RMS weights 40 Hz the same as 3 kHz, where the ear is twenty decibels more sensitive. A
balance struck by either comes out wrong in the same direction every time: the kick too quiet
because it is peaky, the pad too loud because it is not.

What it is *for* is that a fader position is not a level. What a track is worth depends on the
instrument that answered — the composer picks the part and the session finds out which SoundFont,
if any, is installed to play it — and the same number on the same fader is a lead at −18.6 LUFS on
the built-in synth and −25.8 through the shipped font. Before this, which preset you picked decided
how loud your piece was: the eight of them spanned ten decibels, from −17.0 to −27.0 LUFS. They now
sit between −14.0 and −17.9.

Only a track that knows what it is gets moved. The composer writes down what each part is aiming at
and a hand-made track has no such number, so running this over a project you mixed yourself
normalises its loudness and leaves your balance alone. Running it twice does nothing the second
time.

It costs a render per track and two of the whole piece — about two and a half seconds for an
eight-part song, on top of the moment composing already takes — and the window does not answer
while it runs.

Two limits it will tell you about rather than hide. A fader stops at +12 dB, so a part playing an
instrument that is quieter than that can reach may end up short of where it wanted to be, and the
status line says so. And the master fader is *after* the master's effects, so a piece driven hard
after it was measured can push past the limiter's ceiling — the ceiling guarantees what leaves the
effect chain, not what leaves the mixer.

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

The library panel is a tree: instruments, SoundFonts, singer voices and effects, each opening into
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

**Its pianos work now.** A SoundFont says how a sound answers the way it is played partly in
*modulators* — "this controller reaches that parameter" — and the synthesiser library this is built
on read them and threw them away. MuseScore General's acoustic pianos set a filter low and open it
with a modulator driven by velocity, so without them the piano played through a filter nothing ever
opened: twenty decibels under everything else in the font, and *quieter* the harder it was struck,
because a velocity-layer boundary sat in the middle of the range. The library is forked in
`vendor/rustysynth` and reads them; the piano now gets louder and brighter as you lean on it, like
the other hundred and twenty-seven programs always did. Of those, a hundred and one are unchanged
to the sample and the rest move by less than 3 dB.

## Importing a SoundFont of your own

**File → Import SoundFont…** — or dropping the file on the window — reads an `.sf2` and puts its
sounds on the shelf. The library panel lists every imported font; opening one shows the banks it
declares and opening a bank shows its sounds, and clicking a sound points the selected track at
it — switching that track to the sampler in the same edit, so it is one click and one undo step
rather than two.

**The reading happens away from the window.** A two-hundred-megabyte font, and an audio file that
has to be decoded and resampled to the project's rate, are both read on a worker thread: the status
line says which file is being read, and everything else — playback included — carries on while it
is. Files dropped together are read one at a time, so a folder of takes does not arrive in memory
all at once.

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
| `auris.fx.compressor` | Compressor | Soft knee, gain-reduction metering, keyable from another track |
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

**Dragging an effect reorders the chain.** Take hold of its name and move it over another slot in
the same strip, in the inspector or on the mixer; the chain rearranges as the pointer travels, so
what you are looking at is the order itself rather than a line predicting it. Dropping it on the
empty slot at the end puts it last. The chevrons beside each row do the same thing one step at a
time, and the menu on each slot still offers bypass, reorder and remove.

**An effect can listen to another track.** The compressor does, and so does any CLAP plugin with a
sidechain input — those are the slots whose menu has a **Sidechain** row, and whose window carries
one under its title. Pick a track there and the effect keys off *that* signal instead of the one
passing through it: a bass compressor pointed at the kick pulls the bass down when the kick lands
and leaves it alone otherwise, which is how a low end with both in it stays legible.

What the effect hears is what the source puts into the mix — its own chain, fader, pan and mute.
So pulling the kick down ducks less, and muting it stops the duck altogether. The list only offers
tracks that can actually be used: one that would leave a strip waiting for itself is not on it, and
neither is the track the effect is sitting on. Deleting a track clears the keys read from it.

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
you put it until you click it off, and clicking it off hands the aim back to the selection. Only
audio tracks show the button at all.

**Arm several tracks and they all record**, each from its own input channel — a band goes down at
once, one file and one clip per track. A track takes the whole device where that is a pair or less
— a laptop's microphone, a stereo interface — and a single channel otherwise: the lowest one
nobody else is reading. So arming four tracks on a four-input interface gives you inputs 1, 2, 3
and 4 without choosing anything, and recording one track through a stereo interface records both
sides the way it always did. The status line names the input each time, because the button is one
lamp and cannot.

**To choose the input yourself**, right-click the track — in the arrangement or in the mixer — and
open **Record Input**. Every channel is offered on its own and every pair together: a microphone
is one input and a stereo keyboard is two. Picking one arms the track as well, so it is also how
you arm a track on something other than what was picked for it. Inputs are numbered the way they
are printed on the interface, from 1.

Two things follow from there being one device rather than several:

* **A channel the interface does not have records silence.** An arm outlives the box it was made
  for, so a project armed to inputs 5-8 opened on a laptop gives you those tracks silent rather
  than four copies of the built-in microphone.
* **Every track can monitor at once**, up to eight, each through its own input channels: the
  singer hears the microphone their take will be made of, at their own track's fader and through
  its effects. Past eight it says so rather than quietly listening to fewer — every ring is made
  when the device opens, because the input callback may not make one while it is running.
* **Every armed track has its own meter**, a thin bar to the left of the one that shows what the
  track puts out. It reads the channels that track is armed to, so four microphones are four
  readings rather than one number for the interface — which is what the transport bar's input
  meter is, and stays. It appears only while the track is armed and something has the device open.

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

## Counting in

Right-click the metronome button to choose a count-in: **No Count-In**, or one to four bars. Then
press Record from a standstill and the click counts those bars before anything moves — the song
does not play, the playhead sits where the take will begin, and the transport bar shows the beats
left where it normally shows the take's clock.

* **Bars are counted in the meter you are in.** Two bars of 7/8 is fourteen beats; two bars of 6/8
  is four, because 6/8 is felt in two. The tempo is the one where the take begins, and it does not
  change part way through the count.
* **It counts at bar one as well.** The count is a pause in front of the playhead rather than a
  stretch of timeline before it, so a song that starts at the very beginning is counted in like
  any other.
* **The click sounds whether or not the click is on.** Turning it on for the count and off for the
  take is the ordinary way to work, so the count-in does not ask you to leave the click running.
* **Already rolling? No count.** Pressing Record over a song that is playing starts recording at
  once: the bars are already going past.
* **Recording starts immediately, and the count is trimmed off.** The file in `Audio/` therefore
  holds the count as well — a lead-in you played early is still there if you want it back.

## Punching in

One bad bar in a good take does not need the take recorded again. Right-click the ruler → **Punch
In Here** and **Punch Out Here** to mark the stretch, or **Punch Over Cycle Region** if you have
already been looping the bars in question. The transport bar's punch button — the cycle's outline
with a record dot in it, or `⌘P` / `Ctrl+P` — switches it on, and the region is washed over the
timeline in red for as long as it is.

Then record as usual: roll from a bar or two before, play through, and **only what falls inside the
region is kept**. The transport rolls out of the take on its own at the punch-out, which is the
part nobody can do by hand with an instrument in both.

**A punched take removes what it lands on**, on its own track and only where the new clip covers.
That is the point — the bar you were fixing would otherwise play behind the fix. A clip that spans
the region comes back as two, keeping the parts outside it, and the whole thing is one Undo.

Two details worth knowing:

* **Record is still pressed by hand.** Punch says what a take *keeps*, not when one starts. A
  transport that began writing to disk because the playhead crossed a region set an hour ago is one
  nobody would leave rolling.
* **The file holds the whole take**, not just the punch — it is in the project folder's `Audio/`
  under the track's name. If the punch was set to the wrong bar, what you played is still there.
  If nothing at all fell inside the region, the status line says so and the file is still kept.

## Hearing yourself

The **I** button on an audio track's header — or `U` — plays the live input through that track, so
you hear what you are playing. It goes in where the track's own material does: through its effects,
its fader, its pan and wherever it is routed. A singer hears themselves through the reverb they are
about to be recorded into, at the level the fader is set to, and a muted track stays silent.

It works with the transport stopped, which is when you set a level in the first place, and it does
not need a take running. Recording and monitoring are independent switches on the same device.

**A switch per track**, like the arm: press **I** on as many as you like and every one of them
plays, each through its own strip. Each plays the channels *that* track is armed to read, so a
track armed to input 5 monitors input 5, one armed to 7-8 monitors that pair, and one that is not
armed monitors the first pair. A band therefore hears itself the way it will be recorded — each
player through their own fader, their own effects and their own microphone.

**Eight at once** is the limit. Every path back into the mix is a buffer that has to exist before
the input device starts running, so they are made when the device opens rather than on demand; the
ninth says so instead of quietly listening to fewer.

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

Every row answers to its English name as well as to the one on screen, whatever language the
interface is set to. A window drawn in Japanese still finds Save by `save` — that is the name the
documentation, the keystroke chart and every other audio program use, and typing is not the moment
to make somebody switch alphabets. The same goes for the key search in the settings window, so a
query means the same thing in both lists.

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

### Stems

*File → Export Stems…* asks for a folder and writes **one file per track** into it, named after
the track. `auris render Song.auris --stems Stems/` does the same from the command line.

A stem is the track **as it sounds when it alone is soloed**, which is not the same as the track
on its own: it carries the buses it is routed through, so a part sent to a reverb arrives with its
reverb rather than dry. That is the difference between a stem somebody else can mix with and a
file they have to rebuild the session around.

* **One file per track that makes a sound.** Buses do not get their own — what comes out of one is
  already inside the stems of the tracks feeding it, and exporting both would be that reverb twice
  over when the stems are added back up.
* **Muted tracks are left out** rather than written as silence, so what you get is the mix taken
  apart. A solo is ignored: it is how you are listening this minute, not what the piece is.
* **Two tracks of one name still make two files** — the second is numbered.
* **The master chain is in every stem**, because a stem is what the mix sounds like with one part
  in it. Where that chain is linear the stems add back up to the mix exactly; where it is a
  limiter or a compressor they will not, because each stem was limited on its own. Bypass it
  first if the stems have to sum.

Everything is rendered from one graph rather than one per track, so the hosted plugins are
instantiated once — but the rendering itself happens once per track, and a twenty-track project
takes twenty times as long as its mixdown. There is a progress bar and a Cancel; whatever was
finished before you press it stays on disk.

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

`auris-mcp` is the third frontend: the same session behind the
[Model Context Protocol](https://modelcontextprotocol.io), over stdio, so a language model's
harness can drive it. The tools cover the loop of writing a song and hearing it —
`spec_reference` teaches the `.asong` format by example, `check_spec` validates a draft and
answers with every default filled in, `compose` writes the piece and saves the project,
`render` turns a project into WAV files (the mix, or one per track), `describe` reads a
project back with every clip numbered, and `list_presets` / `list_progressions` are the
vocabulary a spec can quote. Answers are written for a reader that will act on the text: a
rejected spec names its lines and fields, a render reports each file's length and peak.

The loop that *improves* a piece is `analyze`: the server renders the project and listens in
the model's place, reporting loudness and peaks for the whole mix, per named section — the
piece's dynamic arc as numbers — and, on request, per track alone. Against that answer the
model either edits the specification and composes again, or aims `another_take` (same ask,
next seed) or `write_again` (same seed, follows the harmony as it stands) at one clip, using
the numbering `describe` prints. `teach_progression` keeps a chord progression by name on
this machine, and `forget_progression` takes it back out.

The mix has a smaller loop of its own, one tool per hand on the desk: `mixer` reads the whole
board — every fader, pan, send and effect parameter with its range — `set_level`, `set_send`
and `set_effect` move one each, and `section_gain` holds a track's (or the master's) gain at a
level across one named section, written as gain automation with short ramps so the fader keeps
ruling outside the stretch and holds on different sections compose. Where `analyze` says a
section is too loud, a part is buried or the master limiter is pinned, these move whole
decibels in one call instead of clawing tenths back through reseeding.

The arrangement can be edited in place, so one more part is an edit rather than a
recomposition: `add_track` puts a new track in an existing project — voiced by a built-in
instrument id (`list_instruments` names them) or by any General MIDI sound, asked for by name
("Electric Piano 1") or program number, with the shipped font adopted into the project as part
of the same step — `add_part` writes a generated part (lead, chords, pad, arp, bass, stab, or
the kit and its pieces) onto a track from the key and chords already under the song, keeping
its recipe so `another_take` and `write_again` apply to it like any composed clip, and
`set_instrument`, `rename_track` and `remove_track` do what they say.

Notes can be placed one by one, which turns the composer around: `add_clip` opens an empty
clip, `edit_notes` places and removes notes by name and position ("F#4", bar 2, beat 3.5 —
removals and additions in one call), `notes` reads a clip back numbered in time order, and
`accompany` reads a melody clip and writes the key, the chords and a backing band under it
without touching a note of the tune — the same command as the window's accompany, so a model
and a person derive the same band from the same melody. The intended shape: the model writes
the tune note by note where it wants control, and derives everything else from the harmony
where it does not.

The song can also sing through this door. `add_track` with kind `singer` makes a track whose
notes carry lyrics; `write_lyrics` lays a phrase across a clip's notes one syllable each, kana
through the built-in table and anything else through the Japanese dictionary where one is
installed (`notes` reads the words back beside the pitches); and `sing` renders the track
through its voice model — chosen once with `voice`, an absolute path to an exported `.onnx`
file — into the take that playback and `render` then play. The same determinism as in the
window: the same notes, lyrics, voice and seed render the same take on any machine.
Twenty-nine tools in all, identical at both model doors.

Registering the server with a client is one line:

```bash
claude mcp add auris -- ./target/release/auris-mcp
```

A project open in the desktop application follows edits made through this door (or by
anything else that writes the file): the window watches the file's modification time and
reloads when it changes — silently while the window holds nothing unsaved, by a Reload button
in the status bar when it does. While that choice stands, autosave holds its fire rather than
write over the other writer's version; saving by hand is how this window's version is chosen
deliberately.

`auris-agent` is the fourth frontend, and the mirror of the third: instead of waiting for a
model's harness to dial in, Auris dials the model — a local [Ollama](https://ollama.com)
server, or anything speaking the OpenAI chat-completions dialect (OpenAI itself, LM Studio,
vLLM, OpenRouter) — hands it the identical tools, and runs the loop itself. Both doors serve
the tools from one shared crate, [`auris-toolbox`](../crates/auris-toolbox), so a model that
has learnt one has learnt the other.

```bash
auris-agent --model qwen3:8b "write me a quiet piece in D dorian"
auris-agent --provider openai --model gpt-5.2 "..."          # takes OPENAI_API_KEY
auris-agent --provider openai --url http://localhost:1234/v1 --model local "..."
auris-agent --model qwen3:8b                                 # no prompt: a conversation
```

The model's answer goes to stdout and the narration of the tool loop — each call, each
result's first line — to stderr, so `auris-agent "..." > notes.md` keeps the answer and shows
the work. An API key is only ever named by environment variable (`--api-key-env`), never typed
into a command line. Without a prompt the program holds a conversation, carrying the whole
transcript forward each turn, which is the improve loop with a person in it: ask for a piece,
hear it, and say what to change.

A model that takes audio input can be handed the audio itself: `--attach mix.wav` sends the
file base64-encoded as an OpenAI `input_audio` content part beside the prompt (wav, mp3,
flac, ogg, aac, aiff, m4a — typed by extension; repeat the flag for more files), and on the
JSON wire a say may carry `"audio": ["mix.wav"]`. This is the `openai` provider's territory —
an audio-capable API, or a local OpenAI-compatible server that implements `input_audio` —
because Ollama's API has no audio field, and the agent says so up front. For everything else,
`analyze` remains the model's ears: it reads levels and peaks as numbers, which any model
understands.

The same conversation lives in the desktop application as the **Agent panel** — View → Agent,
on the right beside the inspector, the way an editor's chat sidebar sits. It spawns
`auris-agent --json` beside its own binary and talks to it over stdin/stdout, so the window
never learns what an LLM client is; provider and model are picked from dropdowns — the panel
asks the provider what it serves via `auris-agent models` — and the URL and key variable are
set beside them, all saved to the shared settings file, where the command line reads them as
its defaults too. A context gauge over the input shows the last turn's prompt tokens against
the chosen model's window, and each tool call's row opens on a click to the full answer the
model saw. The window saves the project before each message so the model
reads it as it stands, and when a tool call writes the project back the window reloads it —
automatically while nothing is unsaved, by an offered button when something is. Each tool call
shows in the transcript as it runs, with its answer's first line when it lands.

