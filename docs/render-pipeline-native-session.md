# Native session integration

The normal Session now captures and prepares a native transition using its existing
engine and resources. Native document commands and guarded format-33 IO are available
as integration components. The default desktop, CLI and agent sessions now save
format-33 documents. They retain a single conventional editing authority while full
native runtime activation is pending.

## Ordinary Session persistence

Normal Save, Save As, autosave, recovery, checkpoints and song-candidate saves use
`stage_session_project` / `save_session_project`. `load_project` dispatches legacy
formats and native format 33. No UI command or model-facing tool must select a
different save operation.

The codec is distinct from renderer lowering. It decodes conventional ownership,
sources, insert chains, routes, sends and automation without creating plugins, then
re-encodes and checks the native document for loss. A file that cannot be represented
by the ordinary editor is refused before replacing the open document. Plugin stable
parameter keys survive decoding; the Session resolves runtime parameter positions.
Missing keys retain their curves under distinct unused transient parameter IDs, so
editing an available control cannot overwrite them. The renderer resolves stable
keys against prepared plugins and ignores unavailable bindings without deleting data.

The editable `Project` carries only a non-serialized immutable identity checkpoint and
stage-ID associations. Native groups, sources, nodes, edges and automation identities
survive compatible edits, saving and reopening. Undo retains allocator high-water
marks and stage reordering preserves stage IDs. There is no second editable native
document in this conventional runtime, and no legacy payload embedded in format 33.

Staging captures the exact destination bytes. Publication checks them again and
preserves the first legacy overwrite as `Name.auris.legacy-backup`. A conflicting
backup or changed destination prevents replacement. Cancelled/stale Session jobs do
not publish; only accepted saves adopt identity and location changes. Autosave adopts
its identity checkpoint while retaining dirty state. Recovery remains unsaved and
owns copied private assets until a permanent Save As.

This storage transition precedes full native command/host activation. The explicit
native editor remains the integration path for general graphs and source sharing.

## Session capture and preparation

`capture_native_transition` refuses active gestures, agent requests and recording,
collects actual hosted plugin state, converts once, and records the originating
session instance, revision and content digest. The returned `NativeSessionTransition`
owns a `NativeFileSession` with one `NativeDocumentEditor` and any isolated prepared host set. It retains no
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
native command authority switch. `open_native_transition` reads persistent native IDs
directly and uses the same existing Session engine for candidate preparation.

Native preparation resolves the candidate's asset folder, decodes newly landed audio,
and creates its own stretch cache. Matching immutable decoded buffers may be reused.
Missing audio is reported on the candidate, and a failed replacement preserves its
last successful prepared state. Save As changes the candidate's asset folder without
changing the conventional Session's folder.

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
| Instrument/effect replacement, parameter/state edits and bypass | Persistent processor and effect-slot identities |
| Effect insertion, removal, reordering and sidechains | Explicit insert edges and sidechain edges |
| Authored note replacement | One shared source clip; existing bounded note contract |
| Append, move and remove performance stages | Stable clip/stage identities |
| Explicit source and processing-branch copies | Core native allocator and ownership operations |
| Track creation, deletion and ordering | Source, processing group, strip and typed edges |
| Clip insertion, copy, removal, movement, name and mute | Canonical shared source and stable clip IDs |
| Completed recording file landing | Full-file metadata plus retained source-frame window |
| Tempo, meter and transport settings | Native project context with conventional snapping/history policy |
| Main routing and auxiliary sends | Group destinations, persistent connection IDs and pre/post-fader taps |
| Automation curves and shared bindings | Stable curve/source/binding IDs, built-in units and point validation |

Presentation-only edits retain the musical revision. Other edits currently request
preparation conservatively. Command receipts describe authored changes; callback
adoption remains a separate runtime observation. Plugin parameter edits store finite,
stable-keyed values. Interactive callers validate range and writability using current
host descriptors; state restoration does not require an available plugin.

Grid, document name, loop/punch switches, metronome and count-in update saved state
without adding Undo entries. They synchronize only their own fields across history;
authored loop/punch region edits remain undoable. Generated identities are returned
only after a whole batch commits. Recorded batches reject duplicate tracks or two
tracks sharing the same authored source.

