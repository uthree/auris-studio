# Changelog

Auris Studio is at `0.x`. **Nothing is stable there** — the project file format, the
configuration files, the key binding ids and every public API may change in any release, without
a migration path. The version number is the promise, and `0` is the promise that there is none.

The release workflow reads the section whose heading matches the tag, so the headings are the
format rather than a convention: `## <version> — <date>`.

## Unreleased

### A clip can be looped

* **Drag the right edge of a clip's name bar**, or *Loop Clip* on its right-click menu, on the Edit
  menu, or **L**. The clip goes on saying itself for as long as the edge is pulled, in faded
  repeats divided by a hairline. Dragging back over the clip's own end stops it — the same gesture
  run the other way, rather than a second thing to know about.
* The **name bar's** edge, and only it. The edge below still resizes, so how long the phrase is and
  how many times it is played stay two separate things you can change. On a clip nobody has looped
  yet the two sit on the same pixel, which is what makes the gesture findable at all.
* A loop is a **length rather than a count**, so the last repeat is cut off wherever the edge was
  let go. That is what makes the drag continuous, and it means a loop can stop half way through a
  bar because that is where the next clip starts.
* Both kinds of clip. On audio the fades stay on the clip's own two edges and the joins between
  repeats run flat: a fade-out at the end of every pass would pump once a bar.
* The repeats are **flattened when the graph is built**, so the audio thread never learns that a
  clip repeats — it plays a list it cannot tell from a song somebody duplicated by hand. Exports
  carry the repeats too, WAV and MIDI both, since a MIDI file has no notion of a region that
  repeats and the notes are the only honest way to write one down.
* Splitting a looped clip leaves neither half looping, because the repeats were of a block that no
  longer exists. Duplicating one puts the copy past the repeats rather than on top of them.
* **`FORMAT_VERSION` is 10.** The field carries backwards on a default, which normally does not move
  the number — but a version 9 build would open a song whose drum loop runs thirty-two bars, play
  the one bar, and write that back on the next save with the other thirty-one gone. Refusing at the
  door is the only honest answer.

### Notes can be quantised

* **Quantise Starts (Q), Quantise Lengths, and Quantise Both**, on the piano roll's right-click menu
  and on the Edit menu. Nothing could put a played part back on the grid after the fact; snapping
  applied while something was being dragged and not one moment later.
* Three commands rather than one with a setting, because the two numbers a note has are separately
  wrong. A part played a shade ahead of the beat wants its ragged lengths evened out and its feel
  left alone, and doing both to a take that needed one is how it stops sounding like anybody played
  it.
* They snap to the **division the grid button is showing**, which is on screen above the notes being
  moved: quantising to a value nobody can see is a jump with no explanation.
* A length never rounds down to nothing. On a sixteenth grid a clipped grace note becomes a
  sixteenth rather than silence — a note vanishing because it was played crisply is not a
  tidying-up.
* The status line says how many notes actually moved, which is the one thing worth knowing
  afterwards: four out of twenty means the other sixteen were already where they should be.

### There is a metronome

* **The button beside the cycle button, Transport → Metronome, or K.** A click on every beat while
  the transport rolls, an octave higher on the bar line. The application has had a tempo map, a
  meter map and a bar ruler since it was written, and no way at all to hear any of them.
* It clicks the beat you **feel** rather than the one the meter is written in: a bar of 6/8 gets
  two clicks, not six. Meter and tempo changes are both followed, so a piece that moves into 7/8
  at bar nine has its accents move with it.
* The click is laid **over** the mix — past the master fader, past the master mute, past the meters
  — so it cannot be turned down by accident, it is audible with every strip muted, and switching it
  on does not move a level meter. It **never reaches an export**: playback and an offline render
  take the same code path in every other respect, and this is the one line that differs.
* Stored with the project, like the cycle region, because whether a piece wants counting in is a
  fact about the piece. Not an undo step, for the reason cycling is not: a practice pass is a run
  of toggles, and those would push the edits the pass was checking off the end of the stack.

### Cut, copy and paste

