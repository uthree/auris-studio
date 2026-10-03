# Real-recording calibration of Physical Choir

The wordless choir's factory tract geometry, lip radiation, Tone, Breath and Attack
are calibrated against sustained vowels from **VocalSet**, using the actual Rust
instrument at 48 kHz. The loss is a fixed, gain-invariant temporal log-mel distance.
The recordings and Python dependencies are development assets; the application
uses the fitted constants in `auris-synth`.

## Reference and split

[VocalSet](https://doi.org/10.5281/zenodo.1442513), by Julia Wilkins, Prem Seetharaman,
Alison Wahl and Bryan Pardo, contains professional singers performing sustained
vowels and other vocal exercises. Its recordings are licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). This experiment uses the
`VocalSet11.zip` archive attached to that record, specifically its 60 mono
`long_tones/straight` recordings of /u/, /a/ and /i/ from all 20 singers.

`tools/eval/references/choir-captures.json` pins the archive, ZIP offsets, CRCs and
original-file SHA-256 values. HTTP range requests fetch only the selected entries.
Audio stays in the selected development directory, with a source/license notice.

The extractor takes up to two reliable 1.2-second sustained notes from each capture.
YIN measures periodicity in 4096-sample windows at 24 kHz. An octave check also
requires spectral evidence of odd harmonics before accepting a longer period;
this prevents a strong second harmonic from becoming the guide's fundamental.
The supplied C/F exercise groups separate register changes without fragmenting
natural vibrato. Periodicity gaps up to 100 ms are smoothed; a note needs a stable
span of at least 1.2 seconds. Onsets are backtracked to 10% local RMS. No recording
is pitch shifted, time stretched, looped or normalized per frame.

The frozen result is **104 notes**. Singers f1, f5, m1 and m2 supply 24 training notes,
covering soprano, mezzo-soprano, baritone and tenor. The other sixteen singers supply
80 validation notes. The optimizer loads only training audio. Validation audio is
loaded by the separate comparison command after parameter selection.
`choir-notes.json` stores extraction times, note hashes and pitch-guide fingerprints,
and the tools reject changes to that cohort or split.

Voice Size is a fixed heuristic from the dataset's voice-type metadata, from 0.15
for soprano to 0.95 for bass. It is not an anatomical measurement or a fitted
per-singer parameter. Both versions receive the same reference-only YIN pitch
guide at 20 ms intervals. Ensemble, Width and Vibrato are zero for this diagnostic,
so the fit measures a coherent tract/source voice. The separate factory-ensemble
check restores each version's normal ensemble and vibrato and omits the guide.

## Objective and search

The metric reuses `tools/eval/physical_mel.py`: downsample production PCM to 24 kHz,
normalize each whole note to RMS 0.1, then take Hann STFTs with 512/2048-sample
windows and a 240-sample hop. Each resolution has 64 area-normalized HTK mel bands
from 30 Hz to 10 kHz, compressed as `log(1 + 10000 * mel_power)`. L1 error in the
first 15 frames and the remaining sustain receive equal weight. Resolutions and
notes are averaged. This is a dimensionless acoustic distance, not a dB error or
a perceptual quality score.

`choir_mel.py fit` uses seeded differential evolution, seed 20261004, a population
of three candidates per dimension, six generations and no polishing. Two rounds
alternate a shared source/radiation fit with separate vowel-geometry fits:

| Variables | Bounds |
| --- | --- |
| Tone | 0–1 |
| Breath | 0–0.5 |
| Attack | 10–500 ms |
| Radiation corner | 30–4000 Hz, logarithmic |
| Seven tract areas per vowel | 1/3–3 times the initial area, clipped to 0.1–10 cm² |

The first area is fixed: a uniform scaling of all areas cancels in pressure-to-flow
conversion and would waste a search dimension. The initial geometry and source
controls are pinned in `choir-mel-initial.json`. The development-only
`choir-calibration` feature allows candidate areas, radiation and output gains to
pass through the production DSP. It can reproduce the pre-fit training PCM exactly;
there is no Python surrogate for synthesis.

After 1066 loss evaluations, the fitted values are rounded into Rust constants.
Three output gains preserve the median pre-fit training-note RMS per vowel;
they are separate from the normalized loss and use no validation audio. Existing
saved parameter values remain explicit; factory defaults apply to new instruments.

## Results at 48 kHz

These values come from the compiled factory instrument, including rounded constants
and output normalization. Full per-note losses and executable hashes are in
`choir-mel-calibration.json` and `choir-mel-validation-48k.json` alongside the cohort locks.

| Cohort / vowel | Notes | Before | After | Mean reduction |
| --- | ---: | ---: | ---: | ---: |
| Training, guided voice | 24 | 0.089050 | 0.071616 | 19.6% |
| Validation, guided voice | 80 | 0.097766 | 0.085592 | 12.5% |
| Validation / oo | 26 | 0.086153 | 0.076982 | 10.6% |
| Validation / ah | 29 | 0.121168 | 0.102043 | 15.8% |
| Validation / ee | 25 | 0.082696 | 0.075463 | 8.7% |
| Validation, factory ensemble | 80 | 0.106988 | 0.098582 | 7.9% |

The averages improve in every vowel; some individual notes worsen. These are
individual professional voices, not recordings of a full choir in a room. The
short sustained excerpts assess onset and vowel spectra, not long phrase dynamics,
release realism, ensemble blend or listener preference. Reverb remains an ordinary
effect outside the instrument.

The preselected listening cases are f2 and m3, the first selected note of each
vowel, in recorded/before/after order. The comparison writes a 26.1-second WAV,
an attribution/ordering manifest and common-scale log-mel plots. Listening copies
alone receive RMS normalization, shared peak headroom and 5 ms endpoint fades.
The measured PCM receives no such fades.

## Reproduce

```sh
uv run tools/eval/fetch_choir_reference.py target/choir-mel/vocalset
cargo build --release -p auris-synth --example physical_fit_render --features choir-calibration
uv run tools/eval/choir_mel.py fit target/choir-mel/vocalset target/release/examples/physical_fit_render target/choir-mel/fit.json
```

On Windows, add `.exe` to executable filenames and use `Copy-Item` for `cp`.
The calibration-enabled worker can replay the pinned initial geometry and source
controls even after the factory constants have changed. Keep a copy before building
the final worker without fitting overrides:

```sh
cp target/release/examples/physical_fit_render target/choir-mel/calibration-renderer
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/choir_mel.py compare target/choir-mel/vocalset target/choir-mel/calibration-renderer target/release/examples/physical_fit_render target/choir-mel/comparison --initial-profile
cargo test -p auris-synth --all-targets --features choir-calibration
uv run --with numpy==2.5.3 --with scipy==1.18.1 --with soundfile==0.14.0 --with pytest pytest tools/eval/test_choir_mel.py tools/eval/test_physical_mel.py
uv run --with ruff ruff check tools/eval/choir_mel.py tools/eval/choir_reference.py tools/eval/fetch_choir_reference.py tools/eval/test_choir_mel.py
```

The measurement uses Python 3.12, NumPy 2.5.3, SciPy 1.18.1 and SoundFile 0.14.0.
Run the symbolic ruler and Audiobox evaluations from [evaluation.md](evaluation.md)
before and after changing any audio constants. The nine shipped presets retain
identical WAV hashes, symbolic rows and learned scores after this choir calibration.
