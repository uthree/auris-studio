# Electric guitar and amplifier

Choose **Electric Guitar** (`auris.physical.electric_guitar`) in the instrument library,
then add **Guitar Amp** (`auris.fx.guitar_amp`) to its track. Both are built in, work without
samples, and save through ordinary instrument/effect state. GM hints 26–31 select the DI;
the amp is an explicit effect. The composer's existing guitar performance stages also
recognize the new instrument ID.

## The signal path

Electric Guitar shares the acoustic guitar's finite-width pluck, passive loss and
allpass-tuned string loop. Its independent pickup observation bypasses acoustic-body
radiation. A position-dependent comb and differentiated motion approximate the induced
voltage; a resonant lowpass approximates pickup/cable coloration. This is a reduced model,
not a reconstruction of a specific guitar's magnet, winding or electrical circuit.

Pick and pickup distances refer to open-string length. Standard tuning E2/A2/D3/G3/B3/E4
and the highest playable string determine the fret and shorten the effective string.
MIDI has no string/fret identity here, so alternate frettings are not represented.
Pickup position changes smoothly over 5 ms and does not alter the vibrating loop or pitch.
Pickup resonance and Q can change during a note. Pick contact changes at the next attack.
Decay is nominal at E4; **Decay per Octave** multiplies it above E4 and divides below it.
The result is bounded to 0.1–40 seconds. Pitches outside the calibrated E2–D6 register
remain playable with bounded geometry and low-note voltage scaling.

| DI control | Factory value | Meaning |
| --- | ---: | --- |
| Contact Hardness | 0.186606 | Width of the initial pluck contact |
| Excitation Position | 0.069559 | Pick distance as a fraction of open-string length |
| Resonance Decay | 7.493398 s | Nominal fundamental T60 at E4, before damping |
| Damping | 0.021633 | Additional fundamental/upper-partial loss |
| Pickup Position | 0.235997 | Neck-like observation; 0.064146 supplies the fitted bridge setting |
| Pickup Resonance | 2717.688 Hz | Electrical-response lowpass frequency |
| Pickup Q | 1.883881 | Resonance sharpness |
| Decay per Octave | 0.55 | Register-dependent decay ratio |

Guitar Amp has Bass/Middle/Treble controls before two biased saturating stages. It uses
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
migration. The earlier diagnostic report retains the pre-migration manifest hash.
Each recording is mono 48 kHz. Onset is its first sample reaching 2% of peak, with a 1 ms
preroll. Three seconds are retained and zero-padded if needed; preparation downsamples
to 24 kHz with a polyphase filter. Both signals receive the same causal 30 Hz highpass
and whole-note RMS normalization to 0.1. There is no frame normalization or time warping.

The primary measure is phase-weighted log-mel L1: Hann windows 512/2048, 240-sample hop,
64 area-normalized HTK bands over 30 Hz–10 kHz, and `log1p(10000 × mel power)`.
Attack 0–50 ms, body 50–250 ms and tail 250 ms–3 s receive weights 0.3/0.3/0.4.
Secondary measures are 20 ms RMS-envelope L1 in dB and absolute 90%-energy-time error
in seconds. The latter refers to the retained three-second window.

DI fitting uses actual 24 kHz Rust PCM, nine bounded controls, seeded differential
evolution, population multiplier 4, eight iterations and no polishing (324 evaluations).
An earlier eight-control training fit initializes the search. The final objective adds
`0.006 × envelope dB error + 0.15 × T90 seconds error` to mel L1. Training diagnostics
showed the earlier constant-decay candidate sustaining high notes too long, motivating
the octave decay control. The final search retained its supplied initial point; this is
bounded calibration rather than proof of a global optimum. Parameter bounds, seed and
training IDs are retained in the fit JSON. Rounded fitted controls become factory defaults.

The amp comparison fits Drive/Bass/Middle/Treble with a causal Python surrogate, six
iterations and 112 evaluations. The existing one-stage `tanh` distortion receives its
own training-only Drive optimization over 0–48 dB. Both receive the same RMS-normalized
recorded clean DI, with cabinet bypass, and both are finally measured using actual
48 kHz Rust output resampled to 24 kHz. The amp's surrogate/native relative RMS difference
is about 0.36%; the reported scores use native PCM. Fitted pedal settings stay evaluation
candidates and do not replace the generic amp's factory settings.