* **⌘X, ⌘C and ⌘V**, meaning notes in the piano roll and clips in the arrangement — the same three
  keys scoped to wherever the keyboard is, exactly as ⌘A and ⌘D already were. On the Edit menu in
  pairs, and on the right-click menus of both surfaces. Duplicate has existed since the beginning
  and only ever laid a copy down *next to* the original; there was no way to move material
  anywhere else at all.
* What is on the clipboard is a **shape** rather than a place. Notes keep the gaps between them; a
  block of clips copied off four tracks lands on four consecutive tracks wherever you aim it, and
  goes on doing so after the tracks it came from have been reordered or deleted, because it was
  never holding those tracks' ids.
* A paste lands at the playhead; *Paste Here* on an empty lane lands it under the pointer, which is
  the one place a paste has a position of its own. What arrives becomes the selection. A paste that
  fits nowhere — the wrong kind of track, or rows running off the bottom of the list — lands what
  it can rather than failing whole.
* Its own clipboard, not the system's. Nothing here reaches another application and nothing copied
  in one arrives here.

### A melody can be given an accompaniment

* **Right-click a clip holding a tune → Accompany This Melody**, or Compose → Accompany the Melody.
  Its key is worked out and written into the harmony lane, one chord per bar is written under it,
  and bass, chords and drums are added as tracks *beside* it. The melody is not touched. One undo
  step for the lot.
* The composer could write a whole song from a specification and could write one part from chords
  that were already there. What it could not do is the thing a person actually has in front of
  them: sixteen bars they played, and no idea what goes underneath.
* The key comes from correlating what the melody plays — weighted by note length and by how hard
  each is struck — against Krumhansl and Kessler's probe-tone profiles. Each bar takes whichever of
  the key's seven triads accounts for most of it, with a thumb on the scale for what the bar
  *arrives* on and a little inertia so the progression does not change on every coin toss. Nothing
  draws a random number, so changing one note and pressing it again says what that note was doing.
* **It will be wrong sometimes, and it is built to be argued with.** A melody is one voice: a tune
  in A minor and a tune in C major play the same notes, and a bar of passing notes reads as the
  chord it passes through. So everything it guessed goes into the harmony lane where it can be seen
  and retyped, and every part it writes carries a recipe — correct a chord, press *Write It Again*,
  and the band follows.
* Each part gets a fitting General MIDI sound where the shipped font is installed, and the built-in
  oscillators where it is not, which the status line says.

### The library list is readable

* **Every row carries a colour mark**, and the marks line up into a column down the panel. A plugin
  wears its category's colour and so does the heading above it. A font's sounds are banded eight at
  a time — General MIDI's own sixteen families, Piano, Organ, Guitar, Bass — which is what turns a
  hundred and twenty-eight rows of small grey text into something an eye can find a place in. The
  percussion bank is one band rather than sixteen: its patches are kits, not programs.
* Nothing depends on the colour. Every coloured row still has its name and its number beside the
  mark, the mark is never the text, and the hues are spread as far apart as their count allows and
  then walked outwards until each clears 3:1 against the surface it sits on — in all four schemes,
  which is checked rather than eyeballed. A fixed lightness put one group in ten at 2.7:1 on
  Midnight, because lightness is not luminance and the gap between them is widest across the hues.

### A note can be placed without holding anything

* **Create can be a plain click.** ⌘-click is Logic's, and it is still the default, but holding a
  modifier to write a note is a thing you have to be told — and the first person to try this
  without being told said so. The Pointer section of Settings now offers the bare click alongside
  the three modifier gestures.
* Choosing it moves the rubber band to ⇧-drag, and a click on empty arrangement then makes a clip
  rather than moving the playhead. The settings page says both at the moment you choose it, and
  they are why the modifier is still the default. ⇧ already means *extend the selection* on every
  other press, so the gesture was there to be used rather than invented for the occasion.
* **Deleting cannot be a plain click**, and is not offered. Creating on a bare click leaves
  something you can see and undo; deleting on one would remove every note you reached for, and
  would leave no gesture anywhere meaning "just this one".

### Sixteen more commands you can put a key on

