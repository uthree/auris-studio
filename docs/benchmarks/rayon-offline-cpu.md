# Offline CPU parallelism

The waveform overview fallback and video backdrop compositing use Rayon for
large inputs. These operations have independent output buckets or rows; each
output retains the previous arithmetic and sample order. Audio callbacks and
video frame encoding do not use the new worker dispatches.

## Preserving results

The regression tests retain independent copies of the previous serial loops
only in test modules. Waveform comparisons check every `f32::to_bits()` value
and the output metadata. Image comparisons check the complete RGBA byte buffer.
Both implementations are exercised in pools containing one, two, and four
threads, including inputs around the serial/parallel thresholds.

Waveform fixtures cover partial buckets, zero stride, silence, signed zero,
non-finite values, denormals, very large stride, unequal channel lengths, and
an empty first channel. Backdrop fixtures cover portrait/landscape geometry,
cover cropping, alpha, and dimming endpoints.

The waveform threshold is 262,144 input samples across channels. Each Rayon
task handles at least roughly 16,384 samples where bucket sizes permit. Video
backdrops use parallel rows at 512×512 pixels or larger. Smaller work remains
serial to avoid scheduling overhead.

## Repeating the measurements

Run the manual tests separately, without a competing build or benchmark:

```powershell
cargo test -p auris-gpu --release --locked waveform::cpu_tests::measure_waveform_cpu -- --ignored --nocapture
cargo test -p auris-video --release --locked benchmark_backdrop_serial_and_parallel_medians -- --ignored --nocapture
```

Each benchmark compares the original serial loop with the production entry
point on the same input. It warms the worker pool, alternates the measurement
order, consumes the result through `black_box`, and reports the median of nine
runs. Input generation is excluded. Backdrop timing includes image resizing,
allocation, and compositing; it measures renderer setup, not whole-video export.

## Windows measurements — 2026-11-04

Windows 11 Home build 26300, AMD Ryzen 7 9800X3D (8 cores / 16 logical
processors), Rust 1.99.0, release profile with Thin LTO, default Rayon pool and
no `RAYON_NUM_THREADS` override. The serial references come from baseline
`df46c02b79fc613ec97e06d57f21d51a0617b470`. Before the waveform implementation
was changed, its timings and the independent reference agreed within 3% on
the tested small/three-minute fixtures.

| Workload | Serial median | Rayon implementation median | Ratio |
| --- | ---: | ---: | ---: |
| Waveform, 4,096 stereo frames, stride 512 | 0.011 ms | 0.011 ms | no meaningful change |
| Waveform, 3 min mono / 48 kHz, stride 512 | 11.726 ms | 1.089 ms | 10.76× |
| Waveform, 3 min stereo / 48 kHz, stride 256 | 22.800 ms | 2.353 ms | 9.69× |
| Waveform, 3 min stereo / 48 kHz, stride 512 | 23.799 ms | 2.310 ms | 10.30× |
| Waveform, 3 min stereo / 48 kHz, stride 4096 | 24.003 ms | 2.551 ms | 9.41× |
| Waveform, 30 min stereo / 48 kHz, stride 256 | 233.695 ms | 20.605 ms | 11.34× |
| Backdrop, 200×120 | 0.847 ms | 0.884 ms | 0.96× |
| Backdrop, 1920×1080 | 31.596 ms | 22.765 ms | 1.39× |
| Backdrop, 3840×2160 | 127.291 ms | 88.717 ms | 1.43× |

The application uses waveform stride 256 when preparing imported/project audio.
The thirty-minute fixture contains 172,800,000 samples (about 691 MB of PCM).
Its complete waveform output also passed the bit comparison before timing.
Large workloads improved clearly. The small cases remain serial; the small
backdrop measured 37 microseconds slower, while the rounded waveform timings
were unchanged. This change prioritizes the large workloads rather than claiming
a universal improvement. These are local warm-pool
measurements, not guarantees for every CPU or concurrent workload.

Validation on the same machine:

- `cargo test -p auris-gpu -p auris-video --locked`: 73 passed, two manual
  benchmarks ignored. The final backdrop iteration change was also checked
  by rerunning its equivalence tests in both debug and release profiles.
- `cargo test -p auris-gpu --release --locked`: 32 passed, one manual benchmark
  ignored; the benchmark was then run explicitly.
- `cargo clippy --workspace --all-targets --locked`: passed. Cargo reports the
  existing future-compatibility notice for `proc-macro-error2 2.0.1`.
- `cargo fmt --all --check` and `git diff --check`: passed.

## Data layout

`ndarray` was considered alongside Rayon. The audio buffer stores channels in
separate allocations and allows unequal lengths. Turning it into a rectangular
matrix would require copying or changing the storage contract. RGBA images are
already contiguous; parallel row slices operate on that storage directly.
These changes therefore retain the existing layouts. Existing `ndarray` use
in WSOLA correlation remains unchanged.
