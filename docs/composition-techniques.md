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

### Chorus lift

Keep the verse and pre-chorus low enough that the chorus can rise a minor third to a fifth on
average, give the chorus the song's highest notes, and let its hook leap: a fifth, a sixth or
an octave, even an octave jumped back and forth. End the pre-chorus on a held note, a rest or
a rising line so the chorus lands on a strong beat. Read `melody_shape` after writing the
melody and act on its warnings before singing.

### Motif repetition and development

Write one short cell with a distinct rhythm or contour, then develop it: keep the rhythm and
change the last interval, move it to another register, place it over another chord, stretch
one value. Repeat it exactly where a hook must be recognised. A repeat that keeps the words'
mora count keeps the motif; see the lyric-fitting rules in the vocal guidance.

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