* Mute, solo and duplicate for a track; duplicate, split-at-playhead and mute for a clip; select
  all, duplicate, and transposition by a semitone or an octave for notes; add a bus. Every one of
  them was already in a right-click menu and reachable from nowhere else, which meant that
  working from the keyboard stopped at the point of actually editing anything.
* **A command can now ship with no key at all** and still be in the list. Mute wants M, solo wants
  S, and the mixer and the structure lane hold both; inventing ⌥⇧K so the row had *something*
  would take that chord from whoever wanted it and bury the commands that earned their key. The
  row is there with a dash on it, and one press puts your key on it.
* ⌘A and ⌘D mean the notes in the piano roll and the clips in the arrangement — the same key,
  scoped to where the keyboard is, which is what the panel outline has been telling you all along.
  ⌥↑ and ⌥↓ transpose by a semitone, ⇧⌥↑ and ⇧⌥↓ by an octave, ⌥X splits a clip, ⌘B adds a bus.
* The settings page groups them as **Notes** and **Clip** rather than as a second **Edit**
  section, which is what a second run of the same group would have printed.

### Composing has a menu

* **A Compose menu**, holding the song sheet and the specification file. It was one row in the
  middle of File, between Open Project and Save — and that row carried the label of the
  *specification file* route while dispatching the song sheet, so the way in that needs no file
  was announced as "Compose from Specification…" and the file route was in no menu at all.

### The tune is a line rather than a walk

* **A third of every melodic interval the composer wrote used to be a fourth or wider.** That is an
  arpeggio's interval distribution, not a tune's, and it is why a composed melody sounded unnatural
  while the accompaniment underneath it — which is a function of the chord and so is right or wrong
  locally — sounded like players. It is now one in seven, against the one in five a corpus of real
  melodies gives for leaps of *any* size.
* The measurement, the literature it is read against, what each of the five rules is for and what
  is still wrong are in **`auris_compose::melodic`**, which is a page of documentation and no code.
  The constants in the melody writer are what it argues for; neither makes sense without the other.
* What changed: the restated figure is *joined* to where the last bar left off instead of restarting
  from its structural pitch — the single worst fault, and one nothing had chosen; the interval table
  is the corpus distribution and has an entry for a repeated note, which it did not; the walk has a
  memory, so a leap is filled in and a step tends to carry on; a dissonance left by a leap resolves;
  and a phrase ends on a chord tone with a beat of air after it.
* No chord and no note count moved in any of the four fingerprint fixtures, which is the report on
  the change: the pieces are the same pieces with a singable line in them. Existing projects are
  untouched — this writes new material and does not migrate old.

## 0.2.0 — 2026-08-07

### What an adversarial read of the composer's harmony found

The music theory, gone through looking for chords the composer plays that nobody wrote. Every one
of these was silent: the wrong chord sounded, the document recorded the numeral that was asked for,
and nothing anywhere said the two had stopped agreeing.

* **A numeral means the same chord wherever it is typed.** Colouring built its chords by hand
  instead of asking the numeral, so a borrowed chord and a seventh took different paths to the same
  question and answered differently. The whole of it now goes through `chord_in`, which is the one
  place that knows what a numeral means.
* **A seventh comes from the key rather than from the triad it lands on.** `vii7` in a harmonic
  minor key came out half-diminished where the key builds it fully diminished — a distinction the
  triad alone cannot make, and the leading-tone chord is where it matters most.
* **A lead-in is a fifth above the tonic it arrives at, in every mode.** It was built from the
  scale's fifth *degree*, which in phrygian and locrian is not a fifth above anything: a
  modulation into a locrian section was prepared by a chord a tritone from where it was going.
* **`ii/V` is the supertonic of V.** Everything in front of the slash was thrown away and every
  applied chord came out as the dominant seventh of its target — `ii/V`, `vii/V` and `IV/V` all
  parsed happily and all sounded as V7-of-V. An applied chord is now read in the key its target
  would be the tonic of, which is what the notation has always meant. `V/x` still takes its
  seventh: the tritone pulling into the target is the whole point of writing one.
