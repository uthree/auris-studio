# Native conventional render bridge

`auris_session::native_conventional::lower_native_conventional` prepares an
ephemeral renderer snapshot from a `NativeDocument`. The native document owns the
authored material and identities. The returned `Project` is renderer input only;
it must never be saved or used to replace the native document.

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

These tests establish numerical compatibility for their fixtures. They do not make
a listening assessment or establish behavior of an external plugin or singer model.

Validation on 2026-10-28: `cargo test --workspace -- --test-threads=1` passed
4,987 tests with 51 ignored across 71 suites, including all 1,241 desktop tests.
The final 15 conventional-bridge tests were rerun successfully after adding the
continuous-control policy guard. `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings` passed.
