# Offline render generations

The offline renderer prepares independent tracks concurrently with Rayon. It derives dependency
generations from the existing topological order, including bus outputs, pre/post-fader sends and
sidechain keys. Track-local source rendering, insert processing, compensation and mono folding
run in parallel. After a barrier, the caller applies sends, faders, mute, taps and output mixing
in the original order within that generation. The master runs last. No audio buffers are cloned
for scheduling, and there are no concurrent writes to shared mix buffers.

Realtime playback retains its serial schedule and never dispatches to Rayon. Caller-placed
instruments/effects also stay on the rendering caller's thread; their native worker-migration
contracts have not been established. Short segments (under 256 frames), generations with fewer
than four active worker-eligible tracks, and malformed dependency orders use serial processing.
Construction, parameter automation, resets and destruction remain on the caller.

The generation order is deterministic, but can change floating-point summation order relative
to the original depth-first schedule. Small rounding differences are accepted; output length,
sample/event timing, routing dependencies and per-instance state progression are preserved.
Tests require finite samples and a per-sample error no larger than
`8 * f32::EPSILON * (1 + abs(reference))`. This is a tested bound for the fixtures, not a
universal error bound for arbitrary nonlinear external plugins.

## Windows measurements

Measured on 2026-11-04 with a Ryzen 7 9800X3D (8 cores / 16 logical processors), Windows 11 Home
build 26300, Rust 1.99.0 and Rayon 1.12.0. Release build, warmed default Rayon pool, 48 kHz stereo,
512-frame blocks. No other compilation ran during the final measurements.

Each number is the median of nine runs, alternating which implementation ran first. Graph/pool
construction and buffer allocation were excluded; rendering and the output copy were included.
Every timed output was compared with the serial reference outside the timer. The timed caller
stayed outside Rayon, matching the normal offline export path.

For the before/after comparison, the unmodified renderer from base commit `f409df6c` was loaded
as a temporary test module. It was removed afterwards. The checked-in benchmark uses the retained
serial playback kernel with the offline flag, so it remains useful without retaining an old
implementation in production or in the repository.

| Fixture | Before | Parallel | Speedup |
| --- | ---: | ---: | ---: |
| 32 instrument tracks, filter per track, 5 seconds | 64.806 ms | 44.822 ms | 1.45x |
| 128 instrument tracks, filter per track, 5 seconds | 257.429 ms | 104.661 ms | 2.46x |
| 32 tracks through 8 nested buses, 5 seconds | 73.413 ms | 57.026 ms | 1.29x |
| 128 tracks through 32 nested buses, 5 seconds | 269.029 ms | 116.257 ms | 2.31x |
| 128 constant-tone tracks without effects, 5 seconds | 56.621 ms | 76.744 ms | 0.74x |

The two-track, two-second control used the serial fallback and remained below 3 ms in both
implementations. Its timing difference is not evidence of parallel speedup. The filter is a
deterministic stateful 16-lane one-pole filter bank implemented in the engine tests, keeping the
engine independent of instrument/DSP crates. These are graph scheduling measurements, not
claims about every instrument, plugin or complete file export. Very cheap sources remain a
counterexample: scheduling costs more than it saves. The implementation prioritizes the larger
DSP workloads, whose measured rendering time decreased by 57–59% at 128 tracks.

## Output checks

- Flat mixes were bit-identical to the original renderer; every timed fixture had zero maximum
  absolute sample error.
- Independent bus branches deliberately interleaved in the original schedule produced a maximum
  absolute error of `1.8626451e-9` and RMS error of `9.28497796e-10`. Output was bit-identical across
  1, 2 and 4 Rayon workers.
- Regression fixtures cover pre/post sends, multiple outputs, sidechain keys, nested buses,
  track/bus/master latency, fader/send automation, mute/unmute, auditions, one-shots, seeking,
  loop splits, stopped tails and a final partial block.
- A caller-placed thread-affinity sentinel remains on the caller while independent peers use
  Rayon. Existing realtime allocation tests continue to use the serial path.
- A session regression uses the actual built-in registry with chiptune, FM, noise drum and
  clarinet instruments plus EQ/reverb, comparing offline rendering with serial playback.

Final validation: `cargo test -p auris-engine -p auris-session --lib` passed 1,690 tests
(8 manual/environment-dependent tests ignored). `cargo clippy --workspace --all-targets`,
`cargo fmt --all --check` and `git diff --check` passed. Cargo still reports the existing
`proc-macro-error2 2.0.1` dependency future-compatibility notice.

Run the reusable release benchmark with no competing builds:

```sh
cargo test -p auris-engine --release benchmark_offline_parallel_render_variants -- --ignored --nocapture --test-threads=1
```

This optimization schedules graph nodes over the existing planar audio buffers. It does not
perform dense multidimensional tensor operations, so adding an ndarray representation would
not improve this scheduling step and would introduce unnecessary storage conversion.
