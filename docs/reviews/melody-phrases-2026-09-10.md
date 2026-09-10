# Melodic calls and responses — 2026-09-10

The previous continuity change redistributed attacks but left the realized pitch
sequence unchanged in all eleven comparison choruses. This iteration develops a
melodic call across a phrase: state it, echo it, continue from its endpoint using
its final intervals, then recall the head while approaching an answer. The return
reserves enough travel for its remaining notes, instead of forcing a final leap.
The old variation could negate absolute scale degrees; a small target inflection
now replaces that operation. Flat calls remain valid, and the usual chord-scale,
register, tension and cadence decisions still realize the contour.

A separate rhythm pass gives a phrase's second nonclosing bar a delayed entry or
an anticipated late arrival. It moves existing notes within their harmonic event,
preserves their order, pitches and velocities, and cannot move a long final note
into the middle of a short meter. Written patterns retain their timing, and the
sustained Ambient and Orchestral palettes bypass this timing pass. Repeated phrases
reuse the same response. Neither change introduces a second archived composer or
a model dependency into the product. All generated notes remain editable.

## Conditions and provenance

The fixed baseline is the previous continuity experiment's completed `after`
corpus. The eleven instrumental cases include Pop-band 102, Rock 105 and City-pop
102 as diagnostics, Pop-band 105 and Rock 107 as references, and seeds 201/202 in
each genre as historical regression cases. Those six cases were held out for the
previous experiment; they are not new held-outs here. A separate score-only check
uses seeds 301–308 in all three genres, first examined after the implementation
was frozen. No candidate is selected by a learned score.

The four audio conditions are baseline, pitch development, rhythm development and
both together. Separate archived CLIs generate whole candidate lead arrays. Only
those arrays and their recipe digests replace the lead in the accepted projects;
the saved backing, mixer, performance, harmony and IDs remain fixed. Every audio
condition uses the same old renderer and SoundFont. Shared effects can still react
to a changed melody. The comparison tool never manufactures a condition by copying
individual pitches onto an unrelated rhythm.

The pitch-writer control retains note count, onsets, velocities and other fields.
One necessary coupling is checked separately: after swing, the existing finalizer
shortens a note at the next attack of the same pitch. Changing the pitch can change
that duration. Both sides must be explained by the same pre-cut duration and their
respective same-pitch retrigger boundaries; arbitrary duration changes are rejected.
Every such exception is recorded. The rhythm control retains the full ordered
pitch/velocity sequence and every field except onset and duration.
The retrigger exception was added after score-only preflight exposed this existing
finalizer behavior, before any candidate audio or learned score was produced. The
frozen musical implementation and the original baseline files were not changed.

Each listening passage is the first eight-bar 4/4 chorus, stereo 48 kHz float WAV,
with 5 ms fades and linear gain to -23 integrated LUFS. Audiobox measures these
same excerpts; CLAP measures their centered ten seconds, using the frozen genre
and instrumentation prompts and `--segments 1`. Before editing the writer, the
baseline model runs were repeated: all 44 Audiobox axis values matched, and CLAP's
positive cosine and contrast margin had zero maximum drift across all eleven cases.
These measurements do not establish hook recall, groove preference or whole-song
quality. No human rating is inferred or carried forward from an earlier version.

The ignored local experiment folder is `target/melody-phrases`. Its frozen plan,
variant specification, source snapshots, binaries and manifests retain the precise
inputs and SHA-256 hashes. The retained result files below summarize those artifacts
without putting WAVs or executables into Git.
Published result references use repository-relative paths and generic public-model
cache/tool roots; the original absolute paths remain only in local ignored manifests.

## Symbolic ruler

The standard `measure` example was run before and after. Its lead-only observations
for the three listening genres are:

| Preset | Step transitions, before → after | Mean interval, before → after | Pitch-class entropy, before → after |
| --- | ---: | ---: | ---: |
| Pop-band | 52.05% → 56.80% | 2.41 → 2.10 | 2.76 → 2.77 |
| City-pop | 52.90% → 56.28% | 2.71 → 2.28 | 2.91 → 2.90 |
| Rock | 60.21% → 62.98% | 2.70 → 2.39 | 2.73 → 2.74 |

