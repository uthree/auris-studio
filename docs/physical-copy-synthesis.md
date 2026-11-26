# Physical instruments against real recordings

[日本語](physical-copy-synthesis.ja.md)

The physical piano, guitar, strings, reeds, whistles and hammered strings are measured against
frozen cohorts of **real recordings**. Each tool plays the recorded notes through the actual
Rust instrument and reports how far the result lies from the recording. The instrument is
played by the worker, `physical_fit_render`: freeze a copy of it before changing anything that
sounds, and measure both builds afterwards. None of these tools changes an instrument.
Downloads, Python, plots and listening files are development tools, and no recording enters
the product.

What the instruments currently read is reported with them, in
[physical instruments](physical-instruments.md). This page is what the tools record, measure
and need.

## Piano, guitar and violin

The [University of Iowa Musical Instrument Samples](https://theremin.music.uiowa.edu/MIS.html)
are created by Lawrence Fritts and the Electronic Music Studios. The university permits their
download and use in projects without restrictions. Recording details and original download links
are on the [piano](https://theremin.music.uiowa.edu/MISpiano.html),
[guitar](https://theremin.music.uiowa.edu/MISguitar.html) and
[2012 violin](https://theremin.music.uiowa.edu/MISviolin2012.html) pages.

The cohort uses the Steinway model B played by Evan Mazunik, Raimundo 118 classical guitar
played by Brian Penkrot, and Nicolai Tambovsky violin played by Leonid Iogansen. Guitar and violin
were recorded in an anechoic chamber; piano was recorded in a room. A distance to these
recordings therefore includes the microphone and, for the piano, the room.

There are 36 source captures and 165 extracted notes, at pp/mf/ff mapped to velocities
0.35/0.65/0.95. These are ordinal performance conditions, not measured MIDI velocities.
The download lock is `tools/eval/references/iowa-captures.json`. The note lock,
`iowa-notes.json`, records each source, cut offset, pitch, measured tuning, split and SHA-256.
The downloader verifies original hashes. The preparation tool can verify the complete note lock.
Prepared float WAVs omit timestamped PEAK metadata so repeated extraction preserves byte hashes.

| Instrument | Pitch coverage | Training notes | Validation notes |
| --- | --- | ---: | ---: |
| Piano | 48, 52, 55, 60, 64, 67 | 6 | 12 |
| Guitar | 40–47, 50–59, 64–71 | 34 | 44 |
| Violin | 55–59, 62–71, 76–83 | 30 | 39 |

Piano pitches 52/60/67 are held out. Guitar/violin pitches whose MIDI number modulo three is one
are held out. All pp recordings are also held out, including pp on otherwise trained pitches.
The split is part of the frozen cohort: a search over an instrument's controls, or over the
voicing dials a `physical-calibration` build of the worker takes in a request's `dials`, may
read the training notes only, and the validation notes are the ones to quote. They are new
pitches and weak dynamics from the same recording sessions, not new instruments.

Pitch-conditioned harmonic tracking locates stable scale notes and requires ascending order.
Onset detection backtracks up to 200 ms and uses a local 2% RMS threshold with a 5 ms guard.
Audible harmonics estimate a fixed tuning offset, checked within 75 cents of the label; weak
harmonics are excluded. The renderer receives this measured offset before note-on. No pitch,
time alignment or per-note synthesis parameter is adjusted to a recording.

### Temporal mel distance

Every note is mono at 24 kHz and covers the first **one second** after onset, with the key held.
This common window fits every capture. Each whole note is normalized to RMS 0.1. Individual
frames are never normalized, so attack and decay remain measurable; there is no time warp.

Two Hann STFTs use 512/2048-sample windows and a 240-sample hop. Sixty-four triangular HTK mel
bands span 30 Hz to 10 kHz with unit-area normalization. Power is compressed as
`log(1 + 10000 * mel_power)`. For each resolution, absolute error is averaged separately over
the first 15 frames and remaining frames; their means have equal weight. The final distance
averages both resolutions and all notes equally. Smaller is closer under this definition. It is
a dimensionless acoustic distance, not a perceptual quality score or a measurement in decibels.
It discards phase and resolves harmonics only within its bands; listening and pitch/stability
tests remain needed. `tools/eval/physical_mel.py` is the definition.

`evaluate_physical_mel.py` reports the distance of both builds for every note of both halves
of the split, with each build's factory parameters and hash. Both builds play at those factory
settings, which for the Strings is a section of three players where the recording is a solo
violin. `--production-rate 48000` plays both builds at 48 kHz and resamples them into the same
24 kHz measure.

It also emits a deterministic audition: piano, guitar, violin, each in the order
**recorded / before / after**, with whole-note RMS matched and common peak headroom. The mf
validation note nearest MIDI 60/55/67 is chosen ahead of scoring; no best-performing note is
selected. Plots use the same color scale for each instrument's three panels.
Audition cuts have 5 ms boundary fades to prevent clicks; scoring uses the untouched PCM.

## A Steinway from C1 to C8

The University of Iowa's [Steinway model B recordings](https://theremin.music.uiowa.edu/MISpiano.html),
performed by Evan Mazunik, supply mf notes on every C from C1 to C8, and pp/ff notes on C2, C4,
C6 and C7. pp C6 and pp C7 lie within a few dB of the recording's noise floor and are not
measured: 14 of the 16 locked captures are. URLs, sizes and hashes are in
[piano-register-captures.json](../tools/eval/references/piano-register-captures.json).
The same mf Cs, with twenty more notes, are what the [stretch tuning](piano-stretch-tuning.md)
and the stiffness of the piano's strings are read beside, under a lock of their own.

`piano_register.py` measures at 24 kHz on onset-aligned notes, every 25 ms, against each file's
own noise floor:

| Measure | What it compares |
| --- | --- |
| decay | Each of up to twelve partials' level relative to itself at 100 ms, with the recording's floor added to the render |
| envelope | Broadband level relative to 100 ms, through the same floor |
| spectrum | Partial levels over 50–400 ms relative to their total |
| level | 50–500 ms level, compared with the same-dynamic C4 through offsets frozen at the start |
| knock | Band power under 0.6 f0 at 20/60/120 ms, absolute through the same offsets, above the floor only |

Each is a mean absolute distance in decibels. Every measure is relative within one note except
`level` and `knock`, which go through the per-dynamic offsets taken at C4. One recorded piano,
one microphone position and three dynamic labels stand behind them, and there is no reference
for the dampers, the release or the pedal. These measures describe resemblance to recordings,
not preference; listen.

### What a note does, rather than what one take did

Both measures above hold a rendered note to one recording frame by frame, which holds the model
to the recorded piano's own beats: a unison that beats as much but at another moment reads as
wrong. `piano_character.py` reads statistics of a note instead, the same way from the recording
and the render, over 83 notes of the same Steinway from A0 to C8 at three dynamics, locked in
[piano-character-captures.json](../tools/eval/references/piano-character-captures.json):

| Measure | What it compares |
| --- | --- |
| balance | Octave bands against the note's total at the blow, in the tone and later, where the recording stands 10 dB clear of its noise |
| level | Each note against the mean of the mezzo-forte notes: the keyboard and the range of touch, without the overall gain |
| decay | The median fall of each octave band's partials over the first second and the next two |
| spread | How unevenly one note's partials fall: the interquartile range of their first-second decays |
| wobble | How far a partial swings about its own decay: the beats of a unison or of a wound string's planes |
| metal | Energy in peaks off the transverse series against the energy on it, from A0 to A4 at mezzo-forte and fortissimo |
| pulse | How strongly the bands from 500 Hz to 4 kHz pulse once a period, from A0 to E4 at mezzo-forte and fortissimo |

Each recording's noise is the quietest tenth of its frames, bin by bin; a partial is followed
only while it stands 15 dB clear of it. Each measure is the rms of model minus recording in
decibels, and the total their mean, so it can be handed to a search. The recordings are measured
once and kept beside them.

## B♭ clarinet

The [University of Iowa Bb Clarinet MIS page](https://theremin.music.uiowa.edu/MISBbclarinet.html)
identifies a Buffet R13 Bb clarinet, Glenn Bowen, recorded on 3 August 1998
in the Wendell Johnson anechoic chamber with a Neumann KM 84, at 16-bit,
44.1 kHz mono. The [MIS collection terms](https://theremin.music.uiowa.edu/MIS.html)
permit downloading and using these recordings in any project without restrictions.
An online preparation run caches both pages beside the original captures.

The frozen cohort uses `D3B3`, `C4B4` and `C5B5` recordings at `pp`, `mf` and `ff`:
34 sounding pitches, MIDI 50–83, and 102 note instances. The filenames identify
concert pitches: D3, C4 and C5 fundamentals are approximately 147, 262 and 523 Hz.
Do not transpose these labels for the instrument's Bb designation. All three
dynamics of each even pitch belong to training (51 notes); odd pitches belong to
validation (51 notes). This holds out pitches within one instrument, player and
recording setup, rather than an independent instrument or player.

Chronological regions are detected using a 5 ms RMS envelope at 2% of capture
peak, with short gaps merged. Each note starts with 5 ms preroll and ends 250 ms
after its detected region, then is cropped or zero-padded to three seconds and
polyphase-resampled to 24 kHz. Detected region duration supplies an approximate
hold, capped at 2.98 seconds in the renderer. Some notes therefore sustain for
the entire retained window; T90 measures energy timing in that window, rather
than a complete physical release. Velocity maps `pp/mf/ff` to 0.35/0.65/0.95;
these are development choices, not measured MIDI velocities. Measured recording
detuning is passed to the renderer. Source and prepared hashes, times and pitches
are retained in `tools/eval/references/clarinet-notes.json`.
The manifest checksum normalizes CRLF to LF so Git's platform line endings
do not invalidate it.

`clarinet_copy.py` measures the worker's `clarinet`, the Reeds instrument. Both signals receive
a causal 30 Hz highpass and whole-note RMS normalization to 0.1. There is no time warping or
framewise gain adjustment. The log-mel configuration is the one above: 512/2048 Hann windows,
240-sample hop and 64 bands from 30 Hz to 10 kHz. Its mean absolute distance is weighted
0.3/0.3/0.4 over attack (0–50 ms), body (50–250 ms) and tail (250 ms–3 s). Beside it stand
the 20 ms RMS-envelope distance in dB and the absolute error of T90, the time containing 90%
of the band-limited energy. A paired bootstrap with 2,000 resamples keeps all dynamics of each
pitch together. The instrument plays at 24 kHz unless `--rate 48000` asks for 48 kHz, which
is then resampled to 24 kHz for the same measure.

## Hammered dulcimer and tin whistle

The reference manifest is `tools/eval/references/folk-notes.json`. It contains 18 notes:
10 dulcimer pitches (six training and four validation) and eight whistle pitches (four training
and four validation). MIDI pitches are disjoint between the splits. The whistle material is one
continuous D-major scale, so its notes are steady excerpts from one recording rather than eight
independent takes. The dulcimer clips are complete public previews, but the available dulcimer
material is a public HQ MP3 preview transcoded to WAV; the original WAV download requires a
Freesound login.

The whistle source is [Whistle.wav on Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Whistle.wav)
([direct WAV](https://upload.wikimedia.org/wikipedia/commons/6/65/Whistle.wav)), licensed CC BY-SA
4.0. Its author field is not machine-readable; the manifest credits Escola Superior de Música de
Catalunya (ESMUC), without asserting that credit as an author identity. Dulcimer previews come
from [iternetcone's Freesound pack](https://freesound.org/people/iternetcone/packs/19445/) and
the individual source pages recorded in the manifest; they are CC BY 4.0. The prepared reference
manifest checksum is `bcf55f3f56d867ec5f3e88aaf7813b9963df7009e891520ded691f182be19016`,
with CRLF normalized to LF. Each raw capture and prepared WAV also has a strict byte checksum.
Harmonic spacing and autocorrelation distinguish the weak fundamentals of C4/D4/E4 from their
stronger upper partials. The separately recorded High C5/High D5 notes remain distinct pitches;
the rounded octave audit is retained in `tools/eval/references/folk-pitch-audit.json`.

`folk_copy.py` measures the worker's `hammered_dulcimer` and `tin_whistle`, the Hammered Strings
and Whistles instruments. Renderer velocities are fixed at 0.65, rather than inferred from
recording dynamics. Measured source detuning is passed to the renderer; available excerpt
duration supplies an approximate note hold. Dulcimer uses the phase-weighted log-mel distance
of the clarinet (30% attack, 30% body, 40% tail), envelope error, and T90; whistle uses
steady-excerpt log-mel and envelope error. The two are different and their numeric values must
not be compared between instruments. Notes are normalized to RMS 0.1 after a causal second-order
30 Hz highpass, without time warping or framewise gain adjustment. Whistle references discard
the first 150 ms and last 100 ms of each stable scale region; the comparison uses up to one
second of the retained steady excerpt and the same duration of synth audio starting 200 ms after
note-on. T90 is the time containing 90% of band-limited energy in the retained three-second
window, rather than a measurement of a complete physical decay beyond the available preview.
`--rate 48000` is as for the clarinet.

Each validation result covers only four held-out pitches, so the paired 95% interval of a
change, from 2,000 paired pitch bootstrap resamples, is wide. These are regression measurements
for a small corpus, not evidence of a perceptual difference.

`folk_tuning.py` is an independent pitch probe. It holds notes for 1.2 seconds at velocities
0.35, 0.65 and 0.95, at 24 and 48 kHz: the whistle over D5–D7 (MIDI 74–98) and the dulcimer over
C3–C6 (MIDI 48–84). It discards 350 ms and checks the strongest peak across 20 Hz to
min(10 kHz, 0.49 × sample rate), alongside the interpolated fundamental peak.

## Slapped bass

A slapped note has no reference of its own to be held against: no two basses, pickups or
recording chains sound alike, and a model voiced toward one bass would be measured as wrong
against the next. `bass_slap.py` therefore measures a *technique*. Its recordings are single
notes of the [IDMT-SMT-Bass dataset](https://zenodo.org/records/7188892) (Fraunhofer IDMT,
Jakob Abeßer; CC BY-NC-ND 4.0), which has every note of three electric basses played
finger-style, with the thumb and popped. Each measure of a thumbed or popped note is taken
less the same measure of the same note, on the same bass through the same pickup, plucked.
That difference is what the hand does, and it is the same kind of number for a rendered note
as for a recorded one.

The notes are those of the neck pickup (`EQ_1`) on all four strings of all three basses. The
open strings and the fifth and twelfth frets, 36 notes each way, are the ones the model was set
on; the third and eighth frets, 24 each way, had no part in it. `fetch` reads only those notes
out of the 1.6 GB archive, by HTTP range requests; nothing is redistributed or committed.

A note is read from its onset, the first sample a tenth as large as its largest, over six
tenths of a second at 44.1 kHz. Its *attack* is the first 40 ms and what it leaves is *held*,
from 0.1 s to 0.4 s.

| Measure | What it reads |
| --- | --- |
| Level | RMS of the first 0.4 s |
| The blow over the note it leaves | RMS of the attack over RMS of what is held |
| The peak over the note | Largest sample over the RMS of the first 0.4 s |
| The fundamental's share | Power within a major third of the fundamental, over all of what is held |
| Fall | RMS from 0.05 s to 0.15 s over RMS from 0.45 s to 0.55 s |
| Bands | Share of the power in each octave band from 90 Hz to 5.76 kHz, in the attack and in what is held |

The model is first voiced toward the recorded basses, whose plucked notes are darker than the
default patch's: **Pickup Resonance** at 1.5 kHz, a fingertip (**Contact Hardness** 0.1) for
the plucked notes and the bone of the thumb (0.9) for the slapped ones, the slapping hand over
the last fret at 32% of the string. Every note is played at a velocity of 0.8; how hard the
recorded notes were played is not known. Five numbers in
`crates/auris-synth/src/physical/slap.rs` were then set by this measure and nothing else about
the model was: the force of a pluck at full velocity (6.5 N), which says how far a string of a
given loudness swings; how much of its speed a string keeps where it meets a fret (a quarter);
the speed of the thumb at full velocity (3.5 m/s) and how much of its mass the string feels
(4 g); and how hard a pop pulls beside a pluck (0.9). The rest is geometry: a 34-inch scale,
twenty frets, strings 4 mm over the line of the frets at the saddle.

Read on 2026-11-24, slapped less plucked, recorded and then rendered:

| | Level | Blow over held | Peak over note | Fundamental | Fall |
| --- | --- | --- | --- | --- | --- |
| Thumb, notes the model was set on | −1.7, −1.7 dB | 4.0, 2.0 dB | 4.6, 3.6 dB | −4.1, −3.5 dB | −0.2, 0.9 dB |
| Pop, notes the model was set on | −1.8, −3.1 dB | 2.9, 3.2 dB | 3.6, 3.2 dB | −0.6, −4.8 dB | 0.8, 1.4 dB |
| Thumb, held-out notes | −3.3, −2.2 dB | 6.0, 3.1 dB | 6.0, 5.1 dB | −4.7, −3.8 dB | 0.5, 1.3 dB |
| Pop, held-out notes | −2.1, −3.9 dB | 2.1, 4.1 dB | 2.6, 3.8 dB | −0.6, −6.2 dB | 0.7, 1.6 dB |

The octave bands from 180 Hz to 2.88 kHz come up by much the same amounts. On the held-out
notes a thumbed attack gains 0, 7, 11 and 19 dB on the recorded basses and 1, 6, 12 and 21 dB
here; a popped one gains −1, 7, 13 and 20 dB and −2, 2, 13 and 22 dB.

Two things are still out. A thumbed note's blow stands 2 to 3 dB less over the note it leaves
than a recorded one's. And the recorded basses' thumbed notes are as loud beside their
plucked ones on every string — 1.8, 2.7, 1.3 and 3.5 dB quieter from the low string to the
high — where the model's run from 0.1 dB louder on the low string to 3.8 dB quieter on the
high: the same thumb at the same speed gives a heavy string more than a light one, and a
player evens that out.
Above 2.88 kHz the difference is not a measure of anything: a rendered plucked note has
nothing there at all, and a recorded one has its noise floor.

The spread between recorded notes is several decibels on every measure and wider than that on
the fundamental's share, which depends on where each bass's pickup lies under each note. These
are regression measurements of whether slapping changes a note as a hand does, not evidence
that a slapped note sounds like any one bass.

## Running the measures

Run Python tools with `uv`. On Windows, use the installed `uv.exe` path if it is missing from
PATH, and `.exe` worker paths. The tools use script environments; no trainer environment or GPU
is required. Preparing the folk cohort also needs `ffmpeg` on PATH for the MP3 previews; the
clarinet and folk fetchers take `--offline` to prepare from cached captures.

```sh
cargo build --release -p auris-synth --example physical_fit_render
# Freeze this executable before changing any audio code, as <before-worker>.

uv run tools/eval/fetch_physical_reference.py target/physical-reference
uv run tools/eval/prepare_physical_reference.py target/physical-reference \
  --lock tools/eval/references/iowa-notes.json
uv run tools/eval/evaluate_physical_mel.py target/physical-reference \
  <before-worker> <after-worker> target/physical-eval

uv run tools/eval/piano_register.py target/piano-register prepare
uv run tools/eval/piano_register.py target/piano-register evaluate \
  <before-worker> <after-worker> target/piano-register/evaluation.json

uv run tools/eval/piano_character.py target/piano-character prepare
uv run tools/eval/piano_character.py target/piano-character compare \
  <before-worker> <after-worker> --json target/piano-character/compare.json
uv run tools/eval/piano_character.py target/piano-character score <worker> \
  --dials '{"77": 0}'

uv run tools/eval/fetch_clarinet_reference.py target/physical-next/clarinet-reference \
  --lock tools/eval/references/clarinet-notes.json
uv run tools/eval/clarinet_copy.py target/physical-next/clarinet-reference <after-worker> \
  target/physical-next/clarinet-validation.json --baseline <before-worker>

uv run tools/eval/fetch_folk_reference.py target/folk-physical/reference \
  --lock tools/eval/references/folk-notes.json
uv run tools/eval/folk_copy.py target/folk-physical/reference <after-worker> \
  target/folk-physical/dulcimer-validation.json --model hammered_dulcimer \
  --baseline <before-worker>
uv run tools/eval/folk_copy.py target/folk-physical/reference <after-worker> \
  target/folk-physical/whistle-validation.json --model tin_whistle \
  --baseline <before-worker>
uv run tools/eval/folk_tuning.py <after-worker> target/folk-physical/folk-tuning.json

uv run tools/eval/bass_slap.py fetch target/slap-reference
uv run tools/eval/bass_slap.py measure target/slap-reference <after-worker> \
  target/slap-reference/measure.json
uv run tools/eval/bass_slap.py fetch target/slap-reference --fret 3 --fret 8
uv run tools/eval/bass_slap.py measure target/slap-reference <after-worker> \
  target/slap-reference/measure-held-out.json --fret 3 --fret 8
```

Pin the recorded dependency versions for exact extraction of the Iowa cohort: NumPy 2.5.3,
SciPy 1.18.1, SoundFile 0.14.0. Its canonical note-manifest SHA-256 is
`878f66a719dd47400ad6246d9a23c8ecb09676c26837bbfa569993099b76c4bc`.
Manifest identity uses sorted, compact JSON encoded as UTF-8, independent of file line endings.
The reports record worker hashes, factory parameters and every per-note distance. A rebuilt
executable's hash depends on its toolchain and build path; compare PCM and metrics rather than
expecting an archived worker's hash.

```sh
uv run --with numpy --with scipy --with soundfile --with pytest pytest \
  tools/eval/test_physical_mel.py tools/eval/test_piano_register.py \
  tools/eval/test_piano_character.py \
  tools/eval/test_clarinet_copy.py tools/eval/test_fetch_clarinet_reference.py \
  tools/eval/test_fetch_folk_reference.py tools/eval/test_physical_ab.py \
  tools/eval/test_bass_slap.py
uv run --with ruff ruff check tools/eval
cargo test -p auris-synth --example physical_fit_render
```

The numeric tests reject silent/non-finite candidates, verify gain and polarity invariance,
detect changed pitch/harmonics/attack/decay, check extraction/tuning, read known decays and
onsets back from synthetic notes, and protect the fixed training/validation separation.

## Listening to two builds

`physical_phrases` saves three fixed, editable projects: piano chords and a line, guitar
picking and strumming, and an overlapping violin phrase with Legato on. Render each with the
`auris` CLI frozen before a change and with the one built after it, as `piano.wav`,
`guitar.wav` and `violin.wav` in a directory per build. `physical_ab.py` then lays the pairs
end to end, before then after, at one RMS with both peaks below 0.9, and writes the gains
beside the WAV, so that a change of level cannot pass for a change of tone.

```sh
cargo run --release -p auris-session --example physical_phrases -- target/phrases
mkdir -p target/phrases/before target/phrases/after
<before-auris> render target/phrases/piano/piano.auris -o target/phrases/before/piano.wav
<after-auris> render target/phrases/piano/piano.auris -o target/phrases/after/piano.wav
# The same for guitar and violin, then:
uv run tools/eval/physical_ab.py target/phrases/before target/phrases/after target/phrases/ab.wav
```