* **A sixth leaves the fifth under it alone.** `Major6` and `Minor6` both hold a *perfect* fifth
  and the sixth was handed out on the strength of the third, so `vii6` came out with a perfect
  fifth and was no longer diminished.
* **A section ends where its progression ends.** A four-bar progression under a six-bar section
  played bars 1–4 and then 1–2, stopping in the middle of the loop; it now plays the whole thing
  and fills from the *end*, so the section lands on the chord the progression resolves to.
* **The octave figure moves an octave.** The bass folded `root + OCTAVE` back into its range, and
  the range is two octaves wide with the roots in the upper one — so for four of the seven degrees,
  the subdominant and the dominant among them, the leap was subtracted straight back and the bass
  restruck the note it was already on.
* **The bass is the bottom of the arrangement.** The pad ran from C2 and the bass from E1, sharing
  sixteen semitones, so a pad voicing could put a chord tone *under* the bass note — an inversion
  nobody wrote, decided by whichever tone happened to fold lowest. The pad now runs C3 to C5, and a
  test holds every pitched role above the bass's floor. No part may read another's notes, so the
  ranges are where this has to be settled.
* A tie between two scale degrees now rounds down rather than off the top of the scale.

### Compound time is counted in dotted beats

* **6/8 is two beats, not six.** The grid divided the note the denominator names, which made a
  "sixteenth" in 6/8 a thirty-second — the grid came out twice as fine as everything placing notes
  on it believed. A step is now a fixed note value in every meter, and the *felt* beat is derived:
  six sixteenths to a dotted quarter. Every part that asks "am I on a beat" gets the answer the
  meter actually has.
* **The metric hierarchy no longer offers a compound beat a halfway point.** A dotted quarter
  divides in three and in nothing else; its midpoint is a syncopation against the meter rather
  than a position the meter offers, and weighting it as a beat handed real weight to the one step
  in 6/8 that most needs to be heard as a departure. Swing is off in compound time for the same
  reason: the shuffle is already there.
* **A groove is mapped onto the bar rather than wrapped round it.** The built-in grooves are one
  bar of 4/4, and under a 6/8 bar the pattern restarted partway through, putting a second downbeat
  where the bar has no beat at all; under 3/4 the turnaround simply never played. The bar's first
  beat now takes the groove's first and its last takes the groove's last, which is what a drummer
  does with a pattern in a meter it was not written for.
* **`six-eight` and `slow-blues`** are grooves written *in* compound time — in eighths of a dotted
  beat rather than sixteenths of a plain one — so a song in 6/8 or 12/8 has a two-beat and a
  four-beat idea to reach for instead of borrowing a four-beat one. A groove now carries how many
  steps make one of its own beats. Nothing picks them automatically; a song names them the way it
  names any other groove.
* The bass reads the kick the same way the drummer does. It was reading the raw step index, which
  wrapped a groove shorter than the bar and truncated a longer one — so in every meter the groove
  was not written for, the bass followed a kick the kit was not striking.
* A rhythm somebody writes by hand is still a repeating cell, because that is what writing four
  steps under a 4/4 bar means.

### The panels answer the pointer

* **The song sheet's dials follow the mouse again.** The sheet is drawn on an occluding overlay,
  and gpui's hit test stops at the first hitbox that blocks — so the root's pointer handlers, which
  every drag in the application is tracked by, never saw a move over the sheet. A dial could be
  pressed and would not turn.
* **The piano roll draws the rest of the track.** The bars either side of the clip being edited
  were empty grid, so there was no way to see what the phrase before it ended on or what the next
  one starts from without closing the roll. The neighbours are now drawn behind it, flat and faint
  — no velocity in the fill and no selection outline, because a ghost that read like a note would
  be an invitation to edit something the roll will not edit.
* **The mixer scrolls, and says so.** A flex item's `min-width` is `auto`, which is the width of
  its content — so a panel holding fifteen channel strips asked the dock for the width of fifteen
  channel strips and got it. Nothing overflowed, because nothing was ever too small; the strips ran
  off the side of the window where no scroll could reach them. There is now a scrollbar under the
  strips, drawn only when there is something to scroll, with the thumb draggable and the track
  clickable to jump.
