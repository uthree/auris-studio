# AI workshop scenario validation — 2026-10-30

## Finding and correction

After explicitly selecting a song, successive revisions started from that original
selection instead of the previous completed revision. A reproducible sequence was:

1. Select the original 120 BPM song.
2. Request a change to 90 BPM and wait for completion.
3. Reopen the conversation and request 3/4 meter while keeping the new tempo.

Before the correction, the final score returned to 120 BPM. The regression test
`selected_song_revisions_accumulate_across_reopening` failed with actual tempo 120
and expected tempo 90.

A successful single-candidate revision now advances the selected baseline to its
completed result. Comparisons, cancellation and completion of an older recovery
leave the current selection intact. Explicitly choosing an older version still
creates a separate branch. Advancing the baseline does not record a preference vote.

## Automated scenarios

The scripted-provider scenarios use actual headless sessions, session commands,
checkpoint storage, project reopening and rendered auditions.

| Scenario | Result |
| --- | --- |
| Selected song: tempo revision, reopen, meter revision | Both edits retained; original file unchanged |
| Unselected single song: consecutive revisions and reopen | Latest completed score inherited |
| Explicitly return to an older song | New branch uses that exact older score |
| Two comparison results complete in reverse order | Both retained; no winner selected automatically |
| Stop after completion has already been queued | Late result refused; selected score unchanged |
| Resume a failed revision from its checkpoint | Prior tempo edit and notes retained without duplicate tracks |
| An older recovery finishes after a newer selection | Newer selection retained |

All seven scenarios passed. The complete `auris-agent` suite passed: 221 tests,
with 19 opt-in tests skipped. The workshop's existing GUI harness suite also passed
all 36 tests, including queued messages, stopping, attachments, Japanese input,
candidate editor buttons and conversation navigation.

## Real model run

The installed Codex backend with `gpt-6.1-terra` completed the following Japanese
requests against disposable projects in a background process:

1. Change the selected song to 90 BPM while preserving its notes and other tracks.
2. Reopen the conversation, retain 90 BPM and change the meter to 3/4.
3. Produce two tempo-only alternatives at 80 and 100 BPM. Both retained the revised
   notes and meter, and the 90 BPM baseline remained selected and unchanged.
4. Create a separate conversation and generate a new 100 BPM drum song with no
   parent candidate.
5. Return to the original conversation and restore its history and selected score.

The run passed in approximately 142 seconds. The revision lineage was `1 → 2 → 3`;
both comparison candidates derived from candidate 3. Assertions checked exact
track equality, meter, tempo, parent identities and preservation of source files.
Finished candidates passed the normal save and non-silent render checks.
The five generated candidates completed 48 Auris tool calls with no failed calls
and no automatic recoveries.

The run kept project and preference storage separate from user projects and settings.
Human listening and other real provider backends were not evaluated.

## Reproduction

```powershell
cargo test -p auris-agent workshop::scenario_tests

$env:AURIS_WORKSHOP_SCENARIO_OUTPUT = Join-Path $env:TEMP ("auris-workshop-qa-" + [guid]::NewGuid().ToString("N"))
$env:AURIS_WORKSHOP_EVAL_BACKEND = "codex"
$env:AURIS_WORKSHOP_EVAL_MODEL = "gpt-6.1-terra"
cargo test -p auris-agent workshop::scenario_tests::installed_workshop_handles_sequential_edits_comparisons_and_thread_switching -- --exact --ignored --nocapture
```

The real test requires an installed, authenticated CLI and an available model.
Its output directory must be new. `state.json` records progress and conversation
state; `report.json`, `stdout.log`, `stderr.log` and candidate artifacts retain the
result for inspection. Individual production rounds have a six-minute test deadline.

Formatting and `cargo clippy --workspace --all-targets --locked` passed. Clippy
reported only the existing dependency future-compatibility notice for
`proc-macro-error2`.
