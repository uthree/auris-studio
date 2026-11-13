# Composition techniques

The common moves of song production, one per section, each with what it is for and how to do
it with the Auris tools. The composing agent's phase instructions and the `auris-composition`
skill name these techniques in a short index; this page is what `search_documentation` returns
for the detail, and the only place it is written. None of them is a rule. Each is a choice a
song may make for a reason, and a song that makes all of them at once is a checklist, not a
song.

## Score phase

### Section roles

A verse (Aメロ) states the situation in the lower, narrower part of the voice, in short
phrases close to speech. A pre-chorus (Bメロ) changes something to raise expectation: harmony
that leaves the tonic, a rising line, a shorter phrase rhythm, or a held note at its end. The
chorus (サビ) is the arrival: the hook, the title words, the song's highest notes. Write the
three so a listener can tell where they are without the words. `melody_shape` checks the
chorus side of this.

### Line roles (起承転結)

Decide what each line of a section does before writing its notes. Four lines can state an
idea, answer it with one thing changed, turn (a new rhythm, a higher register or another
chord), and land on a cadence: 起承転結. A pre-chorus can change the phrase length, two short
lines against one long one; a chorus can state the hook, answer it, reach its peak and land.
Record the plan with `production_record`, then check the written notes against it. Four lines
that sing the same notes to new words are one line heard four times.

### Entrances and anticipation (弱起・食い)

Choose where each line enters: a pickup from the bar before (弱起), the downbeat, or a late
entry after a rest. Starting every line on beat 1 makes every phrase sound like the last. An
anticipation (食い) starts a strong beat's note an eighth or a sixteenth early and ties it
over the beat; on a key word or the chorus's first word it adds drive, and the listener still
hears where the beat is. Give the sections different note values as well: a verse speaking in
eighths, a pre-chorus holding longer notes, sixteenths saved for pickups and turns. In 6/8,
keep the two groups of three eighths audible rather than importing a 4/4 accent pattern.

A pickup or a late entry is not better than the downbeat in itself: an on-beat entrance can be
the drive the line wants. When the user likes a line's rhythm, keep its entrances and its
attack pattern and repair a crowded phrase locally instead of delaying every line.

### Rests and syncopation

A rest is punctuation: a short pickup points into a downbeat, a held note bridges a bar, a gap
lets an answer or a word land. Syncopation works when the backing makes the beat it displaces
audible and the next event confirms or deliberately resists it. Do not humanise a line by
moving every onset at random; change one anticipation, a phrase's gate or its velocities, and
compare with the grid version. Breathing room, intelligibility and groove are three separate
questions, and widening every gap to fix one of them can lose the others.

### Japanese lyrics in feet (2モーラ・拍)

Japanese verse and song are timed in **two-mora feet**, not in moras one by one and not in
syllables: 中道 (1980) calls the pair a タクト, and it is why a 7-5 line runs in four beats with
a rest (ふる|いけ|や・). `analyze_lyrics` returns each phrase's `feet`. Feet are a planning
unit, not a law of melody. In the 100 PJS songs, a foot's first mora starts on a beat 43% of
the time and its second mora 37%, so melodies lean the same way only slightly. Plan a line's
rhythm from them before choosing pitches:

1. **Count feet, not moras.** Sung two moras to a beat, in eighths at a pop tempo, each foot
   fills one beat, and the foot count is the line's length in beats. A section can change pace
   on purpose (four moras a beat in sixteenths for a rapid verse, one a beat for a held chorus
   line), but one pace for every line of the song is what makes it sound recited.
2. **Place the odd mora.** A one-mora foot is where the line breathes. Make it a pickup into
   the next beat (弱起), hold it to fill its beat, or let a rest take the other half. Particles
   (を, に, が) often stand alone, and a pickup suits them.
3. **Shape inside the foot.** Vary the two halves rather than the count: long-short (a dotted
   eighth and a sixteenth) puts weight on a word's first mora, short-long delays the arrival,
   two sixteenths and an eighth hurry through a particle, and one quarter holds a word.
