# Inspecting and editing the rendering pipeline

`pipeline_read` is the shared live/saved read tool for the document's sources,
clip performance, instruments, singer takes, effects, faders, sends and
automation. It projects the existing project model and makes no edits or saves.
The saved tool takes `project`; the live tool uses the open document and omits it.

## Desktop Inspector

Select a track or clip, then choose **Pipeline details** in Signal flow or the
command palette. The same section can be expanded directly in the Inspector.
**Track** and **Clip** choose the scope; selecting a row's owner navigates to that
track or clip using the normal selection. Enter and Space toggle the focused
disclosure. The section shares the Inspector's scrolling and remains collapsed
until opened.

Rows distinguish notes, audio and control data. Performance stages retain their
stored order and indicate deferred curve evaluation; sends identify their tap and
destination. Singer guide and cached-take labels describe their roles without
claiming that a take is fresh or its audio is available. This list describes the
document's structure, not a linear callback execution schedule.

The Inspector keeps one page of up to 32 nodes. Use **Previous** and **Next** for
larger scopes. Selection changes and document edits, including Undo, refresh the
page; unchanged paints reuse it. Disclosure, filtering and paging are transient
view state and create no project edits, audio rebuilds or Undo entries.

Each performance row's **Reorder or remove…** menu moves that exact stage earlier
or later, or removes it. This distinguishes repeated stages of the same kind.
Each operation is one Undo step and preserves the remaining payloads and seeds.
If the project changes while a menu is open, reopen the menu before editing.
Effect rows offer **Parameters…** and the existing actions menu for ordering,
bypass, removal and routing. They address stable effect slots.

**Edit automation** reveals the row's track-owned timeline lane, selecting its
track and opening the timeline if necessary. Repeating it keeps the lane open.
A missing descriptor or a saved plugin key that no longer matches is reported
before navigation.

The focused UI has model and gesture tests for cache reuse, pagination, selection,
Undo, stage/effect order, send taps and singer labels. A Windows debug build was
also exercised with a built-in-instrument fixture containing 51 projected nodes:
Signal flow entry, page continuation, clip selection, keyboard disclosure and
Japanese text at a narrow Inspector width were checked in the native window.
The chain actions were then exercised in a 17-node built-in fixture: stage move
and Undo, opening an effect's parameter window and actions menu, and repeated
navigation to an existing automation lane. The narrow Japanese layout kept the
actions within the Inspector. Removal and stale-menu behavior are covered by
the gesture harness.

### Selecting connections by keyboard

Use **Select connection** to select the first visible, selectable connection.
With the mixer canvas focused, unmodified Left/Right and Up/Down traverse those
connections in stable graph order and wrap at either end. Enter or Return opens
the selected connection's existing action menu, where **Delete connections** remains
the existing destructive command. Escape clears the selection; an open menu keeps
first claim on Escape. Modified arrows remain canvas navigation and do not change
the selection. Hidden and internal chain edges are not keyboard targets. These
actions only change transient selection/menu state until an existing menu command
is chosen.

## Agent read tool

```json
{
  "project": "/absolute/path/Song/Song.auris",
  "scope": "track:1",
  "limit": 32,
  "edge_limit": 64
}
```

Omit `scope` (or use `all`) for the whole project. Use `clip:2` to inspect a clip,
or any other returned node ID such as `effect:3` to inspect that node. Focused scopes
include immediate incoming/outgoing neighbors, so boundary connections remain understandable.
Unknown scopes are errors rather than silently empty results.

For a bounded intermediate snapshot, pass one exact node ID returned by the graph:

```json
{
  "project": "/absolute/path/Song/Song.auris",
  "scope": "clip:2/stage:1",
  "detail": true,
  "at_tick": 0,
  "expected_revision": "copy-from-current-read"
}
```

Detail mode rejects omitted or non-exact scopes, nonzero cursors and paging limits. Clip stages
return stored transform settings and bounded input/output note samples (at most 32
per boundary); stage evaluation is deliberately pass zero. Fader, send and
automation nodes return native parameter values, including static, automation and
effective values when available. `at_tick` is an absolute project tick for those
parameter values and must be nonnegative. Unsupported nodes return an explicit
unavailable result. Unverified parameter bindings retain the stored curve value,
with no claimed effective value, and never load a plugin. Detail reads use the
same content revision guard as graph pages and never write the project, history or
a checkpoint.

The desktop Inspector is a presentation of this read: it shows counts and at most
four note rows for each stage boundary, plus evaluation/bounded/unavailable status.
The agent detail response also includes stored settings. Press **Refresh state**
explicitly to capture the current playhead
tick; repainting or moving the playhead does not silently replace a cached snapshot.

### Detail and keyboard verification

The gesture harness covers parallel-send traversal, pointer/keyboard selection,
menu navigation and dismissal, deletion of the exact selected route and Undo.
State snapshots are read-only and invalidated on edits and Undo. Narrow Inspector
checks exercise English/daylight at 13px and Japanese/midnight at 16px; the native
Japanese preview at 220px also shows the before/after note rows without horizontal
clipping. Session and tool tests cover ordered stage boundaries, deferred pitch
controls, sample and workload limits, live/saved parity, stale revisions and exact
node reads with more than 64 immediate neighbors.