The native mixer adapter accepts existing `ParamTarget` addresses and resolves them
through the validated command bindings. Gain, pan, send levels and automation point
editing therefore use the same stable track/send identities as the conventional
commands. Curve edits affect all bindings sharing that curve. Clearing one target,
including removing its last point, detaches that target and retains a curve still
used elsewhere. Main-route and send tap/destination changes retain connection IDs;
missing or ambiguous conventional routes are rejected instead of selecting an edge
by vector order.

`apply_plugin_command` resolves existing track, effect-slot, `PluginWindow` and
`ParamTarget` addresses to the native processors. Instrument replacement uses the
group's explicit primary instrument, including singer guides. Effect replacement
keeps slot identity and bypass state. Replacing an instance detaches its automation
bindings but preserves curves and source nodes, including curves used by other
instances. An identical replacement is a no-op; parameter and complete-state updates
retain bindings. Numeric parameter IDs never substitute for a missing stable key.

Insert commands require an unambiguous `Sum -> Effect* -> Strip` chain in one group.
Insertion reuses the split connection's ID; reordering preserves existing node,
slot and connection IDs. Removal reconnects the neighboring inserts and removes
only the deleted processor's bindings and incident connections. Sidechain retargeting
retains its connection ID. Topology edits reject cycles, ambiguous branches and
resource-limit violations before committing. Plugin capabilities are checked when
preparing the candidate, separately from document ownership.

Native preparation restores saved state into registry-created effects before handing
them to the render graph. Caller-supplied hosted effects retain their already restored
instance state; strip preparation does not overwrite it with the document snapshot.

The `native_plugin_workflow` regression captures a normal Session, edits its native
candidate, prepares actual built-in instruments/effects and renders stereo PCM. It
checks a -12 dB gain change, audible insert-order and sidechain changes, exact Undo/Redo
restoration, and Save As/reopen equality of identities and samples. Engine regressions
also distinguish registry-created state restoration from caller-owned instance state.
These are offline checks of prepared graphs, not audio-device callback adoption.

## Native persistence and recovery

`NativeFileSession` owns the editor, exact acknowledged disk bytes, an instance-local
save token and asset folder. Detached save work stages bytes; the owner verifies the
revision, generation, path and gesture before publication. A result from another
session or an older revision cannot publish or copy assets. External edits made even
before save begins are compared against the bytes acknowledged at open/save.

Explicitly opening a legacy file captures its bytes once. Its first in-place native
save publishes the current edited document and preserves an exact legacy backup.
Native opening and staging expose the actual source bytes, avoiding an independent
read that could accidentally acknowledge a different file version.

Save As copies inside audio/fonts and self-contained voice connections without
overwriting different existing assets. Checkpoints retain native documents beside
their asset folder and restore as one Undo step without lowering allocation counters.
Checkpoint restore rejects another project's history directory. Ordinary desktop
autosave discovery and crash recovery still use the conventional Session owner.

Native autosave jobs now copy inside assets and stage format-33 snapshots in leased
recovery workspaces. Publication checks owner identity, document revision, job
generation, permanent path, asset folder and transaction state. A stale, cancelled
or foreign completion cannot replace a recovery snapshot. Autosave leaves the
permanent save baseline and dirty state unchanged.

Native discovery uses a separate `native` registry subtree and the existing lease,
metadata and discard rules. Live owners remain hidden. Recovery opens an unsaved
native editor, copies its assets to a new leased workspace, and retains the original
snapshot until it is explicitly discarded. Save As publishes outside disposable
recovery storage before releasing the copied assets. The Session runtime can prepare
this recovered document through `recover_native_transition`.

Capturing an unsaved ordinary Session now retains its actual working asset folder
and unsaved status. Save As asset copies use a synced temporary file and no-clobber
publication, so a failed copy cannot leave a partial destination file.

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

The explicit transition supports a completed-WAV landing, native edit, checkpoint
restore, guarded native Save As and native reopen sequence. This does not migrate
`Session::stop_recording` or the runtime command authority. Ordinary save/open actions
now use the separate lossless persistence codec described above. Those routes cannot
run against a persistent mutable Project beside an independently editable native graph.
The render bridge's temporary Project remains private renderer input. Explicit native
IO round trips do not establish that the runtime has crossed this authority boundary.
See the [migration contract](render-pipeline-migration.md)
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

The native recording workflow regression writes a real WAV with an excluded leading
window, lands its metadata, edits the clip, removes it, restores a checkpoint, traverses
Undo/Redo, copies assets on Save As and reopens. It prepares and renders on the same
Session engine before and after reopen and compares samples and persistent IDs. It
does not open an input device or demonstrate the ordinary Record button workflow.

