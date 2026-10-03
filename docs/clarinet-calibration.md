# Clarinet model and copy-synthesis evaluation

Physical Clarinet is a sample-free, 16-voice instrument. A fractional half-period
delay with an inverted bore reflection supplies the closed-open tube resonance.
A bounded nonlinear pressure-flow approximation excites it with a reed signal
and breath noise. A slow running mean removes the excitation's DC component.
This is a phenomenological reed/bore model, rather than a reconstruction of a
particular mouthpiece, fingering, player or bell radiation pattern.

Breath Pressure, Reed Stiffness and Breath Noise affect sounding voices. Attack
and Release control the shared ADSR. CC7 controls volume; CC11 smooths breath
drive and output expression over 20 ms. Pitch bends retune sounding bores.
Delay storage is allocated in `prepare`; callback processing and control changes
allocate nothing. Reset reproduces the same seeded excitation.

## Reference and preparation

The [University of Iowa Bb Clarinet MIS page](https://theremin.music.uiowa.edu/MISBbclarinet.html)
identifies a Buffet R13 Bb clarinet, Glenn Bowen, recorded on 3 August 1998
in the Wendell Johnson anechoic chamber with a Neumann KM 84, at 16-bit,
44.1 kHz mono. The [MIS collection terms](https://theremin.music.uiowa.edu/MIS.html)
permit downloading and using these recordings in any project without restrictions.
An online preparation run caches both pages beside the original captures.

The frozen cohort uses `D3B3`, `C4B4` and `C5B5` recordings at `pp`, `mf` and `ff`:
34 sounding pitches, MIDI 50–83, and 102 note instances. The filenames identify
concert pitches: D3, C4 and C5 fundamentals are approximately 147, 262 and 523 Hz.
Do not transpose these labels for the instrument's Bb designation. All three
dynamics of each even pitch belong to training (51 notes); odd pitches belong to
validation (51 notes). This holds out pitches within one instrument, player and
recording setup, rather than an independent instrument or player.

Chronological regions are detected using a 5 ms RMS envelope at 2% of capture
peak, with short gaps merged. Each note starts with 5 ms preroll and ends 250 ms
after its detected region, then is cropped or zero-padded to three seconds and
polyphase-resampled to 24 kHz. Detected region duration supplies an approximate
hold, capped at 2.98 seconds in the renderer. Some notes therefore sustain for
the entire retained window; T90 measures energy timing in that window, rather
than a complete physical release. Velocity maps `pp/mf/ff` to 0.35/0.65/0.95;
these are development choices, not measured MIDI velocities. Measured recording
detuning is passed to the renderer. Source and prepared hashes, times and pitches
are retained in `tools/eval/references/clarinet-notes.json`.
The manifest checksum normalizes CRLF to LF so Git's platform line endings
do not invalidate the retained calibration.

## Calibration and results

Four controls were fitted on training notes using actual 24 kHz Rust PCM:
pressure, reed stiffness, noise and attack. Seeded differential evolution uses
four iterations, population multiplier four and no polishing (80 evaluations).
The objective is phase-weighted log-mel L1 plus `0.006 × envelope dB error +
0.15 × T90 error in seconds`. Search bounds, training IDs, initial controls and
full-precision results are retained in `clarinet-training-fit.json`; rounded
controls become factory defaults. This short bounded search is not a global
optimum. The baseline is the new clarinet before calibration, not a previously
released instrument.

Final evaluation uses the actual rounded factory defaults at native 48 kHz,
polyphase-resampled to 24 kHz. Both signals receive the metric's causal 30 Hz
highpass and whole-note RMS normalization to 0.1. There is no time warping or
framewise gain adjustment. The log-mel configuration is the same as the
[physical pack evaluator](physical-pack.md): 512/2048 Hann windows,
240-sample hop, 64 bands from 30 Hz to 10 kHz, and attack/body/tail weighting
0.3/0.3/0.4. Lower error is better.

| Held-out measure, 51 notes | Initial controls | Calibrated factory | Change |
| --- | ---: | ---: | ---: |
| Phase-weighted log-mel L1 | 0.079209 | 0.057546 | 27.35% lower |
| Attack log-mel L1 | 0.086815 | 0.020081 | 76.87% lower |
| Body log-mel L1 | 0.092661 | 0.087211 | 5.88% lower |
| Tail log-mel L1 | 0.063416 | 0.063396 | 0.03% lower |
| Envelope L1 | 4.4996 dB | 4.3681 dB | 2.92% lower |
| T90 absolute error | 0.28772 s | 0.29116 s | 1.19% higher |

Training mel falls from 0.079556 to 0.058190. A paired bootstrap with 2,000
resamples keeps all dynamics of each pitch together. The held-out mel delta's
descriptive 95% interval is [−0.022670, −0.020565]. Most improvement is in the
attack; tail improvement is inconclusive and energy timing slightly worsens.
These are copy-fidelity measures after model development, not perceptual
quality scores. The 24 kHz replay gives held-out mel 0.077159 → 0.055598.
Both native-rate reports, hashes and per-note errors are committed alongside
the cohort in `tools/eval/references/clarinet-validation*.json`.

An independent steady-state FFT probe covers MIDI 48–93 at three velocities
and both rates. In the reference range 50–83 the maximum absolute tuning errors
are 8.89 cents at 24 kHz and 4.93 cents at 48 kHz. Outside that range the 24 kHz
probe reaches 19.38 cents. Numeric Rust tests also cover odd harmonics, DC,
extreme controls/rates, release retirement, deterministic reset, CC7/CC11 and
event processing across block sizes. A session test saves, opens and renders
the native instrument without a SoundFont.

The 16-voice clarinet callback at 48 kHz stereo, 256 frames measures 0.016 ms
mean and 0.017 ms p99 over 512 blocks on the development machine. Timing,
FFT rows, preset regression hashes and matched-level audition records are in
`tools/eval/references/physical-next-regressions.json`. Native demo exports
have no samples at full scale; listening versions use whole-phrase RMS 0.1.

## Reproducing and auditioning

```sh
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/fetch_clarinet_reference.py target/physical-next/clarinet-reference --lock tools/eval/references/clarinet-notes.json
uv run tools/eval/clarinet_copy.py target/physical-next/clarinet-reference <new-worker> target/physical-next/clarinet-validation.json --baseline <initial-clarinet-worker> --rate 48000 --calibration tools/eval/references/clarinet-training-fit.json
cargo run -p auris-session --example clarinet_demo -- target/physical-next/clarinet-demo
cargo run --release -p auris-synth --example physical_bench -- --clarinet
```

Use `.exe` suffixes on Windows. The archived initial worker is identified by SHA-256
in the report; it predates adoption of the calibrated controls. Without `--baseline`
the evaluator can compare a candidate passed through `--params` or `--fit-bounds`
with the current factory. `--offline` prepares cached captures without a network
request and still verifies the frozen hashes. The demo saves an editable project
and a native WAV in a fresh output directory.