* One picker row and one way to open a menu. The song sheet, the inspector and a plugin's choice
  parameters were three copies of the same control, and twenty-eight call sites each wrote out the
  same eight lines to open a context menu.

### A section can change how a part plays

* **`[section.chorus.part.lead] octave = 6`.** A part was one setting for the whole song: whatever
  density, octave, gate and subdivision the roster gave it, it played that way from the first bar
  to the last. A section can now patch any of those, plus `rhythm` and `note` — the lead an octave
  up in the last chorus, the hat on sixteenths in the bridge.
* A **patch**, not a second declaration: what it does not name it does not touch, so a busier
  chorus is one line and adding a field to a part does not silently reset it in every section that
  tweaks one.
* The resolution happens once per part per section, and every pass reads it. That is the whole of
  the change and the only part of it with a trap: `shorten` and `humanise` run over the finished
  part *after* every section has been written, so a gate or a subdivision read off the roster there
  would be the one kind of per-section field that silently does nothing — a chorus cut to the
  verse's note lengths, or a section on triplets having its swing measured against sixteenths.
  Both are pinned by tests that fail when the resolution is taken away.
* Not patchable, by construction: the name, the role, the instrument, the program, the level and
  the pan. Those are not how a part plays, they are what its *track* is — one row, one instrument,
  one fader for the whole song. A chorus on strings where the verse was on a piano is two parts and
  the section roster is what brings each of them in. The line is not waiting to be lifted: a track
  that changed instrument half way through would have to be two tracks, and then it was two parts
  all along.

### A key change is arrived at rather than stumbled into

* **The last chord before a modulation becomes the dominant of the key being arrived at.** A
  transposed section used to begin and that was all: the piece stepped sideways and a listener
  heard the join as an edit. A `V7` names its tonic before that tonic has sounded, which is why
  every arranger reaches for it first and why nothing else does the job.
* One event, replaced in place — the section keeps its bars and its clips keep their lengths — and
  only where the key actually changes, so a piece that does not modulate is untouched whatever the
  field says. It runs *before* the melodic skeleton is chosen, or the tune would be the one part in
  the band still playing the chord that used to be there.
* The section keeps its own key and the chord is renamed against it, exactly as a borrowed chord
  is. The lane draws one key change, at the bar where it happens, with a chromatic chord leaning
  into it — not a second modulation half a bar early.
* This is the one thing in the format that rewrites a bar of a progression quoted by name. The
  trade is deliberate and `lead_in = "none"` refuses it: a modulation is a structural instruction
  asked for by hand, it outranks a chord chart, and there is no way to prepare a key change without
  changing the chord that prepares it.
* The composer's fingerprint test now compares chords rather than the text of their names. A
  numeral knows which letter its degree demands and a chord only knows whether its key leans sharp
  or flat, so B♭ and A♯ are one chord written twice — and the test flagged that as the two
  disagreeing.

### No bar takes the wheel

* **Sliders no longer answer a scroll.** Faders, sends, plugin parameters, clip dials, the song
  sheet's dials and the zoom sliders all took one. Every one of them sits inside a panel that
  scrolls, so rolling down a column of tracks changed the level of whichever fader the pointer
  crossed on the way — silently, with no drag to remember having started, and nothing on screen
  saying which one moved. A bar is swept with the pointer, and that is now the whole of how it is
  edited.
* The handler is gone from `value_slider` and `zoom_slider` themselves rather than from their
  callers, so there is no parameter left to pass one to and it cannot come back one control at a
  time. Scrolling still means scrolling everywhere it used to — the arrangement, the roll, the
  keyboard, the automation strips — and zooming by wheel still works over the timeline and the
  roll, which is where it was always reached for.
* The transport bar's tempo and signature readouts keep theirs. They are typed fields in window
  chrome rather than bars, nothing behind them scrolls, and the wheel there is a documented way to
  reach a near neighbour.

### A section can play at a tempo of its own

