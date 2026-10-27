# Inspecting the rendering pipeline

`pipeline_read` is the shared live/saved read tool for the document's sources,
clip performance, instruments, singer takes, effects, faders, sends and
automation. It projects the existing project model and makes no edits or saves.
The saved tool takes `project`; the live tool uses the open document and omits it.

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

`instrument:track:N` and `singer_take:track:N` identify the current track-owned
roles, not persistent processor or take instances. Replacing an instrument or
regenerating a take changes the payload under that role ID. Read the new revision
before interpreting its plugin or source; these IDs are not native branch IDs.

`revision_scoped` marks stage paths and plugin automation bindings. A stage's
index changes meaning after reordering, and a plugin's numeric parameter index
can change after reload. Read again after edits; the fingerprint is diagnostic,
not a persistent stage identity. Project serialization remains unchanged.