4. **Keep heavy syllables whole.** ん, っ, ー and the い of あい belong to the mora before
   them (窪薗 1999): a foot never splits them, and when a song gives two moras one note, it is
   almost always such a pair (かん, かい). Sing the pair on one note (group かん in the
   `write_lyrics` reading), or give ん its own note. Inside a line, human songs hold that
   note longer than its head about half as often as they do other moras, and `lyric_rhythm`
   notes each place it happens. Where the second half falls in the beat is free: in the PJS
   songs ん arrives on a stronger position than its head as often as any mora does.
5. **Rest between words.** A rest inside a word (マフ・ラー, か・える) cuts it apart. In the PJS
   songs it happens on about 1 in 60 steps inside a word, against 1 in 16 between words. When
   the melody wants a breath where the words do not, move the line break, re-line the words or
   lengthen the note before the gap. `lyric_rhythm` lists each one, reading words a clip at a
   time so a word split across two lines is still found.
6. **Close the line.** Lengthen the line's last mora, or rest after it, so the line ends.
   Koizumi's account of Japanese song (1977) has the line's first and last moras lengthened
   to mark its start and close; the middle runs evenly.

Japanese spends a note on every mora, so it needs more notes than English for the same words
(金田一 1967 gives this as one reason it sits awkwardly in Western song forms). So
write fewer words per line rather than packing every subdivision, and let a held arrival
carry the important word. Since the rhythm cannot be heard while writing, read it:
`lyric_rhythm` draws every sung line on its beat grid, one token per slot, and reports moras per
second, held endings and the short notes that may lose their vowel in synthesis.

### Cadence plan

Mark the note each line ends on and the chord under it. Open endings (degree 2, 5 or 7, or a
chord other than the tonic) ask for more; closed ones (degree 1 or 3 over the tonic) answer.
Pair them, end the song's last chorus on the tonic unless the brief wants it left open, and
write falling and arched lines as well as rising ones: a melody whose lines all climb and end
on the fifth or the seventh never settles.

### Pitch skeleton

Before decorating a phrase, mark one or two target notes against the chords actually written:
chord tones on strong beats and at phrase ends, the third or the seventh where its colour
defines the chord. A plain skeleton such as 3–2–1 over a cadence is enough to test against the
bass. Then fill the path with steps, purposeful skips and a few non-chord tones (passing,
neighbour, anticipation, suspension, appoggiatura), each heading somewhere. A non-chord tone
held on an accent can be expressive; the same note left unexplained at every cadence sounds
like a mistake. Judge the performed timing: a pickup or an anticipation can make an off-beat
note the arrival.

Decide where the interest lives. A one-pitch figure can carry a hook because the chords move
under it; a wide, self-contained tune may want less harmonic movement. Other skeletons than
"chord root, then up the scale" are a repeated-note hook over changing harmony, a guide-tone
outline joined by rests, a line in contrary motion to the bass, and a two-note call answered in
another register.

### Colour tones

When the brief asks for a more colourful line, try a ninth or another extension on a word that
matters instead of keeping every arrival on the triad. Keep three things apart: the note's
degree in the key, its role against the sounding chord, and its register. D is degree 2 in C
major; over C it is the ninth, over G the fifth. A ninth needs no ninth leap and no note
outside the key. Holding D briefly before E over C exposes the ninth and resolves to the third;
an intentional held colour need not resolve at once. Do not add tensions to every chord or
widen the voice to reach a count of them.

### A worked four-bar sketch

An invented example in C major, 4/4, one chord a bar (`C | Am | F | G`); `q` is a beat, `e` half
a beat, `dq` a beat and a half, `h` two beats.

| bar / chord | notes | degree / role against the chord |
| --- | --- | --- |
| 1 / C | `C4 q, E4 q, G4 h` | 1, 3, 5: the cell (a rising third) and a held arrival |
| 2 / Am | `C4 q, E4 q, G4 q, rest e, F4 q` (from beat 4-and, tied over the bar) | same rhythm, changed tail; F anticipates the next chord |
| 3 / F | `F4 (tie) e, rest e, A4 q, C4 q, rest q` | root, third, fifth; a breath |
| 4 / G | `D5 q, B4 q, G4 dq, rest e` | fifth, third, root: a half cadence |

