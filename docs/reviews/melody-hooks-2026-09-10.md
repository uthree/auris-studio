# Instrumental melody rhythm candidate — 2026-09-10

The listener preferred the preceding change's drums and accompaniment, but still found
the melody difficult to groove to and insufficiently catchy. This iteration changes the
instrumental melody and provides auditions over that accepted backing. It is a candidate
for listening review, **not an established improvement**: Audiobox enjoyment fell by
0.1153 on average, with the largest losses in Pop-band and Synthwave. Its quality prediction
rose by 0.0091. Neither result measures whether a hook stays in a listener's memory.

## Diagnosis and change

Saved chorus figures in several seed-101 scores had attacks at sixteenth-grid positions
`[0, 1, 15]`, each only one step long. Independently sampling attacks, then filling the
strongest free steps to meet the minimum note count, could leave the middle of a bar empty.
Reusing the figure repeated that shape. Conversely, Rock seed 103 had an eleven-attack
chorus bar. Truncating a final note could not create a beat of breath when its onset was
already on the last subdivision. A variation operation applied to the complete figure
could also reverse or remove the identifying opening before the tail was developed.

The automatic instrumental writer now:

- Combines a short gesture and held target in two-beat cells, with style-dependent holds,
  pickups and anticipations. Density subdivides a gesture; syncopation can delay a
  preparatory attack while retaining a held target.
- Preserves the first half of a figure during continuation and answer variations, and
  develops the tail. Cadences can shorten the figure to make room for its arrival.
- Plans the closing arrival before the final felt beat, retaining the answer's final
  contour degree. The meter supplies the beat, including compound meters; a bar no longer
  than one felt beat has no such reserved rest.

Pitch-contour sampling is separate from the section rhythm. Pitch-joining and contour
dressing rules are unchanged. Explicit rhythm patterns retain their authored onsets.
The changes write ordinary editable notes and regeneration recipes. This iteration
measures instrumental leads; the lyric/singer writer is unchanged.

## Controlled comparison

The baseline is the **after** side of the preceding composition experiment: saved scores
from `fdf81be`, documented at `e8b5124`. It is not that experiment's original baseline.
Nine presets use seeds 101 and 103, for 18 paired pieces.

Fresh accompaniment can react to melody density. Therefore `tools/eval/melody_ab.py`
transplants only the candidate's complete lead note arrays and matching recipe digest into
each baseline project. All other source content, including harmony, IDs, instruments,
backing notes, mixer, effects and performance transforms, is retained. Source/candidate
layout and musical context must agree. These saved auditions remain editable in Auris.
Shared processing can react to the new melody, so this fixes the backing score and mixer,
not the backing's post-processing waveform in isolation.

Both sides use the same sound sources and full-length, 48 kHz stereo float WAV rendering
with no effect tail. There is no gain normalization. The SoundFont is
`MuseScore_General.sf2`, SHA-256
`ee51d2c4b1525e70f19a45909c4fd7a2e26d91d115fa89dbf5a6bc413d8b9bf3`.
The rendering executable is archived locally as `target/melody-hooks/after-auris.exe`;
its hash matches all 18 render manifests. Subsequent test builds replace the debug binary.

Audiobox was rerun on all baseline WAVs before the writer changed, then on all candidate
WAVs. The existing CLAP baseline was reused after checking every WAV hash. Native CLAP
uses the same music/AudioSet HTSAT-base checkpoint, frozen prompt manifest, three evenly
spaced ten-second windows, CPU inference and four threads as the preceding experiment.
Model hashes, package versions, per-file results and raw-report hashes are retained in
[the accompanying JSON](melody-hooks-2026-09-10.json). Neither model composes audio.

## Measurements

All means below give each piece equal weight; per-style rows average two seeds.

| Measurement | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| Audiobox content enjoyment, CE | 6.8923 | 6.7771 | -0.1153 |
| Audiobox content usefulness, CU | 7.7039 | 7.6980 | -0.0059 |
| Audiobox production complexity, PC | 5.1046 | 4.9389 | -0.1657 |
| Audiobox production quality, PQ | 7.8578 | 7.8669 | +0.0091 |
| CLAP positive cosine | 0.43219 | 0.43077 | -0.00143 |
| CLAP positive-minus-contrast margin | 0.35977 | 0.36132 | +0.00154 |

| Style | CE change | PQ change | CLAP positive change |
| --- | ---: | ---: | ---: |
| Rock | -0.0200 | +0.1295 | +0.00007 |
| City-pop | -0.0680 | -0.0550 | -0.00845 |
| Chiptune | -0.1035 | +0.0980 | -0.00349 |
| Pop-band | -0.3265 | -0.0510 | -0.00412 |
| Jazz-trio | -0.0515 | +0.0475 | -0.01169 |
| Game-loop | -0.1690 | -0.0185 | +0.02034 |
| Synthwave | -0.2845 | -0.0555 | -0.00495 |
| Orchestral | -0.0135 | -0.0060 | -0.00286 |
| Ambient | -0.0010 | -0.0075 | +0.00232 |

