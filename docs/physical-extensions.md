# Physical-model experiments against real recordings

Six causal radiation resonances, register-dependent contact/loss controls and a nonlinear
violin expression response reduce held-out temporal log-mel error by about 2.7% for guitar
and violin. These are incremental changes to the already calibrated instruments. The baseline
is `bb6b39502d502a38f85e7ab69ae5830ca2f0ef28`. Piano and pedal trials are evaluated below.

## Objective and split

The [short-note](physical-copy-synthesis.md) and [long-note/trajectory](physical-trajectories.md)
cohorts, hashes and splits are unchanged. Iowa supplies 36 original captures with 165 notes;
the long-note manifest supplies 212 excerpts, including 24 TU-Note glissandi.

| Model | Short train / validation | Long Iowa train / validation | TU-Note train / validation |
| --- | ---: | ---: | ---: |
| Piano | 6 / 12 | 12 / 24 | — |
| Guitar | 34 / 44 | 48 / 50 | — |
| Violin | 30 / 39 | 19 / 35 | 12 / 12 |

Actual Rust PCM is analyzed at 24 kHz. A separate production-rate render at 48 kHz is polyphase
downsampled before analysis. Reference and synthesis each receive one whole-note RMS normalization
to 0.1; there is no frame normalization or time warp. Hann windows of 512/2048 samples,
240-sample hop, and 64 area-normalized HTK mel bands from 30 Hz to 10 kHz give
`log(1 + 10000 * mel_power)`. The primary distance is mean absolute difference. Short-note
attack/sustain receive equal weight. Long-note attack/early/late/transition/release receive
0.15/0.15/0.40/0.20/0.10, omitting empty phases. Short/long/TU cohorts receive 0.3/0.5/0.2,
renormalized when absent. Every per-note phase, envelope and pitch diagnostic is retained.

Recorded pitch and expression guides are identical inputs on both sides of the violin comparison.
This is conditional copy synthesis; it does not establish automatic reconstruction of a player's
bow gesture. Whole-note normalization removes recording gain while retaining the played contour.

All coefficients are fitted on training notes, starting from the unchanged model. Fixed coordinate
search schedules are: mode gains ±1.5, steps 0.15/0.06/0.02 of the range, with a 0.001 mean-square
penalty; register slopes ±0.2/±0.8/±0.08, steps 0.25/0.10/0.04; bow exponent [0.5, 2], steps
0.20/0.08/0.03; automatic-swell amount [0, 0.7], steps 0.25/0.10/0.04. Frozen profiles precede
validation. Existing cohorts have been used in earlier model development: they are held out from
this optimizer, rather than an untouched final test set. Validation also informs the shipping decision.

Paired 95% bootstrap intervals use 2,000 resamples, seed 20261003, clustering overlapping excerpts
by original capture. They are descriptive, without multiple-comparison correction. Iowa's many
notes do not represent many independently recorded instruments.

## Instrument changes

Six parallel constant-peak bandpasses at 120/240/480/960/1920/3840 Hz, Q=8, follow the existing
broad coloration. Signed gains add or suppress resonances. Their contribution scales with Body
Resonance, anchored at its 0.65 default; body=0 bypasses both banks. Filters are causal and reset
with the instrument. Frequencies above 0.45 of the host rate are bypassed. These fitted spectral
priors include excitation and microphone coloration; they are not isolated measured bridge
admittances or a reconstructed wooden body.

Register uses `x = clamp((pitch - center) / 24, -1.5, 1.5)`. Hardness/damping get additive slopes;
decay gets a multiplicative `exp(slope * x)`, bounded by existing control ranges. Guitar's center
is 55, slopes (-0.156, -0.624, 0.0624); violin's center is 69, slopes (0, 0.224, 0). Attacks,
live loss changes and legato retargeting use the same curve. Violin CC11 maps to `value^1.18`
before the existing bow/output smoothing, preserving endpoints. Factory levels, broad profiles,
saved controls and written notes remain the same. Storage and coefficients are prepared outside
the callback; processing adds no allocation or locking.

