# Lint, offline analysis and bulk-edit measurements

Measured on Windows 11, AMD Ryzen 7 9800X3D (8 cores / 16 threads), Rust
1.99.0 (`b940084d7`), using the workspace release profile. The baseline is
`f409df6ca54245b8de310bad3642f61f61fb20b5`.

## Work removed

- Score intervals look up active notes by nearby pitch, expire notes with an end-time heap,
  and stop considering rules whose diagnostic caps are full. Candidate indices restore the
  previous encounter order before diagnostics are emitted.
- Missing-chord detection retains only notes still sounding in each bar and checks pitch
  classes at interval boundaries. Ordinary block chords and passages with fewer than three
  pitch classes avoid sorting entirely.
- Audio lint expands score events once. Isolated-note selection uses the greatest previous
  end and the next start for positive, sorted spans. An indexed prefix query preserves the
  old overlap predicate for unsorted or degenerate spans.
- Bulk note deletion compacts survivors once; contiguous selections use one range deletion.
  Selection normalization, history recording and graph invalidation keep their existing order.
- Drum analysis reuses one FFT plan, scratch space, data buffer and Hann window across the
  full sound, attack, body and tail. Spectrogram analysis caches the fixed FFT-bin-to-band map.

These are offline analysis and document operations. FFT/window constants, DSP accumulation
order and realtime processing are unchanged. The workloads here are sparse note events or
one-dimensional FFT buffers; introducing a multidimensional array conversion would not remove
the repeated work identified by these measurements.

## Method

Test-only copies of the original functions provide the baseline in the same executable.
Every paired benchmark first checks the outputs, then alternates original/current execution
order for nine pairs and reports the median. Fixture construction and cloning input notes are
outside the timed sections. The initial equality calls warm both implementations.

The differential tests compare diagnostic fields, order and fixes; selected pitch-note fields;
all spectral levels and metadata; and complete drum descriptors. Note-deletion tests compare
the surviving notes and exercise the session's Undo/Redo and no-op behavior. Additional inputs
cover timing boundaries, rule caps, unsorted events, degenerate spans, different sample rates
and channel counts, non-finite audio and spectrogram column pooling.

The measurements isolate these operations. They do not include audio rendering, file encoding,
project history snapshots or a complete UI gesture. The earlier offline-render and waveform
parallelism changes are separate from this baseline.

## Lint results

Paired, nine-run medians (milliseconds). These stress fixtures describe individual rule/helper
costs, not an end-to-end lint run; large speedups occur where the old code kept doing quadratic
work even after a diagnostic limit or a simple negative result was known.

| Workload | Original | Optimized |
| --- | ---: | ---: |
| Interval checks, 32 dissonant notes | 0.0823 | 0.0706 |
| Interval checks, 4,096 overlapping unison notes | 11.5458 | 0.0657 |
| Interval checks, 2,048 dissonant notes | 99.6741 | 0.0904 |
| Interval checks, 4,096 overlapping bass notes | 527.7884 | 0.1233 |
| Interval checks, 2,048 mixed notes | 6.4042 | 0.1096 |
| Missing chords, 32 triads across bars | 0.1342 | 0.1331 |
| Missing chords, 10,000 simultaneous chord tones | 0.1017 | 0.1073 |
| Missing chords, 10,000 simultaneous unison notes | 73.4364 | 0.1011 |
| Missing chords, 10,000 notes with only two classes sounding at once | 20.6662 | 0.3798 |
| Missing chords, held pedal across 128 bars | 0.0193 | 0.0082 |
| Audio lint pitch selection, 160 monophonic notes | 0.0242 | 0.0035 |
| Audio lint pitch selection, 10,000 monophonic notes | 1.2674 | 0.0703 |
| Audio lint pitch selection, 10,000 notes with eligible pitches only at the end | 37.4090 | 0.0499 |
| Audio lint pitch selection, 10,000 fully overlapping notes | 0.0256 | 0.0304 |

All diagnostics and selected notes compare equal. Dense, immediately recognized chords cost
5.6 microseconds more, and fully overlapping pitch candidates cost 4.8 microseconds more.
Those cases already short-circuit cheaply in the original implementation; the large
quadratic cases improve substantially.

## Bulk deletion results

Paired, nine-run medians (milliseconds), excluding selection normalization and history snapshots:

| Clip size / selection | Reverse removal | Stable compaction or range removal |
| --- | ---: | ---: |
| 128 notes / every 100th (2 notes) | 0.0002 | 0.0009 |
| 50,000 notes / every second | 1306.1384 | 0.2809 |
| 100,000 notes / every 100th | 210.1104 | 0.8761 |
| 100,000 notes / every second | 5292.9098 | 0.5283 |
| 100,000 notes / all | 0.7879 | 0.1841 |

Survivors compare equal in every workload. Sparse deletion in the tiny clip costs 0.7
microseconds more; scattered large selections avoid seconds of repeated tail shifting.
These figures describe the deletion kernel, not the complete session edit or UI response.

## Offline analysis results

Paired, nine-run medians (milliseconds):

| Workload | Original | Optimized | Speedup |
| --- | ---: | ---: | ---: |
| Drum analysis, 1 s stereo | 2.4002 | 0.7943 | 3.02× |
| Drum analysis, 10 s stereo | 22.6028 | 7.7039 | 2.93× |
| Spectrogram, 1 s stereo | 1.528 | 1.001 | 1.53× |
| Spectrogram, 10 s stereo | 14.063 | 9.015 | 1.56× |
| Spectrogram, 60 s stereo | 83.931 | 54.196 | 1.55× |

All paired outputs compare equal. The five-run phase timing probe, collected separately
before and after the change, measured the following for 10 s of 48 kHz stereo audio:

| Entry point | Before (ms) | After (ms) |
| --- | ---: | ---: |
| Drum analysis | 25.8565 | 7.6900 |
| Timbre features (includes drum analysis) | 34.7783 | 17.8647 |
| Spectrogram | 16.4994 | 9.1133 |
| Integrated loudness | 6.8534 | 6.8416 |
| Mel analysis | 23.6618 | 23.0041 |

Loudness and mel analysis are unchanged controls; their small timing differences are noise,
not claimed improvements. Timbre extraction benefits from the shared drum-analysis work.

## Reproduction

Run benchmarks without another build or benchmark consuming CPU:

```powershell
cargo test -p auris-session --release --lib benchmark -- --ignored --nocapture --test-threads=1
cargo test -p auris-dsp --release --lib benchmark -- --ignored --nocapture --test-threads=1
```

The broader `offline_analysis_benchmark` profile uses five-run medians after a warmup to
compare drum analysis, timbre features, spectrograms, integrated loudness and mel analysis.
It is a phase-level timing probe, not a sampled CPU profile.

The investigation also reproduced an existing event-cap ordering defect, tracked in
[#164](https://github.com/uthree/auris-studio/issues/164). These optimizations preserve the
current capped-input behavior, including unsorted prefixes, so that the ordering correction
can be reviewed independently.