* **`[section.chorus] tempo = 132`.** A composed piece ran at one speed from the first bar to the
  last: the specification had a single `tempo`, and the whole thing arrived at the document as
  `set_bpm`. A section now names its own, the composer hands over a `TempoMap` rather than a
  number, and the changes are on the timeline's tempo lane where they can be dragged like any
  others. A point is written only where the tempo actually changes, on the same rule the key lane
  already followed.
* The wander follows it. Humanisation asks for a scatter in *milliseconds* and has to convert that
  into ticks, which needs a tempo — so the conversion is now per section. A chorus lifting from 60
  to 180 would otherwise have been scattered by the verse's number of ticks, which is three times
  the time the dial asked for, and that is exactly the failure the millisecond conversion was
  written to stop. `ScoreSettings` no longer carries a tempo at all: it lives on the section plan,
  in one place, so the two cannot disagree.
* It is a **step**, and that is stated rather than glossed. A ritardando slows *through* a passage;
  a section is a stretch of bars. Neither the specification nor `TempoMap`, which is
  piecewise-constant, can express a continuous change, and none of this pretends to.
* The meter is still one for the whole piece. Unlike the tempo, changing it changes the length of
  a bar, and every part is written against one grid.

### A section chooses who plays it

* **The song sheet can sit a part out.** `[section.x] parts = "…"` has been in the format the whole
  time and worked end to end, and there was no way to reach it without hand-editing a `.asong` —
  so a piece composed from the sheet was the same roster from the first bar to the last, however
  long it ran. Every section row now has a `7/7` button listing the roster with a tick against the
  parts that come in.
* The rule the button obeys is not a set toggle, and could not be. An empty list means
  *everything*, so switching the hat off in a section that names nobody has to write down the
  other six rather than remove a name from an empty list — which is what a plain toggle would do,
  and it does nothing at all. Turning the last one back on says everybody again rather than listing
  them, or the section would go on naming six when a seventh part is added and would be the one
  section that new part silently does not play in. The last part left cannot go: a section playing
  nothing is silence, and the spelling for it is already taken.

### Eight presets are eight draws

* **Every preset ships a seed of its own.** All eight left it at the default, so the shipped songs
  were eight arrangements over *one* set of random numbers — the same figure fell in the same bar
  of every piece, and hearing all eight was hearing one draw eight times. Which numbers they are
  does not matter and nothing claims it does; a test pins that no two are the same and that a
  ninth preset added without one fails rather than quietly rejoining the pile. Checked across
  seeds 0 to 8 of every preset: no draw loses a part, and none of the 72 reaches the master
  limiter, which stays where it was put — dormant.

### A cymbal marks where the form arrives

* **The kit has a crash.** A composed piece had nothing at the joins of its own form: the section
  changed and the only thing that marked it was a snare fill running into a bar that sounded like
  every other bar. `crash` is a new part role, and the writer behind it reads the *form* rather
  than a groove — it strikes the downbeat of a section that arrives at something at least as
  strong as the one before it, and stays silent where the arrangement is coming back down. The
  shipped pop form gets three: into the verse and into each chorus, and none on the verse after a
  chorus or on the outro. Six of the eight presets carry one. `DrumVoice::Crash` had existed the
  whole time and every groove returned an empty pattern for it, because a bar-long loop is the
  wrong shape for a thing that happens once a section.
* **The built-in cymbal is voiced as one.** `auris.synth.noisedrum` is a tom — noise through a
  band-pass swept down from where the note puts it — and at its defaults a part striking 49 came
  out at a spectral centroid of **342 Hz, ringing 595 ms**, which is a low tom under the name of a
  crash. It is now 3.6 kHz and 945 ms. A composed track can carry plugin parameters for the first
  time, and this is the only thing that uses it: opening the filter that far let through 13.5 dB
  more than the built-in snare across the first 300 ms, so the voicing carries the level that puts
  it back. The five General MIDI kits the presets use already place their crash within 1.4 dB of
  their own snare, and both sides are then separated by the same role gain.
* Worth knowing, and deliberately *not* changed: the rest of the built-in kit is the same
  algorithm at the same defaults, told apart only by which note each part strikes — measured, the
  kick, the snare and the hat sit at 190, 215 and 246 Hz, three thuds within 56 Hz of each other,
  and nothing about 246 Hz is a hi-hat. That is what the one preset on the built-in voices has
  always sounded like, and revoicing it is a decision about a preset rather than part of adding a
  cymbal.