These describe smoother local movement, not a target distribution or a quality
score. All nonmelodic rows of the standard ruler are unchanged. Phrase signatures
and continuity diagnostics retain their separate meanings; repeated notes, long
notes and rests are not automatically defects.

The pitch condition leaves the first two bars' realized pitches unchanged in all
five diagnostic/reference cases. In Pop-band 105, the third bar changes from
`72,70,72,81` to `67,69,70,70`; Rock 105 changes from `67,66,64,67,78` to
`64,66,67,69,69`. Their descending calls now have ascending continuations, and the
largest within-bar leap across the first four bars falls from 11 to 2 and from 11
to 3 semitones, respectively. The control also retains onset/count/velocity exactly;
its only duration changes are five independently explained same-pitch retrigger
cuts in City-pop 201/202. The rhythm condition passes its exact ordered-pitch,
count and velocity controls in all eleven cases.

There are limits worth hearing. Pop-band 102's three-note fourth bar remains
`77,81,77` despite its different continuation. Rock 107 acquires a closing
`67→72` leap, while its original opening `+9/−7` excursions remain. The shared
endpoint is a contour coordinate, not a promise of identical MIDI pitches across
different chords; for example, Pop-band 105's continuation ends at 70 and its
answer begins at 74. The comparison retains these cases instead of screening them
out by a local interval rule.

## Fresh seeds

The separate [24-case score report](melody-phrases-generalization-2026-09-10.json)
retains every seed from 301 through 308. Chorus and full-lead note counts are
unchanged, pitch-interval sequences change in all 24 cases, and chorus rhythm
signatures change in 9. The continuity diagnostic finds no new early stationary
span: both versions have zero in these nonending bars.

| Preset | Mean step ratio, before → after | Mean leap ratio, before → after |
| --- | ---: | ---: |
| Rock | 53.95% → 59.49% | 14.13% → 6.78% |
| City-pop | 54.79% → 59.71% | 10.67% → 6.34% |
| Pop-band | 62.41% → 64.88% | 9.13% → 2.76% |

A step is 1–2 semitones and a leap at least 5; repeated pitches count as neither.
These are unweighted per-case means. The JSON also records pooled counts and exact
definitions, including the sixteenth-grid rhythm diagnostic and exact-tick phrase
signatures. Lower leap ratios do not imply that all expressive leaps should vanish.
City-pop 306 is the exception: its leaps increase from 1 to 4 of 43 transitions.
Across-seed uniqueness stays unchanged: chorus rhythms are distinct in 7/8 Rock
cases and 8/8 City-pop and Pop-band cases, and pitch-interval signatures are distinct
in 8/8 for each genre. Average distinct bar rhythms within a chorus increase from
3 to 3.25 (Rock/Pop-band) and 3.625 (City-pop). Exact two-/four-bar transposed-motif
matches remain zero in both versions; this strict descriptor provides no evidence
of better motif recall.

These 24 cases are ordinary complete compositions, unlike the audio comparison's
fixed-backing copies. Existing accompaniment cooperation responds to newly opened
lead space: 414 backing notes lose the previous 0.9 velocity duck. City-pop 304 adds
four intro stabs and Pop-band 308 adds eight chorus-key attacks; the other 22 cases
keep backing note count, pitches, starts and lengths. Automatic balancing changes
136 track gains by at most 0.569 dB; pan, routing and effects are unchanged. This is
the existing `arrangement.rs` cooperation and session `balance_now` behavior, not a
new accompaniment writer. Normal composition internally renders for balancing;
the score check runs no separate audio-export or learned-model job. The four-condition
listening comparison instead proves that the original backing and mixer are retained.

## Audio measurements

Open the local [four-condition listening page](../../target/melody-phrases/comparison/listening.html).
The [retained audio results](melody-phrases-audio-2026-09-10.json) include all cases,
all four Audiobox axes, both CLAP comparisons, paired deltas and artifact provenance.

