# Shaping a clip's performance

Select a MIDI clip and use **Performance** in the inspector. These controls work on authored
and generated clips. They change playback and MIDI export while preserving the
original score. Save stores the controls and seeds; disabling a control restores the score's
behaviour for that stage. Dragging a control is one undoable edit.

The piano roll and drum editor have two tabs: **Source** edits the stored notes;
**Performance** previews the notes sent to playback, including inserted articulations,
timing and velocity changes, and each loop pass. The preview updates while you drag an
inspector control, and after undo or redo. It is read-only: switch to Source to edit notes.
Both tabs share the same scroll and zoom so you can compare them in place. Singer pitch
contours and editable expression lanes remain in Source.

Open **Ghost note settings** below the main controls for independent density, duration,
and placement: sixteenth-note gaps, eighth-note offbeats, pickups before an attack,
or a repeating bar pattern. Variation changes how often the repeating pattern takes a
fresh draw in a bar or loop pass. Density changes select more or fewer positions from the
same seeded take; changing loudness does not reroll it. New ghosts default to pickups.
**Preserve long rests and phrase endings** leaves the final tail and gaps over two quarter
notes silent. **Ghost source** can restrict generation to one MIDI pitch (for example the
snare); other kit voices can continue playing underneath it. Existing brush settings use
the same placement engine with their original full-density sixteenth grid.

**Strum settings** adds a continuous eighth- or sixteenth-note hand clock. With alternating
direction, rests advance the hand too; inserting another chord does not flip all later
strokes. The clock follows the project bar lines, including when a clip starts mid-bar.
New strokes use the sixteenth-note clock; existing strokes retain their attack clock until
changed. Upstrokes can strike only the highest one to four pitches and have their own
velocity scale. Downstrokes can emphasize lower pitches. This uses pitch order, not a
guitar fingering model. Skipped pitches remain in Source; freezing explicitly removes
those attacks from the visible phrase while preserving hidden notes outside the clip.

**Expression & groove** separates timing wander from velocity wander without changing the
take's seed. The main Humanize control scales both while preserving their current ratio.
Phrase swell adds a crescendo and decrescendo between the first and last attacks; a rest
longer than a quarter note starts a new phrase. Beat accent is signed: positive emphasizes
quarter-note beats, negative emphasizes eighth-note offbeats. Push / lay back moves the
performance by up to 50 ms in either direction, bounded by the clip's start.

**Shared motion** blends private variation with a timeline-aligned gesture. Clips using
the same ensemble group share that gesture across tracks, pitches and loop boundaries;
their independent timing and velocity strengths still apply. This common pulse is measured
in beats at the 120 BPM humanisation scale so clips with different start tempos agree.
The private component retains its millisecond scale.

**Groove reference** captures another MIDI clip's first performed pass on a sixteenth grid.
It copies timing offsets and relative dynamics, not pitches, durations or note count.
Chord attacks are averaged at each slot; empty reference slots leave the target unchanged.
The pattern repeats over the reference clip's length, rounded up to the grid. Timing and
dynamics have separate strengths. A new groove joins before other stages so it cannot
collapse a later strum. The saved template is independent of its reference: editing or
deleting that clip does not change the result. Select it again to refresh the snapshot.

| Control | What it does |
| --- | --- |
| Swing | Delays offbeats on an eighth- or sixteenth-note grid. |
| Gate | Shortens held notes, opening space for subsequent articulations. |
| Brush strength | Repeats the most recent simultaneous chord on silent sixteenth-note grid positions, for 20 ms, at up to 30% of its velocity. The grid starts at each bar line in every meter. A brush is skipped if any written note occupies its interval. |
| Slide strength | Inserts one intermediate MIDI pitch just before the next attack, at up to 65% of the previous note's velocity. The note lasts at most 60 ms or a quarter of the previous note's duration. Legato gives up that much of the previous note's tail; a short rest can hold it instead. |
| Mute strength | Replaces the final 12 ms inside a note with a same-pitch retrigger at up to 35% of its velocity. The performed source ends where the retrigger starts, and the retrigger ends at the original release. Only notes held for at least an eighth note after earlier stages and clip clipping qualify; conflicting same-pitch tails are skipped. The stored score stays intact. |
| Stroke spread | Spreads simultaneous notes from first to last across 0–100 ms. Choose low-to-high, high-to-low, or alternating directions. Release times are preserved, and a stroke is bounded by the next attack and its shortest note. |
| Humanize | Adds related timing and velocity variation: a smooth four-beat gesture, a one-beat gesture, and a smaller individual difference. |

Articulation strengths at zero are off. Slide connects single-note lines with intervals of
at least two semitones and gaps no longer than a quarter note. Chords and overlapping voices
are left alone because their voice assignment is ambiguous. The intermediate pitch is the
integer midpoint toward the previous pitch; it can be chromatic. Mute and brush are short
MIDI notes, so the chosen sound determines whether they resemble muted strings or audible
retriggered notes. They do not select an instrument's keyswitch articulations or send pitch bend.

The inspector places new stages in the table's order, with stored transpose before gate and
role-specific timing offsets before humanisation. Each stage sees the preceding stage's
result, so brushes can be stroked and inserted notes can be humanised. An existing custom
stack keeps its order. Insertions do not feed back into their own stage.
After timing changes, inserted notes are trimmed or omitted where they would overlap another
note on the same pitch. Written notes take priority, including across scoped drum voices.