### The composer keeps time, and a velocity means one thing

* **The kit does not wander.** Timing humanisation applied to every role including the drums,
  which is not what a kit does — the shipped presets scattered theirs by 4.9 to 14.0 ms, with
  single hits reaching 28.8. The kick, the snare and the hat now sit exactly on the grid. They
  keep their *lean* — the hat a little early, the snare a little late, the same whole number of
  ticks in every bar — because that is a player leaning and not a player being unreliable.
* **The dial reaches zero, and means the same thing at any tempo.** The wander was
  `6 + 19 × humanize` ticks, and the six was multiplied by nothing, so the dial was a step
  function with no setting between "quantised" and "±6 ticks". It is now **15 ms at the top of
  the dial**, converted through the tempo — so ambient at 64 BPM stops being three times looser
  than rock at 148 for no reason anybody chose. A generated clip reads the tempo underneath it
  rather than assuming 120.
* **A velocity means the same thing on every instrument.** The built-in voices were linear and
  the SoundFont sampler was squared, which is the SF2 default and what rustysynth implements —
  so the composer, which writes velocities for a linear instrument, got twice the dynamic range
  in decibels through the font. A part written MIDI 26 to 126 measured **27.4 dB through the
  sampler against 13.7 through a built-in voice**; it is now 13.8. This is a deliberate
  disagreement with other SoundFont players: a DAW where one number means two things depending
  on what is loaded is worse than one that is consistent with itself.
* **A composed piece is audible.** The sampler was voiced 11.5 dB below the rest of the
  application, so a composed mix landed 14 to 19 dB under a finished record. Composed mixes are
  now **13 to 16 dB louder**. They are still 1.5 to 8.3 dB short of a mastered piece, which is a
  crest-factor problem — arrangement and bus compression — and not something a gain constant can
  reach.
* **The shipped font's drum kits are brought level with each other.** They sit 7.95 dB apart at
  unity, which is calibration noise rather than a musical statement, and once everything got
  louder that error landed above full scale — city-pop clipped once a bar. A measured per-kit
  trim is applied where a composed part resolves to a kit, and a composed document carries a
  limiter on its master at −0.3 dB: dormant on 121 of 128 seeds of the one preset that needs it,
  and never touched by any other.
* Existing projects that use the sampler will be about 12 dB louder and half as wide in decibels.
  Nothing needs converting; the faders are where they always were.

### What a review of the whole thing found

Nineteen defects, from an adversarial read of every crate. The pattern worth naming is
that most of them are *asymmetries* — one branch of a pair doing the right thing while
its sibling does not, with a test on the correct half and none on the other.

* **A tempo change no longer erases the ones before it.** A tempo event at tick 0 in a
  later track — which format 1 files write routinely — threw away every change already
  read, and a file whose first tempo arrived partway in played its opening bars at that
  tempo instead of the default. The time-signature branch beside it had both cases right.
* **Stopping the transport lets go of the vibrato.** `Fm2::reset` zeroed the modulation
  wheel without re-deriving the depth it feeds, so a Stop, a Seek or a Panic taken mid
  curve left every later note swinging about fifty cents from a wheel nobody was holding.
  The chiptune never had this.
* **The composer writes down the chord it actually played.** Colouring rewrote the chord
  and left the numeral, and the numeral is what gets stored — so the harmony lane painted
  `Fm` over parts playing F♯ minor, and a generated Chords clip wrote the lane's version.
  Not one note moves; what changes is what the document says about them. The ambient
  preset was the same fault written by hand: `IVmaj7` in C lydian is F♯maj7, a tritone
  from the tonic, and it now uses the mode's own chords.
* **Trimming the front of a short clip does nothing instead of the wrong thing.** A clip
  shorter than the editing grid had its own floor applied as a ceiling, so touching the
  front edge moved it left, made it longer, and in the first bar drove its start negative.
