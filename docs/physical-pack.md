# Copy synthesis for the physical drum kit

[日本語](physical-pack.ja.md)

Factory excitation/loss settings and causal radiation coloration are fitted against real
recordings for seven Drum Kit families. The baseline is `cfcd5a9`; the comparison uses actual
production Rust PCM at 24 and 48 kHz. Every coefficient is selected on training audio.
Validation determines whether the resulting change is useful; it is not an optimizer input.

## References and split

The recordings are Alexander Holm's original
[Salamander Drumkit](https://github.com/endolith/Salamander-Drumkit), mirrored in the
[original archive](https://archive.org/details/SalamanderDrumkit). These are overhead recordings
of a real kit, not sound synthesizer renders. The archive was originally CC BY-SA 3.0; the author
[dedicated his sampled instruments to the public domain in 2022](https://rytmenpinne.wordpress.com/2022/03/04/good-news-everyone/).
Attribution and the original archive's identity are retained in the capture manifest, and each
hit's original member name and hash in the note manifest.

The cohort is fixed before fitting: 151 original drum hits, with no result-dependent
exclusions. Selection uses the first eight numbered hits per original family/layer;
odd hits train, even hits validate. Alternative snares, ghosts, sticks, semi-open hats and other
articulations are outside this fixed cohort. A hit cannot occur in both splits. Named dynamics
are velocity proxies, not measured MIDI velocities.

| Family | Training | Validation |
| --- | ---: | ---: |
| Kick | 12 | 12 |
| Snare | 12 | 12 |
| Closed hat | 8 | 8 |
| Open hat | 10 | 8 |
| Crash | 6 | 5 |
| Ride | 6 | 6 |
| Tom, low/high | 23 | 23 |
| Total | 77 | 74 |

Source hashes, onset offsets, available duration and extracted float32 WAV hashes are
locked in `tools/eval/references/physical-pack-{captures,notes}.json`. Onset is the first
local 2% RMS crossing at 1 ms resolution, including captures with more than one second of
initial silence. Excerpts last three seconds and zero-pad shorter available captures. Synthetic
notes remain held for those three seconds, so an artificial note-off cannot improve the fit.

## Measurements

Both signals receive a causal second-order 30 Hz highpass before one whole-excerpt RMS
normalization to 0.1. This excludes sub-band microphone rumble from the normalization. There is
no per-frame normalization, time warp or synthesized-onset search. Hann windows of 512 and 2048
samples, hop 240, and 64 area-normalized HTK mel bands over 30 Hz–10 kHz produce
`log(1 + 10000 * mel_power)`. Mean absolute distance receives weights 0.3 for 0–50 ms,
0.3 for 50–250 ms, and 0.4 for 250–3000 ms, averaged over the two resolutions.

Secondary measurements are whole-excerpt-normalized 20 ms RMS-envelope error in dB and
absolute error in the time containing 90% of the excerpt's band-limited energy. These measure
different aspects of a strike and remain visible when spectral distance improves. A separate
48 kHz production render is polyphase downsampled for the same 24 kHz analysis.

Paired 95% intervals use 2,000 bootstrap resamples, seed 20261003, grouped by original capture.
They describe this cohort, without multiple-comparison correction. Held-out hits share their
kit, performer and recording chain with training. They establish internal recording-domain
improvement rather than generalization to every drum kit.

## Fitting and runtime behavior

The bounded coordinate search starts from the unchanged model. Contact hardness, strike
position, decay and damping use steps 0.18/0.06 of their declared bounds, followed by a 0.03
refinement. Twelve peaking sections at geometrically spaced 90 Hz–10 kHz centers, Q=0.9, use
gains bounded to ±9 dB and steps 0.25/0.10, with a 0.0001 mean-square gain penalty. These broad
priors include excitation and microphone coloration; they are not measured body admittances.
The Rust worker renders every contact/loss candidate. A causal filter surrogate accelerates
radiation search; final actual PCM must agree within `2e-4` absolute sample error.

The kit fits each family independently, while its shared controls keep their existing defaults:
hardness/position/damping receive factory-relative offsets and decay multiplies the fitted ratio.
Per-family radiation follows the summed current and retiring hits. Median training-note RMS
is preserved through fixed output normalization instead of changing the public Level control.

Snare and tom candidates must also preserve the existing label-free acoustic-role contracts.
The snare must retain fitness at least 0.65 and a wire/head band-amplitude ratio above 2.05 at
25–200 ms. Every tom key must retain a tom-minus-kick fitness margin of at least 0.05.
These guards use synthesized probes and training recordings, not validation labels. They choose
between original/fitted controls and radiation strengths 0/0.125/…/1, then refine feasible gains
with fixed steps 0.125/0.05/0.02. Training envelope error may grow by at most 5% during this
guarded selection. The larger unconstrained snare mel reduction failed its wire/role contract;
the smaller correction below preserves it. This refinement followed numerical regression
checks; intervals are descriptive after model development, not an untouched final test.

Coefficients and seven fixed pad buffers are prepared off the callback. Choking, stealing,
mixing and reset allocate nothing. All-sound-off fades and clears radiation memory over two
milliseconds. Natural radiating tails can continue briefly after the struck mode becomes
inactive. Note timing and written project data retain their existing representation.

## Targeted snare and ride voicing

The 2026-10-19 voicing adjustment follows an isolated-hit investigation of sharp drum sounds.
The snare applies a -4 dB, Q=1.3 peaking section at 3.8 kHz to the wire signal only, with a
fixed 1.145 wire gain to retain its lower rattle. The membrane and its decay are unchanged.
The ride excites plate modes 0 and 8 at 0.6 and 0.8 of their previous amplitudes, respectively.
These modes lie at 2.65 and 12.36 kHz. The reduction follows strike-energy normalization, so
other modes do not receive the removed energy; mode frequencies and losses stay the same.

At 48 kHz and velocity 1, the snare's first-50-ms 2–5 kHz power falls by 1.52 dB. Its
700–1500 Hz wire body and 7–10 kHz sizzle change by +0.94 and +0.55 dB. The existing
25–200 ms wire/head amplitude ratio remains above 2.05. The ride's 50–250 ms line power
falls by 4.45 dB at 2.65 kHz and 1.90 dB at 12.36 kHz. Whole-note RMS falls by 0.39 dB
for snare and 1.14 dB for ride; their 90%-energy times change by less than one millisecond.
These measurements preserve specified parts of the timbre, rather than establish a
perceptual-quality or real-recording-distance improvement.

`tools/eval/drum_harshness.py` compares frozen before/after workers over nine pads/registers,
three velocities and 44.1/48/96 kHz. Unrelated pads must match sample for sample. It saves
absolute power, energy duration, renderer hashes, common-gain listening pairs and an
additional RMS-matched candidate. Summary measurements are in
`tools/eval/references/drum-harshness.json`. The original worker is retained before rebuilding:

```sh
uv run tools/eval/drum_harshness.py <before-worker> <after-worker> target/drum-harshness/fix
uv run --with pytest --with numpy --with scipy --with soundfile pytest tools/eval/test_drum_harshness.py
uv run --with ruff ruff check tools/eval/drum_harshness.py tools/eval/test_drum_harshness.py
```

The symbolic ruler is unchanged. A separate eight-second, four-hit snare diagnostic through
`music.py`, using one window and the fixed rock prompts, changes TuneJury reward by -0.0575
and MuQ-MuLan positive cosine by +0.00089. Conditions and excerpt positions match. These mixed
isolated-hit scores do not establish improved song preference or perceived timbre.

## Held-out production results

These are final actual 48 kHz Rust renders; lower distance is better.

| Family | Before mel L1 | After | Reduction |
| --- | ---: | ---: | ---: |
| Kick | 0.113811 | 0.083642 | 26.51% |
| Snare | 0.565809 | 0.553065 | 2.25% |
| Closed hat | 0.368537 | 0.323287 | 12.28% |
| Open hat | 0.398668 | 0.356545 | 10.57% |
| Crash | 0.484578 | 0.380532 | 21.47% |
| Ride | 0.367159 | 0.269354 | 26.64% |
| Tom | 0.229406 | 0.145699 | 36.49% |

All seven paired mean-delta intervals lie below zero. Snare envelope error is almost unchanged
at 11.39→11.40 dB. Open hat and ride still have large envelope errors of 42.35 and 35.70 dB:
their late recorded tails are not closely reproduced. Per-note phases, intervals and secondary
measurements are retained in `tools/eval/references/physical-pack-validation.json`.

On the development machine, 24 overlapping crashes use approximately 0.22 ms median and
0.23 ms p99 of a 5.333 ms callback budget; motion observation stays within the same budget.
Allocation, timing, reset, rate/extreme-control, pitch, wire, choking and complete-kit
acoustic-role tests verify the runtime behavior.
The composer measurement rows are unchanged. The nine-preset Audiobox measurements and WAV
hashes are identical: this preset probe does not exercise the modified factory sounds and
establishes no quality improvement for them. See `physical-pack-regressions.json` beside the
references for the scores and an additional common-gain kit-groove comparison.
For that 16-second groove, Audiobox's Production Quality rises from 7.54 to 7.67. This is a
single learned regression diagnostic rather than an objective perceptual-quality guarantee.

## Reproducing the experiment

The audio remains ignored under `target/`; manifests, fitted coefficients and reports are
versioned. Keep the pre-change worker before applying coefficients, since an archived executable
is the performer baseline.

```sh
cargo build --release -p auris-synth --example physical_fit_render
uv run tools/eval/fetch_physical_pack.py target/physical-pack/reference --lock tools/eval/references/physical-pack-captures.json
uv run tools/eval/prepare_physical_pack.py target/physical-pack/reference --lock tools/eval/references/physical-pack-notes.json
uv run tools/eval/fit_physical_pack.py target/physical-pack/reference <before-worker> target/physical-pack/fit.json
uv run tools/eval/fit_physical_pack.py target/physical-pack/reference <before-worker> target/physical-pack/validation.json --fit tools/eval/references/physical-pack-fit.json --candidate <after-worker> --audition target/physical-pack/audition
```

Use `.exe` paths on Windows. Baseline/candidate executable hashes, dependency versions and
canonical reference/fit hashes are recorded. The complete reference archive is bounded to
387,611,727 bytes. Hashes are checked before extraction; only explicitly selected drum files
are read from the archive into flat local paths.
