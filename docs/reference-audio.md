# Match a reference recording

**Compose → Adjust by Audio Evaluation…** searches the current project's mix, performance,
generated clip seeds, instrument choices, and non-destructive arrangement. Every candidate is
rendered through the project's instruments, routing and effects, then compared with an excerpt
from a reference recording. The unchanged project is the first candidate and remains the result
when no measured improvement is found. Nothing edits the project until **Apply Best**.

## Use the desktop app

1. Open or compose the project you want to adjust, then choose **Compose → Adjust by Audio Evaluation…**.
2. Select **Reference · audio features** and choose a reference audio file. Set its excerpt start and the project excerpt start separately;
   **Use Playhead** selects the current project position. Choose a length of 1–30 seconds that
   contains the sound you want to compare.
3. Select the search scopes: **Adjust Mix**, **Adjust Performance**, **Explore Generation Seeds**,
   **Explore Instruments**, and **Adjust Arrangement**. Enable at least one. Set the attempt limit
   and search seed. The default run measures 32 candidates, including the unchanged baseline;
   the desktop offers budgets from 8 to 512 candidates. Broader scopes benefit from a larger
   budget, at the cost of more rendering and evaluation time.
4. Choose **Render & Search**. Progress reports the actual candidate render and completed
   evaluations. The project remains editable and unchanged by the search.
5. Compare **Before** and **Best**, including the individual feature distances. Use **Play
   Reference**, **Play Before**, and **Play Best** to listen, and **Stop Preview** to stop audition.
6. Choose **Apply Best** to keep the retained changes. One Undo restores the original state.

The reference and project excerpts need not have the same starting position in their respective
songs. The comparison summarizes their sound; it does not align their notes, beat grids, or
waveforms. Choose representative passages, such as a chorus against a chorus, to make the
comparison useful.

Cancel preserves an already evaluated partial winner. Cancellation before a complete baseline
leaves no comparison to adopt. Editing the project or changing the reference or settings makes
the old comparison stale. Search again before applying. Closing the panel cancels its task;
another run waits for the old worker to return.

The baseline must render and evaluate successfully. After that, an unusable candidate, such as
a silent SoundFont selection, consumes one attempt and is rejected while the search keeps its
best valid result. The completed comparison shows the number of rejected candidates when any
occurred. Missing render dependencies and a changed project still stop the pass explicitly.

## What can change

Mix proposals adjust unmuted non-bus tracks. A gain or pan parameter with an automation lane is
left to its automation. Performance proposals adjust the expression or gate of nonempty,
unmuted instrument and drum clips. Recorded audio and already synthesized singing remain part
of the rendered mix and can have their track balance adjusted.

Generation-seed proposals regenerate generated clips using their existing specifications with a
different seed. Manually written and frozen clips retain their notes. Applying a seed winner
adopts the exact generated notes that were evaluated, without rerunning composition at adoption.
Instrument proposals consider unmuted registry instrument tracks with clips. Melodic alternatives
include built-in Chiptune, FM2 and Vocal sounds, several Chiptune/FM2 patches, and every loaded
SoundFont preset outside percussion bank 128. Supported drum tracks and percussion sources
instead use loaded bank-128 presets; drum maps and recipe voice assignments are retained. Hosted
instruments and NoiseDrum keep their current source. Each track's seeded alternative order visits
all its options before repeating, and skips the baseline-equivalent selection.

Scope enumeration respects track mute and solo. Generated-seed and arrangement proposals skip
clips that start at or after the selected excerpt's end. Earlier clips and instrument sources
remain eligible because release envelopes and routed effect tails can still contribute to the
excerpt. Performance proposals also keep the captured clip selection. The excerpt limits what
is measured; adopting track-wide mix or sound
changes can also affect the rest of the song.

Changing the SoundFont preset on an existing sampler retains its player parameters and
automation. Replacing a built-in sound or instrument source removes that instrument's automation
lanes, whose parameter meanings may differ on the new source; track mix automation is retained.
Arrangement proposals use editable performance transforms and preserve written notes.

The proposal order is seeded and covers the enabled scopes. Continuous-control bounds stay
relative to the original project, including after several accepted improvements. The existing
mix and expression adjustments use the following bounds:

| Control | Maximum change from the original | Proposal step |
| --- | ---: | ---: |
| Track gain | 3 dB | 1.5 dB |
| Track pan | 0.3 | 0.15 |
| Timing wander | 0.2 | 0.1 |
| Velocity wander | 0.2 | 0.1 |
| Phrase swell | 0.2 | 0.1 |
| Beat/offbeat accent | 0.3 | 0.15 |
| Performance delay | 8 ms | 4 ms |
| Note gate | 0.1 | 0.05 |

Controls also obey their normal parameter ranges. Expression and gate are stored as editable
performance transforms; they do not rewrite the score. The exact winning project changes are
applied without a subsequent automatic balance pass that would replace the searched gains.

Arrangement controls are proposed per clip, with the same original-project bounds:

