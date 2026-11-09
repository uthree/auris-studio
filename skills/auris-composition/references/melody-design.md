# Melody design and analysis

This guide turns melody research into decisions an Auris composition agent can
make and inspect. It is a set of hypotheses and checks, not a formula for a
genre. A melody can intentionally violate every item below; record the reason
when it does.

## What a memorable line is doing

Start with a small identity-bearing cell. Yamaha's composition lesson presents
theme, question-and-answer phrasing, and variation as complementary ways to
keep an idea recognizable while changing its rhythm or decoration:

- [Yamaha Music Japan: variation (Japanese)](https://jp.yamaha.com/services/music-pal/study/composition/2nd/index.html)

For a new line, write a short cell whose length fits the meter and lyric (it may
be one beat, a bar, several bars, or a deliberately irregular span). Give it a
distinct rhythm or contour before filling a whole section. Its identity can
survive many kinds of development: transpose it within the key, keep its rhythm
but change the last interval, reverse one turn, stretch one value, move its
register, or place it over a new chord. Do not apply every transformation;
choose one that serves the section's dramatic job, and let an exact return be
an option when a hook needs to stay fixed.

Separate recognition from development: can a listener point to the same
rhythmic fingerprint, and did a meaningful parameter change? Exact copying can
serve a hook; constant novelty can erase foreground identity.

Make development traceable in the score: retain the cell's actual onset pattern
and interval contour in a short production note, with its first statement,
answer, changed return and chorus location. A useful pop sketch is statement,
recognizable answer with a changed tail, then a register-expanded chorus return.
This is a candidate form, not a requirement for every song. Check the written
notes for that relationship; merely naming unrelated phrases "motif A" does not
make them related. Listen for recall separately from score recurrence.

Use contrast between sections as a change in behaviour, not merely more notes:
register, rhythmic density, note length, contour, harmonic arrival, texture, or
silence can open a chorus. A refrain may keep the motif while widening range or
lengthening its arrival; this is a compositional inference, not a pop-law.

For a song that should open at the chorus, make the lift measurable. Compare the
chorus with the section just before it (usually the pre-chorus): its average
pitch should sit at least a minor third higher, it should hold the song's
highest notes, and its hook should leap at least a fourth where the pre-chorus
moved by step. Bolder choruses leap a sixth or an octave, even back and forth.
Keep the pre-chorus lower and narrower so the chorus has room, and prepare the
entrance with a held note, a rest or a rising line. Agents writing well inside
the voice tend to lift a semitone or two and call it a chorus; read the notes
section by section before accepting it.

## Build the pitch skeleton against harmony

Decide whether the interest lives mainly in the line itself or in its interaction
with harmony. Mark Simos's [Berklee lesson on independent melodies](https://www.berklee.edu/berklee-today/fall-2014/composing-independent-melodies)
distinguishes these without ranking one above the other. A one-pitch figure can
work because the chords move beneath it; a wide, self-contained tune may need
less harmonic activity. Compare with accompaniment before rejecting a simple line.

Before adding decoration, mark one or two intended target notes per phrase.
Choose notes from the current chord for arrivals on strong metric positions or
at phrase endings, especially the third or seventh when its colour defines the
chord. Keep a simple skeleton (for example, 3–2–1 over a cadence) and test it
with the actual bass and voicing. The target can be a tension or extension when
that colour is intentional; label the expected resolution or sustained role.

Open Music Theory describes a useful phrase-level model: stability moves through
instability toward a goal, with dominant function carrying a strong tendency
toward tonic. Its own warning is important: these are bendable tendencies, not
rules for every style.

- [Open Music Theory: harmonic syntax and the idealized phrase](https://openmusictheory.github.io/harmonicSyntax1.html)

The chord-root/ascending-scale fallback is only one skeleton. Other useful
starting points are a repeated-note hook with a changing harmony, a
triad/guide-tone outline joined by rests, a bass/upper-voice contrary-motion
plan, a two-note call answered in a new register, or a constrained motif built
from one interval class. Each keeps a different source of identity while
leaving room for the harmony to move.

After the skeleton works, add passing, neighbour, anticipation, suspension, or
appoggiatura tones as gestures with a destination. A non-chord tone held on an
accent can be expressive; the same note left unexplained at every cadence may
sound accidental. Check the *performed* timing: a pickup or anticipation can
make a nominally off-beat note the perceptual arrival.

### Colour without compulsory complexity

When the brief wants a more colourful melody, audition a 9th or another suitable
extension at a meaningful word or target, rather than confining arrivals to a
triad. Separate three questions: key degree, role against the *sounding chord*,
and absolute pitch/register. A ninth is the compound form of a second; D can
colour C harmony as its ninth while remaining degree 2 in C major. This does
not require a ninth leap, a new pitch class or leaving the key. Over G, D is
instead the fifth. A melody that already uses degrees 1 through 7 may already
contain extensions against some chords.

For an original local comparison over C harmony, keep the rhythm/backing and
compare an E5 target with D5 held briefly before E5. The latter exposes a ninth
then resolves to the third; it is an option, not intrinsically better. Check
the bass, chord quality, duration, voicing and next harmony; an intentional
held colour need not always resolve immediately. Do not add 9ths/11ths/13ths
to every chord, require chromatic notes, or widen the singer just to meet a
"tension" count. See [Open Music Theory: intervals](https://human.libretexts.org/Courses/Prince_Georges_Community_College/PGCC_Open_Music_Theory-Fundamentals/04:_Intervals)
and [chord-scale theory](https://viva.pressbooks.pub/openmusictheory/chapter/chord-scale-theory/)
for the vocabulary and the limits of choosing scales chord by chord.

### Worked four-bar sketch (original)

This is an invented score-reading example, not a quotation. Use C major, 4/4,
one chord per bar: `C | Am | F | G`. Pitches are scientific pitch names;
`q` is one beat, `e` half a beat, `dq` one and a half beats, and `h` two beats.
The first two notes make the identity cell (an upward third); the third note is
the held fifth. Bar 2 retains the opening rhythm in the same register but
changes the tail. `F4` begins on bar 2 beat 4-and and lasts one beat: its first
half anticipates the upcoming F chord while Am still sounds, and its second
half confirms that chord after the barline. The tie is intentional.

| bar / harmony | onset and duration | C-major degree / chord-relative role |
| --- | --- | --- |
| 1 / C | `C4 q, E4 q, G4 h` | 1 (root), 3 (third), 5 (fifth); identity cell and held arrival |
| 2 / Am | `C4 q, E4 q, G4 q, rest e, F4 q (starts beat 4-and; tie across barline)` | 1 (third), 3 (fifth), 5 (seventh), rest, 4 (non-chord then next-root anticipation) |
| 3 / F | `F4 (tie e), rest e, A4 q, C4 q, rest q` | 4 (root), rest, 6 (third), 1 (fifth), breath; confirms the arrival |
| 4 / G | `D5 q, B4 q, G4 dq, rest e` | 2 (fifth), 7 (third), 5 (root); held half-cadence arrival and rest |

The complete timelines including rests total four beats: bar 1 `1+1+2`, bar
2 `1+1+1+0.5` plus the first `0.5` of the crossing F quarter, bar 3 tied
`0.5+0.5+1+1+1`, and bar 4 `1+1+1.5+0.5`. No extra bar is needed: the final
eighth rest already supplies the release space. A repeated-note alternative is
`G4 q, G4 q, G4 h` over bar 1:
the rhythm carries identity while harmony, register, or the answer changes.
Because the last bar is G harmony, this sketch deliberately ends with a
dominant half cadence rather than tonic closure; add a following C arrival only
when the brief asks for a closed ending.

## Rhythm, metre, and prosody

Write rhythm independently for one pass. Clap or count the cell at the chosen
tempo, with the bass and drums present, before deciding whether each pitch is
right. Ableton's Learning Music frames music as small ideas combined and changed
over time and treats beat, tempo, melody, harmony, bass, and structure as
interacting building blocks:

- [Ableton Learning Music](https://learningmusic.ableton.com/)
- [Ableton: beat and tempo](https://learningmusic.ableton.com/make-beats/beat-and-tempo.html)

Treat chord-change timing as another rhythmic decision. Scarlet Keys's
[Berklee lesson on harmonic rhythm](https://www.berklee.edu/news/berklee-now/telling-story-harmonic-rhythm)
shows how extending a chord can hold a moment while the melody continues moving.
For an important line, compare the existing progression with the same harmonic
idea paced more slowly, where reharmonization/timing changes are in scope. More
chord changes do not necessarily give the melody more direction.

Source audit (pages fetched 2026-10-25): Yamaha supports theme/question-answer
variation; Open Music Theory supports cadential tendency; Ableton supports
combining small ideas over a clear pulse. The rest is original adaptation.

Use rests as syntax. A short pickup can point into a downbeat; a held note can
bridge a bar; a gap can let an answer or lyric land. Syncopation is effective
when the accompaniment makes the displaced pulse legible and the next event
confirms or intentionally resists the expectation. Do not “humanize” a line by
randomly moving every onset: vary a phrase-level gate, velocity, or one selected
anticipation and compare it with the grid version.

An on-beat entrance can supply the intended drive; a pickup or delayed entry is
not inherently more natural or expressive. When a listener values the existing
rhythmic feel, keep that entrance and the word group's attack pattern as the
baseline. Repair a crowded phrase locally before delaying every line or enlarging
every gap. Breathability, intelligibility and groove are separate listening
questions; improving one is not evidence that the others improved.

For a newly written pop/song melody without a contrary brief, use an eighth-note
pulse as a starting backbone, interrupting it with holds and phrase releases.
Place selected sixteenths at a pickup, turn or answer; avoid giving every word
the same pattern or packing every gap. This preference concerns the foreground,
not automatic quantization of the whole ensemble. In 6/8, keep the two groups
of three eighths legible rather than importing a 4/4 accent pattern. Tempo,
lyric consonants and the performer's articulation can require fewer attacks.
Sparse ambient lines and deliberately continuous riffs retain their own briefs.

For lyrics, map stressed words or important Japanese mora to a meaningful beat,
long vowel, or melodic arrival. Leave consonant-heavy material enough attack
time and place breaths at phrase boundaries or deliberate internal rests. Test
the actual reading and note assignments rather than assuming a count of slots
equals natural prosody; see the vocal guide for singer-specific checks.

## Shape, range, and climax

Describe each phrase as a contour (arch, rise, fall, wave, plateau) and mark its
highest and lowest meaningful points. Put a local high point where the text,
harmony, or section needs emphasis, then release its energy through stepwise
motion, a rest, a descent, or a held consonant-free vowel. A single global high
note is not mandatory: dance hooks may use a narrow repeated pitch, chant may
use a reciting range, and a bass riff may define the identity below the vocal.

Check the line at three scales: **cell** (rhythm/contour recall), **phrase**
(tension, target, breath, cadence), and **section** (range, register, density,
and cadence distinguishing verse, chorus, bridge, and ending).

Prefer a comfortable range for the chosen instrument or singer, then reserve an
extreme note for a reason. Large leaps, rapid syllables, and sustained high notes
need a playable articulation and a release plan. A MIDI gate ending at the grid
line is not automatically a natural ending: leave instrument-dependent decay,
voice breath, and reverb tail space. The arrangement pass contains the project
specific checks for held arrivals and phrase endings.

For a bold chorus, actually sketch a register alternative: the same cell one
octave up, an octave-shifted answer, or a raised final arrival. Retain an audible
identity feature and check the entrance *and* the route back. Whole-phrase
register transfer is not octave doubling, modulation, or a demand for an octave
leap between consecutive notes. Check tessitura (where the phrase spends time),
not only its extreme notes. If a full octave would strain the singer, lower the
verse when allowed, lift only a fragment or use a smaller contrast. Compare
with the same compatible backing before changing density or effects as well.

## A repeatable authoring pass

For each new foreground line:

1. State its role (hook, lyric carrier, theme, response, ostinato, or transition)
   and the section where it must be identifiable.
2. Choose a short rhythm and a contour; make a singable or playable sketch on a
   neutral sound.
3. Mark phrase targets against the *actual* chord changes and bass motion.
4. Fill the path with scale steps, purposeful skips, and a small number of
   labelled non-chord gestures.
5. Recur, transform, or deliberately withhold the cell according to the form;
   vary one parameter only when the answer or later section needs development.
6. Add rests, pickups, held notes, breaths, dynamics, and articulation for the
   selected instrument or singer.
7. Render or preview the foreground alone, then with accompaniment. Check that
   its rhythm remains intelligible, that arrivals agree with the harmony, and
   that the ending leaves the intended amount of space.
8. Read the resulting notes and section boundaries back. A tool success, note
   count, or dense curve is evidence of execution, not evidence of a good line.
   For a monophonic draft, check numeric onset/end positions for unintended
   overlaps and bar overflow before judging phrasing. Map the actual lyric
   mora to notes or explicit continuations; do not hide several mora or an
   incomplete word behind an ambiguous "held word" annotation.

When revising, change one high-impact issue at a time: unclear motif, crowded
prosody, masked target, unplayable leap, flat section contrast, or abrupt release.
Keep the prior candidate for comparison and stop after two local attempts unless
the user asks for exploration.

When listening preferences differ by axis, keep the baseline and the relevant
local alternatives. A preferred breath does not establish better groove or
naturalness; a stronger register contrast does not establish a preferred chorus.
Do not select or combine whole takes by counting axis wins. Use the listener's
stated priority, or retain the tradeoff when no overall choice was made. To learn
whether a rhythmic turn, motif transformation or register lift helped, vary that
intervention alone with compatible backing and other melody features held fixed.
Equal motif preference does not establish equal delayed recall.

## MIDI and transcription analysis without imitation

An exploratory 2026-10-25 study used the final MIDI files 001–030 in
[POP909](https://github.com/music-x-lab/POP909-Dataset), commit
`d83e6edba6872a704f5d3b8b32f5cb540088dae6`, selecting its named MELODY track.
Across those 30 files, the average per-file adjacent unison share was 22.1%;
whole-song pitch ranges varied from 9 to 32 semitones. These observations support
considering repeated pitches and checking range in context, not imposing those
values on a phrase. The data is a consecutive-index sample of piano arrangements,
not a current-chart survey or a singing-range recommendation. Silence between
MIDI gates does not identify lyric breaths; sustain pedals, release and effects
are not represented by those gaps. The source checkout retains the analyzer and
per-file hashes in `tools/eval/references/melody-reference-analysis-2026-10-25.json`.

MIDI can expose structure—onset grids, durations, pitch intervals, velocity,
track roles, repeated n-grams, range, and chord-tone arrivals—but it does not
contain the full sound, articulation, lyrics, or listener context. Treat a
labelled melody track or an automatic transcription as a hypothesis, not a
“best melody” oracle; retain provenance and uncertainty. If the POP909 corpus
is used, keep its train/test split and metadata fixed while comparing generator
changes, and do not tune a held-out evaluation against its output.

For a lawful, reproducible corpus study, use public-domain or user-owned MIDI;
record provenance, parser and tempo-map handling, and the melody-track rule.
Normalize to beats, preserve tempo changes, and report foreground uncertainty.
Useful summaries are:

- phrase length and rest distribution, rather than only average note density;
- interval and contour transitions conditioned on metric position;
- repeated rhythmic cells with pitch variants, and where variation occurs;
- chord-tone share at attacks and cadence targets, plus labelled non-chord paths;
- register/range by section, highest-note timing, and distance between climaxes;
- onset anticipation/syncopation relative to drums and bass;
- note duration, velocity, and controller curves by instrument role.

Compare distributions by genre, era, role, and section. A statistic is a prompt
for listening and inspection, never a target to optimize mechanically. Do not
copy a recognizable sequence from a recent song; abstract the operation (for
example, “same rhythm, different contour and harmony”) and generate an original
candidate, then check it against the user's brief.

## Review questions for the harness

The evaluator should ask concrete, score-readable questions:

- Does the score contain a foreground track, and do its opening notes recur with
  a traceable rhythm or contour? (Score-readable.)
- Does each variation name a changed parameter—rhythm, interval, register,
  harmony, or articulation? (Score-readable.)
- Can the claimed motif relationship be located in actual note onsets/intervals,
  and does the chorus register change fit the declared range and lyric vowels?
- At strong attacks and phrase endings, do target notes fit the chord or carry a
  labelled, resolved tension?
- Are rests, pickups, breaths, and held arrivals preserved at section joins?
- Does the section's range, density, register, and cadence differ in the score
  as intended? (Score-readable; perceptual contrast still needs listening.)
- Are any leaps, note lengths, lyrics, or articulations within the declared
  instrument/singer constraints? (Score-readable; naturalness needs listening.)
- Does the final phrase contain the requested resolving, looping, or suspended
  event and a measured tail/rest? (Score-readable; perceived closure needs
  listening.)

Only listening can establish that the foreground is actually identifiable,
intelligible, expressive, or memorable in the mix. A muted-backing comparison,
render measurement, or successful tool call cannot establish those judgments.

Keep symbolic diagnostics separate from listening claims. Without human listening
feedback, report note-level and measurement checks
only and leave timbral, emotional, and memorability judgments unconfirmed.

The eighth/sixteenth starting point and bold chorus candidates above incorporate
listener production feedback from 2026-10-26. They are scoped creative preferences,
not effects established by the 2026-10-25 A/B trials. Preserve a liked attack
pattern and backing when making a local melody revision; more rests or a wider
range are not automatic improvements.
