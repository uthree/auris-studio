# Physical instrument measurements

Measured on Windows at 48 kHz on 2026-10-01. The comparison build is commit
`64ca54ee60b4105df336e46b53512b5c7d07334d`; the candidate is the physical instrument
initial implementation at `aaf5a0f`, described in [physical instruments](physical-instruments.md).
The subsequent body, guitar, piano and violin changes are measured separately in the
[refinement account](physical-model-refinement.md).

## Conditions

The symbolic ruler (`cargo run -p auris-compose --example measure`) produced identical
before/after output over its eight seeds per preset. The score-writing algorithms were not changed.

`tools/eval/aesthetics.py --preset all` rendered all nine presets at their own default seeds,
with normal session balancing, 32-bit WAV output and no render tail. Audiobox Aesthetics
scored the full renders. These are single-seed diagnostic results, not listening-test results
or evidence of quality equivalence. CE means predicted content enjoyment; PQ means predicted
production quality. Both use a 1–10 scale. The other reported axes are CU (content usefulness)
and PC (production complexity, for which higher is not inherently better).

Three conditions distinguish changing the supported instrument families from removing assets:

* **Old + GM:** the comparison build with MuseScore General.
* **New + GM:** native models for supported hints, with the same optional GM font for other hints.
* **New native:** the candidate with `AURIS_SOUNDFONTS` pointing to an empty directory.

The font was verified as 215,614,036 bytes with SHA-256
`ee51d2c4b1525e70f19a45909c4fd7a2e26d91d115fa89dbf5a6bc413d8b9bf3`.
An optional installed font supplies unsupported families such as flute, brass and synth pads.
Without one, the session uses its reported synthesizer fallback. These conditions therefore
also include the existing drum kit replacing the font's drum presets and fresh level balancing.

## Learned scores

| Preset | Old + GM CE | New + GM CE | New native CE | Old + GM PQ | New + GM PQ | New native PQ |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| chiptune | 5.84 | 5.84 | 5.84 | 7.88 | 7.88 | 7.88 |
| game-loop | 5.95 | 5.95 | 5.95 | 7.96 | 7.96 | 7.96 |
| pop-band | 6.94 | 6.86 | 6.35 | 8.12 | 8.10 | 8.05 |
| city-pop | 7.66 | 7.68 | 6.75 | 8.21 | 8.28 | 8.02 |
| rock | 7.42 | 7.42 | 7.20 | 8.07 | 8.25 | 8.20 |
| jazz-trio | 7.71 | 7.50 | 7.50 | 8.27 | 8.37 | 8.37 |
| orchestral | 7.14 | 4.75 | 4.85 | 7.73 | 7.06 | 7.32 |
| synthwave | 6.68 | 6.50 | 5.62 | 7.98 | 7.90 | 7.64 |
| ambient | 6.56 | 5.19 | 3.53 | 7.67 | 7.27 | 6.67 |

| Mean | CE | CU | PC | PQ |
| --- | ---: | ---: | ---: | ---: |
| Old + GM | 6.879 | 7.825 | 4.914 | 7.988 |
| New + GM | 6.412 | 7.605 | 4.495 | 7.899 |
| New native | 5.955 | 7.482 | 4.377 | 7.791 |

The largest enjoyment drops occur in orchestral and ambient material. Sustained bowed textures
and arrangements relying on unsupported GM families remain listening priorities. Band presets
retain similar production-quality scores with the optional font present; this does not establish
that listeners prefer the native models. These measurements were recorded without tuning the
models to maximize the learned score.

## Reproduction

Keep separate copies of the old and new CLI executables. Point `AURIS_SOUNDFONTS` to the same
verified font directory for the first two conditions, or to an empty directory for the third:

```sh
uv run tools/eval/aesthetics.py --preset all --cli /path/to/old/auris --workdir before --json before.json
uv run tools/eval/aesthetics.py --preset all --cli /path/to/new/auris --workdir after --baseline before.json --json after.json
```

The numerical DSP and session tests also cover pitch, velocity, modal brightness, decay,
pedal, expression, bends, live damping, allocation-free callbacks, explicit parameter priority,
undo, save/open and identical PCM after reopening. `physical_demo` writes a separate six-model
listening probe. Release archives omit the standard font asset; archive compression ratios and
the final executable-size difference were not measured by this experiment.
