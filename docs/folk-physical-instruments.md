# Folk physical instruments

Auris Studio includes two sample-free instruments aimed at folk timbres:

- `auris.physical.hammered_dulcimer`: zero-based GM hint 15 (program 16 in one-based lists).
- `auris.physical.tin_whistle`: zero-based GM hint 78 (program 79 in one-based lists).

Both are ordinary registered instruments. Their parameters are saved in the project and they can
be used without a SoundFont or another sample library.

## Models

The hammered dulcimer has 24 voices, 24 stiff-string partials, and two or three lightly detuned
strings per course. A finite raised-cosine hammer contact supplies the excitation force. Partial
frequencies use a stiffness-normalized inharmonic ratio, and a passive course mixing junction
approximates bridge and soundboard coupling. The controls are hammer hardness, strike position,
string decay, bridge damping, string stiffness, course detune, release, and level.

The tin whistle has 16 voices. Each voice uses a prepared fractional-delay open-open bore and a
band-pass first-bore mode at Q=3 inside the feedback loop. A nonlinear fipple jet is driven by
pressure and returned bore pressure; a small noise term supplies turbulent onset. There is no
separate oscillator. This is deliberately a reduced model: it does not model the complete jet
convection or tone-hole network. The controls are breath pressure, jet shape, breath noise,
attack, release, and level.

All voice storage, delay lines, envelopes, and filter state are prepared before audio processing.
The realtime paths do not allocate, lock, block, or perform I/O. The current implementation has
24 dulcimer voices and 16 whistle voices. A stereo, 256-frame callback at 48 kHz measured
1.030 ms mean / 1.115 ms p99 for 24 dulcimer voices and 0.043 ms mean / 0.046 ms p99 for
16 whistle voices, over 512 blocks after 96 warmup blocks. These figures do
not establish a worst-case burst bound for many simultaneous note-ons.

## Recording data and calibration

The reference manifest is `tools/eval/references/folk-notes.json`. It contains 18 notes:
10 dulcimer pitches (six training and four validation) and eight whistle pitches (four training
and four validation). MIDI pitches are disjoint between the splits. The whistle material is one
continuous D-major scale, so its notes are steady excerpts from one recording rather than eight
independent takes. The primary whistle metric removes the attack and release. The dulcimer clips
are complete public previews, but the available dulcimer material is a public HQ MP3 preview
transcoded to WAV; the original WAV download requires a Freesound login.

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

Calibration starts from the initial controls stored in
`tools/eval/references/dulcimer-initial.json` and `whistle-initial.json`. Those controls define the
baseline of the newly implemented instruments, rather than a previous released instrument. The
optimizer changes only the model controls that are exposed in the calibration files. Seeded
differential evolution uses four iterations, population multiplier four, and no polishing:
140 evaluations for seven dulcimer controls and 60 for three whistle controls. Only training
pitches enter the objective; this short search is not a global optimum. Renderer velocities
are fixed at 0.65, rather than inferred from recording dynamics. Measured source detuning is
passed to the renderer; available excerpt duration supplies an approximate note hold. Dulcimer
uses a phase-weighted log-mel loss (30% attack, 30% body, 40% tail), envelope error, and T90;
whistle uses steady-excerpt log-mel and envelope error. The losses are different and their numeric
values must not be compared between instruments. Both use 64 HTK mel bands from 30 Hz to 10 kHz,
512/2048 sample windows, and a 240-sample hop at the 24 kHz evaluation rate. Notes are RMS
normalized to RMS 0.1 after a causal second-order 30 Hz highpass, without time warping or
framewise gain adjustment. Dulcimer attack/body/tail boundaries are 0/50/250/3000 ms.
Whistle references discard the first 150 ms and last 100 ms of each stable scale region;
the comparison uses up to one second of the retained steady excerpt and the same duration
of synth audio starting 200 ms after note-on. The fit is performed at 24 kHz; the native result below
renders the final defaults at 48 kHz and polyphase-resamples them to 24 kHz for comparison.

The native 48 kHz held-out results are:

| model | validation log-mel, initial | validation log-mel, calibrated | change | envelope dB, initial → calibrated | T90 s, initial → calibrated |
| --- | ---: | ---: | ---: | ---: | ---: |
| Hammered dulcimer | 0.2616068 | 0.2421653 | 7.4316% lower | 6.7731 → 4.7017 | 0.25623 → 0.16363 |
| Tin whistle | 0.0182215 | 0.0179626 | 1.4209% lower | 1.2928 → 1.2981 | not measured |