| Control | Maximum change from the original or available choices |
| --- | --- |
| Swing | 12 percentage points, in 4-point steps within 50–75% |
| Ghost-note density / velocity | 0.45 / 0.2 |
| Ghost-note length / variation | 30 ms within 1–100 ms / 0.4 |
| Ghost placement | Pickup, sixteenths, offbeats, or repeating |
| Mute / slide amount | 0.45 each |
| Chord strum spread | 24 ms within 0–100 ms |
| Strum upstroke velocity / low accent | 0.3 each |
| Strum upstroke pitches / direction | 0–4 / down, up, or alternating |
| Ensemble shared motion | 0.4, where timing or velocity wander is already active |
| Pitch scoop / vibrato depth / fall | 0.6 / 0.15 / 0.75 semitones |
| Pitch connection | 45 ms |
| Active scoop length / vibrato rate | 75 ms / 1.5 Hz |
| Active vibrato delay / fall length | 150 ms / 90 ms |

Slide and pitch gestures target monophonic material; chords and overlapping clips are excluded
from pitch-gesture proposals. Strum controls target chords. Drum arrangements preserve drum
voice identity and exclude pitch, slide and strum changes. A dormant ghost-note stage may be
activated softly so that a placement or variation proposal can be heard; the comparison lists
that activation with the other changes. Written notes, recipe settings, and existing bend and
controller curves remain intact under arrangement proposals.

## What the distance measures

The acoustic evaluator uses deterministic CPU measurements without model downloads. Reference
features are captured once and remain fixed for the entire run. Candidate audio is rendered at
44.1 kHz, and the reference and candidate use the same analysis method.

| Component | Measurement | Weight |
| --- | --- | ---: |
| Frequency balance | Relative energy in 24 logarithmic bands from 40 Hz to 12 kHz; Hellinger distance squared between the distributions | 45% |
| Dynamics | Quantiles of the 50 ms RMS envelope relative to the whole excerpt, plus crest factor; differences use a fixed 24 dB scale | 25% |
| Stereo image | Overall and short-window channel balance and side-energy statistics | 15% |
| Rhythmic texture | Quantiles and activity of normalized positive spectral flux, measured with a 10 ms hop | 15% |

Each component and the weighted total lie in `[0, 1]`. **Lower distance means closer measured
features.** Zero means the measured summaries agree, not that the recordings are identical. The
optimizer maximizes `fitness = -round(distance * 1_000_000) / 1_000_000` and retains the earlier
candidate on ties. This rounds ranking distance to the nearest 0.000001 so tiny floating-point
differences from shared gain do not count as improvements. All diagnostic distances remain
unrounded. This precision applies only to the reference evaluator; other objectives define their
own fitness units and precision.

Analysis normalizes shared gain before measuring features. Turning the whole recording up or
down therefore does not improve its similarity. Stereo channel powers are combined after the
FFT, so opposite-phase stereo is not mistaken for silence. Silence, non-finite samples, and
excerpts too short to measure are rejected rather than assigned a successful score.

These summaries describe tonal balance, envelope, spatial balance and transient activity. They
are useful for searching a particular acoustic direction, but are not judgments of musical
quality, semantic mood, melody, or arrangement. Their distributions also do not reproduce the
reference's rhythmic sequence. A nearby result can still differ audibly: listen to the retained
renders before deciding to apply it.

## Audition and adoption

The baseline and best buffers retain the exact PCM that was evaluated. Audition plays those
buffers directly at the output, bypassing the current project graph so its processing is not
applied twice. Starting audition stops song playback and clears its effect tails without moving
the playhead or editing the document.

Preview preparation resamples to the current output device on a worker, then applies one uniform
gain toward -20 dBFS RMS while holding sample peaks below -1 dBFS. A peak-limited excerpt may
remain below that RMS target. This changes only the audition copy, not the stored render or
project. Applying the winner keeps the evaluated settings and any regenerated notes in one undo
step.

## Replaceable audio objectives

The session layer separates the search from its evaluator:

```rust
pub trait AudioEvaluator: Send + Sync {
    fn evaluate(&self, audio: &AudioBuffer) -> Result<AudioEvaluation, String>;
    fn description(&self) -> String;
}
```

`AudioEvaluation` carries a finite, higher-is-better `fitness` and named `AudioMetric` values.
`validate()` rejects non-finite fitness or diagnostics before ranking. A caller constructs
`ReferenceAudioEvaluator::new(&reference_pcm)` for the feature comparison above and passes the
evaluator to the session's staged reference-matching job. Rendering runs on a worker; the
session thread prepares each next render and explicitly adopts the completed report.

[`ClapAudioEvaluator`](clap-evaluation.md) implements the same trait with a local ONNX model
and text or reference embedding held fixed for the search. The GUI offers both CLAP objectives
alongside the acoustic comparison. CLAP reports cosine similarity, with higher values closer
to the chosen target; the acoustic objective continues to report lower-is-closer distance.

## Reproduce a render comparison

The session examples provide a small built-in-instrument fixture and a headless search runner:

```sh
cargo run -p auris-session --example reference_match_demo -- target/reference-demo
cargo run -p auris-session --example match_reference -- target/reference-demo/Baseline/Baseline.auris target/reference-demo/reference.wav target/reference-demo/matched 32 12
```

Choose new output directories. The fixture supplies an unchanged project and a reference made
by changing pan and gate. The runner writes the exact reference, baseline and best WAVs, a JSON
metric report, and the adopted project. These files can also be opened in the desktop app to
compare the GUI result. Seeds reproduce proposal order within a build; hosted instruments or
effects with their own unpinned randomness may still render different audio between passes.
