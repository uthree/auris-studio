# Typed rendering pipeline: implementation plan

Status: implementation in progress, 2026-10-27. M0 is complete. M1's typed
projection and shared read tool are implemented; its focused UI and richer
inspection remain pending. M2 and later milestones have not started.
The design baseline is `cd205c3c`; the saved format remains 32.

Read [the design](render-pipeline-design.md) for document ownership, port semantics,
migration, UI and agent contracts. Update `auris_session::guide` as each contract
becomes implemented. Keep current user documentation accurate to shipped behavior.

## Delivery sequence

| Milestone | Deliverable | Dependencies | File-format effect | Exit decision |
| --- | --- | --- | --- | --- |
| M0 | Baselines and executable contracts | None | None | Existing behavior characterized |
| M1 | Unified projection and focused UI | M0 | No semantic format change | Useful inspection without a second routing store |
| M2 | Typed model, migration and conventional-project compiler | M0, M1 identities | New format at activation | Existing projects survive native round trips and render equivalently |
| M3 | Shared source, independent instrument branches, automation | M2 | Covered by M2 schema; bump again if incompatible | First complete product workflow passes |
| M4 | Singer branches and dependency-aware jobs | M3 | Version any new stored variants | Independent takes, safe invalidation and landing |
| M5 | Control composition and reusable processing groups | M3; singer support follows M4 | Version incompatible additions | Usability and execution semantics demonstrated |

Each milestone includes frontend and agent coverage for its newly enabled behavior.
Do not accumulate a functioning engine behind an unusable graph UI or an unbounded
tool schema. Implementation units below are commit/PR-sized starting points; split
larger units while preserving their gates. Shipping one milestone does not require
committing to the later milestones' full scope.

## M0 — Characterize current behavior

Implementation progress: fixed fixtures produced by historical format 27–32 core
serializers and 12 passing IO-level semantic/round-trip regression tests are in
`crates/auris-io/tests/project_format_baselines.rs`. Their provenance and reproduction
procedure are in `crates/auris-io/tests/fixtures/project_formats/provenance.md`.
Existing numerical scheduling/rendering tests, a release performance executable,
and an executable saved-tool workflow now complete the M0 reference. See
[baseline coverage](render-pipeline-baselines.md), the
[performance record](render-pipeline-performance-baseline.md), and the
[workflow record](render-pipeline-workflow-baseline.md). UI evidence uses the existing
GPUI gesture harness; the tool replay is synthetic and makes no model-success or
human-usability claim. Callback allocation windows cover built-in fixtures only.

### Units of work

1. Add small checked-in project fixtures for each currently readable format. Use
   actual historical shapes, not only today's JSON with its version number changed.
   Include a conventional instrument track, authored/recipe clips, singer take,
   audio track, bus, master inserts, multi-output routing, sends and sidechains.
2. Record canonical semantic manifests: IDs, notes/lyrics, stage order/seeds, loop
   extents, curves, routes/tap positions, opaque plugin bytes and asset references.
3. Establish deterministic built-in render references plus event schedules. Include
   seeks, loops, sustain beyond note release, tempo changes, plugin latency and tails.
4. Measure existing graph build time, callback allocations, snapshot cost and render
   time for a small song and a generated larger project. Record workload sizes,
   hardware and build profile so later comparisons are repeatable.
5. Capture current UI workflows and tool transcripts for equivalent conventional
   operations. Count discovery/read/edit calls, payload size, failures and recovery.

### Owners and exit gate

Core/project fixtures, IO tests, engine scheduling/routing tests, session history
tests and toolbox/live transport tests own the evidence. Existing routing, automation,
singer and performance tests remain the behavioral reference. Exit when all fixtures
load and their semantics are independently asserted, with baseline results recorded.
Do not modify composer writers or audio constants to make migration tests pass.

## M1 — Unified view over the existing model

### Implemented foundation

`auris_session::pipeline_graph` now derives typed sources, clip placements and
performance stages, instruments, singer takes, inserts, faders, sends and
automation from the current document. Event, audio and control edges distinguish
offline synthesis dependencies from playback. Stored stage order is explicitly
separate from deferred pitch/controller evaluation. No new routing store or
runtime processor is created.

The `pipeline_read` tool shares one live/saved session command. Track, clip and
node scopes include immediate boundary neighbors; node and edge pages have
independent cursors bound to one document revision. Reads leave file bytes,
checkpoints and Undo unchanged. The [inspection contract](render-pipeline-inspection.md)
documents addresses, limits, error recovery and unchecked runtime state.

Keep the model-facing scope a compact address (`track:1`, `clip:2`, or a returned
node ID), converted into the internal typed enum. An initial tagged-enum tool
schema exceeded the default native-agent context budget; the compact input passes
the existing budget guard without raising it. Detailed guidance remains available
through `tool_help` and scoped reads.