A separate descriptive timing probe reads active generated lead clips in these saved
3/4 and 4/4 scores. It excludes unrecipe'd endings and does not apply performance
transforms. A sixteenth is 240 ticks; a quarter is 960. A hollow bar has sounding notes
but no note overlapping its middle half. Terminal rest is the distance from the last
note end to its clip boundary, in quarter-note beats. Fractions are computed per project,
then averaged; they are not objectives to maximize.

| Written melody timing | Before | Candidate |
| --- | ---: | ---: |
| Sixteenth-or-shorter notes | 40.38% | 34.59% |
| Quarter-or-longer notes | 26.12% | 42.98% |
| Active bars with an empty middle half | 3.83% | 0.00% |
| Mean terminal rest, beats | 0.496 | 0.995 |
| Clip endings with at least 0.75 beat of rest | 39.57% | 100.00% |

The intended holds and breaths are present, and the particular hollow-bar failure is
absent from this probe. This does not establish a memorable tune. Falling enjoyment and
complexity predictions suggest examining whether the regularized gestures remove too
much musical interest, especially in Pop-band and Synthwave; that is a hypothesis, not
a causal conclusion. Two seeds per style and models that do not measure hook recall are
insufficient for a general claim. Human listening preference is still pending.

The existing eight-seed symbolic ruler and saved-score structural evaluator were also
run before/after. Their raw outputs are retained under `target/melody-hooks`. All existing
pitch interval, density, syncopation and variation guards pass without threshold changes.

## Listening files

These local seed-101 excerpts use exactly the same first chorus on both sides, with only
a five-millisecond fade at each cut edge. Models scored the complete unmodified WAVs,
not these excerpts. **Before** already includes the preferred backing from the previous
iteration; **candidate** replaces its instrumental melody.

| Style | Length | Before | Candidate |
| --- | ---: | --- | --- |
| Rock | 12.97 s | [WAV](../../target/melody-hooks/iteration-1/excerpts/rock-s101-before-chorus.wav) | [WAV](../../target/melody-hooks/iteration-1/excerpts/rock-s101-after-chorus.wav) |
| City-pop | 18.11 s | [WAV](../../target/melody-hooks/iteration-1/excerpts/city-pop-s101-before-chorus.wav) | [WAV](../../target/melody-hooks/iteration-1/excerpts/city-pop-s101-after-chorus.wav) |
| Chiptune | 13.71 s | [WAV](../../target/melody-hooks/iteration-1/excerpts/chiptune-s101-before-chorus.wav) | [WAV](../../target/melody-hooks/iteration-1/excerpts/chiptune-s101-after-chorus.wav) |
| Pop-band | 15.48 s | [WAV](../../target/melody-hooks/iteration-1/excerpts/pop-band-s101-before-chorus.wav) | [WAV](../../target/melody-hooks/iteration-1/excerpts/pop-band-s101-after-chorus.wav) |
| Synthwave | 17.14 s | [WAV](../../target/melody-hooks/iteration-1/excerpts/synthwave-s101-before-chorus.wav) | [WAV](../../target/melody-hooks/iteration-1/excerpts/synthwave-s101-after-chorus.wav) |

Complete baseline WAVs/projects remain in `target/composition-eval/after`. Complete
candidate auditions are in `target/melody-hooks/iteration-1/audio`; their editable
projects and per-pair manifests are in `iteration-1/fixed-backing/<preset>-s<seed>/`.
These audio/model assets are local and ignored by Git.

## Reproduction and verification

Use the same assets, seeds and saved baseline. For each candidate, compose through the
new CLI, then retain the baseline backing with the comparison tool:

```sh
auris compose --preset rock --seed 101 -o candidates/rock-s101.auris
uv run tools/eval/melody_ab.py --source before/rock-s101/rock-s101.auris --candidate candidates/rock-s101/rock-s101.auris --output fixed/rock-s101/rock-s101.auris --cli path/to/auris --wav audio/rock-s101.wav
uv run tools/eval/aesthetics.py audio --baseline before-aesthetics.json --json after-aesthetics.json
uv run tools/eval/clap.py audio --baseline before-clap.json --json after-clap.json
uv run tools/eval/structure.py fixed --baseline before-structure.json --json after-structure.json
cargo run -p auris-compose --example measure
```

Independent artifact verification checked all 36 complete WAVs, 54 projects and ten
excerpts: hashes agree, paired duration/rate/channels match, every candidate melody
changed, and all non-melody source content is preserved. All audio is finite, non-silent
and below full scale. Each excerpt exactly matches its declared source slice and fade.
The detailed audit is `target/melody-hooks/iteration-1/integrity-audit.json`.

Seven added composer tests cover held targets, protected openings, phrase rests, retained
arrival pitch, authored late attacks, density and 7,776 meter/style/seed combinations.
The controlled-comparison tool has 19 tests; all 41 evaluation-tool tests pass and Ruff
passes. Full workspace tests pass (3,270 passed, eight ignored), as do `cargo fmt --all
--check`, workspace/all-targets Clippy with `-D warnings`, and `git diff --check`.