## Measured results

Lower error is better. Validation includes 23 held-out pitches and both pickups.

| Comparison | Before | After | Change |
| --- | ---: | ---: | ---: |
| DI log-mel L1 | 0.284290 | 0.123607 | 56.52% lower |
| DI envelope L1 | 4.8095 dB | 2.2680 dB | 52.84% lower |
| DI T90 absolute error | 0.2901 s | 0.1875 s | 35.35% lower mean |
| Blues Driver log-mel L1 | 0.067339 | 0.079678 | 18.32% higher |

The DI baseline is the pre-change, already acoustic-calibrated Guitar with Pickup blend=1,
not a newly fitted electric model. The pedal baseline is the separately optimized existing
distortion, avoiding comparison against an arbitrary Drive default. DI training mel falls
from 0.282219 to 0.128512. The earlier candidate without octave decay reduced validation
mel by 49.70% but worsened envelope and T90; those diagnostic artifacts remain available.

Paired bootstrap uses 2,000 resamples of pitches, keeping both pickups together. Validation
DI mean mel delta has a descriptive 95% interval [−0.184786, −0.131593], and envelope delta
[−3.0386, −1.9211] dB. T90 delta's interval [−0.2082, +0.0128] s crosses zero, so its
lower mean does not establish an equally clear improvement. The pedal mel delta interval
[+0.008795, +0.016097] favors the tuned simple distortion: the new amp is not a closer
Blues Driver emulator. Cabinet frequency-response and anti-aliasing tests establish DSP
behavior, not fidelity to a recorded amplifier/cabinet. These intervals are descriptive
after model development rather than an untouched final test or a perceptual-quality score.

Per-note errors, factory/fitted parameters, reference/renderer hashes and metric settings
are in `tools/eval/references/electric-validation.json`. The frozen cohort and fit artifacts
are alongside it. Numeric Rust tests cover tuning, independent pickup coloration, extreme
controls/rates, measured FIR latency, alias suppression, cabinet response, allocation-free
callback/control changes, reset and block-size independence. A session test checks composition,
saved instrument/effect controls, reopening and identical rendered PCM.
On the development machine, a 24-voice electric/amp callback at 48 kHz stereo, 256 frames,
uses 0.365 ms mean and 0.523 ms p99 of a 5.333 ms budget over 512 blocks. This is a local
timing measurement, not a guarantee for every CPU.
The symbolic ruler's rows and all nine preset WAV hashes/Audiobox scores are unchanged.
Those presets do not exercise the new plugins, so this establishes regression behavior
only. `tools/eval/references/electric-regressions.json` retains the paired probes, timing
and equal-RMS audition provenance.

## Reproducing and auditioning

Preserve a release `physical_fit_render` worker from commit `ba18d5e` as the baseline.
The old worker builds in a separate checkout; keep it before building the new executable.
Use `.exe` suffixes on Windows. To replay the frozen measurements:

```sh
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/fetch_electric_reference.py target/electric-guitar/reference
```

Copy `electric-di-fit.json`, `electric-amp-fit.json` and `electric-simple-fit.json` from
`tools/eval/references/` to `target/electric-guitar/` as `di-fit.json`, `amp-fit.json` and
`simple-fit.json`, then run:

```sh
uv run tools/eval/electric_copy.py target/electric-guitar/reference --stage evaluate --renderer <new-worker> --baseline <before-worker> --out target/electric-guitar/validation.json
```

To repeat the searches instead of replaying retained controls:

```sh
uv run tools/eval/electric_copy.py target/electric-guitar/reference --stage di --prior tools/eval/references/electric-di-initial-fit.json --iterations 8 --out target/electric-guitar/di-fit.json
uv run tools/eval/electric_copy.py target/electric-guitar/reference --stage amp --iterations 6 --out target/electric-guitar/amp-fit.json
uv run tools/eval/electric_copy.py target/electric-guitar/reference --stage simple --out target/electric-guitar/simple-fit.json
```

Create three editable, sample-free projects and their native WAV exports:

```sh
cargo run --release -p auris-session --example electric_demo -- target/electric-guitar/demo
cargo run --release -p auris-synth --example physical_bench -- --amp
```

The example saves one project per folder, preserving the project's asset rules. Listen
at matched whole-phrase level when comparing the three drive settings; no framewise
normalization or limiter is needed. The native exports retain the actual plugin levels.