Verification includes numerical M0 references, seven focused projection tests,
three toolbox live/saved/schema tests, the existing agent catalog budget guard,
and the complete workspace tests, Clippy and warning-denied API documentation.
Singer source metadata is not evidence of file availability or take freshness;
plugin parameter owners are checked without loading plugins or resolving keys.

### Next UI unit

Extend `crates/auris-gpui/src/ui/mixer_graph.rs` with a collapsible pipeline detail
list for the existing selection. Reuse `selected_clip`, then `selected_track`,
and the existing selection commands; keep the mixer canvas as the routing editor.
Show performance stages in stored order with evaluation labels, explicit
pre/post-fader sends, automation targets, and singer take/guide roles.

Cache bounded pages in transient presentation state keyed by `Session::revision()`
and scope. Do not hash the whole document on each paint or persist this cache in
`MixerGraphView`. Expose continuation and empty/error states; navigating or opening
the disclosure must not rebuild audio or change history. Only existing track and
clip owners become navigation targets in the first slice.

Add harness coverage for selection changes, repeated-stage order, pre/post-fader
labels, singer guide audition, and unchanged document/layout state. Follow with
chain editing, keyboard paths, detailed parameter inspection, localization, zoom
and small-window checks before declaring the complete M1 acceptance gate passed.

### Units of work

1. Add session-side typed projection DTOs for sources, clip-stage paths, instruments,
   automation bindings, effects and strips. Derive stable projection IDs from existing
   owners; make the mapping to future persistent IDs explicit.
2. Add scoped read/describe operations with revision-bound pagination. Keep
   `mixer_graph` behavior compatible. Return type/unit/evaluation metadata without
   constructing plugins just to read graph structure.
3. Extend Signal flow with source/performance disclosure and selected-neighborhood
   filtering. Add the selected-group chain inspector, using current commands for
   stage edits, automation and effect ordering. Show the actual pre/post-fader order.
4. Add keyboard connection selection, intermediate-state inspection and cross-view
   selection. Expose pending/unavailable state without implying a signal is current.

The projection cannot provide arbitrary instrument branching while the old model
still owns one instrument per track. Feature availability comes from capabilities;
only enable edits supported by the current underlying commands. Projection wires
inside clip stages are derived, not a new saved routing list.

### Acceptance

- A transform/automation edit from the chain or timeline updates the graph and console
  consistently; Undo restores the same underlying state.
- Layout/filter/disclosure changes do not change note schedules or rebuild audio.
- Existing output/send/sidechain edits preserve their exact connection identities.
- Keyboard, Japanese labels, themes, zoom and small-window behavior pass the UI matrix.
- Read pages use a single revision; a changed document cannot return a mixed graph.

M1 can ship independently. Preserve its useful inspection work even if native graph
storage needs more design iteration.

## M2 — Native model and one-way migration

### M2a: Pure data and validation

Implement typed IDs, authored-source ownership, nodes/ports/edges, group references
and automation targets in `auris-core`. Add version-independent pure validation for
duplicate/dangling IDs, port/layout/domain mismatches, incompatible drivers, mixed
dependency cycles and bounded resource use. Cycles report the offending path.

Retain existing clip performer arithmetic. Add stable wrappers to clip-local stages
and keep internal ordering authoritative there. Define identity mappings for track,
effect and send adapters, and a primary instrument reference for conventional commands.
Test cloning, source sharing, branch deletion and independent-source duplication.

### M2b: Versioned decoding and migration

Implement the design's conversion algorithm in the IO boundary with dedicated legacy
representations. Expand the supported-version dispatcher from the current source,
not a stale hard-coded list copied from this proposal. Update the version constant's
history when the native writer activates.

Implement exact-byte legacy backup before first upgraded overwrite. Reuse staged
atomic publication and no-clobber backup creation. Include recovery/checkpoints,
Save As, collected assets and missing external plugins. Migration never runs a
generator, loads a model or regenerates a take.

Do not publish native-format saves until the compiler and command adapters below
are ready. Pre-activation work may use test-only models/readers. A runtime rollout
flag must not permit writing a file that the same installed build cannot reopen.

### M2c: Compile conventional projects

Build preparation and realtime plans from native topology. Convert hosted instrument
placement, editor ownership and audition addressing from track-only keys to stable
instance keys. Preserve plugin main-thread ownership and core trait boundaries.
Cover strip order, pre/post sends, latency, solo/mute, tails, note chase and monitoring.

Validate the full document before allocating runtime structures. Pre-size buffers
using existing event/loop budgets; derive new graph limits from M0 stress workloads.
Keep previous valid audio while a replacement plan is pending or failed and expose
the document/audible-plan revision difference. Test successful publication and failure.

