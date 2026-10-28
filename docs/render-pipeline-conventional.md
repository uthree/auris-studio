# Native conventional render bridge

`auris_session::native_conventional::prepare_native_conventional` prepares an
engine-owned `NativeRenderInput` from a `NativeDocument`. The native document owns
the authored material and identities. The conventional snapshot is private inside
the prepared input, with no document accessor or serialization path.

## Preparation contract

Ownership and the complete typed dependency graph are validated with caller-supplied
limits before constructing the snapshot. The caller supplies authoritative plugin
capabilities; preparation does not discover or load external plugins. Plugin curves
must have resolved domains, and stable parameter keys are mapped to the supplied
runtime descriptor positions. Unavailable metadata fails without modifying input.

The bridge reconstructs conventional tracks from source bindings, group ownership
and signal edges. Each group has a sum, ordered inserts and a strip. Track outputs,
pre/post-fader sends and sidechains retain their destinations. The master strip
feeds exactly one final output. Instrument/drum identity, clip-local stage order,
opaque plugin state, audio references and singer guide/take selection are retained.

Every signal edge and audible processor must be represented. Extra processors,
unsupported branches and disconnected output shapes fail explicitly. Disabled
automation bindings use the stored static value. Unbound curves remain in the
unchanged native document and contribute no runtime driver. An enabled curve retains
its original points and interpolation when lowered to a legacy parameter target.
Active drivers must use continuous control, matching the conventional renderer's
sampling contract; event-sampled drivers require an execution path that preserves
their event-pairing semantics.

This preparation path performs allocation and validation on a normal thread. It
does not publish a realtime graph or change the live session's document authority.
The format-32 writer and existing command/history path remain in use until the M2
activation gate in the [implementation plan](render-pipeline-plan.md) passes.

## Runtime identity and plugin placement

Preparation requires a complete one-to-one mapping from mutable native nodes to
conventional strip, instrument, singer, effect and send identities. Each prepared
graph owns the resulting runtime positions. Source, sum, output and automation
nodes have no mutable runtime address. Singer and guide-instrument identities remain
distinct even though the conventional renderer stores them in one render track.

`NodePlacedInstruments` and `NodePlacedEffects` accept caller-built render halves
by `NodeId`. Before taking any supplied object, the builder validates all placement
roles, plugin IDs/kinds, parameter descriptors and required sidechain support. The
session supplies the complete descriptor/domain/policy contract used during lowering; metadata
drift therefore fails before automation can use an outdated parameter position.
Hosted slots require a supplied render half. The session still owns plugin loading,
main-thread instances and editors; this API does not open plugin files.

Native graph construction and offline preparation share the validated placement
path. Initial placement or metadata failures preserve supplied maps. Later scheduling,
descriptor changes during state loading/preparation, or offline-range failures may
consume render halves, but return no publishable graph; the caller must retain the
previously playing graph.

`auris_session::native_host::NativeHostedPlayback` is the pre-activation owner for
this boundary. It owns an `AudioDevice`, an `EngineHandle`, isolated fresh CLAP/VST3
control instances, and their render halves. It is deliberately separate from the
legacy `Session`: preparation and snapshots do not mutate the native document, the
format-32 `Project`, command history, or legacy hosted-plugin maps. Plugin trust is
checked with the same standard/configured plugin-folder policy as the conventional
session before a file is loaded. Runtime descriptors are checked after state restoration.

Queueing requires the caller's current document revision to match the prepared
revision. Only one candidate may be queued. A full command queue returns the intact candidate
for retry; a silent engine rejects queueing; a stopped device retires an unadopted
pending candidate without disturbing the published one. Publication occurs only
when `EngineHandle::adopted_native_generation` exactly matches the candidate. The
published host set retains persistent node identities and window intent, while old
hosts remain alive until their returned graph no longer owns the corresponding
render halves. `NativeHostSnapshot` reports node, document revision and graph
generation together with opaque state and readable values for an explicit M2d
reconciliation step.

`EngineCommand::Node` resolves a persistent node against the current graph without
allocation or locking. Unknown nodes, wrong roles and invalid numeric values are
ignored. Plugin parameter commands use a stable-key lookup performed before graph
publication. Its copyable handle includes a unique graph generation and accepted
range; rebuilding invalidates old handles even when the node survives. Diagnostics
may read runtime positions, but queued native commands never depend on those positions.

`RenderGraph::native_generation` identifies a prepared native candidate, and
`EngineHandle::adopted_native_generation` reads the last generation adopted by the
audio callback. Queuing a graph does not change this observation. A full command
queue or saturated retirement storage retains the previous generation until the
callback can apply the replacement. Adopting a conventional graph clears the native
generation. The observation describes the selected render plan, not transport state
or when a sample becomes audible through the hardware. Native session integration
must compare this identity before committing reader/editor handover.

