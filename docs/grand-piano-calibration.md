# Grand-piano calibration

The native Physical Piano factory voice is calibrated against the University of Iowa's
[Steinway model B recordings](https://theremin.music.uiowa.edu/MISpiano.html), performed
by Evan Mazunik. The 2026-10-05 revision adjusts felt hardness, strike position, string
stiffness, loss and the twelve-band soundboard coloration. The oscillator remains the
finite-contact, coupled-unison model; these changes add no callback allocation or I/O.

## Reference and selection

Baseline: `7e1bf881f4c188feceffb55384dfbb1015e2d537`. The original AIFF hashes, onset offsets,
fixed tuning offsets and splits come from `tools/eval/references/iowa-captures.json` and
`iowa-notes.json`. Each of 18 original notes supplies unwarped 1-, 3- and 6-second cuts.
Pitches are MIDI 48, 52, 55, 60, 64 and 67 at pp/mf/ff (ordinal velocities 0.35/0.65/0.95).

Only the six mf/ff notes at pitches 48/55/64 enter optimization, using the **3-second**
window. All twelve other notes are held out, including every pp note. The 1-/6-second
windows are duration checks, not search objectives. The bounded two-stage search and
regularized coloration fit reuse `fit_physical_mel.py`: eight differential-evolution
generations per stage, population five per parameter, seeds 20261005/20261006.

The objective is the fixed [temporal mel distance](physical-copy-synthesis.md): whole-note
RMS normalization, two STFT resolutions, equal-weight attack and sustain L1. It measures
spectral and temporal resemblance, not perceived piano quality. The Python coloration
surrogate agrees with the baseline Rust worker within 0.0000019 maximum sample error.

## Compiled results

The final measurement renders both archived baseline and compiled factory controls. At
48 kHz, after resampling to the fixed 24 kHz measurement space:

| Held-out duration | Before | After | Reduction | Improved notes |
| --- | ---: | ---: | ---: | ---: |
| 1 second | 0.171541 | 0.162723 | 5.1% | 12/12 |
| 3 seconds | 0.218340 | 0.205345 | 6.0% | 12/12 |
| 6 seconds | 0.259861 | 0.243660 | 6.2% | 12/12 |

At 24 kHz, the corresponding reductions are 4.7%, 5.5% and 5.7%, also 12/12 in each
window. Training error improves at both rates and all three durations. Full per-note
values, executable hashes, factory parameters, package versions and reference hashes are
in [grand-piano-validation.json](../tools/eval/references/grand-piano-validation.json).
The [training report](../tools/eval/references/grand-piano-calibration.json) preserves
search bounds, initial coloration gains, both stages and the selected controls.

The output multiplier preserves the median three-second training-note RMS against the
baseline. It changes from 0.164845 to 0.186700, while the public level default stays -12 dB.
Explicit saved plugin parameters retain precedence over new factory defaults. The
soundboard coloration is part of the evolving instrument implementation.

These checks cover C3–G4 on one recorded grand piano, held notes and three dynamic labels.
They do not establish fidelity across the entire keyboard, other grands or pedal gestures.
The deterministic audition uses the preselected mf C4 validation note, in the order
**recorded / before / after**, with whole-note RMS matching, common peak headroom and
5 ms boundary fades. Scoring uses the unfaded PCM.

## Reproduction

Build and preserve `physical_fit_render` at the baseline commit first. The commands below
assume its executable is `target/piano-grand/before-renderer.exe`; omit `.exe` on macOS.
To repeat fitting against that baseline from the updated source tree, pass the archived
initial coloration profile explicitly.

```powershell
uv run tools/eval/grand_piano.py target/piano-grand/references prepare
uv run tools/eval/grand_piano.py target/piano-grand/references fit `
  target/piano-grand/before-renderer.exe target/piano-grand/new-fit.json `
  --initial-profile tools/eval/references/grand-piano-calibration.json
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/grand_piano.py target/piano-grand/references evaluate `
  target/piano-grand/before-renderer.exe target/release/examples/physical_fit_render.exe `
  target/piano-grand/new-validation.json
uv run --with pytest --with numpy --with scipy --with soundfile `
  pytest tools/eval/test_grand_piano.py tools/eval/test_physical_mel.py
uv run --with ruff ruff check tools/eval/grand_piano.py tools/eval/test_grand_piano.py
```

Evaluation writes the audition beside its JSON report with a `.wav` extension. Downloaded
originals and generated audio stay under `target/`; no recordings or Python tools ship
with the application.
