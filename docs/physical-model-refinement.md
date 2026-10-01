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

The initial nominal fully wet filter reduced the fitted target-envelope error from 3.71 to 1.17 dB
for piano, 7.76 to 1.12 dB for guitar, and 6.18 to 1.73 dB for violin. These are fitting errors,
not measurements of the default dry/wet output or perceptual quality. Excitation refinements
are evaluated separately before a final calibration against the same fixed reference cohort.

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

## Piano

The hammer supplies a finite raised-cosine force pulse whose duration depends on velocity,
contact hardness and register. Its unit-integral drive replaces the former initial modal
amplitudes; contact duration filters upper partials through the excitation itself. The model
is a reduced contact approximation with an explicit force pulse, rather than a solved felt
collision. The mode frequencies retain editable stiffness, normalised at the fundamental.

The string group has one member below 65 Hz, two below 180 Hz, and three above. The latter
groups are detuned by ±0.9 and ±1.8 cents respectively. Each partial couples the unison
members through a convex mixing of their complex modal states. It conserves the common
component and damps differential motion, allowing evolving beats and decay without energy
growth. Up to 64 partials are kept below 45% of the host rate. Each voice's storage is allocated
in prepare. Modal strengths below 1e-12 retire before entering denormal arithmetic.

Tests check register-dependent string count, shorter hard-strike contact, the additional bass
partials, detuned rotations, energy contraction after hammer contact, and retirement of
inaudible modes, alongside the shared pitch, pedal, velocity, stiffness and allocation checks.
The initial 24-voice implementation consumed 2.254 ms per 256-frame callback in the development
profile. Moving divisions out of the modal loop and retiring inaudible modes reduced its measured
mean to 0.831 ms, p99 1.704 ms. The extended string group carries a higher cost than the former
single-string model; this measurement describes one machine and profile, not a latency guarantee.

