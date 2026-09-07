# Music analysis: research and implementation proposal

Research date: 2026-09-07. Repository inspected at `95dea35`.
Status: proposal; algorithms and models have not been implemented or benchmarked in this task.

Start with CPU-based symbolic chord recognition, then audio tempo and chord analysis.
Evaluate small pretrained models for transcription of an isolated instrument and for instrument
tags before integrating them. Evaluate full-mixture, instrument-separated transcription as a
separate compute project: [compute assessment](music-analysis-compute.md).

The deliverable should be an editable analysis draft with alternatives and uncertain regions.
The first transcription output is editable notes and MIDI; readable staff notation needs its
own rhythm and voice interpretation stage. No result should silently replace a project's
tempo, harmony or written notes.

## What the repository already provides

| Existing implementation | What can be reused; what the new work needs |
| --- | --- |
| `crates/auris-compose/src/analysis.rs`: `detect_key`, `harmonise`, `read_melody` | Key-profile correlation and duration-weighted pitch classes. Accompaniment chooses among seven diatonic triads, one per bar; it carries harmony through silence. Recognition needs chromatic chords, changes within a bar and an explicit uncertain/no-chord result. |
| `crates/auris-session/src/session/accompany.rs` | An explicit command that writes inferred harmony and accompaniment. Analysis needs a read-only path before any apply command. |
| `crates/auris-session/src/session/musical_analysis.rs` | `analyze_music` measures written-note density, range and repeated bars. It does not recognize a chord sequence. |
| `crates/auris-session/src/session/analysis.rs` and `crates/auris-gpu/src/analysis.rs` | Rendered-mix loudness, RMS/peaks and section/track reports. These are not audio-file BPM, instrument or transcription algorithms. |
| `crates/auris-dsp/src/spectrum.rs`, `spectrogram.rs`; session `spectrogram.rs` | FFT infrastructure and immutable worker jobs with stale-source validation. Display spectrograms pool time columns, so recognition must compute appropriately resolved features from source audio. |
| `crates/auris-core/src/theory/chord.rs`, `numeral.rs`, `harmony.rs` | Twenty chord qualities, bass notes and timeline harmony. Reuse the vocabulary; verify absolute chord to key-relative numeral conversion when applying results. |
| `crates/auris-io/src/midi.rs` | MIDI input/output and tick/tempo conversion. Reuse through session commands for accepted transcriptions. |
| `crates/auris-singer/Cargo.toml` | Existing `ort = 2.0.0-rc.9`, including Windows DirectML. This demonstrates an inference deployment path, not that transcription models are compatible with it. |

The architectural authority remains `auris_session::guide`. Its sections on worker jobs and
“The other direction” describe the existing behavior. Update that guide first when implementing
new boundaries; this proposal does not declare them implemented.

## Research supports different methods for different inputs

### Symbolic notes: segment the music and rank chord hypotheses