Each validation result covers only four held-out pitches. The paired 95% confidence interval for
the log-mel change, using 2,000 paired pitch bootstrap resamples, includes zero for both models
(`[-0.05373, 0.01485]` dulcimer and
`[-0.000522, 0.0000651]` whistle). The whistle envelope error is slightly worse after fitting.
These are useful regression measurements for this small corpus, not evidence of a universal
perceptual improvement. Dulcimer T90 is evaluated; whistle T90 is not part of the steady excerpt
objective. T90 is the time containing 90% of band-limited energy in the retained three-second
window, rather than a measurement of a complete physical decay beyond the available preview.
Per-note scores, full-precision fit results, initial controls, search bounds, evaluator hashes
and runtime versions are retained in `tools/eval/references/{dulcimer,whistle}-*.json`.

An independent pitch probe uses held notes at velocities 0.35, 0.65, and 0.95 at 24 and 48 kHz.
Whistle covers D5–D7 (MIDI 74–98), and dulcimer covers C3–C6 (MIDI 48–84), giving
150 conditions for whistle and 222 for dulcimer. Maximum absolute pitch error is 1.8523 cents
for whistle and 0.1567 cents for dulcimer. The strongest detected component was the fundamental
in every condition; maximum DC fractions were 0.000830 and 0.003590 respectively. The probe
uses 1.2-second holds, discards 350 ms, and checks the strongest peak across 20 Hz to
min(10 kHz, 0.49 × sample rate), alongside the interpolated fundamental peak.

## Reproduction

The following commands run from the project checkout. Reference preparation also needs `ffmpeg`
on PATH for the MP3 previews. `uv` manages the Python dependencies. Fetch and lock the references
(add `--offline` to use cached captures):

```bash
uv run tools/eval/fetch_folk_reference.py target/folk-physical/reference --lock tools/eval/references/folk-notes.json
```

Build the renderer used by the evaluator:

```bash
cargo build --release -p auris-synth --example physical_fit_render
```

Render the final 48 kHz defaults and compare them with the frozen initial controls. The
`--calibration` file validates and records fit provenance; it does not override current defaults.
`--initial-params` makes the baseline reproducible. On Windows the executable is the `.exe` file
shown below; omit `.exe` on macOS/Linux:

```bash
uv run tools/eval/folk_copy.py target/folk-physical/reference target/release/examples/physical_fit_render.exe target/folk-physical/dulcimer-validation.json --model hammered_dulcimer --initial-params tools/eval/references/dulcimer-initial.json --calibration tools/eval/references/dulcimer-fit.json --rate 48000
uv run tools/eval/folk_copy.py target/folk-physical/reference target/release/examples/physical_fit_render.exe target/folk-physical/whistle-validation.json --model tin_whistle --initial-params tools/eval/references/whistle-initial.json --calibration tools/eval/references/whistle-fit.json --rate 48000
```

To repeat the 24 kHz search, replace `--calibration` with
`--fit-bounds tools/eval/references/dulcimer-bounds.json` or `whistle-bounds.json`, use
`--rate 24000`, and retain `--initial-params`. Use a different output filename to preserve
the 48 kHz comparison. The pitch probe is:

```bash
uv run tools/eval/folk_tuning.py target/release/examples/physical_fit_render.exe target/folk-physical/folk-tuning.json
```

The editable listening examples are generated with:

```bash
cargo run -p auris-session --example folk_demo -- target/folk-physical/final-demo
```

The demo is 48 kHz stereo, 12 seconds, with no effects, normalization, or balance adjustment.
The measured peak magnitudes are 0.275070071 for the dulcimer solo, 0.190560580 for the whistle
solo, and 0.417004585 for the combined file. The generated projects retain the notes and
instrument parameters so they can be edited directly.

## Verification

Numeric DSP tests cover pitch, modal decay, contact hardness, release, controller response,
large bends, finite output at extreme rates/controls, block-size independence, reset and
callback allocation counts. Session tests select both native/GM sounds, Undo selections,
save/load custom controls and reproduce rendered PCM without a SoundFont. The workspace
run passed 4,056 tests with 15 ignored; the Python evaluator suite passed 43 tests.
Workspace Clippy, Ruff, formatting and rustdoc with warnings denied also passed.

The symbolic preset report and all nine existing preset WAVs remain byte-identical to the
pre-change checkpoint, with identical learned Audiobox scores. This verifies existing presets;
those learned scores do not evaluate the new instruments. Hashes, scores, demo levels and
callback timing are retained in `tools/eval/references/folk-regressions.json`.

```bash
cargo test --workspace -- --test-threads=1
cargo clippy --workspace --all-targets
cargo run --release -p auris-synth --example physical_bench -- --folk
```
