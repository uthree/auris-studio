# Typed rendering pipeline: implementation plan

Status: implementation in progress, 2026-10-28. M0 is complete. M1's typed
projection, shared read/edit tools and focused Inspector support chain editing.
Keyboard connection selection and bounded intermediate inspection are implemented;
the complete M1 acceptance gate is still pending. M2a includes pure typed topology,
shared-source ownership, stable clip stages, conventional-command identity bindings,
shared automation curves, processor-derived capability resolution and the standalone
native document envelope. M2b's pre-activation conversion, captured-source report and
exact-byte backup primitive are implemented. M2c now has a conventional preparation
and render-parity bridge, a private prepared-input boundary, native node controls,
callback adoption acknowledgments and isolated host/editor ownership by generation.
The normal Session can now capture and prepare a revision-bound native transition
using its existing engine. Native document edits have atomic batches and snapshot
history. Explicit native IO APIs support bounded format-33 files, guarded updates
and backed-up in-place upgrades. The default Session authority, playback publication,
Save, recovery and checkpoints still require the remaining command migration.
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

### Implemented focused Inspector

Signal flow's **Pipeline details** button and the command palette reveal a
collapsible section in the existing Inspector. The list follows `selected_clip`,
then `selected_track`, with explicit Track/Clip scope controls and existing owner
selection commands. The mixer canvas remains the routing editor. Rows show
performance stages in stored order with evaluation labels, explicit pre/post-fader
sends and their destination, automation targets, and singer take/guide roles.

Transient presentation state caches one 32-node page, keyed by
`Session::revision()` and scope. Unchanged paints do not hash the document.
Previous/next controls and empty/error states remain inside the Inspector's
existing scroll area. Disclosure and navigation do not change the project,
`MixerGraphView`, history or audio graph. Track and clip owners are navigation
targets; plugin and take readiness remain explicitly unchecked.

Model and harness coverage exercises caching, bounded pagination, selection
changes, repeated-stage/effect order, transform edits and Undo, send taps, singer
guide roles, keyboard disclosure, unchanged document/layout state, and narrow
Inspector layouts in English/Japanese with both themes and enlarged text.

### Implemented chain editing

Performance rows expose move earlier/later and remove operations. Each discrete
operation validates the proposed stack and records an independent Undo step,
preserving authored notes and stage payloads, including random seeds. Menus carry
the session revision so an old ordinal cannot edit a different stage after a change.
Effect rows open the existing parameter editor and slot-based actions menu.
Automation rows reveal their track's existing timeline lane idempotently; a saved
plugin key must still match its parameter descriptor before navigation.

The compact `pipeline_edit` tool provides the same three performance-stage
operations in live and saved sessions. It requires an exact stage node and the
content revision from `pipeline_read`. Changes use the existing document owner;
edge moves are no-ops, and failed/stale/no-op saved requests create no checkpoint.
Effect and automation edits continue through their existing tool surfaces.

### Implemented keyboard connections and bounded detail

The mixer canvas has a transient keyboard selection path for visible, selectable
connections: the Select connection control selects the first edge, unmodified arrow
keys traverse in stable order, Enter/Return opens the existing edge menu, and Escape
clears selection. Modified arrows remain reserved for canvas navigation. Internal
chain edges and hidden edges are excluded; no document state changes until an
existing menu operation is invoked.

`pipeline_read` detail mode accepts an exact returned node ID and the same content
revision used by graph paging. It exposes bounded stage boundary samples and passive
fader/send/automation parameter values. Stage evaluation is pass zero; `at_tick` is
an absolute project tick for parameter values. Detail rejects paging/cursor options,
negative ticks and stale revisions, and never creates plugins or writes a project.
The tool keeps the fixed sample bound and does not add a per-request sampling knob.

### Remaining M1 work

The gesture matrix now also covers actual toolbar zoom to both limits, followed by
keyboard connection traversal, in 640×480 windows across English/Japanese and
daylight/midnight themes. It checks that these actions preserve musical state,
routing and history. Revealing a selected connection's destination in the Inspector
also preserves graph layout and history. See the inspection document for the
earlier native-window evidence.

The complete manual acceptance gate, including visual focus/geometry and
accessibility review, remains open; the display-free harness does not verify pixels
or the accessibility tree. This does not block the independent M2a data contract.

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
4. Add cross-view selection and graph zoom coverage around the implemented keyboard
   connection selection and intermediate-state inspection. Expose pending/unavailable
   state without implying a signal is current.

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