## Held-out results

These are final production Rust PCM results at 48 kHz, not only surrogate predictions.

| Model | Before weighted mel L1 | After | Reduction |
| --- | ---: | ---: | ---: |
| Piano | 0.156354 | 0.156354 | 0% |
| Guitar | 0.132132 | 0.128538 | 2.72% |
| Violin | 0.080924 | 0.078716 | 2.73% |

At 24 kHz guitar improves from 0.132085 to 0.128505 (2.71%), violin from 0.078594 to 0.076219
(3.02%). Violin's short-note cohort worsens by 0.34% at 24 kHz and improves by 0.22% at 48 kHz;
both short-note confidence intervals include zero.

| 48 kHz cohort | Reduction | Improved / total | 95% interval of mean after-minus-before |
| --- | ---: | ---: | ---: |
| Guitar short | 2.30% | 32 / 44 | [-0.004158, -0.002357] |
| Guitar long Iowa | 3.02% | 32 / 50 | [-0.007059, -0.001205] |
| Violin short | 0.22% | 19 / 39 | [-0.001872, 0.001469] |
| Violin long Iowa | 3.30% | 26 / 35 | [-0.005954, -0.001654] |
| Violin TU-Note | 6.87% | 12 / 12 | [-0.003868, -0.002533] |

Envelope error improves for long guitar (9.978 → 9.801 dB) and long Iowa violin
(1.960 → 1.923), but worsens for TU-Note violin (3.022 → 3.534). Spectral improvement does not
guarantee better phrasing or perceptual quality. Substantial differences from real recordings,
including missing harmonic structure, remain.

The duration-conditioned bow trial supplies a generic 50 ms sine-swell guide, with no recorded
expression on either side. Fitted amount 0.175 reduces unweighted validation mel error from
0.095232 to 0.092425 (2.95%), 65/86 excerpts improved; envelope error slightly worsens.
This ablation is separate from the shipped CC11 response.

Piano's combined radiation/register prototype improves aggregate error by 1.20%, but both cohort
intervals include zero, only 5/12 short and 11/24 long excerpts improve, and long envelope error
worsens from 9.517 to 10.158 dB. Production piano stays unchanged. Full ablations are in the
[training](../tools/eval/references/physical-extension-training.json),
[24 kHz validation](../tools/eval/references/physical-extension-validation-24k.json) and
[48 kHz validation](../tools/eval/references/physical-extension-validation-48k.json) reports.

## Piano pedal trials with synchronized real MIDI

