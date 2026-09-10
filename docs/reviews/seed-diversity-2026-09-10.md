# Seed diversity and blinded listening — 2026-09-10

The user explicitly preferred the preceding melody rhythm candidate, despite its lower
Audiobox enjoyment prediction. This follow-up holds that composer fixed at `c6711de` and
compares seeds 101–108 in Rock, City-pop and Pop-band: 24 complete pieces. Seeds were
selected before scoring, and every generated sample is retained. No composer or preset
code changed during this experiment.

Within each style, all eight choruses have distinct normalized rhythms and distinct pitch
interval sequences. Exact uniqueness is a weak diversity criterion, however: Rock 103 and
106 share their opening-bar rhythm and have a whole-chorus onset/duration Jaccard of 0.90.
Changing the seed produces different material, but does not guarantee a different-feeling
groove. Human ratings for this new cohort are **not yet supplied**.

## Subjective comparison

Open the local [listening page](../../target/seed-diversity/listening-final.html). Each
genre presents the same eight-bar first chorus as samples A–H, independently shuffled by
a stable hash that does not use the model scores. Seed numbers, model scores and filename
links start hidden. This is presentation blinding, not protection against inspecting the
page's source. Audio and ratings stay local.

Rate the excerpts with a fixed device volume before revealing the identities:

- **Preference:** 1, not to my taste; 5, strongly preferred.
- **Groove:** 1, difficult to feel the beat; 5, naturally easy to move with.
- **Memorability:** 1, little impression; 5, easy to hum. This is immediate perceived
  memorability, not a delayed recall test.
- **Set diversity:** after listening to A–H within a genre, 1, very similar; 5, very different.

All ratings start blank. Revealing seed/model values displays a table of the eight human
ratings alongside Audiobox CE/PQ and CLAP similarity. Browser storage retains entered
ratings; JSON export/import supports later analysis and checks the corpus identity. Missing
ratings remain absent, and the generator never substitutes a model prediction for a person.
The earlier favorable feedback applies to the melody change, not to these 24 new ratings.

The excerpts are lossless stereo float WAVs, with five-millisecond edge fades and **linear
gain only** to a common -23 LUFS target. No limiter, dynamic normalization, time stretching,
or audio-generative model is involved. Input and final loudness were measured with FFmpeg;
final excerpts span -23.01 to -22.99 LUFS, with true peaks between -11.22 and -8.05 dBTP.
Lengths are 12.97 s for Rock, 18.11 s for City-pop, and 15.48 s for Pop-band. The page also
links the complete raw WAV and editable Auris score after identities are revealed.

## What changes with the seed

Within each genre the key, tempo, chord progression, form, instruments and song-specification
settings other than seed are identical:

| Style | Key | Tempo | Main melody instrument |
| --- | --- | ---: | --- |
| Rock | E minor | 148 BPM | Overdriven guitar |
| City-pop | A major | 106 BPM | Alto sax |
| Pop-band | F major | 124 BPM | Sawtooth lead |

These are complete normal compositions. Accompaniment notes, voicings, fills, performance
variation and automatic track-gain calibration can also change with the seed. Actual lead
gain ranges are -2.36 to -0.78 dB (Rock), -4.66 to -3.57 dB (City-pop), and -5.23 to -4.49 dB
(Pop-band). Other track mixer settings agree. Thus the audio comparison measures the
diversity and preference of whole seed takes; the symbolic comparison isolates the written
main melody. This is a different experiment from the preceding fixed-backing transplant,
so its model scores are not a before/after improvement claim.

## Objective symbolic diversity

`seed_metrics.py` reads the first eight-bar instrumental lead chorus. Onsets and lengths
are rounded to the nearest 240 ticks (a sixteenth note), with halves rounded upward. The
position relative to the start of the bar is retained: an initial rest is not shifted away.
This removes small timing/swing differences from the pattern comparison. Rounding errors
are reported separately; the original score and performance transforms remain untouched.

Pitch shape is the sequence of signed semitone intervals. Uniform transposition does not
change it, octave differences do, and a different number of attacks changes the sequence.
These are descriptive fingerprints, not a musical-quality measure.

| Style | Distinct opening-bar rhythms | Distinct eight-bar rhythms | Distinct eight-bar interval sequences | Identical eight-bar rhythm pairs |
| --- | ---: | ---: | ---: | ---: |
| Rock | 7 / 8 | 8 / 8 | 8 / 8 | 0 / 28 |
| City-pop | 8 / 8 | 8 / 8 | 8 / 8 | 0 / 28 |
| Pop-band | 8 / 8 | 8 / 8 | 8 / 8 | 0 / 28 |