The first slice is `auris_core::pipeline`: typed node/edge/port handles and a pure
validation contract over resolved port descriptors. Event, audio and control
dependencies share cycle detection and deterministic topological ordering. Explicit
input cardinality, sidechain roles, control domains and parameter policies make
connection errors inspectable before a compiler builds plugins or buffers.
Caller-supplied limits bound validation work; they are not product capacity claims.
See the [topology contract](render-pipeline-topology.md) for interfaces and diagnostics.

This is a topology contract, not the canonical native project model. It is not
stored on `Project`, used as a parallel routing store, or connected to playback.
Node capabilities must eventually be resolved from their owning processors before
validation; a structurally valid descriptor is not proof that a plugin implements
it. The next slice, `auris_core::pipeline::content`, now supplies independent shared
source ownership, processor/group payloads, stable clip stages and conventional
command identity resolution. Source and branch copies have different ownership
semantics; deletion and ID allocation are atomic candidate operations. See the
[ownership contract](render-pipeline-ownership.md) for implemented APIs and tests.

Independent automation curves and bindings now preserve project-time interpolation,
share compatible drivers and retain disabled connections. Processor-derived
capabilities and full fragment resolution validate policies, domains, driver conflicts
and cycles under cumulative budgets. Missing legacy domains remain unresolved rather
than inferred; the [automation contract](render-pipeline-automation.md) describes this
boundary. The standalone native document envelope also validates asset-registry
identities and track presentation/source roles. Existing session commands still operate
on the current project; the new resolver is their future identity boundary, not an
activated command backend. The M2 activation gate still applies to file writing
and playback.

Implement typed IDs, authored-source ownership, nodes/ports/edges, group references
and automation targets in `auris-core`. Add version-independent pure validation for
duplicate/dangling IDs, port/layout/domain mismatches, incompatible drivers, mixed
dependency cycles and bounded resource use. Cycles report the offending path.

Retain existing clip performer arithmetic. Add stable wrappers to clip-local stages
and keep internal ordering authoritative there. Define identity mappings for track,
effect and send adapters, and a primary instrument reference for conventional commands.
Test cloning, source sharing, branch deletion and independent-source duplication.

### M2b: Versioned decoding and migration

The pre-activation IO path now converts every supported historical fixture into a
standalone `NativeDocument`. It preserves ordered track presentation, shared timeline
context and asset registries alongside the authoritative content graph. Sources,
stable stages, inserts, sums, output routes, sends, sidechains, singer guide/take
selection and automation targets are explicit. Caller-supplied budgets and the entire
ID reservation are checked before construction. Plugin domains remain unresolved when
metadata is unavailable; missing stable plugin keys reject the candidate unchanged.

`load_native_candidate` reads bounded source bytes once, reuses the current decoder
and repairs, and reports the original version and repair facts. Exact captured bytes
can be backed up with synchronized no-clobber publication and byte-verified idempotence.
The explicit `publish_native_upgrade` IO operation now combines that candidate with
backup, source checks and staged format-33 publication. Native updates require the
observed destination bytes. See the [migration contract and verification](render-pipeline-migration.md).

The ordinary Session writer switch, legacy-schema separation, checkpoint/recovery and
Save As integration remain coupled to M2c/M2d. The guarded native IO APIs are tested
independently; default Session loading/writing continues to use format 32.

Implement the design's conversion algorithm in the IO boundary with dedicated legacy
representations. Expand the supported-version dispatcher from the current source,
not a stale hard-coded list copied from this proposal. Update the version constant's
history when the native writer activates.

Implement exact-byte legacy backup before first upgraded overwrite. Reuse staged
atomic publication and no-clobber backup creation. Include recovery/checkpoints,
Save As, collected assets and missing external plugins. Migration never runs a
generator, loads a model or regenerates a take.

Do not activate ordinary native-format saves until the compiler and command adapters
below are ready. Pre-activation work may exercise explicit IO APIs in isolated files. A runtime rollout
flag must not permit writing a file that the same installed build cannot reopen.

### M2c: Compile conventional projects

The pre-activation conventional bridge validates native ownership and capabilities
before constructing transient renderer input from sources, group chains and explicit
signal edges. It retains track kinds, opaque plugin state, singer guide/take choice,
clip stages, sends, sidechains and enabled automation. Unsupported native shapes fail
without changing the input. This provides a numerical comparison path; it does not
switch the live session, publish realtime graphs or authorize native saves. See the
[conventional render contract](render-pipeline-conventional.md).

