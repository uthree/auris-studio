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
The same mf Cs, with seven more notes, are what the [stretch tuning](piano-stretch-tuning.md)
is read beside, under a lock of their own.

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
  tools/eval/test_clarinet_copy.py tools/eval/test_fetch_clarinet_reference.py \
  tools/eval/test_fetch_folk_reference.py tools/eval/test_physical_ab.py
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