### M2d: Commands, history and transport compatibility

Adapt all document mutation paths to canonical sources/nodes. Prioritize existing
track/clip/instrument/effect/automation commands and the graph facade, then audit:

- Recording and monitoring; imported audio/MIDI; drum lanes and kit assignments.
- Composer insertion/regeneration and song-spec import/export.
- Sound preset application, instrument replacement and plugin state capture.
- Duplicate/delete track, clip freeze, audio freeze and stem selection.
- Save As, asset collection, recovery, snapshots and agent checkpoints.
- Agent note tools, performance tools, revision checks and human/agent Undo ownership.

Use one candidate validation/commit path. Existing transactions are not nested:
an agent edit must reject an active human gesture rather than finish it implicitly.
Keep musical revision distinct from layout changes and transport-specific history
counters. Request receipts and their timestamps are excluded from the semantic
revision/digest to avoid self-referential hashes.

### Activation gate

- Every supported legacy fixture migrates without authored-content or opaque-state loss.
- New native saves reopen with stable IDs and topology; supported legacy readers
  reject the new version before interpreting its contents.
- Deterministic built-in playback/export match the conventional-project references;
  differences have an explained numerical tolerance, not a blanket acceptance threshold.
- Fault injection at backup, staging and publication leaves a valid prior document;
  migration errors never overwrite original bytes.
- Undo/Redo and restored checkpoints remain correct, including missing-plugin cases.
- All audited editing paths operate on the native model. No production shadow copies
  of old routes, instrument ownership or automation targets remain.

M2 activation is an atomic integration boundary. Data types can land earlier, but
format activation waits for all four units and the gate. Rollback before release
keeps the old writer. After native files ship, recovery for an older executable uses
the preserved legacy backup; native edits require a compatible reader.

## M3 — First complete layered-instrument workflow

### Units of work

1. Add `layer_instrument` and explicit branch/source duplication commands. One source
   fans out to distinct instrument instances, and an explicit sum precedes the group's
   existing inserts. Expose branch levels without silently normalizing the output.
2. Add static branch transforms using the shared core performer, with independent
   IDs/seeds and note provenance. Preserve branch-local pitch/release and controllers.
3. Bind automation to stable instance parameters; distinguish effective and static
   values. Support shared curves only through compatible domains or explicit mapping.
4. Enable graph branch editing and chain branch containers. Route audition, plugin
   editors, mute/solo and stem selection to the intended branch/group.
5. Add bounded typed edit batches and high-level agent operations; implement optional
   validation, conflict diagnostics, alias resolution, request receipts and persistence
   status. Test saved MCP, live agents and UI commands against the same results.

Start with limits of 64 operations per edit, 64 nodes per read page and 64 parameters
per description page, aligned with existing graph paging. These are API bounds, not
the maximum project size. Make neighborhood depth/result caps explicit and return
continuation information. Publish graph/event memory limits only after M0/M2 measures
establish their safe values. Reject oversized requests before any edit or allocation
proportional to untrusted counts.

### End-to-end acceptance script

1. Open a migrated instrument project and add a second instrument layer through the
   chain command. Read back the shared source and independent node identities.
2. Transpose only the second branch by an octave. Edit an authored note once; both
   branches reflect it while the second retains its transpose.
3. Automate the shared filter cutoff with values in its reported domain. Confirm its
   static value remains stored and returns when automation is disabled.
4. Compare the full mix and each branch, including sustained notes, loop wrap and
   seek into a note. Test plugin/editor state isolation using a real installed plugin
   when available, recording absence as untested rather than fabricating coverage.
5. Undo/Redo the layer and automation edits; save, close, reopen and export. Check IDs,
   routes, stage order, parameter values, event timing and deterministic output.
6. Perform the equivalent operation through model tools. Retry an accepted request,
   send a stale revision and submit a batch containing a cycle. Assert one successful
   creation and no partial mutation/checkpoint for failed batches.
7. Race two saved-file edits from the same revision, fail a save after validation,
   and retry after Undo and Save As. Assert conflict detection, atomic receipt/edit
   persistence and separation of request epochs from musical history.

This milestone is the first product release candidate for the proposal's main outcome.
Compare task completion and tool-call/error counts with M0 before widening the editor.

## M4 — Singer branches and dependency-aware preparation

### Units of work

1. Move singer planning, take identity and fingerprint handling to per-node ownership.
   Preserve existing voice model/path semantics and preview behavior.
2. Build reverse dependency indexes and conservative invalidation windows. Shared score
   edits invalidate all dependent singers; downstream mix edits leave takes current.
3. Implement node-scoped jobs with bounded concurrency, cancellation, staging and
   dependency revalidation at landing. Integrate durable assets with history/checkpoints.