Build preparation and realtime plans from native topology. Convert hosted instrument
placement, editor ownership and audition addressing from track-only keys to stable
instance keys. Preserve plugin main-thread ownership and core trait boundaries.
Cover strip order, pre/post sends, latency, solo/mute, tails, note chase and monitoring.

Validate the full document before allocating runtime structures. Pre-size buffers
using existing event/loop budgets; derive new graph limits from M0 stress workloads.
Keep previous valid audio while a replacement plan is pending or failed and expose
the document/audible-plan revision difference. Test successful publication and failure.

The prepared-input boundary now keeps the conventional snapshot private in
`NativeRenderInput`, with a complete mutable `NodeId` address table per graph.
Native placements use node keys and validate live plugin descriptors against the
lowering contract. Realtime node commands and generation-bound parameter handles
prevent reordered positions from changing an operation's target. Offline builds
use the same preparation path. See the conventional render contract for failure
ownership and verification limits.

The engine exposes a separate callback adoption observation for native generations.
Preparing or queuing a graph does not advance it. The pre-activation
`auris_session::native_host::NativeHostedPlayback` component now owns an
`AudioDevice` and `EngineHandle`, isolated fresh CLAP/VST3 instances, pending and
published host generations, and the retiring host set. It is not a second Session
authority: it receives a caller-owned `NativeDocument`, performs no document/history
mutation, and exposes snapshots tagged with node, revision and generation.

Preparation validates configured trusted plugin paths and host descriptors before building a
candidate. Only one candidate may wait for adoption; a full queue returns it intact
for retry, a silent engine rejects it, and a dead device retires an unadopted pending
candidate while retaining the published generation. Promotion requires an exact
`adopted_native_generation` match. Persistent node identities and editor/window intent
survive replacement; old hosts remain until the returned graph releases their render
halves. The host/session transaction consuming this evidence remains to be integrated.

The remaining integration order is:

1. Adapt `HostedPlugins` in `session/hosted.rs`, `Vst3Plugins` in `session/vst3.rs`
   and plugin-window selection together. The pre-activation host intentionally uses
   fresh isolated instances first; retain persistent node/window intent and retirement
   across graph replacement, but do not introduce spare-instance reuse until the
   native handover is proven. Preserve opaque state transfer.
2. Integrate preparation with `Session::rebuild_graph` in `session/mod.rs`. Validate
   the candidate and its revision before changing meter/scope readers or publishing
   to the engine. A failed preparation/publication retains the audible plan and
   reports the revision difference.
3. Complete the M2d mutation/history audit before replacing the session's document
   authority. Activate native persistence only with that integration; the render
   bridge's legacy-shaped output is never a shortcut for save or command execution.

`Session::capture_native_transition` and `prepare_native_transition` now implement
the capture/preparation portion of this seam. They collect actual host state, convert
once, and retain native IDs across candidate edits. The borrowed host manager uses
the existing engine geometry, assets and plugin trust; the transition owns no second
audio device. Origin, revision and content checks reject stale candidates. This seam
does not enqueue the graph while ordinary commands still write the legacy document.
See [native session integration](render-pipeline-native-session.md) for implemented
command families and the remaining authority boundary.

Host integration must stage an instance handover, not just rename placement-map keys.
The isolated `NativeHostedPlayback` path keeps fresh candidate instances separate
from the legacy `HostedPlugins::place`/`place_instruments` and VST3 equivalents.
Its preparation-owned record retains the published node/window selection until
validation and exact callback adoption succeed. Failure drops candidate render
halves on the preparation thread and retires their control hosts without replacing
the current editor. State copied from a host and document overrides must be
associated with the same document revision used for preparation. Session integration
must later adapt this record without creating a second document or command authority.

Publication tests must distinguish queue acceptance from the audio callback adopting
the graph. Meter/scope readers, the audible revision and native parameter handles
must follow the adopted generation; a rejected or stale candidate must not redirect
those readers. Cover both a full command queue and a retired graph still holding
the previous plugin render half. Conventional `TrackId`/`EffectSlotId` editor adapters
can resolve through native command bindings, while native instance ownership uses
the persistent processor identity. Do not derive new processor IDs by reconverting
the legacy document on every edit.

### M2d: Commands, history and transport compatibility

The `NativeDocumentEditor` boundary now owns native snapshots, bounded command
batches, owner-scoped gestures and Undo/Redo. Presentation and musical revisions are
separate; history restores identities while preserving the allocation high-water
mark. Initial adapters cover presentation, strips, effect bypass, authored notes,
stable clip stages and explicit source/branch copies. These adapters operate directly
on native ownership; the default Session command implementations have not switched.

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
