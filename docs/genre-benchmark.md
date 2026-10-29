# Genre benchmark for agent composition

This benchmark is a development harness for comparing composition guidance. It
does not claim that a model score is a musical verdict.

The versioned manifest contains fourteen instrumental cases: one development
case and one holdout case for each of pop, city pop, lo-fi, neo-soul, dance and
electronic, anime song, and game BGM. Every case is sixteen bars with an
explicit key, tempo, 4/4 meter, and A/B sections covering bars 1–8 and 9–16.
The cases use built-in instruments and leave the project unsaved. The holdout
briefs request another take with different register and accents while keeping
the development tempo, key and genre purpose. They test a held-back phrasing of
the request, not generalization to different tempos, keys or song lengths.

## Paired experiment

Run the existing composition-quality runner with a separately frozen binary for
each condition. Keep the same backend, model, manifest, repetition count,
candidate-tool setting, prompt profile, and context budget. Record the source
suite and any prompt condition so the exact instruction package can be
reconstructed.

```powershell
uv run tools/eval/composition_quality.py run `
  --manifest tools/eval/references/genre-benchmark-v1.json `
  --executable <frozen-before-test-executable> `
  --source-suite <before-source-suite.json> --prompt-condition <before-condition.json> `
  --output <before-development-suite> --split development --repetitions 1

uv run tools/eval/composition_quality.py run `
  --manifest tools/eval/references/genre-benchmark-v1.json `
  --executable <frozen-after-test-executable> `
  --source-suite <after-source-suite.json> --prompt-condition <after-condition.json> `
  --output <after-development-suite> --split development --repetitions 1
```

Repeat those commands for `--split holdout`, reusing each condition's frozen
binary, source suite and prompt condition after development decisions are fixed. Retain
the WAV/MIDI files produced by the agent. Score
the retained WAVs with the pinned local SongEval adapter and its frozen
checkpoint, MuQ weights, upstream revision, and runtime configuration.

Build input manifests with the trial IDs preserved, then use the separately
managed evaluator interpreter described in [composition refinement](composition-refinement.md):

```powershell
uv run tools/eval/genre_benchmark.py inputs --suite <before-development-suite> --suite <before-holdout-suite> --output <before-inputs.json>
<songeval-python> tools/eval/songeval.py --config <songeval-config.json> --manifest <before-inputs.json> --output <before-songeval.json>
```

Repeat for the after suites. Missing WAVs remain listed in the input manifest;
they are not substituted with another take. Comparison requires the complete
expected cohort even when some inputs could not be scored.

Finally compare the complete cohorts:

```powershell
uv run tools/eval/genre_benchmark.py compare `
  --before-suite <before-development-suite> `
  --after-suite <after-development-suite> `
  --before-suite <before-holdout-suite> `
  --after-suite <after-holdout-suite> `
  --before-scores <before-songeval.json> `
  --after-scores <after-songeval.json> `
  --manifest tools/eval/references/genre-benchmark-v1.json `
  --output <comparison.json>
```

The comparison freezes and checks suite conditions, source/runtime metadata,
SongEval scores, artifact hashes, and hashes of the files that are actually on
disk. A missing, failed, duplicated, changed, or invalid trial remains visible
and makes the cohort ineligible; it is never silently removed.
Successful deliveries require intact WAV and MIDI artifacts, and encoded-audio
measurements must identify that same WAV. Input and comparison paths cannot
escape their trial folders or follow linked trials/artifacts. Each condition
requires recorded agent runtime metadata; before and after binaries may differ
because prompt or implementation changes are the treatment.

## Adoption rule

### Add independent repetitions

Keep the first pilot and run another complete genre cohort with the same frozen
executable, source suite, model and prompt condition. Set `--repetition-start 2
--repetitions 1` to produce `*-02` trial IDs in a fresh output directory. This is
a new independent take, not a retry selected to replace a low score.

Combine the retained suites before comparison:

```powershell
uv run tools/eval/composition_quality.py combine-repetitions `
  --input <pilot-suite> --input <second-repetition-suite> --output <combined-suite>
```

The combination requires identical frozen conditions and complete, disjoint,
contiguous repetition IDs starting at one. It verifies recorded artifact hashes,
copies only recorded trial artifacts, and retains failed trials. Seeded-project
suites are currently unsupported. The combined directory is a comparison bundle;
the original suites retain the executable snapshots.

A runner revision mismatch is rejected by default. After reviewing a deliberate
harness-only change, `--runner-change-reason "<specific reviewed change>"` records
that exception with each input runner hash. This never permits a changed agent
binary, prompt, model or musical brief within one condition. Build a new SongEval
input manifest from the combined suite and score those retained files with the
same pinned evaluator. Repeating the evaluator does not create new musical takes.

### Screening and confirmation

The selection policy was amended before holdout execution: **overall mean
improvement has priority**. The policy identifier is
`overall-mean-priority-v2`. The development cohort is a screening gate: a
candidate must have all valid pairs and a mean improvement of at least 0.03
across the five equally weighted SongEval axes. Genre means, the worst genre,
and the count of strictly improved genres remain prominent diagnostics, but a
genre regression is reported rather than used as a blocking veto. A
development-only run is never accepted: the benchmark may report it as an
eligible candidate but keeps `accepted=false` until the holdout comparison is
present.

The holdout cohort must then have a positive mean delta under the same frozen
conditions, and the combined development-plus-holdout mean must be at least
0.03. All pairs in both cohorts must remain valid; failures are retained and
prevent adoption. Only after these checks pass is the candidate eligible for
adoption. These thresholds are pre-registered engineering gates, not
statistical significance tests. A single repetition is a pilot and can be
noisy; the result is retained as evidence, and a fresh seed/repetition cohort
is planned before drawing a stable conclusion.

Reports generated before this amendment retain the former fixed worst-genre
and improved-genre gates in their `acceptance_rule` metadata. They are archived
pilot evidence and must not be reinterpreted as reports generated under
`overall-mean-priority-v2`.

The [official SongEval toolkit](https://github.com/ASLP-lab/SongEval) predicts five
axes, retained here with equal weight. A supplementary sensitivity
view recomputes the mean using its four non-Naturalness axes. Naturalness is
reported because it is part of SongEval, but these cases are instrumental and
cannot validate vocal naturalness. Genre fit is determined from the prompt and
score inspection; SongEval is not a genre-fit classifier.

The agent's composition seed is not controlled. Paired trials therefore compare
instruction packages and cohort behavior, not identical note-by-note random
draws. Keep elapsed time, tool errors, transcript/provenance data, and generated
artifacts with each suite so tool usage and delivery failures remain auditable.

For this optimization study, stop after two successive, fully evaluated
candidates fail to improve the retained condition under the development gates,
or when the available account quota ends. An infrastructure failure is an
incomplete experiment, not evidence of a score plateau. Preserve every attempted
candidate and qualify a plateau as applying to the tested changes, not all
possible prompts. Extend the manifest under a new version when adding genres.
