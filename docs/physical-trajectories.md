# Sustained and moving-pitch copy synthesis

This continues the [one-second real-recording calibration](physical-copy-synthesis.md).
The offline worker now keeps one excitation alive while applying sample-timed pitch bend
and expression. It renders the actual Rust instrument, including the production sample rate.
The optimizer and recordings remain development tools; releases carry only model code and
small coefficients.

## References and supplied performance

The fixed cohort adds 212 excerpts to the original 165 short notes:

| Reference | Excerpts | Training | Validation |
| --- | ---: | ---: | ---: |
| Iowa piano, 3 and 6 seconds | 36 | 12 | 24 |
| Iowa guitar, 3 and 6 seconds | 98 | 48 | 50 |
| Iowa violin, 3 and 6 seconds | 54 | 19 | 35 |
| TU-Note violin glissandi, approximately 6–10 seconds including release | 24 | 12 | 12 |

Iowa uses the original pitch/dynamic split. A duration is included only when the original
pitch-checked stable region exceeds it by 100 ms. Shorter recordings are recorded as exclusions;
no clip is stretched, looped, or extended into the next note.
Iowa note-off times are not annotated: these windows are rendered held through their ends,
and cannot validate piano pedal timing or an inferred release gesture.

[TU-Note](https://depositonce.tu-berlin.de/handle/11303/7527), by Henrik von Coler, Jonas Margraf
and Paul Schuladen, with violinist Michiko Feuerlein, supplies real upward/downward five-semitone
glissandi, with and without vibrato, at mp and ff. We use the DPA microphone recordings and
hand-labeled attack, transition and release boundaries. G/D strings train; A/E strings are held
out from this optimizer. This adds a different player, instrument and recording session to Iowa.
It does not establish generalization to an arbitrary player or recording environment.

TU-Note is [CC BY-ND 4.0](https://creativecommons.org/licenses/by-nd/4.0/). The tool fetches just
the selected archive members with bounded HTTP ranges and verifies sizes, CRCs and SHA-256.
Original recordings and locally processed excerpts stay in ignored development directories.
The repository stores metadata, control hashes, coefficients and numerical results. The listening
montage uses synthetic renders for TU-Note, and Iowa recordings under Iowa's unrestricted terms.

For violin, performance information is supplied from each reference:

* Fundamental estimates use a 4096-sample Hann window, 10 ms hop, and log-parabolic spectral
  peak interpolation in a known sub-octave interval. Missing attack/silent frames are interpolated
  from voiced measurements. This retains played vibrato and portamento; it is not polyphonic
  transcription. Pitch tracking is measured again on synthesized PCM, including voiced coverage.
* Expression uses 20 ms RMS, a 60 ms Gaussian smoothing width, the held section's 95th-percentile
  level and 50 ms control points. The square root of relative amplitude accounts approximately
  for CC11 affecting both bow motion and output gain. These are fixed performance guides, not
  optimized per validation recording or inferred automatically during ordinary playback.
* TU note-off is the annotated beginning of release, not the end of the WAV. Pitch/CC events
  occur strictly before it. Decimal rounding is guarded before conversion to Rust `f32`.

Every build comparison receives identical guides. A separate constant-expression ablation
shows how much improvement comes from supplied expression rather than changed factory timbre.
Whole-note normalization removes recording gain; it does not remove the played crescendo.

## Objective and calibration

The two-resolution, 64-band log-mel representation is unchanged. Long excerpts receive separate
attack, early sustain, late sustain, pitch-transition and release losses, with weights
0.15/0.15/0.40/0.20/0.10, renormalized over nonempty phases. Only feature centers strictly inside
an excerpt count; its boundary does not invent a note-off or release phase. A matching attack cannot obscure
several seconds of incorrect sustain. Reports also retain envelope error in dB, pitch error in
cents, voiced coverage and every per-note phase loss. Missing voiced output receives a
100-cent pitch penalty. Pitch and envelope measurements are diagnostics, not alternate fitting
targets.

Dataset weights are short Iowa 0.3, long Iowa 0.5, TU-Note 0.2, renormalized for piano/guitar.
The old short-note objective remains a regression component. Bounded seeded differential
evolution fits excitation/loss controls. A fixed local coordinate schedule refines around the
best training point. A regularized twelve-band coloration proposal is accepted only if the
primary phase-balanced mel objective improves. Validation recordings never select search
parameters. Their supplied performance guides are inputs to this conditional synthesis task.

Violin adds release and `bow_response` to the search. Bow response controls the velocity ramp
and an output-expression smoother with half that time constant. This prevents CC11 from
introducing a discontinuous output gain on an already vibrating string. Initial expression is
set directly before note-on; subsequent changes preserve the wave. Live response automation
recomputes coefficients without allocating. The constructor's default is approximately 57 ms;
the fitted release is 1.5 seconds, the upper search bound, so release reconstruction remains
limited by the present model/search interval.

The Python coloration surrogate respects the exact Rust ordering: body first, moving output
expression afterwards. A causal filter and dynamic gain do not commute. The surrogate undoes
the dry output's positive expression, filters it, and restores the same smoothed expression;
it does not undo expression already supplied to the bow. Its PCM agreement is checked against
the actual renderer before searching. The final reports compare complete Rust builds directly.

Guitar receives a long-decay calibration. Piano retains its current calibration. Median training
RMS preserves the factory level convention through a model normalization scalar. Explicit project
parameters remain explicit; composition roles can override factory parameters.

## Measured results

The following are mean held-out mel losses from complete frozen Rust builds, starting at
`797a19b`. Lower is better. Both builds receive the same recorded pitch and expression guides.
The 3/6-second Iowa views are overlapping excerpts, not independent performances.

| Validation cohort | 24 kHz before | 24 kHz after | 48 kHz before | 48 kHz after |
| --- | ---: | ---: | ---: | ---: |
| Piano, long Iowa | 0.145584 | 0.145584 | 0.147241 | 0.147241 |
| Guitar, long Iowa | 0.127096 | 0.122735 | 0.127138 | 0.122772 |
| Violin, long Iowa | 0.091258 | 0.090017 | 0.094414 | 0.091412 |
| Violin, TU glissandi | 0.044139 | 0.043970 | 0.046787 | 0.046636 |
| Guitar, original short Iowa | 0.148237 | 0.147670 | 0.148302 | 0.147732 |
| Violin, original short Iowa | 0.084689 | 0.082639 | 0.088016 | 0.086301 |

At 48 kHz this reduces long guitar loss by 3.4%, long violin loss by 3.2%, and short violin
loss by 1.9%. The glissando timbre/control calibration improvement is modest, about 0.3%.
The constant-expression ablation gives a different comparison: the preceding violin's TU
loss is 0.083713 with a fixed bow, versus 0.046636 for the new build with recorded expression
(44.3% lower). This measures conditional performance following as well as the model change;
it does not establish automatic generation of an equivalent bow gesture.

Per-note pitch, envelope, coverage and phase diagnostics are archived with the
[24 kHz](../tools/eval/references/physical-trajectory-validation-24k.json),
[48 kHz](../tools/eval/references/physical-trajectory-validation-48k.json), and
[constant-expression](../tools/eval/references/physical-trajectory-constant-bow-48k.json)
reports. Training-only [guitar](../tools/eval/references/physical-trajectory-guitar-calibration.json)
and [violin](../tools/eval/references/physical-trajectory-violin-calibration.json) calibration
reports retain fitted values before rounding into Rust.

The symbolic preset measurement is byte-identical before/after
(SHA-256 `68a1be1d19d62d88d822022a67f7dd549e3875dd8f5cf6eb66800f18d6006496`).
Across the nine fixed song presets, learned CE/PQ means change from 5.887/7.734 to
5.896/7.765, while PC changes from 4.473 to 4.443; these song-level diagnostics are mixed,
not optimization targets. On this development machine, 24 held voices at 48 kHz in 256-frame
callbacks take 0.081 ms mean / 0.104 ms p99 for guitar and 0.075 / 0.103 ms for violin.
These local timings are not a worst-case realtime guarantee.

## Reproduction

Use `uv` script environments and the dependency versions recorded in the reports. No trainer
environment or GPU is required. On Windows, append `.exe` to worker paths.

```sh
uv run tools/eval/fetch_physical_reference.py target/copy-synthesis/iowa
uv run tools/eval/prepare_physical_reference.py target/copy-synthesis/iowa \
  --lock tools/eval/references/iowa-notes.json
uv run tools/eval/fetch_violin_transitions.py target/long-copy/tu-note \
  --lock tools/eval/references/tu-note-captures.json
uv run tools/eval/prepare_physical_trajectories.py target/copy-synthesis/iowa \
  target/long-copy/tu-note target/long-copy/references \
  --lock tools/eval/references/physical-trajectories.json
cargo build --release -p auris-synth --example physical_fit_render
# Freeze a worker before changing the audio implementation or factory calibration.
uv run tools/eval/fit_physical_trajectories.py target/long-copy/references \
  target/copy-synthesis/iowa target/long-copy/before-worker target/long-copy/fit.json \
  --fit --model violin --profiles tools/eval/references/physical-long-initial-profiles.json
uv run tools/eval/fit_physical_trajectories.py target/long-copy/references \
  target/copy-synthesis/iowa target/release/examples/physical_fit_render \
  target/long-copy/validation.json --before target/long-copy/before-worker --rate 48000
uv run tools/eval/fit_physical_trajectories.py target/long-copy/references \
  target/copy-synthesis/iowa target/release/examples/physical_fit_render \
  target/long-copy/constant-bow.json --before target/long-copy/before-worker \
  --model violin --constant-expression --rate 48000
uv run tools/eval/audition_physical_trajectories.py target/long-copy/references \
  target/long-copy/before-worker target/release/examples/physical_fit_render \
  target/long-copy/audition --constant-before
```

To reproduce the recorded violin fit, the starting worker includes bow-response smoothing but
uses the previous factory parameters/body gains (`797a19b`); use six global iterations and the
default local schedule (bound-width fractions 0.05, 0.02 and 0.008), with seed 20261001 and
differential-evolution population multiplier 4. An isolated source checkout and independent Cargo target directory keep
baseline builds reliable. The worker reports whether its instrument supports expression smoothing,
so copying the example into the preceding source preserves the correct surrogate behavior.
Guitar uses the same six-iteration global search and fixed local schedule against its original
`797a19b` worker.

The compact reference lock retains WAV hashes and canonical control-array hashes. Full control
points are regenerated locally. `--start-from` resumes a training-only local fit only if the
renderer and full reference hashes match. Reports include defaults, sample rate, seed, bounds,
versions, worker hashes and per-note errors. Both 24 and 48 kHz checks use actual frozen binaries.

```sh
uv run --with numpy --with scipy --with soundfile --with pytest pytest \
  tools/eval/test_physical_trajectory.py tools/eval/test_physical_mel.py \
  tools/eval/test_fit_physical_body.py tools/eval/test_physical_ab.py
uv run --with ruff ruff check tools/eval/*physical*.py tools/eval/fetch_violin_transitions.py
cargo test --workspace
cargo test -p auris-synth --example physical_fit_render
cargo clippy --workspace --all-targets
```

Numeric regressions cover twelve-second violin sustain at multiple rates/dynamics, sounding
response automation, output continuity, callback allocations, pitch-tracker accuracy, long
crescendos, fractional note-off, late-sustain loss and sample-exact control events across
64/257/1024-frame blocks.