Bar 2 keeps the cell's rhythm and changes its end; the tied F is heard as arriving with the F
chord. The sketch ends open on G on purpose; add a C arrival only where the section should
close. Over bar 1, `G4 q, G4 q, G4 h` is the repeated-note alternative: the rhythm carries the
identity while the harmony moves.

### Section harmony

A verse may loop a simple progression; the pre-chorus and the chorus need their own. Change
the progression, its rhythm (two chords a bar instead of one, or one chord held for two bars)
or its bass line (a descending bass, a pedal). The verse's loop started on another chord is
the same loop, and a chorus over it sounds like more verse. More chord changes do not give a
melody more direction: holding one chord longer can let a moment hang while the line moves.

### Chorus lift

Keep the verse and pre-chorus low enough that the chorus can rise a minor third to a fifth on
average, give the chorus the song's highest notes, and let its hook leap: a fifth, a sixth or
an octave, even an octave jumped back and forth. Put the leap where it matters; the same leap
at the head of every chorus line stops being heard. End the pre-chorus on a held note, a rest
or a rising line so the chorus lands on a strong beat or just before it. Read `melody_shape`
after writing the melody and act on its warnings before singing.

Agents writing comfortably inside the voice tend to lift a semitone or two and call it a
chorus. For a bold chorus, sketch the register alternative: the cell an octave up, the answer
moved up an octave, or a raised final arrival, and check the way back down as well as the
entrance. Judge where the phrase spends its time (tessitura), not only its highest note. When
a full octave would strain the singer, lower the verse, lift only a fragment, or settle for a
smaller contrast.

### Phrase contour and climax

Give each phrase a shape (arch, rise, fall, wave, plateau) and mark its high and low points.
Put a local high point where the words, the harmony or the section want weight, then release
it by step, with a rest, a descent or a held open vowel. One high note for the whole song is
not compulsory: dance hooks may sit on one repeated pitch, a chant on a reciting tone. Check a
line at three scales: the cell (is its rhythm and contour recognisable), the phrase (tension,
target, breath, cadence) and the section (range, register, density and cadence that set verse,
chorus, bridge and ending apart).

### Motif repetition and development

Write one short cell, two to six notes over a beat to two bars, with a distinct rhythm or
contour, then develop it: keep the rhythm and change the last interval, move it to another
register, place it over another chord, stretch one value, reverse one turn. Choose the one
change the section's job calls for, not all of them. A cell is not a whole lyric line.
Repeat it exactly where a hook must be recognised, and fit those repeats' words to its notes;
elsewhere keep the cell's head and let the line around it change. See the lyric-fitting rules
in the vocal guidance.

Record the cell's onsets and intervals in `production_record`, with where it is stated,
answered, changed and brought back in the chorus, and check the written notes against that.
Calling two unrelated phrases "motif A" does not make them related.

### The chorus hook in the intro

Open with the chorus hook played by an instrument, or with its first two bars, so the chorus
is already familiar when it arrives. Copy the chorus melody clip to the intro with
`edit_clip`, move it to an instrument track, and thin it to the hook's first phrase.

### Drop chorus (落ちサビ)

Before the last chorus, sing one chorus with almost nothing under it: the voice with a held
pad or piano, drums and bass out. The return of the full band at the last chorus is then the
biggest contrast in the song. In the score this is a section of its own with the chorus
melody and lyrics; the arrangement phase strips its backing.

### Last-chorus key change (ラスサビ転調)