| Condition | Mean CE | Mean PQ | Mean CLAP positive cosine | Mean contrast margin |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 7.3179 | 8.2368 | 0.34366 | 0.24528 |
| Pitch development | 7.3334 | 8.2336 | 0.34383 | 0.23768 |
| Rhythm development | 7.3335 | 8.2355 | 0.34549 | 0.24630 |
| Combined | 7.3478 | 8.2300 | 0.34539 | 0.23947 |

The combined mean CE difference is +0.0299, with PQ -0.0068, positive cosine
+0.00173 and contrast margin -0.00580. These small, mixed changes are not proof of
better music or statistical significance. CLAP compares style/instrumentation
descriptions, not catchiness. Audiobox predicts ratings but supplies no human
preference for this experiment. Its native JSON contains only scores, so the wrapper
records the exact scored-WAV hashes separately. The retained report records the
local cached Audiobox snapshot's weight/config hashes after scoring; its evaluator
does not embed the loaded checkpoint identity in each inference result as CLAP does.

| Preset, combined minus baseline | CE | PQ | CLAP positive cosine |
| --- | ---: | ---: | ---: |
| Rock, 4 cases | -0.0285 | -0.0423 | +0.00556 |
| City-pop, 3 cases | +0.0943 | +0.0267 | -0.00386 |
| Pop-band, 4 cases | +0.0400 | +0.0035 | +0.00209 |

CE rises in seven cases and falls in four, including both Rock diagnostic/reference
cases and Rock 202. The genre means make this disagreement visible instead of
hiding it behind the overall average.

## Validation

`cargo test --workspace --locked` passes 3,297 tests with 8 existing ignored tests;
denied-warning workspace Clippy and formatting checks pass. The composer includes
380 unit tests and 2 doctests, covering reachable returns, flat/short calls, recurring
responses, authored rhythms, meter/subdivision boundaries and note-preservation
constraints. The four whole-composition snapshots retain harmony and note counts
and record their intentionally changed note digests.
The final Python regression subset passes 132 tests; Ruff passes for all four new
evaluation scripts and tests.

An independent artifact audit passes 4,422 checks across 297 hashed files, including
all 44 saved projects, full renders, crops and normalized excerpts. It recomputes the
stored PCM transformations, verifies complete melody replacement and preserved
backing, checks all model-input hashes, and matches final production sources to
the frozen combined snapshot. The corpus runner did not capture helper hashes at
run start; current helper hashes and the independently recomputed transformations
are recorded without claiming historical helper immutability.

The local browser check decodes and starts all 44 audio files, verifies exclusive
playback, genre/metric controls, all 132 audio/project links, 44 finite model rows,
and no horizontal overflow at desktop and narrow widths: 58 checks pass. Output is
muted during this automated check; it is not a listening judgment. The dedicated
preview server binds only to `127.0.0.1`, and the earlier comparison servers remain
available.

## Reproduction

Build the baseline and each explicitly isolated composer revision before changing
its source, and save the executable hash and source snapshot in `variants.json`.
The final application contains the combined implementation. The input schema and
control checks are documented in [the evaluation guide](../evaluation.md#four-condition-melody-comparisons).

```sh
cargo run -p auris-compose --example measure --locked
uv run tools/eval/melody_phrase_ab.py --baseline target/melody-continuity/after/manifest.json --variants target/melody-phrases/variants.json --out target/melody-phrases/comparison
uv run tools/eval/aesthetics.py target/melody-phrases/comparison/combined/excerpts --baseline target/melody-phrases/before-aesthetics.json --json target/melody-phrases/comparison/combined/aesthetics.json
uv run tools/eval/clap.py target/melody-phrases/comparison/combined/excerpts --segments 1 --threads 4 --baseline target/melody-phrases/before-clap.json --json target/melody-phrases/comparison/combined/clap.json
uv run tools/eval/melody_phrase_listening.py --manifest target/melody-phrases/comparison/manifest.json --scores target/melody-phrases/scores.json --output target/melody-phrases/comparison/listening.html
```

Repeat both model commands for the pitch and rhythm conditions, retaining every
case and comparing the exact same excerpt files. The output directory must be new.
