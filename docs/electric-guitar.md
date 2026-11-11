# Electric guitar and amplifier

[日本語](electric-guitar.ja.md)

Choose **Electric Guitar** (`auris.physical.electric_guitar`) in the instrument library,
then add **Guitar Amp** (`auris.fx.guitar_amp`) to its track. Both are built in, work without
samples, and save through ordinary instrument/effect state. The instrument is the guitar as
its pickup hears it, before any amplifier, and is described with the other
[physical instruments](physical-instruments.md). The amp is an explicit effect, and this page
is its account. The composer's guitar performance stages recognize the instrument ID.

## Preset processing check, 2026-10-05

The baseline is `7e1bf88`, after presets adopted the native DI but before they supplied
amplifiers. Frozen before/after CLI binaries rendered all nine declared preset seeds with
ordinary automatic balance, 48 kHz stereo float32 exports and no render tail. A separate
seed 501 checks the four affected presets. The eight-seed symbolic ruler is identical;
all thirteen saved scores retain their notes, performance transforms, harmony, sections
and tempo. All samples are finite and below full scale. The five unaffected presets have
identical WAV hashes and learned scores.

Whole-song RMS changes by less than 0.10 dB in every affected render. Rock's mono spectral
power fraction at or above 2 kHz increases by 0.0271 at the declared seed and 0.0214 at
seed 501. This is a signal measurement of changed coloration, not a listening judgment.
Ten-second excerpts from the first chorus are retained in
`target/preset-effects/audition`, with a common before/after RMS target of -20 dBFS,
reduced if necessary to keep both peaks below 0.8.

The fixed candidate was also measured with the pinned TuneJury/MuQ-MuLan evaluator and
unchanged prompts, using three ten-second windows without gain normalization. These
declared-seed deltas are `after - before`:

| Preset | TuneJury reward | MuQ positive cosine | MuQ contrast margin |
| --- | ---: | ---: | ---: |
| pop-band | -0.005291 | -0.005002 | -0.002724 |
| city-pop | -0.000226 | -0.000302 | -0.000478 |
| rock | -0.527206 | -0.002160 | +0.004092 |
| synthwave | -0.018346 | +0.001063 | +0.002663 |

Rock's preference reward falls materially, while its contrast margin rises slightly.
The driven amplifiers address the requested guitar tone; these measurements do not
establish improved listener preference. No learned-score search or listener study was
performed. Seed 501 checks audio safety and score invariance, not learned-score
generalization. Full per-window scores, checkpoint/package provenance, source and audio
hashes, signal measurements and excerpt positions are in
[`preset-effects-2026-10-05.json`](../tools/eval/references/preset-effects-2026-10-05.json).

To repeat the comparison, freeze baseline and candidate CLI binaries, then run the
[symbolic ruler and learned evaluator](evaluation.md) on each build, passing
`--preset all --seeds 1 --cli <frozen-cli>` to the latter. For seed 501, compose each
affected preset with `--seed 501`, then render its saved project with
`--bit-depth 32 --no-tail`. Existing projects retain their saved effects; compose a
new piece from the updated preset to receive these initial chains.

## The amplifier

Guitar Amp has Bass/Middle/Treble controls before a single symmetric soft-clipping stage.
It uses
8× oversampling with 257-tap Blackman-windowed sinc interpolation/decimation and an
oversampled DC blocker. The fixed FIR latency is 32 host frames and is reported to the
engine. Drive and Output smooth over 20 ms. The analytic open/closed cabinet follows
decimation and can be bypassed; it uses resonant EQ and two lowpass sections, not a
measured speaker impulse response. All channel storage is allocated in `prepare`.

Drive defaults to 12 dB, tone controls to 0 dB, cabinet to Open and Output to −6 dB.
Input level matters: increasing Drive changes both harmonic content and compression.
The demo uses Drive 0/18/34 dB for clean/crunch/lead with the same MIDI phrase.

## Recording reference and protocol