Ordinary `Session` recording finalization also retains the complete WAV in its source
registry and buffer bank. Count-in and punch exclusions are stored as clip offsets
and lengths. Previously only the in-memory buffer was sliced while its path still
named the full WAV, so reopening selected the wrong frames. The regression covers
both count-in and punch, normal Undo/Redo and save/open with sample-equal renders,
then native capture, clip editing, checkpoint restore, native Save As and asset reload.
It exercises the recorder's actual landing helper with finalized WAV files, without
opening a microphone device. Default document authority remains conventional.

### Host-integration baseline, 2026-10-28

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

### Native editing and persistence validation, 2026-10-28

`cargo test --workspace -- --test-threads=1` passed 5,104 tests with 53 ignored across
71 suites. The focused native Session run passed 109 tests with two installed-plugin
tests ignored. Workspace Clippy and rustdoc with warnings denied also passed. These results cover
the explicit native recording-file/edit/checkpoint/save/reopen workflow described
above; they do not establish default Session authority activation or microphone capture.

After the ordinary recording-window correction, the full serial workspace run passed
5,105 tests with 53 ignored across the same 71 suites. Workspace Clippy, formatting
and Session rustdoc with warnings denied also passed. The added test covers both
count-in and punch windows through normal save/open and the explicit native workflow.

### Native routing and recovery validation, 2026-10-28

The routing regression lands an actual WAV through a captured Session, changes its
main route, adds a send, and writes gain automation using the existing parameter
address. Rendering verifies a -12 dB change and the different result of switching
the send to pre-fader. Format-33 save/reopen retains the document IDs and produces
sample-equal stereo output.

The recovery regression publishes a native autosave containing a recorded source
window, removes the original unsaved Session's working folder, discovers the
abandoned snapshot, and recovers it through another Session's runtime. It then
edits, traverses Undo/Redo, saves to a permanent folder and reopens. Source samples,
clip offsets and persistent IDs are retained. Separate checks cover a blocked
primary registry, stale/foreign/out-of-order completions, cancellation, active
gestures, post-save invalidation, recovery-folder Save As rejection and injected
asset-copy failure. These are explicit native APIs with finalized WAV files, not
physical microphone capture or default desktop native activation.

The final serial workspace run passed 5,136 tests with 53 ignored across 71 suites.
Workspace Clippy (`--all-targets -- -D warnings`), workspace rustdoc
(`RUSTDOCFLAGS=-D warnings`), formatting and diff checks passed on the same code.

### Ordinary UI persistence validation, 2026-10-28

The desktop harness exercises the normal background Save/Open paths after edits and
Undo/Redo, and the recovery prompt through permanent Save As and reopening. These
checks inspect the actual format-33 envelope and adopted Session state.

The normal recovery regression uses a real WAV in an abandoned private workspace,
recovers it into a new Session, saves to a permanent project folder, and reopens it.
Offline stereo PCM is sample-equal before recovery, after recovery and after reopening.
The existing ordinary recording-finalization test now passes through the same native
writer. These tests do not exercise a physical microphone or make a listening claim.

Additional checks cover exact legacy backups, publication conflicts, cancellation,
stale save jobs, source/stage IDs through Undo and new allocations, checkpoints,
unsupported graph rejection, and automation keys whose installed parameter positions
have changed or disappeared. Runtime automation uses prepared plugin descriptors;
unresolved curves remain saved and cannot overwrite an available parameter's lane.

The final serial workspace run (`cargo test --workspace --no-fail-fast --
--test-threads=1`) passed 5,159 tests with 53 ignored across 71 suites, including
1,242 desktop tests, 1,283 Session tests and 199 IO unit tests. Workspace Clippy
(`--all-targets -- -D warnings`) also passed.

Workspace rustdoc (`RUSTDOCFLAGS=-D warnings`), formatting and diff checks passed.
The workspace-built `external_hosted_parameters` test executable was then run
separately for two installed instruments, without opening editors or an audio device:

| Instrument | Control | Left-channel RMS before / after setting minimum |
| --- | --- | --- |
| Vital VST3 | Volume | 0.12513068 / 0 |
| Surge XT CLAP | Global Volume | 0.06997609 / 0.00173658 |

Both runs passed live tool edits, Undo/Redo, ordinary native saving, saved-project
tool edits, no-op retry byte preservation, reopening and rendered-level restoration.
The RMS measurements establish a signal change, not a musical-quality judgment.
