# Physical model refinement

The refinement order is radiation/body coloration, guitar, piano, then violin. Listening probes
and numerical regressions complement the full composition measurements; a spectral fit or a
learned score alone does not establish instrument realism.

## Radiation profile

Piano, guitar and violin use twelve broad parametric filters, with fixed centre frequencies
between 90 Hz and 10 kHz and Q = 0.9. Their gains were independently fitted from eighteen
matched notes per instrument: nine pitches at velocities 0.5 and 0.85, rendered at 48 kHz.
Each note is RMS-normalised before averaging and smoothing the power spectra. The reference
is MuseScore General, patches 0, 25 and 40, available through the optional font library.
The reference file's SHA-256 is
`ee51d2c4b1525e70f19a45909c4fd7a2e26d91d115fa89dbf5a6bc413d8b9bf3`.

These coefficients are a regularised spectral prior. The reference includes excitation,
string vibration and recording coloration, so the fit does not identify an isolated soundboard
impulse response or bridge admittance. It supplies no waveforms to the runtime. Gains are
bounded to ±6 dB per section; the target difference is bounded to ±12 dB, with penalties on
gain magnitude and adjacent gain differences. The body control blends the dry signal with the
cascade output. Sections above 45% of the host rate are bypassed rather than folded down.
Bass, bell and mallet retain their original three parallel resonances.

The nominal fully wet filter reduces the fitted target-envelope error from 3.71 to 1.17 dB
for piano, 7.76 to 1.12 dB for guitar, and 6.18 to 1.73 dB for violin. These are fitting errors,
not measurements of the default dry/wet output or perceptual quality. Excitation refinements
are evaluated separately rather than repeatedly fitting a profile to obtain a favourable score.

Reproduce the fit with the development tools:

```sh
cargo run -p auris-session --example physical_probe -- target/reference /path/to/MuseScore_General.sf2
cargo run -p auris-session --example physical_probe -- target/dry native dry
uv run tools/eval/fit_physical_body.py target/reference target/dry target/body-fit.json
```

The JSON records every input's hash. Numerical tests cover DC gain, spectral shaping, stable
impulse tails at 8–192 kHz, gain-invariant fitting inputs, and the instrument's existing pitch,
extreme-parameter and zero-allocation contracts.

The first radiation-only checkpoint passes the full workspace tests and clippy, plus three
fitter tests and ruff. Symbolic measurement output is byte-identical to the baseline. Across
the nine default-seed native compositions, mean Audiobox CE/PQ changes from 5.96/7.79 to
5.85/7.73; orchestral and ambient drop by 0.36/0.41 CE respectively. Matching the spectral
prior alone therefore does not resolve the reported quality problem. Subsequent excitation
changes are assessed against this checkpoint as well as the original baseline.

## Guitar

A finite contact width smooths the initial triangular displacement. Velocity and pick hardness
control this width; subsequent loop losses depend on decay and damping independently of pick
hardness. A first-order allpass supplies the fractional tuning delay, avoiding the high-frequency
attenuation of linear interpolation in a feedback loop. Its phase is calibrated at the fundamental,
including the loss filter's exact phase. The fractional delay stays within 0.5–1.5 samples.

The requested fundamental attenuation is divided between a scalar gain and a one-pole lowpass.
Both remain passive. The fundamental T60 is `decay / (1 + 3 × damping)`, while damping also sets
the share of loss assigned to the filter, so upper modes die sooner. Pitch bends approach the
new period over about 5 ms; tuning coefficients update every eight frames during a bend.

The acoustic output follows string-motion velocity rather than raw displacement. Pickup blend
instead observes a displacement comb at the excitation position and bypasses the acoustic
radiation profile. It is an editable approximation of pickup geometry, not an amplifier model.
Output scaling matches the radiation-only listening probe's RMS approximately; this avoids a
large default level change from differentiating the string signal.

Tests measure fundamental decay within 1.5 dB of its requested loss over a half-second interval
at four pitches and three rates, independent contact/pickup spectral changes, and bounded bends
to the correct octave. The implementation uses preallocated string storage throughout.

The allpass design follows Smith's
[Extended Karplus–Strong algorithm](https://www.dsprelated.com/freebooks/pasp/Extended_Karplus_Strong_Algorithm.html).

The guitar checkpoint passes synth, session and i18n tests and workspace clippy. The symbolic
output remains identical. Full-preset mean CE/PQ is 5.89/7.74, versus 5.85/7.73 at the radiation
checkpoint; rock CE rises by 0.18 and orchestral by 0.13. The other seven cases remain identical.
These are a fixed single-seed regression cohort, not an estimate of general perceptual quality.
On the development machine, the 24-voice guitar callback averages 0.077 ms for 256 frames at
48 kHz in the optimised development profile; `physical_bench` makes this reproducible.

`uv run tools/eval/physical_ab.py before.wav after.wav comparison.wav` produces a listening
file with piano before/after, guitar before/after, then violin before/after. It matches each
pair's RMS and keeps peaks below 0.9, with gains recorded beside the WAV. Inputs are the
six-model `physical_demo` renders, in their standard order.
