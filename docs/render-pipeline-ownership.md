# Native pipeline ownership

`auris_core::pipeline::content` is the second M2a preparation slice. It is an
independent serializable ownership fragment, not a second editable store inside
the current `Project`. The system boundary is documented in
`auris_session::guide`; delivery follows [the pipeline plan](render-pipeline-plan.md).

## Authoritative owners

| Type | Owns | Refers to |
| --- | --- | --- |
| `SourceDocument` | Note/singer or audio clips | Audio assets through existing clip references |
| `StableMidiClip` | Notes, curves, recipes, ordered `ClipStage` entries | Retained `ClipId` and persistent `StageId` |
| `Processor` | Instrument, singer, effect, strip, send or branch-transform state | Optional owning `GroupId`; source nodes reference `SourceDocumentId` |
| `ProcessingGroup` | Group identity and explicit command roles | One owned strip and optional primary instrument |
| `TrackBinding` | Retained `TrackId` and conventional command roles | Optional source and home processing group |
| `PipelineEdge` | The connection | Stable endpoint node/port IDs |

Source nodes have no group owner. Multiple tracks and processing branches can
refer to one source without copying its clips. Every other processor belongs to
one group. A singer's guide instrument is an ordinary instrument node; offline
voice settings and the existing synthesized take belong to a separate singer
node. No synthesis, plugin loading or asset IO occurs in these data operations.

Effect payloads retain `EffectSlotId`, and send payloads retain `SendId`. Inserts,
sidechains, send taps and destinations are represented by edges rather than a
second route list in processor payloads. Group membership creates no connection.
Assets, timeline presentation and transport metadata remain outside this fragment.

## Stable clip stages

`MidiClip<T = NoteTransform>` retains the current default clip shape and performer.
`StableMidiClip` selects `ClipStage { id, transform }` for each ordered entry; it
does not retain a parallel transform stack. Explicit `with_stage_ids` conversion
preserves authored fields, order and seeds. `performer_clip` projects back to the
existing performer shape for preparation; it is not an audio-callback operation.

Stage move, replacement and removal resolve `StageId`. Reordering changes the
local index while keeping identity attached to its transform. Zero, duplicate
and missing stage IDs fail before mutation. These local methods validate the clip;
the complete ownership validator additionally rejects collisions across clips
and other object types. New IDs must come from the containing document allocator.

## Identity and validation

The fragment uses one numeric identity namespace across sources, groups, nodes,
edges, stages and retained track/clip/effect/send IDs. Callers supply every
externally owned identity, including audio assets, to validation and counter
repair. References do not allocate identities. `next_id` is a checked high-water
mark: repair can raise it, deletion never lowers it, and allocation refuses
overflow before changing the original candidate.

`validate_ownership` checks duplicate IDs, dangling owner references, group roles,
explicit primary instruments and edge endpoints. Count budgets are supplied by
the caller and checked before identity indexing or copy allocation. These bounds
cover identity work, not serialized bytes, note expansion or plugin payload size.

Ownership validation is one stage of full validation. The separate
[`Pipeline` topology contract](render-pipeline-topology.md) checks resolved ports,
parameter policies, cardinality and dependency cycles. Asset existence, source
event compatibility and processor capabilities require their owning registries.
An ownership-valid copy is therefore a candidate, not a publishable execution
plan. In particular, copying an outgoing control edge can introduce a second
driver; full validation must reject it before publication.

## Copy and removal operations

Each operation validates the original fragment, constructs a candidate and checks
the candidate before replacing it. A returned error preserves all original data
and the ID counter.

| Operation | Result |
| --- | --- |
| `duplicate_source` | Independent source and clips with new source/clip/stage IDs; authored values, seeds and asset references preserved; consumers unchanged |
| `duplicate_branch` | New group and owned processors, independent plugin states and effect/send IDs; shared source nodes retained; incident edges copied with owned endpoints remapped |
| `remove_group` | Owned processors, incident edges and home-track bindings removed together; source documents and source nodes retained |
| `remove_source` | Removal only after all direct track and source-node references are detached; otherwise returns the affected references |

Branch copies return typed old-to-new maps for nodes, effects, sends and edges.
Source copies return maps for clips and stages. Branch copying creates no timeline
binding or implicit output route. The master group cannot be copied or removed.
Group deletion deliberately retains unused sources; future garbage collection
must be an explicit document operation.

## Existing-command identity boundary

`command_bindings` borrows an ownership-validated snapshot. That immutable borrow
prevents mutation while callers resolve conventional addresses.

| Existing address | Native resolution |
| --- | --- |
| Track score/clip edit | Track's authored source |
| Track mixer edit / absent track | Home-group strip / master strip |
| Track instrument edit | Explicit primary instrument; no membership/order fallback |
| Effect slot / send ID | Corresponding node, with owning-group verification |
| Clip ID | Authoritative source containing the clip |
| Clip + stage ID | Source and current stage position; the position is temporary |
| Built-in parameter target | Strip/send node and `gain_db`, `pan` or `level_db` |
| Plugin parameter target | Resolved instance node plus caller-supplied stable metadata key |

Numeric plugin parameter indexes are insufficient for a persistent address.
Missing keys fail; an unknown supplied key still needs capability validation.
This resolver does not execute session commands or modify agent tool schemas.

## Verification and activation

Focused core tests cover stable-stage round trips and performer parity, shared
sources, independent copies, retained command identities, ownership failures,
limits and atomic counter handling. Existing project-format and rendering tests
continue to exercise the default `MidiClip` representation.

Validation on 2026-10-28: `cargo test --workspace -- --test-threads=1` passed
4,902 tests with 51 ignored across 61 suites, including 35 new ownership, copy,
binding and stable-stage tests. All 1,241 desktop tests and the historical
project-format fixtures passed. `cargo clippy --workspace --all-targets -- -D warnings`
and the formatting checks passed. `cargo doc --workspace --no-deps` also passed
with `RUSTDOCFLAGS=-D warnings`.

The current project writer remains format 32. Native activation requires the
remaining automation/capability model, versioned conversion, conventional-project
compiler and the complete session command/history audit. Integrate these through
the plan's activation gate rather than enabling only the writer.
