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
expression sliders. **Takes** is one row: ◀ steps back to the previous take, the take number (the seed) can be
clicked to type one, ▶ is **Another Take**, and **Write It Again** rewrites the current take.
**Keep This One** sits under it. After editing the chords, use **Write It Again** to follow
them with the current take. **Keep This One** preserves the notes and removes the
generation controls. These controls edit the selected clip, so different sections on the
same instrument track can have different settings.

The controls mean:

| | |
|---|---|
| **Subdivision** | How finely the beat divides: 1/8, 1/16, or either of them in triplets |
| **Octave** | Which register, ±2 from where the preset sits |
| **Density** | How busy the part is; chords above 75% fill more of the subdivision, reaching every step at 100% |
| **Syncopation** | How far the figure pulls off the beat, without making it busier |
| **Intensity** | How hard it is played |
| **Dynamics** | How far apart the hardest and softest notes are struck |
| **Fill** | How much of a bar the snare runs where the kit turns: its last bar, and in a long clip the end of every second phrase — drums only |

Intensity and dynamics are the level and the spread, and they are separate because they are two
questions: a part can be played hard and flat, or softly with everything in it moving. At 0 the
dynamics leave every note struck alike — a sequencer, which is sometimes exactly the point — and
the level stays where the intensity put it rather than sagging with the spread. It reaches every
source of variation at once: the metric hierarchy, the accents, the lean across a phrase and the
crescendo of a drum fill.

The dynamics dial is narrow at the top on purpose: a part's strokes sit within about a tenth of
their own level there. That is a band phrasing; wider and the same idea reads as a band that has
not rehearsed, which is what it was until it was measured across the presets and pulled in.

How a clip is *played* — how loosely, how swung, how much of each note is held — is not a recipe
dial at all. The humanise, the swing and the gate live in the clip's **Performance** section,
once each, because they are how the text is played rather than what it says: a generated
phrase is written straight and legato, and the piano roll keeps showing notes
on the grid, the wander is turned without rewriting the clip, and a looped clip is loose
differently on every pass. A generated clip arrives with its part's feel already installed — at
the top of the dial a pitched note sounds within six milliseconds of where it is written, at
whatever tempo the piece is playing, and the kit keeps only its constant lean, because a drummer
holding the time is what everybody else is loose against.

A kit reads the density around the middle, and the middle **plays the groove** as it is spelled:
every stroke of it in every bar, with only a snare's ghost notes coming and going with how hard
the section is played. Below the middle the groove thins from its finest strokes upward —
sixteenths, then offbeat eighths, then the beats — and never loses a downbeat or a backbeat;
above it the steps the groove left empty start filling in, with ghosts on the snare, the strokes
between its strokes on the hat and an offbeat kick. Whatever the dial takes or adds, it does
over two bars and then again, so a sparser or a busier groove is still a groove — which is how a
drummer gets busier without playing something else. *Which* rhythm it plays is
still the groove picker, and that is a choice from a drummer's own vocabulary rather than a
number: `basic-rock`, `eight-beat`, `sixteen-beat`, `four-on-the-floor`, `shuffle`, `breakbeat`,
`bossa-nova`, `half-time`, `sparse` and `open-rock`, which keeps time on the hat left open.

A kit turns where its phrases do. The last bar of a clip runs a fill, as long as **Fill** asks
for; a clip of eight bars or more may run twice as much of that bar, up to the whole of it, and
also turns at the end of every second phrase — the eighth bar of sixteen. A whole kit may run
the back half of a fill onto a tom, and may land on a crash in the bar a fill leads into, the
clip's first bar included; a section played hard crashes more often than a quiet one. One take
in four opens a single hat stroke in the bar that closes each phrase. A clip
that is one drum on a track of its own stays that drum, and a kit whose sound assignment has no
crash, no tom or no open hat keeps those strokes on the hat and the snare. With **Fill** at zero the kit does
none of this and the clip is a loop.

A bass line follows the kick of a groove as well, without reading the kit's notes: a new bass
clip takes the groove of the generated kit most of it lies under, and keeps it in its own recipe.
It draws one figure for the section and holds it, and plays mostly the root. A root-and-fifth
line is played on the kick, on the beats, or on the two halves of the bar. Every second bar
turns the figure its own way — a pickup into the next bar, the last note dropped, or the last
note moved onto the pickup — and the bar that closes a phrase may run up the scale into the
next. Most takes let a note go before a syncopated one, or just before a backbeat. A chord's root
is played from A1 to G♯2 unless the octave is moved.

An arpeggio breaks each chord in closed position, led from the chord before it the way the chords
part is, so the figure stays under the hand while the harmony moves. It runs in one of six
shapes drawn for the section — up, down, either of them turning round, every other note, or each
note against the top one — at a rate the density sets, and it sits around the chords and under
the middle of the tune. The rate is set beat by beat: quarters at the bottom of the dial, eighths
round the middle and every step at the top, and between them the beats turn over one at a time,
the weakest first — a little above quarters is long, long, long, short-short.

Only the rows a preset can actually hear are drawn. A kit has no subdivision, octave or
syncopation — its pitches are drum numbers rather than notes, and where it plays is which groove
it plays. Nothing but a kit has a fill, because nothing else has a last bar to announce. A pad has
no syncopation, because it sounds each chord where the chord is.

The subdivision is per part, not per song, so a stab hammering triplets over a straight kit is a
setting rather than a fight. A drum kit ignores the subdivision entirely, since a groove is
written in sixteenths and read by index.

A chord part picks its figure once for the section and restates it, the way a keyboard player picks
a feel and keeps it; only the closing bar of a planned phrase can turn it over, and only
sometimes. Nearly eight takes in ten let a chord go before a syncopated strike or just before
a backbeat, and the rest hold every chord into the next. A chord change may be *pushed*: struck half a beat
early and held over the line. Which changes are is read off the two chords, as degrees of the
key, and the **Syncopation** dial — one change in thirteen at its default, one in four at its top
— and not off the seed. So the chords and the bass push the same changes however
their clips are cut, a progression pushes the same change each time it comes round, and another
take of either part does not change it; turning the dial down stops it. Above 75% density, chords keep their figure and fill in the steps
round it.
At 100%, every step is struck: sixteen attacks per 4/4 bar with a 1/16 subdivision, or
twenty-four with 1/16 triplets. Foreground-aware arrangement preserves this explicit dense
rhythm. Authored rhythms retain their rests regardless of density.

**Chords** and **pad** read the same harmony through the same writer, and what separates them is
what a pad does at a chord change: it holds whatever the two chords have in common and moves only
the voices that have somewhere to go, where a comp restrikes every one of them. That is the
difference between a chord *changing* and a chord *drifting*, and it is the reason the two presets
are two presets rather than one with the rhythm turned off.

For short, repeated piano chords, choose **Chords**, set density near 100% and subdivision to
1/16, then turn the Performance gate near 30%. Density controls the attacks; the gate controls
their length. Lower
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