* **An instrument takes its automation with it.** Swapping a track's instrument cleared
  the saved parameters and left the lanes, which bind by track and raw parameter id — so a
  curve drawn for one plugin swept an unrelated control on the next, in the exported file
  as well as in playback. An audition of a second SoundFont preset still keeps its curves.
* **A missing sample is no longer replaced by any file wearing its name.** The search
  passed no expected size, so the first match on name alone was adopted and written into
  the document. `AudioSource` now carries the fingerprint the SoundFont reference always had.
* **Save As takes a collected SoundFont with it.** A font stored inside the project folder
  was carried across as a reference to a file that was not there, and the copy opened
  elsewhere silent — with Collect Assets then answering "nothing to do".
* **A saved file carries the version of the build that wrote it**, rather than the version
  it was loaded with.
* **A muted track lets go of what was played into it.** Auditioning into a muted track
  filled a queue nothing drained, discarding note-offs once full and leaving voices
  sounding after the unmute.
* **The curve lane's grab zone is seven pixels at any scroll.** It was measured as a
  position rather than a length, so five bars along it had swollen to most of a bar: a
  press on empty strip seized a distant point, and a second point could never be added.
* **The arrangement lets go of a deleted clip and takes hold of what is drawn.**
  Alt-clicking a clip out of a swept selection left a dead id behind, which surfaced later
  as a failed Duplicate; and the rightmost column of a section's grab bar did nothing.
* **⌃⌘ chords can be bound on macOS**, and Ctrl+Win chords off it — both dropped a modifier
  and stored a chord the user had not pressed. The settings footer now shows ⌘S where it
  used to print `secondary-s`.
* **An "inside" asset path cannot escape the project folder on Windows.** A drive prefix in
  a hand-edited or shared document walked out of the folder the way a leading slash does.
* Counts in the README, the guide and `CLAUDE.md` that the code contradicted.

### A slash bass keeps its accidental

* A numeral's slash bass now carries an accidental of its own, so `v/b7` is a symbol a chord can
  be stored as. Without it, respelling a progression into a key whose scale is not the major's —
  harmonic and melodic minor, dorian, phrygian, locrian — resolved the bass as that key's own
  unaltered degree, and `@junjo` in A harmonic minor came out as `Em/G#` where it should be
  `Em/G`: a bass part sounding a minor second against the chord's own third, and the wrong numeral
  written into the document.
* **`Project::FORMAT_VERSION` is 9.** A version 8 build has no reading for the accidental — it
  falls through to the secondary-dominant branch, finds no roman numeral, and rejects the numeral,
  which fails the whole document rather than the one chord. The version is what makes that happen
  at the door rather than halfway through a harmony lane.

### A General MIDI SoundFont comes with it

* **Auris Studio now ships with MuseScore General**, 128 instruments and a percussion bank under
  the MIT licence. It is in the library panel from the moment the window opens, with nothing to
  import. Two oscillators and a noise drum were enough to hear the engine working and never enough
  to write anything, and "install a SoundFont from somewhere" is not a first five minutes anybody
  enjoys.
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

### The modulation wheel goes all the way through

* **View → Modulation** (`⌘⌥W`) puts a second strip under the piano roll, beside the bend. A clip
  carries the curve itself, the engine schedules it, the instruments answer it, and a `.mid` takes
  it out and brings it back as controller 1.
* One set of gestures and one painter for both strips. They differ in exactly two things — the bend
  goes both ways from a line across the middle, the wheel goes up from a floor — and two copies
  would have been two chances for the wheel to behave differently from the bend for no reason
  anybody could see. The same goes for the four session commands, which now take *which* curve.
* A clip's bend is now a `CurvePoint` list shared with its modulation, so the stored field is
  spelt `value` where it was `semitones`. **`Project::FORMAT_VERSION` is 8**: a version 7 document's
  bends would otherwise read as zeroes, silently, because the field has a default — and a slide
  somebody wrote would simply stop happening.
* Like the bend, a modulation curve that does not end at zero is let go before the clip ends. Both
  are channel state, and a clip finishing with the wheel up would leave everything after it
  wobbling.

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
