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
