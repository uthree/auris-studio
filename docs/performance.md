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

| Control | What it does |
| --- | --- |
| Swing | Delays offbeats on an eighth- or sixteenth-note grid. |
| Gate | Shortens held notes, opening space for subsequent articulations. |
| Brush strength | Repeats the most recent simultaneous chord on silent sixteenth-note grid positions, for 20 ms, at up to 30% of its velocity. The grid starts at each bar line in every meter. A brush is skipped if any written note occupies its interval. |
| Slide strength | Inserts one intermediate MIDI pitch just before the next attack, at up to 65% of the previous note's velocity. The note lasts at most 60 ms or a quarter of the previous note's duration. Legato gives up that much of the previous note's tail; a short rest can hold it instead. |
| Mute strength | Retriggers the same pitch at its release for 12 ms, at up to 35% of its velocity, only when the held note is at least an eighth note long after earlier stages such as gate. It skips releases where that pitch is still occupied or the clip has ended. |
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

Saved projects use format version 23 for configurable ghost-note placement. Current builds read
older clips with their existing defaults; older builds must reject this newer format.