Pairwise Jaccard is the intersection divided by the union of two sets of chorus events.
Onset-only sets compare attacks; onset/duration sets require both the position and length
to agree. Positions include the bar within the chorus. One means identical sets, zero no
shared events. Density and phrase length influence these values; neither end is inherently
better. Exact fingerprints also retain sequence information.

| Style | Mean onset Jaccard | Mean onset/duration Jaccard | Most similar pair | Pair onset/duration Jaccard |
| --- | ---: | ---: | --- | ---: |
| Rock | 0.4922 | 0.2501 | 103 / 106 | 0.9000 |
| City-pop | 0.3887 | 0.1890 | 103 / 108 | 0.5660 |
| Pop-band | 0.4530 | 0.1784 | 104 / 108 | 0.6226 |

Across styles, 3 of 24 matched-seed pairs have exactly the same full-chorus rhythm:
City-pop versus Pop-band at seeds 101, 102 and 106. None shares the complete pitch interval
sequence. This is consistent with the limited shared rhythm vocabulary and common seeded
draws identified earlier. Different seeds reduce that particular cross-style coincidence;
they do not replace style-specific musical vocabulary. Eight seeds per style are a small
probe of the available space, and these proportions are not population estimates.

## Learned model measurements

Audiobox and CLAP score the complete **unmodified** WAVs, not the gain-adjusted excerpts.
The same previously cached CPU models and frozen CLAP prompts are used. CLAP averages
three ten-second windows spanning each piece. Full metadata, per-seed values, spread,
fingerprints, file hashes and measurement conditions are in
[the accompanying JSON](seed-diversity-2026-09-10.json).

| Style | Audiobox CE mean [min, max] | Audiobox PQ mean [min, max] | CLAP positive mean [min, max] |
| --- | --- | --- | --- |
| Rock | 7.335 [7.224, 7.470] | 8.012 [7.907, 8.115] | 0.4796 [0.4670, 0.4919] |
| City-pop | 7.690 [7.627, 7.763] | 8.236 [8.200, 8.276] | 0.4055 [0.3861, 0.4259] |
| Pop-band | 6.817 [6.689, 6.991] | 8.061 [7.993, 8.135] | 0.3878 [0.3631, 0.4001] |

CE predicts enjoyment and PQ predicts production quality. CLAP measures alignment with
the style/instrument descriptions. A narrow range of these scores does not mean the
melodies are identical, and a high score does not establish that a listener prefers a
seed. Human ratings are needed before comparing model ranking with personal preference.

## Reproduction and evidence

The local corpus is `target/seed-diversity/`: `projects/`, `audio/`, `raw-excerpts/`,
`excerpts/`, `manifest.json`, `symbolic.json`, model reports, meter logs and the listening
page. Audio, model caches and browser test profiles are ignored by Git. The renderer is
the archived `target/melody-hooks/after-auris.exe`; its hash and both melody-writer hashes
are recorded and verified. The SoundFont is the same `MuseScore_General.sf2` used previously.

```sh
uv run tools/eval/seed_diversity.py --out target/new-seeds --cli path/to/auris --ffmpeg path/to/ffmpeg --assets path/to/assets --presets rock city-pop pop-band --seeds 101 102 103 104 105 106 107 108
uv run tools/eval/seed_metrics.py --manifest target/new-seeds/manifest.json --json target/new-seeds/symbolic.json
uv run tools/eval/aesthetics.py target/new-seeds/audio --json target/new-seeds/aesthetics.json
uv run tools/eval/clap.py target/new-seeds/audio --json target/new-seeds/clap.json
uv run tools/eval/seed_listening.py --manifest target/new-seeds/symbolic.json --aesthetics target/new-seeds/aesthetics.json --clap target/new-seeds/clap.json --output target/new-seeds/listening.html
```

Independent verification checked 24 projects and 72 WAVs: hashes, duration/rate/channels,
finite non-silent samples, score controls, exact source slicing/fades and exact linear gain
all agree. Every final excerpt's LUFS/true-peak metadata matches the meter's input readings.
The detailed audit is `target/seed-diversity/integrity-audit.json`.

Python evaluation tests, Ruff, full workspace tests (3,270 passed, eight ignored), formatting
and denied-warning workspace/all-targets Clippy pass. The local listening page is also
checked in an isolated headless Chrome profile for audio decoding, initial blinding, blank
ratings, selection, persistence, reveal/hide and narrow-screen layout. Synthetic UI-test
ratings are separate from the user's still-unrated listening cohort.
