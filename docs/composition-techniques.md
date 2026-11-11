# Composition techniques

The common moves of song production, one per section, each with what it is for and how to do
it with the Auris tools. The composing agent's phase instructions name these techniques in a
short index; this page is what `search_documentation` returns for the detail. None of them is
a rule. Each is a choice a song may make for a reason, and a song that makes all of them at
once is a checklist, not a song.

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
eighths, a pre-chorus holding longer notes, sixteenths saved for pickups and turns.

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
5. **Close the line.** Lengthen the line's last mora, or rest after it, so the line ends.
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

### Section harmony

A verse may loop a simple progression; the pre-chorus and the chorus need their own. Change
the progression, its rhythm (two chords a bar instead of one, or one chord held for two bars)
or its bass line (a descending bass, a pedal). The verse's loop started on another chord is
the same loop, and a chorus over it sounds like more verse.

### Chorus lift

Keep the verse and pre-chorus low enough that the chorus can rise a minor third to a fifth on
average, give the chorus the song's highest notes, and let its hook leap: a fifth, a sixth or
an octave, even an octave jumped back and forth. Put the leap where it matters; the same leap
at the head of every chorus line stops being heard. End the pre-chorus on a held note, a rest
or a rising line so the chorus lands on a strong beat or just before it. Read `melody_shape`
after writing the melody and act on its warnings before singing.

### Motif repetition and development

Write one short cell, two to six notes over a beat to two bars, with a distinct rhythm or
contour, then develop it: keep the rhythm and change the last interval, move it to another
register, place it over another chord, stretch one value. A cell is not a whole lyric line.
Repeat it exactly where a hook must be recognised, and fit those repeats' words to its notes;
elsewhere keep the cell's head and let the line around it change. See the lyric-fitting rules
in the vocal guidance.

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

## Arrangement phase

### Part roles and registers

Give every part one job — foundation (kick and bass), harmony (pads, chords), pulse (hats,
strummed or muted guitar), colour (counterlines, fills, effects) — and its own register, so
no two parts fight for the same octave at the same time. Most muddiness is two parts in one
register, not too many parts.

### Voicings out of the voice

Keep chord voicings out of the octave the melody is singing in, or thin them while the voice
is there: below it for warmth, above it for air. Piano and guitar voicings written by
`accompaniment` or `generate_phrase` can be moved with `edit_notes` or `set_performance`
(`octave_above`, `octave_below`).

### Kick and bass together

Write the bass's attacks on the kick's, or deliberately in its gaps, and keep its rhythm
simpler than the drums'. Bass notes that start just off the kick are heard as a band out of
time. Read both parts' notes side by side before accepting either.

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
beat arrives out of silence. A break needs the pre-chorus melody to end before it.

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

### Generators for scaffolding, authored notes for what is remembered

Use `accompaniment` and `generate_phrase` for plain chords, pads, simple arpeggios and basic
drum pulses. Write by hand what a listener will remember: the counterline, the bass hook, the
fill into the chorus, the backing vocal.

## Mix phase

### Volume and pan first

Set faders and pans with `mixer_graph` before any processing, with the voice clearly on top.
Most "needs EQ" turns out to be "is too loud". Pan supporting parts off centre and keep kick,
bass and voice in the middle.

### Low cut on everything but the bass

Turn on the high-pass (`hp_enabled`, `hp_freq`) of `auris.fx.eq` on parts that do not need
their lowest octave — pads, guitars, keys, the voice — so the bass and kick own it.

### Kick-triggered sidechain

Duck the bass or pads by a few decibels on every kick with `auris.fx.compressor` and
`set_sidechain` from the kick track, so the kick cuts through without being louder.

### Reverb and delay sends for the voice

Put `auris.fx.reverb` and `auris.fx.delay` on a bus, send the voice to it with `add_send`, and
keep the bus quieter than it sounds right in solo. Shared sends make parts sit in one room;
an insert reverb on each track makes many rooms.

### Bus processing

Route related parts (drums, backing vocals, strings) to a bus with `set_outputs` and treat
them together: one compressor glues a drum kit better than one on each drum.

### Section automation

Automate faders and sends section by section with `automation`: the chorus a decibel up, the
drop chorus's reverb longer, the last chorus wider. A mix that is the same everywhere makes the
arrangement do all the work.

### normalize_mix last

Run `normalize_mix` after the last balance change, then measure the whole song with
`inspect_audio`. Normalising first and balancing after undoes it.