[Saarland Music Data v2](https://www.audiolabs-erlangen.de/resources/MIR/SMD/midi) supplies real
piano audio and corrected synchronized MIDI including CC64. Three original pairs are selected
from the [v2 archive](https://zenodo.org/records/13753319) before rendering: Chopin Op.28/4 trains;
Chopin Op.28/15 and Beethoven Op.27/1 movement 3 validate. Each excerpt is the first 12 seconds,
using original MIDI timing and pedal values, without alignment search. The
[cohort lock](../tools/eval/references/smd-pedal-captures.json) records hashes and attribution;
original audio remains local research input.

The development example adds 88 damped resonators driven by piano output. Pedal position changes
their passive decay radius with 15 ms smoothing. This feed-forward approximation does not solve
bidirectional bridge coupling. The half-pedal trial changes existing release to
`0.03 + 0.7 * pedal^2`, retaining binary key-holding logic: a release-curve experiment rather
than a full damper-contact model. The primary distance uses unweighted whole-excerpt log-mel L1.

Training chooses sympathetic gain from 0/1/2/4/8/16; 16, the upper bound, wins. Validation
mel error improves by 0.88% on Chopin and 0.55% on Beethoven; envelope error improves on neither.
Half-pedal is almost neutral on Chopin but worsens Beethoven by 0.81%, with envelope error
4.181 → 5.929 dB. Two pieces and an upper-bound optimum provide insufficient evidence to adopt
this approximation. The [report](../tools/eval/references/physical-pedal-validation.json) preserves
all variants. A larger independent MIDI/audio cohort and a more complete damper/bridge model
would allow a stronger test.

## Reproduction and listening

Prepare locked references as in [physical-trajectories.md](physical-trajectories.md). Build the
baseline `physical_fit_render` in a separate checkout at the commit above; preserve its executable.
The commands below assume this baseline is `target/physical-experiments/before-renderer.exe`.
Omit `.exe` outside Windows. Executable hashes identify the binaries used; different compiler
builds may produce different hashes. With a rebuilt baseline, fit a new training file and use it
for validation, then compare coefficients and losses with the archived results.

```powershell
cargo build --release -p auris-synth --example physical_fit_render --example physical_pedal_render
uv run tools/eval/fit_physical_extensions.py target/physical-experiments/references `
  target/physical-experiments/iowa target/physical-experiments/before-renderer.exe `
  target/physical-experiments/new-training.json --fit
uv run tools/eval/fit_physical_extensions.py target/physical-experiments/references `
  target/physical-experiments/iowa target/physical-experiments/before-renderer.exe `
  target/physical-experiments/new-validation.json `
  --profiles target/physical-experiments/new-training.json --rate 48000 `
  --candidate target/release/examples/physical_fit_render.exe `
  --audition target/physical-experiments/audition
uv run tools/eval/fetch_piano_pedal_reference.py target/physical-experiments/smd `
  --lock tools/eval/references/smd-pedal-captures.json
uv run tools/eval/evaluate_piano_pedal.py target/physical-experiments/smd `
  target/release/examples/physical_pedal_render.exe target/physical-experiments/new-pedal.json
```

Evaluation rejects changed training renderer/reference hashes and refuses to overwrite results.
The experimental surrogate also requires the preserved worker before these extension curves;
the final worker is used only as `--candidate`, preventing accidental double application.
Audition selects mf three-second validation notes by metadata, near piano C4/guitar G3/violin G4.
Each WAV plays **recording, before, after**, separated by 250 ms. Whole-note RMS matching and
common peak protection keep levels comparable. Five-ms fades apply only to listening copies.
PNGs use a common log-mel color scale. These illustrative cases are not chosen by improvement.

```powershell
$env:PHYSICAL_RENDERER = Join-Path $PWD 'target/release/examples/physical_fit_render.exe'
uv run --with numpy==2.5.3 --with scipy==1.18.1 --with soundfile==0.14.0 --with pytest `
  pytest tools/eval/test_physical_extensions.py tools/eval/test_physical_mel.py `
  tools/eval/test_physical_trajectory.py tools/eval/test_fit_physical_body.py tools/eval/test_physical_ab.py
```

Integration tests compare actual PCM with the causal surrogate at both rates, including moving
expression, release and multiple registers; maximum sample error must stay below 0.0002.
Final production mel losses agree with extension predictions within 4e-9. Rust tests cover
callback allocations, sustained stability, pitch, decay, legato and body reset. The
[system measurements](../tools/eval/references/physical-extension-system-evaluation.json) record
the same preset scores, symbolic baseline and callback benchmark before and after.

The symbolic measurements match exactly after normalizing CRLF/LF. Audiobox's nine-preset
means change CE 6.395 → 6.413, CU 7.518 → 7.525, PC 4.599 → 4.597, PQ 7.786 → 7.792.
These single-seed song diagnostics are nearly flat; the main measured gain is reference
spectral similarity. With 24 voices at 48 kHz and 256-frame blocks, observed mean callback
time changes guitar 0.081 → 0.084 ms and violin 0.074 → 0.077 ms. These are single runs
under uncontrolled development-host load, rather than a statistically controlled CPU benchmark.
