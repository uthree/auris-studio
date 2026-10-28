# Native session integration

The normal Session now captures and prepares a native transition using its existing
engine and resources. Native document commands and guarded format-33 IO are available
as integration components. The default desktop, CLI and agent sessions still edit
and save format-32 `Project` documents. M2 activation is still pending.

## Session capture and preparation

`capture_native_transition` refuses active gestures, agent requests and recording,
collects actual hosted plugin state, converts once, and records the originating
session instance, revision and content digest. The returned `NativeSessionTransition`
owns a `NativeDocumentEditor` and any isolated prepared host set. It retains no
mutable legacy Project and owns no audio device.

`prepare_native_transition` validates the origin before collecting host state again
and rechecks afterwards. It uses the current engine sample rate/block geometry,
rate-matched audio bank, registry, project folder and configured plugin locations.
An explicitly opened CLAP library retains its exact-file permission. Candidates do
not change the audible graph, meter/scope readers, editor bindings or legacy history.
Failed replacement retains the previous prepared candidate. A later native edit
invalidates the prepared revision until it is prepared again.

The extracted `NativeHostManager` borrows the existing engine/device for its runtime
operations. The standalone `NativeHostedPlayback` delegates to that same manager,
so fixture and installed-plugin tests exercise the shared preparation/adoption path.
The owner must destroy device/engine graphs before the manager's published hosts.
The transition currently prepares only: callback publication waits for the complete
native command authority switch.

## Native commands and history

`NativeDocumentEditor` owns the current native document, saved-state comparison and
bounded native snapshot history. It never reconverts a Project after an edit. All
commands enter through one candidate validation and commit boundary:

1. Check the expected document revision and transaction owner.
2. Check the batch's operation count and encoded byte budget before cloning state.
3. Apply commands to a private native candidate and validate its ownership.
4. Commit the candidate and one history entry, or retain it within the owner's gesture.
5. Advance the document revision and, for audible edits, the musical revision.

The current batch limits are 256 operations and 1 MiB of encoded payload. These are
command-interface limits, separate from caller-supplied ownership and graph limits.
No-op and rejected batches preserve revision, redo and dirty state. An agent request
cannot edit or finish a human gesture, and different agent request identities cannot
share one. Gesture cancellation restores its initial state and retains the old redo
branch. Undo and Redo preserve native identities and never lower the allocator's
high-water mark. A save acknowledgement must match the current revision and cannot
finish an open gesture.

Implemented adapters cover:

| Command family | Native ownership |
| --- | --- |
| Track name, color and height | Track presentation |
| Strip gain, pan, mute and solo | Persistent strip processor |
| Effect bypass | Persistent effect processor |
| Authored note replacement | One shared source clip; existing bounded note contract |
| Append, move and remove performance stages | Stable clip/stage identities |
| Explicit source and processing-branch copies | Core native allocator and ownership operations |

Presentation-only edits retain the musical revision. Other edits currently request
preparation conservatively. Command receipts describe authored changes; callback
adoption remains a separate runtime observation. Plugin parameter commands need
host-supplied descriptors and policies before they can join this boundary.

## Remaining activation work

The audit found direct Project mutation and snapshot ownership across the existing
Session. Activation must migrate these routes together:

- Track/clip insertion, deletion and duplication; routing, sends and automation;
  instrument replacement and parameter writes.
- Recording/monitoring, audio and MIDI imports, composer and sound preset application,
  singer work, freezes and candidate adoption.
- Ordinary undo transactions, agent history and checkpoints, external reload/merge,
  autosave/recovery, asset collection, Save As and detached save completions.
- Callback-adopted readers, native command handles, document/audible revisions and
  reconciliation of editor snapshots while another graph is pending.

Those routes cannot run against a persistent mutable Project beside a native graph.
The render bridge's temporary Project remains private renderer input. Explicit native
IO round trips do not establish that ordinary Session reopen/save or recovery has
crossed this authority boundary. See the [migration contract](render-pipeline-migration.md)
for guarded publication and backup semantics.

## Verification scope

The focused tests cover native atomic batches, stale revisions, gesture ownership,
shared-source edits, stage identity through undo and save/reopen, monotonic allocation,
payload budgets, and session capture/preparation without changing its audible readers.
An actual built-in render checks that a native gain edit changes prepared samples and
Undo restores them. Filesystem tests cover backup conflicts/aliases, in-place upgrades,
changes during staging, stale native updates, lock contention and dropped staging files.
The existing installed CLAP/VST3 test exercises the same extracted host manager.

The additional installed-plugin regression opens a legacy project through the normal
Session, captures nonempty opaque instrument state, saves and reopens that native
snapshot, and prepares the candidate twice on the Session engine. It checks native
identity/state equality and unchanged conventional state and revision after preparation.
This is explicit native IO plus Session preparation, not ordinary Session native reopen.

These checks establish document/runtime behavior, not subjective musical quality.

### Windows validation, 2026-10-28

`cargo test --workspace` passed 5,063 tests with 52 ignored across 71 suites,
including all 1,241 desktop tests. The final focused native run passed 16 IO tests
and 70 Session tests with two installed-plugin tests ignored. The additional
normal-Session installed-plugin regression was added after the workspace build and
then executed in both separate installed-plugin runs below.

Each installed-plugin run passed all 14 native-host tests, including both opt-in
regressions. Both checked capture, native IO round trips, repeated preparation and
real callback adoption. The first two host generations are rendered directly for
the numeric comparison; the real-device adoption uses stopped transport.

| Instrument | Prepared RMS, original to candidate | Editor check |
| --- | --- | --- |
| Vital VST3, Volume | 0.1429963044 to 0 | Real HWND preservation, transfer and destruction |
| Surge XT CLAP, Global Volume | 0.0895036133 to 0.0004383253 | Deterministic CLAP fixture |

The commands use `AURIS_NATIVE_PLUGIN`, `AURIS_NATIVE_CONTROL` and
`AURIS_NATIVE_HOST_DEVICE=1` with `cargo test -p auris-session --lib native_host --
--include-ignored --test-threads=1 --nocapture`. The Vital run additionally sets
`AURIS_NATIVE_HOST_GUI=1`.

`cargo clippy --workspace --all-targets -- -D warnings`, workspace rustdoc with
`RUSTDOCFLAGS=-D warnings`, formatting and diff checks also passed.
