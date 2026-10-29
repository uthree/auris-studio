# Agent composition study: 2026-10-27

The tested prompt changes increased automatic backing use, but did not improve
the retained two-take SongEval mean. Production composition prompts and the
installed composition skill remain unchanged. The independently verified API
repairs and evaluation harness are retained.

There were 56 composition attempts and 55 delivered, scored recordings across
seven genres. One v2 pop attempt failed during Codex startup; it is retained
as a failure, not replaced with a more successful take.

## Selection and results

Each score is the equally weighted mean of Coherence, Musicality, Memorability,
Clarity, and Naturalness. The user explicitly prioritized the overall mean.
The policy amendment was recorded before any holdout execution; original
reports with the earlier genre vetoes remain archived. The current development
gate requires all valid pairs and a mean delta of at least +0.03. Independent
holdout confirmation would require a positive mean delta and a combined delta
of at least +0.03. These are engineering gates, not significance tests.

Initial one-take pilots gave v1 -0.063529 and v3 +0.092054 relative to the
original workflow. The v2 valid-six comparison was +0.070503, but its failed
pop trial made the cohort incomplete. No pilot established adoption.

The baseline, v3 and v4 each retained two independent development takes per
genre. The following means use all 14 recordings in each condition, including
low scores. These are development-only results; no holdout was run.

| Genre | Baseline | v3 | v3 delta | v4 | v4 delta |
|---|---:|---:|---:|---:|---:|
| Pop / J-pop | 2.837780 | 2.977910 | +0.140130 | 2.672860 | -0.164920 |
| City pop | 2.628100 | 2.736820 | +0.108720 | 2.458650 | -0.169450 |
| Lo-Fi | 2.399640 | 2.419100 | +0.019460 | 2.320430 | -0.079210 |
| Neo-soul | 2.106930 | 2.317890 | +0.210960 | 2.047750 | -0.059180 |
| EDM / house | 2.145390 | 2.149310 | +0.003920 | 1.956100 | -0.189290 |
| Anime song | 2.932670 | 2.794130 | -0.138540 | 2.704000 | -0.228670 |
| Game BGM | 2.688720 | 2.224680 | -0.464040 | 2.342720 | -0.346000 |
| **Overall** | **2.534176** | **2.517120** | **-0.017056** | **2.357501** | **-0.176674** |

The four-axis sensitivity check excluding Naturalness gives v3 -0.018723 and v4 -0.190513.
Both complete candidates failed the development gate. The study stops under
the predeclared two-successive-candidate rule. This is a plateau for the tested
changes, not evidence that every possible prompt has been exhausted. No
candidate advanced to holdout or replaced the production prompts.

## What changed in the candidates

- v1 preferred bounded generators for repetitive support while retaining the
  original detailed musical guidance and adding short genre cues.
- v2 shortened the core prompt, clarified foreground/backing separation,
  relative recipe octave and combined drums, and included early API repairs.
- v3 added explicit drum-map/schema discovery, genre-appropriate groove choice,
  melody-aware spacing and restrained starting density to the shorter policy.
- v4 retained the v3 musical policy and included the current API repairs plus
  MIDI duration metadata guidance. Its executable is a separate frozen
  condition; v3 does not validate an unevaluated integration on a newer build.

## Automatic generation and delivery

| Condition | Delivered recordings | Used automatic backing | Mean elapsed seconds |
|---|---:|---:|---:|
| baseline | 14/14 | 0/14 | 806.5 |
| v1 | 7/7 | 7/7 | 817.2 |
| v2 | 6/7 | 6/6 | 707.7 |
| v3 | 14/14 | 14/14 | 719.3 |
| v4 | 14/14 | 14/14 | 641.5 |

Generation uptake increased. Elapsed times are descriptive because trials
ran concurrently and did not control contention or LLM sampling. They do not
establish a causal speedup or compensate for a lower quality mean. Token
counts were unavailable; recorded zeros must not be treated as zero cost.

Independently justified repairs:

- Protected-only accompaniment clips may be empty or drums; foreground clips
  must still contain pitched notes. Preservation and atomicity remain enforced.
- Recipe schemas advertise valid numeric bounds, relative octave and groove
  choices, reducing unsupported parameter guesses.
- Switching a drum track to the built-in kit installs its authored role map.
  Reselecting the same kit preserves explicit empty/partial custom maps.
- MIDI end markers retain clip rests and loop extents, and export returns
  ticks, seconds, PPQ and note count. Audio-track extent is excluded.

The final native-output audit and MIDI-command inspection are retained with
the local evidence. They cover observed structured calls, not arbitrary shell
writes or all possible file inspections. No causal musical effect is inferred
from the delivery guidance.

## Reproduction and limits

See [the benchmark procedure](genre-benchmark.md) and the
[machine-readable study summary](../tools/eval/references/genre-benchmark-study-2026-10-27.json).
The summary contains original trial scores and WAV hashes, source/prompt
fingerprints, executable hashes, evaluator package/weight provenance, and
comparison-report hashes. Full WAV/MIDI, project snapshots, transcripts,
prompt conditions and source differences are retained in the local study.

- All cases are 16-bar A/B instrumental demos using built-in sounds. Results
  do not establish vocal, full-song, external-plugin or human listening quality.
- gpt-6.1-sol, Codex backend, legacy profile, 65,536 context tokens and candidate
  tools were held constant. The LLM sampling seed was not exposed; generated
  clip seeds and every actual artifact were retained.
- Some models also read the common installed composition skill. That skill
  stayed unchanged during the recorded verification interval; context was not
  fully isolated from external skill discovery.
- The evaluator used pinned SongEval/MuQ sources and weights, CPU with four
  threads, and no loudness normalization. Naturalness cannot validate vocals
  in these instrumental cases. SongEval is not a genre-fit classifier.
- The reserved holdout uses the same key/tempo with alternate register/accent
  wording. It would not test generalization to new keys, tempos or lengths.
- Post-hoc thinning, FM-parameter and gain-only diagnostics are excluded from
  adoption cohorts. They are not fresh agent-composed takes.

Validation for the retained implementation: workspace cargo tests, workspace
Clippy, formatting, 56 Python benchmark tests and Ruff passed. No musical
writer algorithm or audio DSP constant was changed.
