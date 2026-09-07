# Music analysis

Auris can estimate chords from written notes, estimate constant tempo and major/minor chords
from audio, and turn isolated monophonic audio into an editable note draft. These operations
use Rust signal processing and fixed templates on the CPU. They load no learned weights and
require no GPU. The research and longer-term evaluation plan is in
[music-analysis-plan.md](music-analysis-plan.md).

Optional CPU models now add instrument-presence tagging and multi-instrument note drafts.
See [local model setup, license notices and measured limitations](music-analysis-models.md).
MuScriptor is a separate noncommercial option requiring acknowledgement on every invocation.

## Use the desktop controls

The inspector's **Analyze Chords across Tracks** button analyzes the project. A note track's
context menu offers **Analyze Written Chords** for that track. Known percussion and muted
material are excluded. The default resolution is one quarter-note beat.

An audio clip's context menu offers **Analyze Audio BPM / Chords** and **Transcribe Isolated
Melody**. Import a file normally to use these controls. Analysis reads one pass of the trimmed
source before effects and time stretching. A background worker reports progress and can be
cancelled from the inspector. The existing synchronous decode/resample steps finish before
cancellation is observed; the analysis itself checks between frames.

**View Analysis Draft** shows chord alternatives and their scores, BPM alternatives, and note
timestamps when transcription was requested. Previews show up to 256 intervals/notes; JSON
output contains the full result. Audio preview times are original source seconds, including
the clip's trim offset. Written-note times are zero-based quarter-note beats.

Nothing changes until an acceptance control is used:

* **Apply Recognized Chords** replaces recognized intervals and clears silent intervals in the
  harmony timeline. Unknown intervals and harmony outside the analysis range are preserved.
  Symbolic candidates can also be accepted individually, including ambiguous alternatives.
* **Add Draft as New Note Track** adds a new instrument track and an editable MIDI clip. Placement follows
  the source clip's trim, stretch, repeats and the project's tempo map. The original audio stays
  intact. Raw timing is retained; use the existing editing/quantization tools to refine it.
* **Use as Clip Source BPM** adopts the chosen estimate as the audio clip's source tempo. It does
  not rewrite the project's tempo map. A clip following project tempo may consequently stretch.

Each acceptance is one undo step. Any document edit invalidates the report; run analysis again
after editing or accepting one action. Replacing the audio buffer also invalidates audio
reports. Accepted chords use the existing harmony representation, including inversions and
key changes; accepted notes use existing MIDI clips. Reports are transient and add no project
format fields.

## CLI and model tools

The following commands print JSON. Beat arguments are zero-based quarter-note beats, independent
of the time signature. Replace example paths with paths on your machine.

```sh
auris analyze-chords song.auris --track Piano --from-beat 0 --to-beat 32 --window-beats 1
auris analyze-chords song.auris --apply
auris analyze-audio recording.wav
auris transcribe-audio melody.wav
auris transcribe-audio melody.wav --midi new-draft.mid
auris transcribe-audio melody.wav --project song.auris --apply --at-beat 8 --name Melody
```

`--track` also accepts `id:number`; omitting it selects all pitched note tracks. `--apply` is
required to change an existing project, which is then saved with a checkpoint. MIDI export
contains only the new transcription, starting at beat zero, and refuses an existing destination.
`--at-beat` applies to project insertion. File analysis needs no project, audio device or import.

MCP and agent tools expose `analyze_chords`, `analyze_audio` and `transcribe_audio` through the
same session commands. Their argument schemas document `apply`, beat positions and output
paths. Scores measure template agreement or periodicity, **not calibrated probabilities**.

## Algorithms and practical limits

Written-note analysis accumulates duration-weighted pitch classes on a fixed grid, caps octave
and unison doubling, and considers the sustained bass. It searches all twelve roots for major,
minor, diminished, augmented, suspended, seventh and sixth templates. A small dynamic-programming
transition penalty discourages unnecessary changes. The result retains four alternatives per
window. Too few pitch classes, weak agreement or tied interpretations remain unknown. This is
harmonic labeling, not key detection or a complete functional analysis. Grid boundaries limit
change timing, and brief passing tones can still change a reading.

Audio is resampled to 11,025 Hz using the existing band-limited resampler. A 2,048-sample FFT and
roughly 20 ms hop extract spectral peaks. Channel magnitudes are pooled after transformation,
so opposite-phase stereo does not cancel. Half-second pitch-class windows are compared against
major/minor templates. Equal temperament at A4 = 440 Hz is assumed. Harmonics, percussion,
detuning and dense mixtures can confuse these estimates; there is no source separation.

Tempo uses positive level change and spectral flux, onset autocorrelation over 40–240 BPM,
and dynamic-programming beat alignment. Up to three tempo candidates expose competing pulse
rates. A short or nonperiodic signal may have no estimate. The report estimates one constant
tempo; beat timestamps do not assert meter, downbeats or tempo automation.

Transcription retains several continuous YIN pitch candidates per frame, then uses a fixed-cost
Viterbi decoder to stabilize note identity across vibrato and brief competing estimates. The
compared signal windows are centered on their reported timestamp. Level gating preserves rests;
pitch changes and level attacks split note events. It assumes one isolated pitched
voice at a time, approximately 65–1,000 Hz. Events shorter than about 60 ms are filtered out.
The strongest channel supplies pitch at each frame. Reverb, overlapping notes, breath/noise,
wide or slow pitch bends and strong overtones can produce missed or spurious notes. Estimated relative level
is mapped to note velocity; it does not recover the original MIDI performance.

Requests are limited to thirty minutes/eight audio channels, 16,384 chord intervals and 200,000
notes. The backend stores low-rate features, not a full spectrogram. The current file decoder
still loads the complete file before checking the analysis-duration limit, and a clip job copies
its trim before resampling. Long high-rate files can therefore use substantially more memory
than the feature arrays alone. There is no claim of a streaming memory ceiling.

## Reproduce the baseline

```sh
cargo test -p auris-analysis
cargo test -p auris-session --lib recognition
cargo test -p auris-gpui --bins music_analysis
cargo run -p auris-analysis --release --example measure_analysis
```

Fixtures are generated from explicit note lists, silence, sine tones, chords and pulse timings;
they require no corpus, checkpoint or network. Numeric tests cover twelve-root chord spelling,
ambiguity/silence, loops and inversion, 72/120/180 BPM pulses, beat alignment, known pitches and
note boundaries, cancellation and invalid samples, and opposite-phase stereo. Session tests
cover key/tempo mapping, trim/stretch/repeats, stale reports and exact undo. Window tests drive
acceptance buttons through the headless application.

The measurement example uses a deterministic three-minute stereo 120 BPM A4 pulse fixture
and prints elapsed time, real-time factor, tempo alternatives and note counts. It measures the
prepared-audio backend, excluding file decoding and resampling. These synthetic checks establish
regression behavior; they do not establish accuracy on real recordings. The held-out recording
benchmark and subgroup metrics in the research plan remain the quality gate for broader claims.

On 2026-09-07, Windows x86_64, AMD Ryzen 7 9800X3D, release build, one sequential analysis
worker, the example measured 0.206 s for tempo/chords and 0.381 s including transcription
(real-time factors 0.0011 and 0.0021). The leading BPM estimate was 120.127; transcription
returned 360 notes, all A4, matching the 360 generated pulses. This is one warm-machine run
on a simple prepared fixture, not a real-recording benchmark. Peak process memory was not
measured.

The follow-up CPU transcription evaluation, reproducible before/after results, and current
timing/memory measurements are in [music-analysis-evaluation.md](music-analysis-evaluation.md).