4. Add singer-layer controls, generation status and explicit stale-take export policy.
   Expose the same job lifecycle and currentness through model tools.

### Acceptance

- One lyric source feeds two voices with independent settings/takes. A source edit
  marks both stale; a voice change marks only that singer stale.
- Cancelling or deleting/undoing a singer during generation prevents a late result
  from overwriting current state. Unrelated layout/mix edits allow a valid result.
- Generation failure preserves a previous take and reports its stale state. Reload
  clears transient jobs while retaining durable takes and their fingerprints.
- Asset collection/Save As preserves all referenced takes; cleanup cannot remove
  assets used by history or checkpoints.
- Export with `require_current` rejects unresolved stale/missing takes. Explicit
  existing-take export identifies exactly which takes were used.
- Run backend integration only with available, authorized resources; deterministic
  tests use a fake backend that can delay, fail and complete out of order.

## M5 — Additional control and reuse

Deliver these as separate features after measured M3 usability, not as a prerequisite
for layered instruments:

1. Typed control mapping/combiners with explicit units, ordering, clamping and one
   resulting absolute target driver. Test mixed-domain cycle rejection.
2. Event-sampled transform automation with documented source-time sampling and paired
   note release. Phrase-aware parameters require a separate preparation contract.
3. Reusable groups with declared typed inputs/outputs and macro parameters. Instances
   own independent processor state; sharing a source remains an explicit choice.
4. Expanded cache reuse and finer invalidation only after comparison against full
   recomputation proves equivalent results and failure behavior.

Each stored incompatible variant triggers the version process again. UI presets and
high-level agent operations accompany each feature so users need not construct its
internal plumbing manually.

## Cross-cutting verification matrix

| Boundary | Required evidence |
| --- | --- |
| Model | Stable IDs, sharing/deletion semantics, type checking, combined cycles, driver conflicts, bounded inputs |
| Persistence | Historical fixtures, native round trip, backup/atomic-save fault injection, future-version rejection, recovery |
| Performance | Authored notes unchanged, provenance, seeds/passes, phrase context, negative timing offsets and freeze scope |
| Engine | No callback allocation/locking/I/O, bounded events, independent instance state, seek/loop/chase, latency/tails |
| Jobs | Cancellation/deletion/Undo races, fingerprint validity, unrelated edits, retained takes and protected assets |
| Commands | Atomic batches, one Undo step, no-op behavior, human gesture isolation, stale revision and retries |
| Tools | Shared schema/behavior across transports, pagination consistency, discovery, units, permission aggregation |
| UI | Chain/graph/console consistency, keyboard paths, focus, errors, themes, zoom, localization and resizing |

Before each implementation commit, run `cargo fmt --all`, `cargo test --workspace`
and `cargo clippy --workspace --all-targets`. GPUI-targeted tests use
`cargo test -p auris-gpui --bins`. Build docs with warnings denied for changed public
contracts. Do not add production features merely to support a test harness.

For changes to a writer or an audio constant, run both measuring instruments from
[evaluation.md](evaluation.md) before and after. Numeric/symbolic checks and listening
assessments must be reported separately. No such constants change in this proposal.

## Risk register and decision checkpoints

| Risk | Mitigation | Evidence required before release |
| --- | --- | --- |
| Two sources of truth diverge | Derived M1 projection; one native owner after M2 | All mutation paths audited; no dual writes |
| Old projects change sound | Exact field transfer, preserved stage order/taps, reference renders | Migration and conventional-render gate |
| Shared score deletion damages other parts | Consumer-aware scope and explicit detach/delete commands | Shared-source history fixtures |
| Plugin state resets or belongs to the wrong branch | Stable instance identity across hosts, UI and engine | Two-instance integration and swap tests |
| Timing edits fail at seeks/loops | Shared evaluator, provenance, lookahead and chase contracts | Event and waveform boundary assertions |
| Complex UI obscures the task | Chain-first workflows, scoped graph, progressive disclosure | Complete M3 scenario by keyboard and pointer |
| Agent schema/context becomes too large | Small typed operations, discovery, scoped paged reads | Tool transcripts, payload/call/error comparison |
| Repeated tool calls duplicate work | Revision checks and payload-bound request receipts | Lost-response/reopen/Undo retry tests |
| Background work overwrites a newer edit | Dependency digest revalidation on session-owned landing | Controlled race/failure tests |
| CPU/memory grows with branch count | Shared immutable source results, independent stateful processors, bounded plans | M0 workload comparison and documented limits |

Do not set a release date from this proposal alone. M0 establishes costs; M2 is the
largest integration risk. At each exit gate, record implemented scope, measurements,
remaining failures and any revised design decision before starting dependent work.