The design follows the contact, unison and coupling principles described in Smith's
[Piano synthesis](https://www.dsprelated.com/freebooks/pasp/Piano_Synthesis.html).

The piano checkpoint passes synth and session tests and workspace clippy, with identical
symbolic output. Its nine-preset mean CE/PQ is 5.82/7.71. Relative to the guitar checkpoint,
CE drops by 0.17 in pop-band, 0.07 in city-pop and 0.33 in jazz-trio; the other six cases are
unchanged. A richer excitation model is therefore not being presented as a learned-score win.
The fixed isolated-note demo's piano RMS changes only from 0.03651 to 0.03675; the A/B utility
still level-matches it. The release-profile 24-voice benchmark averages 0.827 ms, p99 1.783 ms.

## Violin

The bow junction now distinguishes static and sliding friction with hysteresis. Bow pressure
sets the static force limit; sliding friction is weaker and falls with slip velocity. Friction
is bounded by the relative velocity, so it cannot reverse the direction of relative slip or
inject energy independently of the moving bow. The force law is a bounded reduced approximation,
not a full contact solver. Contact hardness adjusts sliding friction separately from string loss.

Bow speed is a separate parameter, scaled by velocity and CC11, with a 12 ms motion ramp.
The bow junction position moves over 15 ms without resetting the wave buffers; loss depends on
damping at a sample-rate-scaled cutoff. Pitch transitions approach their target over 5 ms.

Optional legato uses a fixed 128-key table with repeated-key counts and last-note priority.
Overlapping notes retarget one voice without resetting string waves or the amplitude envelope.
An off for the former key cannot stop the new note; releasing the newest key returns to a
previous held key. All-notes/sound-off clears the table. Changing the legato toggle releases
held notes so a polyphonic/monophonic transition cannot leave untracked held voices.
The general voice allocator's retarget operation only changes bookkeeping and retains the
voice's measured level. Default violin remains polyphonic; the session starts melody roles
with legato and pads with slower bow motion and polyphony, preserving explicit controls.

Tests assert friction passivity/hysteresis, exact waveform continuity for same-pitch legato,
correct return pitch and stale-off handling, polyphonic defaults, independent motion/position
controls, bow stopping, and zero allocations under legato and live bow automation.

The junction follows the travelling-wave geometry in Smith's
[bow/string scattering junction](https://www.dsprelated.com/freebooks/pasp/Bow_String_Scattering_Junction.html).

The violin checkpoint passes 4,009 workspace tests, workspace clippy, six Python measurement
tests and ruff. Symbolic output remains byte-identical. Mean CE/PQ is 5.94/7.74; relative to the
piano checkpoint, ambient rises by 0.82 CE / 0.38 PQ, orchestral by 0.16 CE with a 0.07 PQ drop,
and pop-band by 0.03 CE. The 24-voice violin callback averages 0.073 ms at 48 kHz/256 frames.

`physical_phrases` saves fixed editable projects containing piano chords and a line, guitar
picking and strumming, and an overlapping violin phrase with legato enabled. Render each
project with frozen before/after CLI executables and use `physical_ab.py --phrases` on the two
WAV directories. The projects and note events are identical across renderers; all saved controls
are held fixed. An older instrument ignores the new bow/legato controls. The isolated-note
`physical_demo` comparison separately captures the change in factory defaults.

## Final radiation calibration

The excitation changes alter the dry spectrum, especially the guitar's motion-velocity output.
Re-running the identical matched-note calibration after `760e969` shows that the initial
piano/guitar filters no longer match the fixed reference as closely. Only these two profiles
are updated; the violin profile already has the same 1.59 dB fitting error as a fresh fit.

| Model | Dry target difference | Initial filter on final excitation | Recalibrated filter |
| --- | ---: | ---: | ---: |
| Piano | 3.75 dB | 2.35 dB | 0.98 dB |
| Guitar | 4.92 dB | 4.73 dB | 1.62 dB |
| Violin | 6.11 dB | 1.59 dB | retained at 1.59 dB |

These are regularised, nominal fully wet spectral-fitting errors. They do not measure the
default mixed output or instrument realism. All pitches, velocities, font, normalisation,
filter bank, gain bounds and penalties are unchanged. This recalibration uses reference spectra,
independently of the Audiobox scores. Pass `--prior initial-fit.json` to the fitter to measure
a prior filter against the new probes. The runtime retains twelve fixed gains per instrument.

## Final comparison

The original native baseline is `aaf5a0f`. The final renderer includes all four model refinements
and the piano/guitar radiation recalibration. Both conditions use an empty `AURIS_SOUNDFONTS`
directory and normal session balancing. All nine presets use their default seeds, 48 kHz,
32-bit WAV and no render tail. No score writer or existing clip was changed.

| Preset | Before CE | After CE | Before PQ | After PQ |
| --- | ---: | ---: | ---: | ---: |
| chiptune | 5.84 | 5.84 | 7.88 | 7.88 |
| game-loop | 5.95 | 5.95 | 7.96 | 7.96 |
| pop-band | 6.35 | 6.08 | 8.05 | 7.94 |
| city-pop | 6.75 | 6.67 | 8.02 | 8.01 |
| rock | 7.20 | 7.40 | 8.20 | 8.29 |
| jazz-trio | 7.50 | 7.19 | 8.37 | 8.19 |
| orchestral | 4.85 | 4.73 | 7.32 | 6.99 |
| synthwave | 5.62 | 5.62 | 7.64 | 7.64 |
| ambient | 3.53 | 3.94 | 6.67 | 6.82 |
| Mean | 5.96 | 5.94 | 7.79 | 7.75 |

Mean CU changes from 7.482 to 7.490 and PC from 4.377 to 4.506. PC describes predicted scene
complexity, for which higher is not inherently better. Results are mixed: rock and ambient
improve, while piano-led arrangements and orchestral production quality remain regression
cases for listening. These model changes add performance control and more physical structure;
the single-seed learned evaluation does not establish an overall improvement in realism.

Final validation passes 4,009 workspace tests, workspace clippy, and documentation with warnings
denied. Six Python measurement tests and ruff pass. Symbolic output is unchanged, SHA-256
`68a1be1d19d62d88d822022a67f7dd549e3875dd8f5cf6eb66800f18d6006496`.
The frozen native before/after CLI hashes are respectively
`f062b030d387bd131084f2302b9f24f4510a2238fa9fd09e2c5f2c7e1680764b` and
`d81bc46a28685bcbb9ca13bd53c08c1be844bcf39c1d754b9e897cfad081b643`.

Listening artifacts are development output, not release assets:

* `target/refinement-ab.wav`: default isolated notes, piano A/B, guitar A/B, violin A/B.
* `target/refinement-phrases-ab.wav`: identical saved chords/phrases through both renderers,
  in the same instrument and A/B order, about 38 seconds total.
* `target/refinement-phrase-projects/`: the three editable source projects.

Each pair is RMS-matched with a shared peak constraint; gain manifests sit beside the WAVs.
The six-second phrases hold saved controls fixed, including body amount 0.65 and violin legato.
The before renderer ignores the new speed/legato controls. Listen to attacks, decaying partials,
string beats, bow starts/stops and transitions independently of overall level. No human listening
rating is inferred from the numeric tests or fitting errors.
