# Render-pipeline performance baseline

This is the M0 release-build baseline for the document-to-render pipeline. The executable is
[`pipeline_baseline.rs`](../crates/auris-session/examples/pipeline_baseline.rs); the checked-in
machine-readable result is
[`render-pipeline-performance-baseline.json`](fixtures/render-pipeline-performance-baseline.json).

Run it from the repository root with the cache wrapper disabled on Windows:

```powershell
$env:RUSTC_WRAPPER=''
cargo run -p auris-session --release --example pipeline_baseline
```

The workload is deliberately synthetic and stable within a build: 8 and 64 instrument tracks,
256 authored notes per track, 48 kHz, 256-frame blocks, `auris.synth.chiptune`, and a master
`auris.fx.limiter`. It measures three document/graph builds and reports their median, then
separately measures the first callback, 127 steady callbacks, and 128 callbacks that seek to
frame 1900 in a 1024..2048 loop. Each of the latter callbacks crosses the loop end, and the
transport is asserted to wrap to frame 1132. The render graph is asserted to have no resource
error before rendering.

The callback allocation window counts allocations, zeroed allocations, reallocations, requested
bytes, and deallocations through a local global allocator. The window is RAII-scoped, so it is
disabled during cleanup and also when unwinding. The observed callback counts are zero in this
baseline. This is a measurement of this executable's selected render intervals, not a claim that
unrelated setup or plugin-host code never allocates.

The executable renders the same workload twice through fresh graphs and asserts exact sample
equality, finite samples, and non-zero output. It intentionally does not record an archival sample
hash: DSP implementations and floating-point behavior are same-build determinism contracts, not
cross-platform file-format compatibility guarantees.

## Reference machine

The checked-in JSON was produced on:

- 2026-10-27, with the renderer/project implementation at `8807f0d6`
- Windows 11 Home `10.0.26300`
- AMD Ryzen 7 9800X3D, 8 cores / 16 threads
- 32,564,596 KiB RAM (approximately 31.1 GiB)
- `rustc 1.99.0 (b940084d7 2026-09-28)`, host `x86_64-pc-windows-msvc`, LLVM 23.1.1
- command: `cargo run -p auris-session --release --example pipeline_baseline`
- workspace release profile: thin LTO, one codegen unit

Callback `microseconds` fields are aggregate window totals (one, 127, or 128 blocks),
not maxima or per-block deadlines. The host was also used for development builds;
repeat on an idle machine when comparing timing regressions. This short fixture
does not substitute for device-driver or third-party-plugin stress testing.

The JSON values are observations rather than pass/fail limits. Re-run on the target release
machine when changing DSP, graph construction, or the project snapshot representation.

Detailed correctness coverage for tempo, event positions, looping, seeking, latency compensation,
tails, and realtime/offline equivalence remains in the deterministic `auris-engine` tests. In
particular, see `graph::schedule::tests`, `renderer::tests`, and `offline::tests` in
`crates/auris-engine/src`.