Pardo and Birmingham's *Algorithms for Chordal Analysis* (2002) treats segmentation and chord
labeling together, using template matches over symbolic note events. This is a useful basis
for a deterministic CPU implementation. Merely collecting the pitches sounding at each onset
misses arpeggios and mistakes passing notes for chord changes.
[Paper](https://interactiveaudiolab.github.io/assets/papers/pardo-birmingham-cmj02.pdf).

Proposed recognizer:

1. Select clips/tracks and a time range; collect overlapping written notes in absolute ticks.
   Exclude percussion by track semantics, not by MIDI pitch. Clip offsets, repetitions and
   sounding-note carryover must be handled explicitly. Keep the choice of written notes versus
   playback transforms visible in the request; use written notes initially.
2. Use note onsets/ends and beat boundaries as candidate segmentation points. Calculate overlap
   duration per pitch class, onset strength and independent bass evidence. Cap duplicate/octave
   reinforcement so doubled parts do not dominate the answer.
3. Score roots across all twelve pitch classes. Initially cover major/minor, diminished,
   augmented, sus2/sus4, dominant seventh, major seventh, minor seventh and half-diminished
   seventh. Penalize missing defining tones and unexplained tones; do not reward a larger chord
   solely for including more pitches. Key is a weak prior, not a diatonic restriction.
4. Compare adjacent segment hypotheses with dynamic programming and a change penalty. Bound
   candidate spans and vocabulary so long clips cannot cause unbounded search. Retain the best
   alternatives, their score margins, and whether the bass supports a slash chord.
5. Distinguish `NoChord` (silence/non-harmonic material) from `Unknown` (insufficient or conflicting
   evidence). A single note may support candidates but should not confidently invent a triad.
   A score margin is not a calibrated probability.

For example, C–E–G over bass E should offer C/E, while C–E–G–A may reasonably offer both Am7/C
and C6. Displaying that ambiguity is more useful than a forced, apparently certain label.
Extended jazz chords can use the existing vocabulary once validated; initial coverage must be
reported separately from the full representational capacity of `Chord`.

### Audio chords and tempo: establish a reproducible DSP baseline

Chroma features summarize energy in the twelve pitch classes. Template matching provides a
simple chord baseline; a hidden Markov model (HMM) and Viterbi decoding can favor plausible
sequences over noisy frame-by-frame labels. The FMP notebooks provide both methods and discuss
feature extraction and segmentation.
[Templates](https://www.audiolabs-erlangen.de/resources/MIR/FMP/C5/C5S2_ChordRec_Templates.html),
[HMM](https://www.audiolabs-erlangen.de/resources/MIR/FMP/C5/C5S3_ChordRec_HMM.html).

For tempo, Ellis (2007) estimates onset strength, finds periodicity through autocorrelation and
uses dynamic programming to locate beats. This supplies an interpretable CPU baseline with
known limits around tempo variation.
[Paper and implementation](https://www.ee.columbia.edu/~dpwe/LabROSA/matlab/beat_simple/).

Proposed audio pipeline:

- Decode through `auris-io`; record original sample rate, channel count and source-time range.
  Reuse resampling primitives with task-specific rates. Compute channel features before pooling
  for DSP analysis to preserve opposite-phase stereo. For models requiring mono, test channel
  cancellation and record the selected downmix policy.
- Extract short-time spectral changes for onsets, a tempogram for tempo candidates, and beats
  with a continuity constraint. Return half/double-tempo alternatives, beat timestamps and
  local tempo stability; silence and free rhythm may return no reliable BPM. Meter and downbeat
  confidence are separate from a BPM estimate.
- Extract tuned chroma and bass features; compare plain features with harmonic/percussive
  separation. Start audio recognition with 24 major/minor labels plus no-chord and an uncertainty
  gate. Decode over time and optionally pool by confident beats. Keep a time-based path when
  beat tracking is unreliable. Expand sevenths and inversions only after separate evaluation.

Deep Chroma learns cleaner features for chord recognition; it is a relevant second-stage
comparison if the DSP baseline fails on dense mixes.
[Korzeniowski and Widmer, 2016](https://arxiv.org/abs/1612.05065).
For beats/downbeats, *Beat this!* (2024) offers published small and main checkpoints of about
8.1 MB and 78 MB respectively, with code and weights under MIT. It belongs in the lightweight
inference comparison, not automatically in the cloud-only category.
[Official implementation](https://github.com/CPJKU/beat_this).

### Instruments: identify audible families with multiple labels

Instrument identification should initially answer “piano and drums are likely present in this
interval,” with several simultaneous labels. It cannot establish the exact synthesizer, plugin,
sample library or recording equipment used to make a sound.

| Candidate | Evidence and proposed use | Limit |
| --- | --- | --- |
| YAMNet | MobileNet-based classifier with 521 AudioSet event classes; first small-model comparison for broad audible families. [Official documentation](https://www.tensorflow.org/tutorials/audio/transfer_learning_audio) | General sound labels do not map directly to all General MIDI programs; weak sources can be masked. |
| PANNs (2020) | Pretrained audio tagging and sound-event models over 527 AudioSet classes; comparison if YAMNet is insufficient. [Authors' repository](https://github.com/qiuqiangkong/audioset_tagging_cnn) | Runtime depends on the selected architecture/checkpoint. Overall AudioSet mAP does not establish accuracy on Auris instrument labels. |
| Source separation followed by tagging | A diagnostic comparison for overlapping sources. [Demucs](https://github.com/facebookresearch/demucs) | Standard stems are broad groups, not arbitrary individual instruments; artifacts may worsen both tags and notes. |

Define a versioned mapping for piano/keys, guitar, bass, strings, brass, woodwinds, drums,
voice and synth-like sounds, including unsupported/unknown categories. Keep raw model labels
for inspection. Calibrate class-specific thresholds on mixtures, evaluate rare families
separately, and never assign all extracted notes to the highest-scoring instrument tag.

### Audio to notes and audio to readable notation are separate milestones

pYIN (Mauch and Dixon, 2014) estimates fundamental-frequency candidates and tracks them
probabilistically. Pair it with onset/offset segmentation as a CPU monophonic baseline, useful
for an isolated lead or voice. It does not solve polyphonic mixtures.
[Paper](https://www.eecs.qmul.ac.uk/~simond/pub/2014/MauchDixon-PYIN-ICASSP2014.pdf).

Basic Pitch (Bittner et al., 2022) is a lightweight, instrument-agnostic polyphonic transcription
model. Spotify supplies ONNX and other runtime formats; its documentation says it works best
on one instrument at a time. Evaluate its ONNX CPU path on isolated piano, guitar and voice
before considering DirectML. Instrument-agnostic means it need not be specialized for each
instrument; it does not mean it identifies or separates instruments.
[Paper](https://arxiv.org/abs/2203.09893),
[Official implementation](https://github.com/spotify/basic-pitch).

MT3 (2022) jointly predicts note events and instruments using T5X; YourMT3+ (2024) extends this
approach with new attention/decoding architectures and data mixing. The latter's paper also
reports limitations on pop recordings. Both are research comparisons for full-mixture
transcription, with reproducibility and compute checked separately.
[MT3](https://github.com/magenta/mt3),
[YourMT3+ paper](https://arxiv.org/abs/2407.04822).
MuScriptor (2026) is a more recent multi-instrument candidate whose model-size and weight-license
constraints are covered in the compute report.
[Authors' implementation](https://github.com/muscriptor/muscriptor).

Transcription should first preserve source-time onsets, offsets, pitch and optional bends,
instrument hypotheses and model scores. Converting those events to a readable score then needs
beat/downbeat alignment, meter, quantization including tuplets and swing, rests, ties across
barlines, voice/staff assignment and pitch spelling. Preserve unquantized events so the user
can change this interpretation. MusicXML represents those notation distinctions explicitly.
[MusicXML 4.0 notation tutorial](https://www.w3.org/2021/06/musicxml40/tutorial/notation-basics/).

## Proposed integration keeps analysis off the realtime thread

Introduce an `auris-analysis` backend for symbolic recognition, audio feature orchestration and
optional inference. Its local dependencies would be `auris-core` and `auris-dsp`; decoding stays
in `auris-io`, and document commands stay in `auris-session`. It must not depend on a frontend,
the engine or the composer. Keep FFT/resampling primitives in DSP and music vocabulary in core.
Do not couple music analysis to the singer's voice-model schema or training project.

The session prepares immutable jobs, executes them on workers, exposes reports, and applies
selected results as one undoable transaction. Reuse the existing spectrogram-job pattern with
source-buffer identity; add document/revision and options validation for note selections and
tempo mappings. Jobs need progress, cancellation, bounded chunks and stale-result rejection.
Model loading, inference, file I/O and feature allocation never run on the audio callback.

Proposed command families, with names finalized at implementation:

| Command | Result or effect |
| --- | --- |
| `analyze_chords(selection, options)` | Tick-based chord intervals, alternatives and unsupported/uncertain regions. |
| `analyze_audio(source_or_file, range, features)` | Source-second BPM/beat/chord/instrument reports; no forced audio import or project mutation. |
| `transcribe_audio(source_or_file, options)` | Source-second note hypotheses, model provenance and optional instrument grouping. |
| `apply_analysis(result, destination, options)` | Explicit harmony/tempo edits or new note clips; preview, conflict checks and undo. |

Reports need input identity, selected range, algorithm/model version and hash, options, time
units, status, warnings and score meaning. Audio results remain in source seconds; application
maps them through clip offset, stretch, repeats and the chosen tempo map. Offer an explicit
choice to use the current grid or accept estimated tempo. Preserve tempo outside the applied
range and prevent stale mapping from shifting accepted notes.

Expose commands through `auris-toolbox` for both model frontends and through the CLI. The GPUI
frontend supplies range selection, progress/cancel, alternative chord labels, beat overlays and
note preview; human text belongs in `auris-i18n`. Export accepted notes through the session MIDI
path. Persist accepted edits using existing types where possible; keep recomputable reports in
a cache initially. If a persisted representation changes, review `Project::FORMAT_VERSION` at
that point, including conversion of chord qualities/bass to existing harmony numerals.

## Stage the work around measurable acceptance gates

Effort below is a planning estimate for one engineer familiar with the repository, not a
delivery commitment. Model packaging, dataset access and score UI are the largest uncertainties.
Stages 0–4 total roughly 5–10 engineer-weeks. Each stage is an independently reviewable unit.

| Stage | Deliverable and dependencies | Acceptance gate | Estimate |
| --- | --- | --- | --- |
| 0 | Freeze evaluation manifests, result schema and CPU baseline harness. | Dataset rights/provenance and splits recorded; silence/unknown/time semantics agreed; existing analysis behavior measured. | 2–3 days |
| 1 | Symbolic chord recognition, session command, toolbox/CLI report and harmony preview/apply. Depends on 0. | Exact clean-chord fixtures pass; changed-within-bar, bass/inversion, arpeggio and ambiguity fixtures pass; analysis changes nothing and apply undoes exactly. | 5–8 days |
| 2 | Audio-file BPM/beats and major/minor chord baseline, worker progress and timeline overlays. Depends on 0–1. | Frozen numeric DSP tests pass; real-recording benchmark published by subgroup, including half/double tempo; long-file/cancel/stale-source tests pass. | 7–12 days |
| 3 | Isolated-source pYIN/Basic Pitch comparison, selected CPU transcription path, note preview and MIDI output. Depends on 0 and time mapping from 2. | Onset/offset note F1 and CPU time/memory reported; acceptance preserves source and undo; overlap-chunk stitching creates no duplicate notes. | 5–10 days |
| 4 | YAMNet/PANNs small-model comparison, instrument-family report with multiple labels. Depends on 0; can follow 2 independently of 3. | Per-family precision/recall, macro-F1 and unknown handling measured on held-out mixtures; checkpoint/license/runtime pinned. | 4–7 days |
| 5 | Full-mixture transcription comparison on external compute. Depends on 0, 3 and the compute decision. | Better instrument-aware note results and useful correction effort on real mixes justify integration; exact artifact terms resolved. | Separate proposal |
| 6 | Rhythm/voice interpretation and MusicXML export, then staff preview if desired. Depends on accepted notes from 3 or 5. | Bars have valid durations; rests/ties/tuplets/voices survive export/import; musician review confirms readable notation. | 10–20 days for interpretation/export; staff UI estimated separately |

For stage 2, the starting performance target is analysis of a three-minute stereo file in at
most 30 seconds and at most 512 MiB of incremental analysis memory, excluding the already decoded
source. For stage 3, target at most the audio duration for CPU inference on that file and at most
1 GiB incremental memory. These are proposed budgets, not measured capabilities. Freeze the
actual CPU, thread count and settings in stage 0 and revise budgets openly if needed. Decode
memory must also be measured: bounded feature chunks do not make a whole-file decoder streaming.

### Use both controlled fixtures and held-out recordings

Create clean symbolic fixtures independent of the composer: all twelve roots, supported
qualities, inversions, octave doubling, sustained and rolled chords, passing notes, modulation,
meter changes, overlapping clips, rests and ambiguous voicings. Synthesized audio makes tempo
and note ground truth exact; use more than one sound source and hold out timbres. An Auris-only
render benchmark would overstate performance on recordings made elsewhere.

Start a real-audio evaluation with 30–50 short, manually reviewed excerpts covering isolated
instruments, small ensembles and dense mixes. Keep development and evaluation songs/artists
separate. Pin excerpt boundaries, reference annotations, source hashes and versions. This is
a pilot for failure discovery, not enough data to claim broad state-of-the-art performance.

| Resource/metric | Role and caveat |
| --- | --- |
| [Isophonics annotations](https://isophonics.net/content/reference-annotations.html) | Human chord/beat references for real recordings; obtain corresponding audio separately under suitable terms and verify alignment. Annotation availability is not audio availability. |
| [Slakh2100](https://zenodo.org/records/4599666) | Aligned synthetic mixtures, stems, instrument labels and MIDI. Use a held-out subset for instrument-aware transcription; real-recording results must be separate. The published archive is about 105 GB, so start with selected accessible excerpts rather than downloading the full corpus. |
| [MAESTRO](https://magenta.tensorflow.org/datasets/maestro) | Aligned piano audio/MIDI for piano-specific transcription. Published under CC BY-NC-SA 4.0; evaluate suitability before using it. It cannot establish multi-instrument performance. |
| [Chord metrics](https://mir-eval.readthedocs.io/latest/api/chord.html) | Duration-weighted root, major/minor, seventh and inversion scores; segmentation and coverage. Score no-chord and unknown separately; do not hide abstentions in a high accuracy on a tiny accepted subset. |
| [Transcription metrics](https://mir-eval.readthedocs.io/latest/api/transcription.html) | Note precision/recall/F1 with onset-only and onset-plus-offset matching. Use 50 ms onset, 50-cent pitch and offset tolerance max(50 ms, 20% of reference duration), and record these settings. Also require instrument agreement for the instrument-aware score. |

For tempo report strict accuracy within 4%, an explicitly separate metrical-alternative score,
beat F1 within 70 ms and continuity; test downbeats separately. For tagging report per-family
precision/recall, macro-F1 and mAP with class thresholds chosen on development data. Measure
correction time on a small blind human review: note F1 alone does not measure an editable score's
usefulness. Report real-time factor (wall seconds/audio seconds), cold/warm startup, peak RAM,
VRAM if used, failure/cancel behavior and audio length for every backend.

Set real-audio release thresholds after measuring the frozen baselines in stage 0, before
tuning candidate systems. Do not invent an accuracy promise from scores measured on a different
dataset or chord vocabulary. Check whether a pretrained model saw each evaluation dataset;
its nominal test split alone does not prove an unseen evaluation.

When code work begins, run numeric unit tests plus session and GPUI harness tests, then
`cargo fmt --all`, `cargo test --workspace`, `cargo clippy --workspace --all-targets` and doc checks.
Any Python evaluation tool uses a separate `uv` environment with `pytest`/`ruff`; downloading
models or research datasets must not become a normal Rust test requirement.

## Research method and remaining decisions

This is a targeted review of primary papers, author-maintained implementations, runtime
documentation and the existing Rust code, covering classical baselines and deployable learned
systems. Publication dates come from papers, not search-engine crawl timestamps. The initial
research stage downloaded no weights or evaluation corpora. Subsequent CPU implementations
and optional model trials are recorded in [the user guide](music-analysis.md) and
[local model measurements](music-analysis-models.md), including the separate MuScriptor
noncommercial opt-in.
This is not an exhaustive literature review or a ranking of methods across incompatible tests.

Before stage 0 ends, settle the first target genres, the vocabulary for instrument families,
the CPU performance machine, and whether the first score deliverable ends at MIDI or includes
MusicXML. The proposed defaults are tonal popular music, broad families, CPU-first deployment
and editable MIDI followed by MusicXML. Cloud execution and any training remain separate
decisions in the compute assessment. The implemented CPU baseline and local model drafts
now need broader real-recording evaluation; the commercial multi-instrument backend remains
a separate artifact and runtime evaluation.