Raise the final chorus a semitone or a whole tone. Set the new key from that bar with
`edit_harmony`, and write the raised melody either by copying an earlier chorus clip with
`edit_clip` (`copy` takes a `transpose` in semitones) or by patching its notes' pitches with
`edit_notes`; then sing it again. The arrangement phase transposes the backing the same way.
Check that the raised peak still sits in the voice's useful range: a key change that pushes
the voice past it is worse than none. A short pivot (the new key's V in the bar before) makes the change land.

### Singable range

Keep the melody inside the voice's useful range, which for most synthesised voices is about an
octave and a half. Place the verse in the lower part and the chorus in the upper part rather
than spreading every section over all of it; that is what leaves the chorus room to lift.
Large leaps, rapid syllables and long high notes each need a release after them, and a gate
that stops at the grid line is not an ending: leave room for the breath, the decay and the
reverb.

### Lyrics that develop

Give the song a concrete scene and let the perspective move across the verses. The chorus
holds one central thought and a hook that is both verbal and melodic. A second verse has new
words that advance the story and something changed in its phrasing or backing; copying the
first verse's clip is not a second verse. A full song has a second verse, later choruses and
an ending unless the brief asks for a one-chorus demo or an excerpt; a bridge is optional.
Write words and melody together, and never fill spare notes with filler syllables.

### Vowels on held high notes

For an exposed high or powerful arrival, try a word whose held vowel is あ or お. Know which
vowel is actually held, on what pitch and for how long, and which consonant leads into it. Put
ん lower or in the middle of the range, or at a short phrase end, when that keeps the contour;
it keeps its own mora. None of this outranks the word's meaning or a fixed lyric: try timing,
articulation or a permitted transposition before proposing a lyric change. These are
preferences from listening to synthesised voices, not a ranking of vowels, and the voice
actually chosen is what decides.

### One revision at a time

When revising a melody, fix the one problem that matters most (an unclear motif, crowded
words, a masked target, an unsingable leap, sections that do not differ, an abrupt ending),
keep the previous version to compare against, and stop after two local attempts on the same
problem. Change one feature at a time with the backing held fixed, or you cannot tell what
helped. When a listener prefers one take's melody and another's backing, keep the preferences
apart rather than counting wins.

## Arrangement phase

### Part roles and registers

Give every part one job — foundation (kick and bass), harmony (pads, chords), pulse (hats,
strummed or muted guitar), colour (counterlines, fills, effects) — and its own register, so
no two parts fight for the same octave at the same time. Most muddiness is two parts in one
register, not too many parts. Overlap is not always a fault: a unison or octave doubling can
be the point. An answering part fills the foreground's gaps instead of doubling its attacks.

### Role palettes

Starting points, not templates; remove or swap a part when the foreground needs room.

| context | intimate | rising | contrast |
| --- | --- | --- | --- |
| piano and voice | low root and broken chords in the middle, voice above | sparse verse piano, bass enters at the chorus, a high answering figure | held voicings under phrases, fills only in the voice's gaps |
| pop band | kick and bass, offbeat guitar, one pad | drums held back in the verse, bass and wide guitars open in the chorus | steady bass while the guitar goes from muted to sustained chords |
| electronic | sub bass on the root, one mid pulse, dry lead | filtered pulse in the verse, full transients at the arrival | lead leaves holes for an answer; automation carries the transition |
| game cue | ostinato, sparse harmony, the motif | a register or counterline at the scene's turn, thinned for the loop return | texture changes while tempo and motif stay put |

Mute each role in turn and name what the music loses; two parts that answer the same question
at once want one of them cleared away or their entrances interleaved.

### Subtract before adding

When a section is flat or crowded, remove, delay, shorten or thin a supporting part before
adding another. Contrast can come from register, articulation, note length, rhythm, a change
of lead sound or silence, and a restrained verse is what makes a full chorus mean something. A
cue meant to be sparse stays sparse. Solo the foreground and bring the roles back one at a
time to hear what each adds.

### Voicings out of the voice

Keep chord voicings out of the octave the melody is singing in, or thin them while the voice
is there: below it for warmth, above it for air. Piano and guitar voicings written by
`accompaniment` or `generate_phrase` can be moved with `edit_notes` or `set_performance`
(`octave_above`, `octave_below`). Try a thinner voicing, shorter notes, another octave or a gap
before reaching for EQ, and keep low chord stacks out of the bass's octave.

### Bass and inner voices

A bass line supports the roots and moves between them by step or with purpose. Over
`C | Am | F | G` in 4/4, `C3 dotted half, B2 quarter | A2 dotted half, G2 quarter | F2 dotted
half, E2 quarter | G2 whole` walks down to each root, and the passing notes (B over C, G over
Am, E over F) are colours the melody must allow. Above it, sustained pairs
`E4/G4 | E4/A4 | F4/A4 | G4/B4` keep common tones and move by small steps. Check both against
the lead and the kick, and shorten a connection or move an inner voice when it competes.

### Kick and bass together

Write the bass's attacks on the kick's, or deliberately in its gaps, and keep its rhythm
simpler than the drums'. Bass notes that start just off the kick are heard as a band out of
time. Read both parts' notes side by side before accepting either. Decide which of the two
owns the deepest sustained energy, and shorten or move the other before compressing anything;
a deliberate rub or distortion that serves the style stays.

### Fills and crashes at transitions

Mark the end of a section with a one-beat or one-bar drum fill and land the next section's
first beat with a crash. Not every transition: the strongest ones (into a chorus, out of the
drop chorus) earn them.

### Drum groove by section

Give each section a main groove of its own instead of one bar played everywhere with the
cymbals changed: closed hats and a sparse kick for verse 1, sixteenth hats or snare ghosts in
verse 2, half time or toms in the pre-chorus, the ride or crash on eighths with a busier kick
in the chorus, half time again in the bridge. At fast rock tempos a two-beat feel (kick and
snare alternating on eighths) is the classic chorus drive. Scale fills with the phrase: a
small one every four bars, a larger one at eight, the largest into a chorus. Line hits up
with the riff or the vocal's accents for a キメ.

### Build-up and break

Before a chorus, build: add a snare roll, rising notes, a filter opening, or density that
grows bar by bar. Or break: stop everything for a beat or two before the chorus, so its first
beat arrives out of silence. A break needs the pre-chorus melody to end before it. Not every
boundary: a riser or crash at each one stops being heard.

### Interludes and bridges

Decide whether an interlude has a job (a breath after a dense chorus, a harmonic turn, an
answer to the voice, a reset of register, a set-up for the return) or only colours a stable
passage; a decorative one need not promise a new section. Reuse a small motif in another
rhythm, register, sound or as call and response rather than pasting the chorus. Listen to both
joins, into the interlude and out of it.

### Loop return

In a looping cue a one-off gesture recurs every cycle, so a fill at the end becomes an accent
every time round. Play the end straight into the beginning, reverb tail included, rather than
judging each edge alone, and keep the loop boundary on a bar line.

### More in verse 2

Make verse 2 differ from verse 1 in its backing, not only its words: add a counterline, move
the drums from the hi-hat to the ride, double the bass in octaves, bring in a part that was
saved for later. A second verse that sounds like the first is heard as a loop.

### Copy, then vary

Write a pattern once, copy it with `edit_clip`, and change the copy: a fill at its end, a
different last bar, a part dropped for two bars. A copy that is never changed reads as a loop;
a pattern rewritten from scratch each time loses its identity.

### Performance controls

`set_performance` changes how notes are played without rewriting them: `octave_above` to
double a chorus line, ghost notes and `humanize` on hats, `strum` on guitar chords, `swell`
for phrase dynamics, `gate` for detached playing. Swing is the song's feel, not seasoning: a
straight song stays at 50, and a swung song swings every part, vocal included, at the same
amount.

Typed notes play exactly on the grid until a performance stage moves them. List
`performance_presets` with the clip, start from its `suggested_preset` or a preset that suits
the genre better, read `inspect_performance`, then tune with `set_performance` and read back
what took effect. Leave a part machine-tight only where the style wants it (chiptune, a drum
machine). An instrument's controls are its own: do not assume a tutorial's CC1, CC11 or
keyswitch maps onto an Auris parameter of a similar name.

### Sounds for the part's job

Choose a sound for what the part does and how it can be played. A fast repeated figure needs a
clear attack and a short release; a held line needs a convincing sustain and tail. A short
gate on a sustained patch is not a sampled staccato. In orchestral writing, hand a returning
theme between families or octaves instead of doubling the whole ensemble throughout. Leave
rests that match the actual instrument's decay.

### Strummed chords

A hand sweeping a pick across strings never strikes them at once. Apply a guitar performance
preset (`rock_rhythm_guitar`, `acoustic_strum`, `funk_cutting_guitar`) from
`performance_presets`, then tune its `strum`. Keep `spread_ms`, the time from the first string
to the last, at the preset's value or wider: listeners need about 15 to 20 ms to tell which of
two sounds came first, so a chord spread over 7 ms is heard as one hit. `string_ms` sets the
time per string instead, so a six-string chord takes longer than a three-string power chord.
`variation` changes each stroke's speed and string spacing a little; at zero every chord is
struck identically. Alternate the direction on the strumming clock, with lighter upstrokes on
fewer strings (`up_notes`, `up_velocity`).

### Generated takes are drafts

`generate_phrase` writes a take of any part over the written harmony: lead (on a singer track
too), chords, pad, arpeggio, bass or drums. Its `settings` start the take where the song needs
it: `density`, a pitch `range` the part is moved into by octaves, a grid `rhythm` and `swing`.
Read what it wrote and edit it where the song needs something else; a generated line is a
starting point, and what a listener remembers (the hook, the bass figure, the fill into the
chorus) is usually where the edits go.

## Mix phase

### Volume and pan first

Set faders and pans with `mixer_graph` before any processing, with the voice clearly on top.
Most "needs EQ" turns out to be "is too loud". Pan supporting parts off centre and keep kick,
bass and voice in the middle.

### Low cut on everything but the bass

Turn on the high-pass (`hp_enabled`, `hp_freq`) of `auris.fx.eq` on parts that do not need
their lowest octave — pads, guitars, keys, the voice — so the bass and kick own it.

### Choosing a processor

Pick the processor by the change wanted: EQ changes the spectral balance, a compressor the
level envelope (its attack and release decide how transients and sustain relate), saturation
the colour. Compare with the processor bypassed at a matched level; a louder version is not a
clearer one. A pumping or distorted sound that is the idea of the track stays. Processing does
not replace a missing part or fix two parts that clash; change the notes first.

### Kick-triggered sidechain

Duck the bass or pads by a few decibels on every kick with `auris.fx.compressor` and
`set_sidechain` from the kick track, so the kick cuts through without being louder.

### Reverb and delay returns for the voice

Put `auris.fx.reverb` and `auris.fx.delay` on a bus, give the voice a second output into it with
`add_output`, and keep the bus quieter than it sounds right in solo. Shared returns make parts sit
in one room;
an insert reverb on each track makes many rooms.

Dense reverb blurs words and attacks. When the voice needs depth but must stay clear, try a
short or tempo-synced delay before more reverb, filter the returns' highs, and raise the
reverb's pre-delay until the consonant stands apart from its tail; a pre-delay that suits one
tempo or voice does not carry to another. Check the end of the song for tails that pile up.

### Bus processing

Route related parts (drums, backing vocals, strings) to a bus with `set_outputs` and treat
them together: one compressor glues a drum kit better than one on each drum.

### Section automation

Automate faders and output levels section by section with `automation`: the chorus a decibel up, the
drop chorus's reverb longer, the last chorus wider. A mix that is the same everywhere makes the
arrangement do all the work. To make a section recede without an obvious drop in level,
automate a low-pass or a send instead of the fader, taking care not to remove the melody's
upper partials or the voice's consonants.

### normalize_mix last

Run `normalize_mix` after the last balance change, then measure the whole song with
`inspect_audio`. Normalising first and balancing after undoes it.
