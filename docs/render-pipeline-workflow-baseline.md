# Render-pipeline workflow baseline

This is the M0 executable protocol baseline for the current mixer DAG and the
adjacent non-destructive performance/automation tools. It is a deterministic
fixture replay, not a usability study or an assessment of live model behavior.
No runtime API or project schema is introduced by this baseline.

## Executable fixture

`mixer_graph::tests::saved_workflow_replay_measures_graph_automation_and_performance_contracts`
in `crates/auris-toolbox/src/mixer_graph.rs` calls the actual saved-project
dispatcher (`runtime::call_saved`) and performs this sequence:

1. Create two bus tracks and one instrument track with a MIDI clip, then save.
2. Read the mixer graph and assert that returned node IDs are the current
   `track:N`/`effect:N` forms. The effect key is `effect:<slot-id>`; it does not
   contain a track ID.
3. Add a send, repeat the identical connect (no-op), attempt a cycle (error),
   and recover by disconnecting the original send.
4. Read and set track-gain automation using the returned document revision.
5. Set non-destructive clip performance (`gate: 0.5`) and read it back through
   `inspect_performance`.
6. Measure UTF-8 serialized request and response bytes for every call.

This replay uses the production dispatcher and production session validation;
the only synthetic part is the temporary project fixture. The graph tool
implementation is in `crates/auris-toolbox/src/mixer_graph.rs`; the shared
actions and stable-ID contract are in
`crates/auris-session/src/mixer_graph/commands.rs`; automation and performance
commands are in `crates/auris-session/src/live_agent/automation.rs` and
`crates/auris-session/src/live_agent/performance.rs`.

## Observed run

Command (run without `RUSTC_WRAPPER`):

```powershell
$env:RUSTC_WRAPPER = $null
cargo test -p auris-toolbox saved_workflow_replay_measures_graph_automation_and_performance_contracts -- --nocapture
```

Observed output on 2026-10-27:

```text
workflow-baseline bytes: read request=133, read response=1577,
connect request=166, connect response=1760,
automation read request=150, automation set request=308,
performance request=114, performance read request=90
workflow-baseline calls: count=10, errors=1, request_bytes_total=1566, response_bytes_total=10595
test ... ... ok
```

The request sizes include the temporary absolute project path and therefore are
measurements of this run, not schema-size golden values. Response sizes are
also recorded by the test rather than hand-authored. A future schema change
must update this fixture and record a new run.

The neighboring `file_and_live_tools_share_the_graph_and_failed_calls_do_not_save`
test exercises the lower-level `mixer_graph::run` helper directly and compares
it with the live session command. The executable replay above is the production
saved dispatcher path (`runtime::call_saved`), so the two tests deliberately
cover different boundaries; the direct helper is not presented as an MCP or
saved-runtime transcript.

## Current operation contract

* Discovery comes first. `read` returns processing-order nodes, outgoing edges,
  available effects, and `next_offset`; later calls copy exact IDs from that
  result.
* `connect`, `disconnect`, strip edits, effect edits, and parameter edits return
  `changed`. Duplicate operations are no-ops; invalid ports, stale IDs, invalid
  values, and cycles fail before document mutation.
* Saved-project writes are checkpointed by the saved adapter only when the
  operation changed the document. Read-only calls do not rewrite the file.
* Automation writes require the revision returned by automation read. They use
  native units and preserve static values and authored notes.
* Performance writes alter playback transforms only; the fixture starts with
  two authored notes and reads both pitches back after the performance edit to
  verify that the clip content was preserved.

The focused session tests in
`crates/auris-session/src/mixer_graph/commands/tests.rs` cover topological
pagination, stable effect IDs after reordering, atomic validation, mixed-cycle
rejection, duplicate no-ops, and one-step Undo. Existing GPUI harness tests in
`crates/auris-gpui/src/ui/mixer_graph_wires_tests.rs` and
`crates/auris-gpui/src/ui/mixer_graph_persistence_tests.rs` cover exact wire
selection/deletion, gesture Undo, and layout persistence. Those tests are
reused as the UI baseline; this M0 fixture does not claim render-pipeline UI
coverage.

## M0 acceptance boundary

The baseline is satisfied when a pipeline prototype preserves the following:

1. fresh paged discovery before dependent edits;
2. stable opaque IDs rather than name/position guesses;
3. one atomic history step for a successful compound edit and no history for a
   no-op or failed validation;
4. fresh discovery as the recovery action after a stale-ID or revision error;
5. independently readable automation and performance state.

This record does not measure latency, model completion rate, visual clarity, or
musical quality. Those require separate M1 replay fixtures and human review.