The native preview also distinguished parallel sends #17 (pre-fader) and #18
(post-fader), navigated the connection menu with Enter/Down/Escape, and resumed
connection traversal after dismissal. Outside the focused graph, Right retained
its transport behavior. The fader snapshot stayed at tick 0 after seeking and
changed to tick 240 only on **Refresh state**; selection and reads left the project
clean.

Validation on 2026-10-27: formatting, the all-target workspace build, Clippy with
warnings denied, and documentation generation with warnings denied passed. The
parallel workspace run passed 4,834 tests with 51 ignored; one existing agent
permission test encountered a deny-list rule from shared test configuration.
The full desktop test binary then passed serially: 1,239 passed, 13 ignored.
All other workspace suites, including the new session/tool tests and doctests,
passed in the workspace run.

## Reading pages

Nodes and edges have independent streams. A response contains `revision`,
`total_nodes`, `total_edges`, `next_offset` and `next_edge_offset`. Keep the same
scope and revision when continuing. Node limits are capped at 64; edge limits
at 128. An edge's endpoints may be on another node page in the same snapshot.

For each stream, use its next offset. If it is exhausted while the other stream
continues, use its total count to keep it exhausted. Restart both offsets at zero
after a revision mismatch; reusing old offsets with a new revision can skip data.

```json
{
  "project": "/absolute/path/Song/Song.auris",
  "scope": "track:1",
  "offset": 32,
  "edge_offset": 64,
  "expected_revision": "copy-the-first-response-revision"
}
```

Errors expose a machine-readable `code` with a recovery `message`:
`revision_required`, `revision_mismatch`, `unknown_scope`, `invalid_offset`,
or `serialization`. A continuation without a revision is rejected. Reading
does not create Undo entries or saved-project checkpoints.

## Interpreting the structure

- `inputs`, `outputs` and edge `domain` distinguish event, audio and control data.
  Instruments convert events to audio; automation targets a control input.
- `track:N` is the strip's summing input, followed by inserts and an explicit
  `fader:track:N`. A pre-fader send taps after inserts, before strip gain/pan/mute;
  output branches and sidechains leave the post-fader signal. Actual mute/solo
  audibility, fades and latency buffers belong to the renderer.
- `send:N` has its own gain and automation target. The numeric send ID is the
  same one used by `mixer_graph`.
- Clip stages retain stored order and a transform fingerprint. Their `evaluation`
  distinguishes per-pass note operations, pitch/controller gestures evaluated
  after the notes, and composite drum-voice stacks. These are authored dependencies,
  not a callback execution schedule.
- A singer take has an audio source and an offline `synthesis_dependency` from
  its score. A score edit does not regenerate that take. Its guide instrument
  remains audition-only when registered take metadata exists. Missing metadata
  selects the current renderer's guide fallback. A registered source does not
  prove that the corresponding audio buffer/file is available.
- Automation reports its saved key, current target, interpolation and point count.
  Mixer units are known structurally. Plugin parameter resolution is unchecked:
  reading graph structure neither creates plugins nor guesses their parameter list.

The graph describes the saved/open document. It does not assert that this exact
revision is currently audible, that a plugin has loaded, or that a singer take
is fresh. Use `inspect_singer`, `inspect_instrument`, `automation` and
`mixer_graph` parameter inspection for their corresponding detailed states.

## Editing the owners

Use existing musical commands: `set_performance` for clip performance,
`automation` for parameter curves, and `mixer_graph` for inserts, routing and
mix controls. Only `track:N` and `effect:N` are mixer node addresses; other
projection IDs identify dependencies, not new mixer editing endpoints.

For an individual performance stage, `pipeline_edit` accepts the exact node ID
and content revision returned by `pipeline_read`:

```json
{
  "project": "/absolute/path/Song/Song.auris",
  "node": "clip:2/stage:1",
  "expected_revision": "copy-the-read-response-revision",
  "action": "move_earlier"
}
```

The actions are `move_earlier`, `move_later` and `remove`. The live tool omits
`project`. A successful response includes `changed` and the resulting `revision`.
Read the clip again after a change before addressing another ordinal; the
response's `node` identifies the request and does not follow the moved stage.
Stale revisions and unknown targets are rejected. Moving the first stage earlier
or the last stage later changes nothing. Saved no-op and rejected requests leave
file bytes and checkpoints unchanged. Session commands record discrete Undo steps;
the live agent retains its usual turn-level grouping. Saved edits use the existing
project checkpoint workflow.

`instrument:track:N` and `singer_take:track:N` identify the current track-owned
roles, not persistent processor or take instances. Replacing an instrument or
regenerating a take changes the payload under that role ID. Read the new revision
before interpreting its plugin or source; these IDs are not native branch IDs.

`revision_scoped` marks stage paths and plugin automation bindings. A stage's
index changes meaning after reordering, and a plugin's numeric parameter index
can change after reload. Read again after edits; the fingerprint is diagnostic,
not a persistent stage identity. Project serialization remains unchanged.
