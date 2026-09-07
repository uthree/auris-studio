# CPU transcription: temporal decoding and evaluation

The second CPU iteration reduces note fragmentation caused by vibrato. It runs through the
existing desktop, CLI and model-tool commands; no new setup or weights are needed. The audio
report's algorithm identifier is `cpu-spectral-yin-v2`.

## What changed

The first implementation selected one period and rounded it independently in every frame.
When vibrato crossed a semitone's halfway point, this could split one sounding note into many
short notes. The new implementation retains up to six continuous period candidates, projects
them onto nearby note identities, and finds a low-cost sequence through time. It penalizes
unnecessary changes but permits genuine sustained semitone steps. Silence has its own state.
The difference-function windows are centered on their timestamp to avoid a systematic delay
from measuring the earlier half of a frame.

The candidate/decoder separation is informed by
[Mauch and Dixon's pYIN paper](https://webspace.eecs.qmul.ac.uk/s.e.dixon/pub/2014/MauchDixon-PYIN-ICASSP2014.pdf).
This implementation uses hand-set costs and a compact note-state search, not the paper's
threshold distribution or probabilistic model. It is not a pYIN reproduction or a learned
predictor. At most eight voiced states and one silence state are retained per frame; subtracting
the shared minimum cost avoids accumulating floating-point error over long files. Cancellation
is checked in both forward decoding and backtracking.

## Identical inputs show less fragmentation

The diagnostic generator is original repository code. It produces nine 4.5-second mono PCM
fixtures at 11,025 Hz. Each tonal fixture contains eight reference notes; the two negative
controls contain none. All nine PCM SHA-256 hashes match between the saved
[v1 results](benchmarks/transcription-v1.json) and [v2 results](benchmarks/transcription-v2.json).
The v1 baseline used the backend from commit `895bf15`; only evaluation tooling was added when
capturing it. The same fixture generator was then used with the changed backend.

| Diagnostic | v1 estimated notes | v2 estimated notes | v1 onset F1 | v2 onset F1 | v1 onset+offset F1 | v2 onset+offset F1 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Clean sine notes | 8 | 8 | 1.000 | 1.000 | 1.000 | 1.000 |
| Vibrato, ±65 cents at 5.5 Hz | 29 | 8 | 0.216 | 1.000 | 0.000 | 1.000 |
| Weak fundamental, stronger second/third harmonics | 8 | 8 | 1.000 | 1.000 | 1.000 | 1.000 |
| Legato semitone steps | 8 | 8 | 1.000 | 1.000 | 1.000 | 1.000 |
| Repeated same-pitch notes | 8 | 8 | 1.000 | 1.000 | 1.000 | 1.000 |
| Four note-level gains | 8 | 8 | 1.000 | 1.000 | 1.000 | 1.000 |
| 20 ms internal dropouts | 10 | 10 | 0.889 | 0.889 | 0.667 | 0.667 |
| Deterministic noise | 0 | 0 | — | — | — | — |
| DC signal | 0 | 0 | — | — | — | — |

The metrics use maximum one-to-one bipartite matching, so duplicate predictions cannot each
claim the same reference. Pitch tolerance is 50 cents; onset tolerance is 50 ms; offset tolerance
is the larger of 50 ms and 20% of reference duration. These criteria follow
[mir_eval's transcription metric](https://mir-eval.readthedocs.io/latest/api/transcription.html).
Both onset-only and onset-plus-offset scores require matching pitch. Empty cases report zero
F1 in JSON; evaluate their false-positive counts instead, hence the dashes above.

These are engineering diagnostics, not held-out or real-recording accuracy estimates. The
cases are visible during development and are too simple to represent microphones, room
reverberation or a range of real instrument articulations. The unchanged dropout result is a
specific limitation: a brief interruption can resemble a genuine same-pitch reattack. Smooth
vibrato into another note can also move its inferred boundary by a few frames. Inspect and
correct the draft as before.

## CPU cost stays small on the development machine

On 2026-09-07, Windows x86_64, AMD Ryzen 7 9800X3D, release build, one sequential analysis
worker, the three-minute stereo pulse fixture measured 0.218 s for BPM/chords and 0.416 s
including transcription (real-time factors 0.0012 and 0.0023). The estimated leading tempo was
120.127 BPM and the draft contained the expected 360 A4 notes. A separate run during compilation
measured 0.282 s and 0.433 s, illustrating ordinary timing variation.

The standalone measurement process reported a peak working set of 21.56 MiB, observed through
Windows `PeakWorkingSet64` with 10 ms polling. This includes fixture construction and both
analysis passes, not a before/after incremental allocation measurement. It excludes the DAW,
file decoding and source resampling; it is not a memory ceiling for imported high-rate files.
This workload provides no reason to request external compute.

## Reproduce or evaluate a recording

```sh
cargo test -p auris-analysis --all-targets
cargo run -p auris-analysis --release --example evaluate_transcription > diagnostics.json
cargo run -p auris-analysis --release --example measure_analysis

auris transcribe-audio isolated-melody.wav > estimate.json
cargo run -p auris-analysis --release --example evaluate_transcription -- reference.json estimate.json
```

The two-file mode reads only the `notes` field of each JSON file. Use MIDI pitch numbers
(fractional semitones are allowed) and unstretched seconds from the start of the audio file:

```json
{"notes":[{"pitch":69,"start":0.2,"end":0.8}]}
```

It prints separate precision/recall/F1 and false-positive/negative counts, plus hashes of the
reference and estimate files. Invalid pitches/timestamps and excessive comparison sizes are
rejected. Comparisons are limited to 10,000 notes per file and four million candidate pairs;
use excerpts for larger material. Original CLI reports include extra fields, which are ignored
by this metric reader.

For a recording benchmark, retain the audio hash, recording/performer identity, rights,
excerpt boundaries and independently reviewed annotations alongside these reports. Keep
development and evaluation recordings separate. The 30–50 excerpt pilot in the research plan
remains necessary before claiming real-recording accuracy. The committed generator and JSON
reports make the current synthetic claim inspectable without downloading a dataset or model.