Humanisation stores its seed. Reopening and replaying the same arrangement gives the same
result; different clip-loop passes have different variation. Nearby times share a gesture,
but pitches retain small individual differences. The timing scale remains tempo-independent
in milliseconds (nominal 6 ms at full strength, bounded to about 18 ms before tick rounding).
The smooth mixture has less variance than independent per-note noise. The corresponding
nominal velocity scale is 6% of the written velocity.

**Keep the Performance** writes the first pass's performance, including inserted notes, into
the score and clears all performance stages. It is undoable. A frozen loop repeats that take.
Generated-clip recipe freezing is a separate control.

## Automatic pitch gestures

On an instrument clip, open **Melodic expression** in Performance. Scoop and fall have separate
depth and duration controls. Vibrato has depth, rate in Hz, and onset delay; it fades in over
100 ms and fades out toward the release. Melodic connection controls the duration on each
side of a note change: the outgoing note bends toward the midpoint of the two pitches, and
the incoming note starts at that same pitch and settles onto its own. Ascending and descending
lines use the same rule. Connected ends use this transition instead of a scoop or fall.

Use a monophonic instrument track: channel pitch bend affects every sounding voice on it.
Overlapping notes, chords and notes shorter than 80 ms receive no generated gesture. Connections
stop at gaps over 60 ms or intervals over two octaves. Each attack/release gesture is capped
at half the note's duration. Duration and vibrato rate follow actual elapsed time, including
tempo changes. The generated bend follows the final performed notes, including humanisation,
and is added to the authored bend within the existing +/-12-semitone range.

The **Performance** tab shows the resulting pitch line and a read-only bend lane, including
loop repeats. Controls update both immediately. Source notes and hand-drawn curves remain
unchanged until **Keep the Performance** writes the first pass's bend as editable points.
Undo restores the original notes, points and controls. Gaps and clip/loop ends reset generated
bend; written bend values still apply in gaps. MIDI export includes these curves and selects
a +/-12-semitone range with RPN 0 when needed, so deep falls survive export and reimport.
Built-in instruments and CLAP note-expression instruments receive bends in semitones.
Live VST3 and MIDI-only CLAP playback retain the host's existing +/-2-semitone range;
the deeper control range is audible with instruments that accept it.

**Auto modulation** sends CC1 with the same monophonic eligibility, delay, 100 ms fade-in,
and release fade as vibrato. Its depth is independent of pitch vibrato, so pitch depth can
remain zero while the instrument's modulation wheel opens. The instrument determines the
wheel's effect. Authored modulation is retained and added to the generated motion.

**Long-note volume** sends CC7 on monophonic notes lasting at least 600 ms. It holds the
attack briefly, dips early, stays low through the middle, rises to a late peak, and softens
the release. Strength blends this contour with the existing volume curve, or full volume
when none is written. Gaps retain authored volume and clip/loop ends restore full volume.
SoundFonts and the built-in melodic synths respond to CC7; hosted instruments must support
MIDI channel volume. The read-only Performance lanes show both generated controllers.

**Upper octave** and **Lower octave** independently add copies at +12 and -12 semitones.
Their percentages set velocity relative to the original. Out-of-range copies are skipped;
existing notes take priority over overlapping copies at the same pitch. Exact octave layers
with matching starts and releases share the melody's pitch and controller gestures. Other
overlapping voices still suppress automatic channel gestures.
All four controls default to zero (off). Source notes and curves remain editable; **Keep
the Performance** writes the heard notes and controllers, and Undo restores the settings.

Saved projects use format version 27 for octave doubling and generated controllers.

## Starting from automatic composition

Song presets install a genre-specific performance stack on their clips. The source
score and generation recipe are unchanged. Open a generated clip's Performance tab to adjust
the same controls used on hand-written clips; regeneration keeps those adjustments. Another
Take updates inherited ghost/expression seeds and preserves their settings and ensemble group.

| Preset | Initial performance |
| --- | --- |
| Chiptune | Tight shared expression and small lead bends/vibrato |
| Game Loop | The chiptune palette over its sixteen-bar loop |
| Pop Band | Lead scoops/connections, light keyboard attack spread, bass and snare pickups |
| City Pop | Sax scoops, vibrato and falls; offbeat bass ghosts, slides and muted tails |
| Rock | Alternating partial guitar strums, sparse sixteenth brushes, muted tails and lead bends |
| Jazz Trio | Piano attack spread and offbeat phrasing, quiet snare pickups; piano pitch stays fixed |
| Orchestral | Shared phrase dynamics, restrained flute/cello vibrato and cello connections |
| Synthwave | Tight rhythm, connected saw lead, delayed vibrato and subtle falls |
| Ambient | Slow shared phrase dynamics and subtle cello motion; bells stay at their written pitch |

Instrument changes in the song sheet are respected: bend, strum and mute choices depend on
the instrument and role, not the part's name. Ghosts preserve long rests and final tails;
drum ghosts stay scoped to their named kit voice. Singing melodies use the voice pipeline's
own ornaments.

The `.asong` root field `performance = "city-pop"` selects the palette; `game-loop` uses the
`chiptune` palette. Omitting the field keeps the plain lean/wander behavior. The
`humanize` dial controls random timing/velocity and lean; intentional articulations remain
independent. Existing saved clips are never upgraded or rewritten when a preset changes.
