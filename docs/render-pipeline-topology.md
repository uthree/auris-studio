# Typed topology foundation

`auris_core::pipeline` is the first part of M2a in the
[implementation plan](render-pipeline-plan.md). It validates resolved processor
interfaces and their dependencies without loading plugins or allocating audio
buffers. It does not alter `Project`, the format-32 writer, playback or the existing
M1 projection. The native ownership and compiler work will supply these descriptors
from the canonical model; arbitrary descriptors alone cannot establish processor
capabilities or runtime availability.

## Identities and ports

`NodeId` and `EdgeId` are separate types. A `PortId` is a stable processor-defined
string key, unique across both input and output directions within one node.
Connections identify both endpoints and retain independent edge IDs for parallel
paths. Node vector order and edge vector order are not processing order.

| Data | Compatibility | Input cardinality |
| --- | --- | --- |
| Events | Event stream to event stream | One driver |
| Audio | Identical explicit layout | One driver, or an explicitly declared sum |
| Control | Identical representation, unit and finite range | One absolute driver |

Audio ports distinguish mono, stereo and an ordered discrete-channel layout.
There is no inferred upmix, downmix or summing operation. A sidechain is an explicit
audio input role; the producing port remains an ordinary audio output. Each edge's
role must match the receiving port. Fan-out is valid for all three data types.

Normalized control ranges stay within 0..=1. Native control domains use `ParamUnit`
and their actual value ranges. A normalized value and a native value with the same
numeric range are distinct contracts. A mapping processor must declare the input
and output representations of the conversion it implements.

## Parameter bindings

Each parameter has a stable string key, a read-only flag, an evaluation policy and
an optional control-input reference. Static metadata does not require a control
port. A parameter input represents the single absolute driver of that parameter.
Two keys cannot alias the same driver port, and duplicate keys are rejected rather
than overwritten during construction.

A connected parameter must be writable and declare a continuous or event-sampled
evaluation contract. Preparation-only, regeneration-required and structural
parameters reject incoming automation. This is a descriptor-level check: it does
not enable new runtime automation or evaluate a curve. Static fallback values and
curve ownership belong to the subsequent native model.

Generic control inputs can also describe mapping processors. Their inputs need not
be parameter targets. The processor owner is responsible for distinguishing an
ordinary control stream from a parameter's driver in the resolved interface.

## Validation and diagnostics

The caller supplies budgets for node, edge, port and parameter counts, plus total
UTF-8 bytes in keys and references. The validator checks these before building
indexes, including duplicate text in endpoint and parameter-port references. These
budgets bound validator work; decoding, event expansion, plugin state and render
buffers require their own limits. The module deliberately provides no default
product capacity before compiler measurements establish it.

Validation checks identities, port declarations, parameter bindings, endpoint
existence/direction, domains, edge roles and input drivers. Errors retain affected
node, port or edge identities; a driver conflict identifies both connections.
Disconnected nodes and empty graphs are valid. Validation never repairs routing,
adds a master output or changes a candidate in place.

All edge domains participate in one dependency graph. A successful result orders
all nodes, choosing the smallest ID among the nodes ready to execute. A cycle
diagnostic contains a closed node path and the exact edge for each consecutive
pair. Iterative traversal avoids a call-stack limit on deep graphs, and excludes
acyclic tails downstream of a cycle from the reported path.

The result belongs to the inspected snapshot. Any mutation requires revalidation;
the returned dependency order is not a prepared realtime graph.

## Verification

`crates/auris-core/tests/pipeline_topology*.rs` exercises layered event/audio/control
dependencies, explicit sums, shared automation, mixed-domain cycles, malformed
references, parameter-policy rejection and resource budgets. The downstream-tail
regression first reproduced an invalid one-node/zero-edge cycle report, then checks
that the report identifies the actual closed cycle. Deep-graph coverage checks the
iterative traversal with 10,000 nodes. An independent transitive-closure oracle checks
all 65,536 directed graphs of four nodes, including self loops, and verifies every
reported cycle edge. These tests construct no plugins, play no audio and write no projects.

Validation on 2026-10-27: `cargo test --workspace -- --test-threads=1` passed
4,867 tests with 51 ignored, including the 30 new topology tests and 1,241 desktop
tests. The desktop additions cover connection-to-track navigation and actual
minimum/maximum zoom with keyboard traversal at 640×480 in both languages and themes.
Formatting, `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings` passed.

The next M2a unit adds authored-source and stable stage ownership, group references
and conventional-command identity adapters. Native saving remains gated on migration,
compiler and command-adapter acceptance as specified in the implementation plan.