The reference is [EGFxSet version 1.0](https://zenodo.org/records/7044411), by Hegel Pedroza,
Gerardo Meza and Iran R. Roman, licensed CC BY 4.0. Cite Pedroza, Meza and Roman,
“EGFxSet: Electric guitar tones processed through real effects of distortion, modulation,
delay and reverb,” ISMIR Late Breaking Demo, 2022. The
[dataset account](https://egfxset.github.io/) describes clean Stratocaster recordings
replayed through real effects and normalization of both clean and processed audio.
The selected pedal recordings use Boss BD-2 Blues Driver, Level/Tone/Gain = 0.5/0.5/1.0.
This pair measures pedal coloration; it supplies no recorded speaker cabinet target.

One canonical fretting is selected for each of MIDI 40–86, using the highest playable
string. Neck and Bridge pickups supply 94 clean notes and their 94 matched pedal notes.
Even pitches are training (48 notes per stage); odd pitches are validation (46 per stage).
Both pickups, clean/wet pairs and all occurrences of one pitch remain in the same split.
This holds out pitches within one recording setup, not guitar, performer, pickup type or
velocity. Synthesized velocity is fixed at 0.8 because the recordings do not provide a
calibrated MIDI-velocity mapping.

The fetcher reads bounded ZIP byte ranges, verifies each selected member's CRC, and freezes
original/prepared SHA-256 hashes. Published whole-archive MD5 values are metadata only;
the entire archives are not downloaded or verified. Audio stays ignored under `target/`.
Prepared float WAVs use deterministic headers without libsndfile's timestamped PEAK chunk.
During development, all 188 decoded waveforms were checked bit-identical across that header
migration. Each recording is mono 48 kHz. Onset is its first sample reaching 2% of peak, with a 1 ms
preroll. Three seconds are retained and zero-padded if needed; preparation downsamples
to 24 kHz with a polyphase filter. Both signals receive the same causal 30 Hz highpass
and whole-note RMS normalization to 0.1. There is no frame normalization or time warping.

The primary measure is phase-weighted log-mel L1: Hann windows 512/2048, 240-sample hop,
64 area-normalized HTK bands over 30 Hz–10 kHz, and `log1p(10000 × mel power)`.
Attack 0–50 ms, body 50–250 ms and tail 250 ms–3 s receive weights 0.3/0.3/0.4.
Secondary measures are 20 ms RMS-envelope L1 in dB and absolute 90%-energy-time error
in seconds. The latter refers to the retained three-second window.

The first amp comparison used a two-stage candidate and is retained as a historical
diagnostic. The refined amp uses one `tanh` stage and the same 0–48 dB Drive range as the
existing one-stage distortion baseline. Drive is fitted on all 48 training notes only;
the final comparison renders all 48 training notes and 46 held-out validation notes in
8-note chunks through the native 48 kHz worker, then resamples to 24 kHz. Both candidates
receive the same RMS-normalized recorded clean DI with cabinet bypass. The full report and
fit are `tools/eval/references/amp-refinement.json` and `amp-refinement-fit.json`;
the chunked evaluator is `tools/eval/amp_refinement.py`.

## Measured results

Lower error is better. Validation includes 23 held-out pitches and both pickups.

| Comparison | Before | After | Change |
| --- | ---: | ---: | ---: |
| Blues Driver log-mel L1, validation | 0.067339 | 0.066224 | 1.66% lower |

The pedal baseline is the separately optimized existing distortion, with the same Drive
bounds and training-only fitting.

Paired bootstrap uses 2,000 resamples of pitches, keeping both pickups together. The refined
amp's validation mel falls from 0.067339 to 0.066224, with a paired delta interval
[−0.001333, −0.000912]; its training mel falls from 0.0707509 to
0.0693664. Cabinet frequency-response and anti-aliasing tests establish DSP
behavior, not fidelity to a recorded amplifier/cabinet. These intervals are descriptive
after model development rather than an untouched final test or a perceptual-quality score.

Per-note errors, fitted parameters, reference/renderer hashes and metric settings
are in `tools/eval/references/amp-refinement.json`. The frozen cohort is
`electric-notes.json` beside it. Numeric Rust tests cover extreme
controls/rates, measured FIR latency, alias suppression, cabinet response, allocation-free
callback/control changes, reset and block-size independence. A session test checks composition,
saved instrument/effect controls, reopening and identical rendered PCM.

## Reproducing and auditioning

For the amp comparison, preserve a release `physical_fit_render` worker
from `f022ceb` as `<before-worker>` in a separate checkout. Use `.exe` suffixes
on Windows. To replay the full-cohort amp comparison:

```sh
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/fetch_electric_reference.py target/electric-guitar/reference
uv run tools/eval/amp_refinement.py --renderer <new-worker> --baseline <before-worker> --fit tools/eval/references/amp-refinement-fit.json --out target/physical-next/amp-validation.json
```

To repeat the amp's training search instead of replaying retained controls:

```sh
uv run tools/eval/amp_refinement.py --renderer <new-worker> --baseline <before-worker> --out target/physical-next/amp-validation.json
```

Create three editable, sample-free projects and their native WAV exports:

```sh
cargo run --release -p auris-session --example electric_demo -- target/electric-guitar/demo
```

The example saves one project per folder, preserving the project's asset rules. Listen
at matched whole-phrase level when comparing the three drive settings; no framewise
normalization or limiter is needed. The native exports retain the actual plugin levels.