## Verification

Deterministic fixtures compare both channels through the existing offline renderer,
using built-in instruments/effects and in-memory audio assets. They check nonzero
reference output as well as waveform equality, and directly edit native strip or
automation-binding state to check the resulting change in output. Shape tests first
resolve a valid native graph, then require a representability error without input
mutation. Missing-plugin tests exercise the earlier capability-validation boundary.

The fixtures cover instrument/drum tracks, pre/post-fader sends, parallel outputs,
bus/master inserts, master sidechains, clip-stage transposition and singer
guide/cached/missing-take selection. Their paired renders require exact sample
equality. A native gain edit checks the -12 dB peak ratio within `1e-5`; mute requires
zero output, and disabling gain automation restores the complete static waveform.
A separate metadata fixture reorders parameter descriptors and verifies stable-key
resolution to the new runtime position after explicit domain resolution.

Prepared-input tests require exact stereo waveform equality with the conventional
offline path and verify native identities after track reordering. Engine fixtures
exercise complete address coverage, plugin identity and sidechain failures, mandatory
descriptor contracts, and descriptor changes during `prepare`. A reordered effect
fixture proves that an old parameter handle cannot write to a different key, while
its new handle can; read-only controls have no writable handle.
The session-to-engine tests also preserve control policies: only continuous writable
parameters receive realtime handles; preparation, regeneration, structural and
event-sampled settings require their corresponding command/execution path.
Heap counters require zero allocations and deallocations for accepted and stale
node parameter commands.
A singer with a declared take but no source metadata must render silence instead of
falling back to its guide instrument.

Adoption tests queue native candidates through `EngineHandle` and drive the actual
callback implementation. They distinguish accepted-but-pending candidates, rejected
commands, saturated return queues and retirement storage, completed replacements,
and replacement by a conventional graph. A silent engine never acknowledges its
queued candidate. The callback adoption and deferred paths require zero heap
allocations and deallocations.

These deterministic tests establish numerical compatibility for their fixtures.
External-host checks below exercise selected installed plugins separately.

Validation on 2026-10-28: `cargo test --workspace -- --test-threads=1` passed
5,007 tests with 51 ignored across 71 suites, including all 1,241 desktop tests.
After preserving parameter policies across the prepared-input boundary, 21
conventional-bridge tests passed. The subsequent callback-adoption checkpoint passed
the complete engine suite (342 tests plus its doctest).
`cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings` passed.

Native host ownership tests cover editor isolation and transfer, deferred instance
release, silent/disconnected output, stale document revisions, and rejection of
untrusted plugin files before the loader runs. A CLAP fixture processes audio before
checking both opaque state and readable parameter overrides: a queued override is
not evidence that the plugin has already received it.

The opt-in `native_host::external_tests::native_hosted_candidate_is_isolated_and_renders`
test prepares two native documents with the same processor IDs and different volume
settings. It renders those actual prepared graphs, checks their RMS difference and
the unchanged published host state, then verifies editor handover and retirement.
`AURIS_NATIVE_PLUGIN` names the installed instrument and `AURIS_NATIVE_CONTROL`
names its volume parameter. `AURIS_NATIVE_HOST_GUI=1` enables VST3 editor checks
(including real HWND preservation/count/destruction on Windows).
`AURIS_NATIVE_HOST_DEVICE=1` additionally queues a third candidate to a real output
device and requires callback adoption before publishing its revision.

On Windows, 2026-10-28, the complete native-host test group passed with each
installed instrument (13 tests per run, including the opt-in test):

| Instrument | Control | Prepared audio RMS, original to candidate | Real callback adoption | Native editor |
| --- | --- | --- | --- | --- |
| Vital VST3 | Volume | 0.1429963 to 0 | Passed | HWND preservation, handover and destruction passed |
| Surge XT CLAP | Global Volume | 0.0875456 to 0.0004495 | Passed | Covered by deterministic CLAP fixture |

Both runs preserved the old host's complete state while preparing the changed
candidate and released its owner after returning the old graph. These are numeric
and lifecycle checks; they do not evaluate musical quality. The real-device check
uses stopped transport and verifies adoption, without treating that acknowledgment
as a measurement of hardware output latency.

The host-integration checkpoint passed `cargo test -p auris-engine -p auris-session
-- --test-threads=1`: 1,541 tests passed and 11 opt-in tests were ignored across six
suites. Both installed-plugin runs above passed separately. Workspace Clippy with
warnings denied, workspace rustdoc with `RUSTDOCFLAGS=-D warnings`, formatting and
diff checks also passed.
