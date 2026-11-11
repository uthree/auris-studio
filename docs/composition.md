# Automatic composition

[日本語](composition.ja.md)

Clips you can regenerate against the current chords with **Write It Again**, and a band written
under a melody you played. The rest of the application is in [Features](features.md).

## Clips that write themselves

A single clip can be written from the chords underneath it:
Right-click an empty stretch of a melodic instrument track to generate lead, chords, pad,
arpeggio or bass. Drum tracks offer the full kit, kick, snare and hi-hat presets. Separate
drum tracks allow their mixer levels and processing to be controlled independently.

The new clip fills the free interval containing the right-click position, within that section.
Existing clips on the same track bound either edge. The final section uses the end of the
project's existing clips when that end is beyond the pointer. Without a bounded section, the
phrase starts at the pointer's bar and lasts at most four bars, stopping at nearby clips or
section boundaries. Clicking an occupied position cannot add an overlapping part.

The clip keeps the recipe that produced it, so **Another Take** is
the next seed, **Write It Again** follows the chords when they move, and **Keep This One** drops
the recipe when a take turns out to be the keeper. A track's own menu has **Keep Every Take Here**,
which does that to all of them at once and says how many it acted on. The dials are in the
inspector.

Select a generated melodic clip to open **Session Player** in the inspector. The five role
buttons choose lead, chords, pad, arpeggio or bass, highlighting the current choice.
The same XY pad used by Drummer controls **Complexity** horizontally (the recipe's density)
and **Intensity** vertically: right makes the part busier, up plays it harder. The readouts
show both values. Each drag rewrites the selected clip as one undo step and keeps its seed
and Performance settings.

**Phrasing** groups subdivision, five octave buttons from −2 to +2, and the applicable
expression sliders. **Takes** groups the editable take number (the seed), **Write It Again**,
**Another Take**, and **Keep This One**. After editing the chords, use **Write It Again** to
follow them with the current take. **Keep This One** preserves the notes and removes the
generation controls. These controls edit the selected clip, so different sections on the
same instrument track can have different settings.

The controls mean:

| | |
|---|---|
| **Subdivision** | How finely the beat divides: 1/8, 1/16, or either of them in triplets |
| **Octave** | Which register, ±2 from where the preset sits |
| **Density** | How busy the part is; chords above 75% fill more of the subdivision, reaching every step at 100% |
| **Syncopation** | How far the figure pulls off the beat, without making it busier |
| **Gate** | How long each note sounds, as a share of the gap to the next |
| **Intensity** | How hard it is played |
| **Dynamics** | How far apart the hardest and softest notes are struck |
| **Fill** | How much of the last bar the snare runs into what follows — drums only |
| **Swing** | How late the offbeats are |

Intensity and dynamics are the level and the spread, and they are separate because they are two
questions: a part can be played hard and flat, or softly with everything in it moving. At 0 the
dynamics leave every note struck alike — a sequencer, which is sometimes exactly the point — and
the level stays where the intensity put it rather than sagging with the spread. It reaches every
source of variation at once: the metric hierarchy, the accents, the lean across a phrase and the
crescendo of a drum fill.

The dynamics dial is narrow at the top on purpose: a part's strokes sit within about a tenth of
their own level there. That is a band phrasing; wider and the same idea reads as a band that has
not rehearsed, which is what it was until it was measured across the presets and pulled in.

How *loosely* a clip is played is not a recipe dial at all. The humanise — a timing and velocity
wander — lives in the clip's **Performance** section along with a swing and a gate of its own,
because it is how the text is played rather than what it says: the piano roll keeps showing notes
on the grid, the wander is turned without rewriting the clip, and a looped clip is loose
differently on every pass. A generated clip arrives with its part's feel already installed — at
the top of the dial a pitched note sounds within six milliseconds of where it is written, at
whatever tempo the piece is playing, and the kit keeps only its constant lean, because a drummer
holding the time is what everybody else is loose against.

A kit reads the density around the middle, and the middle **plays the groove**: a beat is never
thinned there, a weak sixteenth sometimes is, and a quiet section thins further. Below the middle
the groove thins from its weakest hits upward and never loses a downbeat; above it the steps the
groove left empty start taking ghost notes — which is how a drummer gets busier without playing
something else. *Which* rhythm it plays is
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
a feel and keeps it; only the closing bar of a planned phrase can turn it over, and only
sometimes. Above 75% density, chords keep a driving figure and fill more of the subdivision.
At 100%, every step is struck: sixteen attacks per 4/4 bar with a 1/16 subdivision, or
twenty-four with 1/16 triplets. Foreground-aware arrangement preserves this explicit dense
rhythm. Authored rhythms retain their rests regardless of density.

**Chords** and **pad** read the same harmony through the same writer, and what separates them is
what a pad does at a chord change: it holds whatever the two chords have in common and moves only
the voices that have somewhere to go, where a comp restrikes every one of them. That is the
difference between a chord *changing* and a chord *drifting*, and it is the reason the two presets
are two presets rather than one with the rhythm turned off.

For short, repeated piano chords, choose **Chords**, set density near 100%, subdivision to
1/16, and gate near 30%. Density controls the attacks; gate controls their length. Lower
the Performance humanize amount for tighter timing. Changing presets preserves dials that
were deliberately adjusted.

## Accompanying something you played

The composer read backwards. Write a melody into a clip — by hand, from a MIDI file, on the
keyboard — then right-click it and choose **Accompany This Melody**, or use **Compose → Accompany
the Melody** with it selected.

What happens is three things, in one undo step:

1. The melody's **key** is worked out and written into the harmony lane at the clip's start.
2. One **chord per bar** is written under it, for as many bars as the melody covers.
3. **Bass, chords and drums** are added as tracks beside it — not over it. The melody is not
   touched.

The parts are ordinary generated clips: they carry recipes, so every dial in the table above
applies to them, **Another Take** gives a different band, and **Write It Again** follows the chords
when they move. Each part gets a fitting native sound — physical bass, piano and the built-in
drum kit — with editable instrument parameters.

### What it can and cannot know

The key comes from correlating what the melody actually plays, weighted by how long each note is
held and how hard it is struck, against the probe-tone profiles Krumhansl and Kessler measured in
1982 — the standard basis for machine key-finding, and listeners' ratings rather than a theory of
the scale. Each bar's chord is then whichever of the key's seven triads accounts for most of that
bar, with a thumb on the scale for what the bar *arrives* on and a little inertia so the
progression does not change on every coin toss. Nothing draws a random number: the same melody
always reads the same way, so changing one note and pressing it again tells you what that note was
doing to the harmony.

A melody is one voice, and one voice cannot settle every question it raises. The two failures to
expect are a **relative-key swap** — a tune in A minor read as C major, which shares every note —
and a **bar of passing notes** read as the chord they pass through. Both are nudged rather than
solved, and neither is a dead end: what it guessed is in the harmony lane where you can see it and
retype it, then choose **Write It Again** on the generated clips to follow the corrected harmony.

A clip with no notes in it is refused rather than accompanied — there is no melody there to read,
and a bass line written from nothing would be an accompaniment to nothing.
