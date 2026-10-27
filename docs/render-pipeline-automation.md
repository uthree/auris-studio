# Native automation and capability resolution

The pre-activation model in `auris_core::pipeline::content` stores project-time
automation separately from its consumers. It complements the [ownership
contract](render-pipeline-ownership.md) and [typed topology](render-pipeline-topology.md).

## Authored curves and bindings

`AutomationDocument` owns one stable ID, interpolation policy and strictly ordered
point list. All ticks are absolute project ticks. Evaluation shares the existing
`AutomationLane` endpoint and interpolation implementation. A resolved control domain
declares units and inclusive bounds; invalid values are rejected without sorting,
clamping or dropping points.

A legacy lane whose parameter metadata is unavailable can retain `domain: None`.
Its point shape still validates, and serialization preserves it. `resolve_domain`
accepts authoritative metadata only when every preserved point is compatible;
failure leaves the prior document unchanged. Capability resolution reports an
unresolved curve instead of manufacturing a port domain.

An unowned automation processor references the curve. Each `AutomationBinding`
has a stable edge ID, that source node, a target processor/key and an enabled flag.
Several bindings can share one curve when their domains match exactly. Signal
connections live in `edges`; raw control edges there are rejected so each control
connection has one authoritative owner.

Disabled bindings retain their target, points and identity. Resolution still checks
their target, policy and domain, but excludes them from dependency edges and the
single-driver rule. `ResolvedContent::automation_value` returns `None` for a parameter
without an enabled binding, meaning the caller uses its stored static value. This is
an immutable preparation/inspection API, not a realtime callback lookup.

Branch duplication copies bindings aimed at the copied processors with fresh edge
IDs and remapped targets; curve sources stay shared. Explicit curve duplication
copies the curve only. Removing a group removes its incoming bindings, while shared
curves and source nodes survive. A curve cannot be removed while a source processor
still references it. All IDs share the existing monotonic namespace.

## Processor capabilities

`CapabilityProvider` supplies borrowed host metadata for each plugin processor.
The resolver verifies plugin identity and kind, stable parameter keys, ranges,
read-only flags, automation policy and value domains. Built-in ports come from their
processor kind. Gain, pan and send use the same core descriptors as session commands,
including the canonical keys `gain`, `pan` and `send`.

The resolved ports distinguish events, stereo audio, sidechains and scalar controls.
Parameters use namespaced `param:<key>` ports. Read-only, preparation-only and static
parameters have no control input. Offline singers and branch transforms expose only
their event/audio interface in this contract. A singer receives its guide instrument
through `guide_in`; its event input supplies offline synthesis material. Playback
selects the guide only when no take is declared. A stale take remains selected, and
a missing take asset means silence, preserving existing playback semantics.

`PipelineContent::resolve_pipeline` validates ownership first, resolves capabilities,
then validates signal and active control edges as one DAG. It also validates disabled
bindings, reports incompatible domains and duplicate active drivers, and returns
deterministic dependency order. Count and cumulative text budgets cover the complete
fragment, including disabled bindings, before unbounded cloning. Plugin factories,
audio devices and asset IO are outside this boundary.

## Verification

Core integration tests cover interpolation parity, malformed points/domains,
unresolved-domain round trips and atomic resolution, shared curves, disabled static
fallback, identity collisions, duplication/deletion, canonical command keys, plugin
metadata policies, incompatible domains, driver conflicts, mixed dependencies and
resource limits. Existing automation tests exercise the shared legacy evaluator.

The native project writer and runtime are enabled only at the M2 activation gate in
the [implementation plan](render-pipeline-plan.md).
