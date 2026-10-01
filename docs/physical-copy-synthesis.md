# Physical instrument copy synthesis

The native piano, guitar and violin are calibrated against **real recordings**, using a fixed
temporal mel-spectrogram loss. The Rust instrument renders every candidate; the search adjusts
shared excitation/loss parameters and twelve radiation-filter gains. Only those numbers enter
the product. Downloads, Python, optimization, plots and listening files are development tools.

## Reference and split

The [University of Iowa Musical Instrument Samples](https://theremin.music.uiowa.edu/MIS.html)
are created by Lawrence Fritts and the Electronic Music Studios. The university permits their
download and use in projects without restrictions. Recording details and original download links
are on the [piano](https://theremin.music.uiowa.edu/MISpiano.html),
[guitar](https://theremin.music.uiowa.edu/MISguitar.html) and
[2012 violin](https://theremin.music.uiowa.edu/MISviolin2012.html) pages.

The cohort uses the Steinway model B played by Evan Mazunik, Raimundo 118 classical guitar
played by Brian Penkrot, and Nicolai Tambovsky violin played by Leonid Iogansen. Guitar and violin
were recorded in an anechoic chamber; piano was recorded in a room. Thus radiation fits still
include excitation, microphone and recording coloration; they are not isolated body admittances.

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
Only training notes enter either optimization stage. Validation is loaded after fitting finishes.
This tests new pitches and weak dynamics from the same recording sessions, not new instruments.

Pitch-conditioned harmonic tracking locates stable scale notes and requires ascending order.
Onset detection backtracks up to 200 ms and uses a local 2% RMS threshold with a 5 ms guard.
Audible harmonics estimate a fixed tuning offset, checked within 75 cents of the label; weak
harmonics are excluded. The renderer receives this measured offset before note-on. No pitch,
time alignment, or per-note synthesis parameter is optimized against the validation recordings.

## Objective and search

Every note is mono at 24 kHz and covers the first **one second** after onset, with the key held.
This common window fits every capture. Each whole note is normalized to RMS 0.1. Individual
frames are never normalized, so attack and decay remain measurable; there is no time warp.

Two Hann STFTs use 512/2048-sample windows and a 240-sample hop. Sixty-four triangular HTK mel
bands span 30 Hz to 10 kHz with unit-area normalization. Power is compressed as
`log(1 + 10000 * mel_power)`. For each resolution, absolute error is averaged separately over
the first 15 frames and remaining frames; their means have equal weight. The final loss averages
both resolutions and all notes equally. Smaller is closer under this definition. It is a
dimensionless acoustic distance, not a perceptual quality score or a measurement in decibels.

`fit_physical_mel.py` alternates two stages twice:

1. Seeded differential evolution searches the actual Rust excitation/loss controls, with
   population size five per parameter, eight generations, and no polishing. Bounds and defaults
   are saved in the report. The seeds are 20261001 and 20261002.
2. A causal filter-bank surrogate fits the twelve gains by regularized least squares, with
   each gain bounded to ±9 dB. Anchor and neighboring-gain penalties are 0.012 and 0.01. A fit
   is retained only if it improves the primary temporal L1 objective. Surrogate PCM agreement
   with the actual Rust bank is checked before optimization, within 0.0002 maximum error.

Piano searches hardness, strike position, decay, damping and stiffness. Guitar searches hardness,
pluck position, decay and damping. Violin also searches bow pressure and speed. Release is held
fixed because this window does not measure key release. Body blend remains 0.65. The fitted
filter gains and factory controls are rounded into Rust; the final measurement uses those actual
compiled values. Existing explicit project parameters still take precedence over new defaults.

A separate scalar preserves median training-note RMS relative to the previous factory voice.
It applies within the instrument, including when level is automated, instead of changing the
public level default. It does not enter the gain-invariant objective. Bass, bell and mallet
settings are unchanged. SoundFont import and its playback path are unaffected.

## First calibration result

Baseline: `5a6ccac`, before this calibration. The final tools repeated the training search with
the same seeds and obtained the same result. Evaluating two actual worker binaries at 24 kHz:

The complete calibration and per-note validation reports are committed in
`tools/eval/references/physical-mel-calibration.json`, `physical-mel-validation.json`,
and `physical-mel-validation-48k.json`.

| Instrument | Training before → after | Validation before → after | Validation reduction | Improved notes |
| --- | --- | --- | ---: | ---: |
| Piano | 0.258680 → 0.161204 | 0.234200 → 0.169513 | 27.6% | 10/12 |
| Guitar | 0.183417 → 0.119994 | 0.211496 → 0.148237 | 29.9% | 42/44 |
| Violin | 0.114397 → 0.077856 | 0.124071 → 0.084689 | 31.7% | 35/39 |

A separate 48 kHz render, resampled into the same 24 kHz feature space, also improves validation:
piano 0.234175 → 0.171541 (26.7%), guitar 0.210189 → 0.148302 (29.4%),
violin 0.126719 → 0.088016 (30.5%). This check reconstructs the earlier defaults and filter bank
through the unchanged string/modal algorithms. Before using that reconstruction, it verifies PCM
agreement with the frozen 24 kHz baseline. The 24 kHz table above uses the frozen binary directly.
Future oscillator changes require a separately frozen worker at the production rate instead.

The evaluation emits a deterministic audition: piano, guitar, violin, each in the order
**recorded / before / after**, with whole-note RMS matched and common peak headroom. The mf
validation note nearest MIDI 60/55/67 is chosen ahead of scoring; no best-performing note is
selected. Plots use the same color scale for each instrument's three panels.
Audition cuts have 5 ms boundary fades to prevent clicks; scoring uses the untouched PCM.

The symbolic measurement is byte-identical before/after. Nine normally balanced composition
presets still give mixed Audiobox Aesthetics results: mean CE 5.938 → 5.887, PQ 7.749 → 7.734.
Pop-band and rock CE improve; orchestral and ambient CE decrease by about 0.36 and 0.43. Ambient
PQ decreases by about 0.17. A smaller isolated-note mel loss therefore does not establish that
every composition or sustained pad sounds better. These learned scores remain secondary checks,
not search targets.

At 48 kHz, 256 frames and 24 held voices, a release-build measurement gives piano mean/p99
1.025/1.681 ms, guitar 0.083/0.123 ms, violin 0.073/0.116 ms, against a 5.333 ms callback budget.
Longer-lived piano modes increase mean CPU from the preceding calibration's roughly 0.82 ms.
Callback allocation checks still pass. These timings describe this machine and workload.

The first pass covers attack and early sustain/decay. The subsequent
[sustained/trajectory calibration](physical-trajectories.md) adds 3/6-second notes and real
violin glissandi with pitch/expression guides and annotated releases. Some piano EQ gains reach
the ±9 dB bounds, indicating that the compact
excitation/radiation structure still limits reconstruction. Mel distance also discards phase
and resolves harmonics only within its bands; listening and pitch/stability tests remain needed.

## Reproduce and continue

Run Python tools with `uv`. On Windows, use the installed `uv.exe` path if it is missing from
PATH. The tools use script environments; no trainer environment or GPU is required.

```sh
uv run tools/eval/fetch_physical_reference.py target/copy-synthesis/iowa
uv run tools/eval/prepare_physical_reference.py target/copy-synthesis/iowa \
  --lock tools/eval/references/iowa-notes.json
cargo build --release -p auris-synth --example physical_fit_render
# Freeze this executable before changing any audio code; use .exe on Windows.
cp target/release/examples/physical_fit_render target/copy-synthesis/before-renderer
uv run tools/eval/fit_physical_mel.py target/copy-synthesis/iowa \
  target/copy-synthesis/before-renderer target/copy-synthesis/fit.json
# Review held-out loss, apply candidate controls/gains and cumulative output normalization,
# then rebuild the example and compare the actual Rust builds:
uv run tools/eval/evaluate_physical_mel.py target/copy-synthesis/iowa \
  target/copy-synthesis/before-renderer target/release/examples/physical_fit_render \
  target/copy-synthesis/evaluation
```

To repeat this first calibration with the archived worker, pass
`--profiles tools/eval/references/physical-body-before-copy.json` to the fitter. The 48 kHz
cross-check additionally uses `--production-rate 48000 --calibration target/copy-synthesis/fit.json`.
Pin the recorded dependency versions for exact extraction: NumPy 2.5.3, SciPy 1.18.1,
SoundFile 0.14.0; the original run used Python 3.12.14. The canonical note-manifest SHA-256 is
`878f66a719dd47400ad6246d9a23c8ecb09676c26837bbfa569993099b76c4bc`.
Manifest identity uses sorted, compact JSON encoded as UTF-8, independent of file line endings.
The reports also record worker hashes, defaults, bounds, stage history and every per-note error.
To rebuild the original DSP baseline, use source at `5a6ccac` in an isolated checkout and copy
the current worker example there, adding `serde_json` as an `auris-synth` dev dependency. Use an
independent Cargo target directory. A rebuilt executable's hash depends on its toolchain and
build path; compare PCM and metrics rather than expecting the original worker binary hash.

```sh
uv run --with numpy --with scipy --with soundfile --with pytest pytest \
  tools/eval/test_physical_mel.py tools/eval/test_fit_physical_body.py tools/eval/test_physical_ab.py
uv run --with ruff ruff check tools/eval/*physical*.py
cargo test --workspace
cargo test -p auris-synth --example physical_fit_render
cargo clippy --workspace --all-targets
```

The numeric tests reject silent/non-finite candidates, verify gain and polarity invariance,
detect changed pitch/harmonics/attack/decay, check extraction/tuning, and protect the fixed
training/validation separation. Standard DSP tests continue to check tuning, loss, velocity,
controls, live level normalization, extreme notes and zero allocations.
