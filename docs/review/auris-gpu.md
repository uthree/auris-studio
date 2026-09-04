# Review findings: auris-gpu

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 2 verified findings: 2 medium.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-168 | medium | `crates/auris-gpu/src/analysis.rs:9` | auris-gpu's crate/module docs still claim no shipped code reports the true peak, but Session::analyze has surfaced it via the MCP analyze tool since commit […] |
| F-198 | medium | `crates/auris-gpu/src/waveform.rs:116` | compute_peaks/compute_peaks_cpu gate on channel-0-only frame_count()==0, zeroing all channels' waveform peaks when only channel 0 is empty, unlike […] |

### F-168 · medium · auris-gpu's crate/module docs still claim no shipped code reports the true peak, but Session::analyze has surfaced it via the MCP analyze tool since commit 4e16f12.

`crates/auris-gpu/src/analysis.rs:9` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** No end user sees wrong output — the MCP `analyze` tool correctly reports and warns about the true peak. A developer or agent reading auris-gpu's crate docs, however, is told "nothing shipped reports the inter-sample peak below" and "Nothing calls it yet," which is false as of commit 4e16f12; someone trusting that doc could re-implement true-peak reporting that already exists in `Session::analyze`, or wrongly deprioritize wiring the GPU-accelerated export path because the doc implies the whole gap is still open.

**Trigger.** Call the `analyze` MCP tool (or `Session::analyze` directly) on a project whose rendered mix has an inter-sample overshoot; the returned report includes and narrates `true_peak_db`, computed by `auris_gpu::analysis::analyze_loudness_cpu`.

**Mechanism.** analysis.rs's module doc (lines 8-14) states: '**Neither exists.** The export dialog reports `AudioBuffer::peak` instead — the loudest sample, found by a CPU scan — so nothing shipped reports the inter-sample peak below, and a mix reading -0.3 dBFS in the dialog may be over full scale between its samples without anything saying so.' lib.rs makes the identical claim at lines 14-22 ('**Nothing calls it yet.**... says nothing about the inter-sample peak this measures'). Both were written in commit d04b715 ('Stop the GPU crate claiming a caller it does not have', 2026-08-20). Ten days later, commit 4e16f12 ('Give the loop its ears', 2026-08-30) added `Session::analyze` in crates/auris-session/src/session/analysis.rs, which imports this crate's own `analyze_loudness_cpu` (line 19), calls it on the rendered mix (line 87), and returns `true_peak_db: loudness.true_peak_db()` as a public field of `MixAnalysis` (line 128). `crates/auris-toolbox/src/lib.rs` then formats this into the `analyze` MCP tool's human-readable report: 'peak {:.1} dBFS (true peak {:.1})' (lines 2604-2610), and […]

**Expected.** The doc should state that `Session::analyze` (via this crate's own `analyze_loudness_cpu`) already reports and warns about the estimated true peak to MCP/agent callers, and narrow the 'nothing shipped reports this' claim to what is still actually true: the GUI export dialog / `RenderJob::render_to_wav` path, and the GPU-accelerated `analyze_loudness` wrapper specifically, per this crate's own convention of updating documentation when behavior changes (CLAUDE.md's own account of the […]

**Fix direction.** Update the doc comments in crates/auris-gpu/src/analysis.rs:8-14 and lib.rs:14-22 to state that Session::analyze (via this crate's own analyze_loudness_cpu) already computes and surfaces true_peak_db to every MCP/agent caller, narrowing the "nothing shipped reports this" claim to what remains true: the GUI export dialog / RenderJob::render_to_wav path still uses AudioBuffer::peak only, and the GPU-accelerated analyze_loudness wrapper specifically still has no caller.

**Written rule it breaks.** Every public item carries a doc comment... (CLAUDE.md conventions); the doc's own stated purpose: "said out loud because a doc comment claiming a caller that does not exist is how the next person comes to believe the number on screen is a true peak" — now inverted, since a real caller now exists and the doc denies it.

### F-198 · medium · compute_peaks/compute_peaks_cpu gate on channel-0-only frame_count()==0, zeroing all channels' waveform peaks when only channel 0 is empty, unlike analyze_loudness's guard against the same trap.

`crates/auris-gpu/src/waveform.rs:116` · correctness · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** On a stereo/multi-channel buffer whose channel 0 happens to be shorter (empty) than a later channel — a shape the crate's own type permits via `channels_mut` but which no current production caller (file import, recording, singer synthesis) actually produces — the waveform view would render as a flat empty line for every channel instead of showing the real peaks in the non-empty channel(s). Today this is only reachable by a caller deliberately building a ragged buffer, so no shipped feature currently hits it.

**Trigger.** Build a buffer whose channels are not all the same length and whose first channel is the empty one, e.g.:
```rust
let mut buffer = AudioBuffer::from_planar(vec![vec![0.0; 1000], vec![0.5; 1000]], 48_000.0).unwrap();
buffer.channels_mut()[0].clear(); // channel 0 now empty, channel 1 still has 1000 real samples
compute_peaks_cpu(&buffer, 256);  // or GpuContext::compute_peaks
```
`AudioBuffer::channels_mut()` returns `&mut [Vec<f32>]` specifically to let a caller give channels independent lengths (the crate's own tests use it the same way — `buffer.channels_mut()[1].truncate(3)` in `output_stays_rectangular_for_a_ragged_buffer` — just never with channel 0 as the short one), so this is a […]

**Mechanism.** `auris_core::AudioBuffer::frame_count()` is defined as `self.channels.first().map_or(0, Vec::len)` (crates/auris-core/src/buffer.rs:77-79) — it reports only channel 0's length. Both waveform-reduction entry points use that single number as their entire "is there anything to draw" test:

```
114	let channel_count = buffer.channel_count();
115	let frames = buffer.frame_count();
116	if frames == 0 {
117	    return WaveformPeaks::empty(channel_count, stride);
118	}
```
(`compute_peaks_cpu`, lines 112-118) and identically in `GpuContext::compute_peaks`:
```
189	let channel_count = buffer.channel_count();
190	let frames = buffer.frame_count();
191	if frames == 0 {
192	    return Some(WaveformPeaks::empty(channel_count, stride as u32));
193	}
```
(lines 183-193). `WaveformPeaks::empty` returns a value with `min`/`max`/`rms` all empty for every channel, not just channel 0 — so if channel 0 happens to be empty while another channel is not, both functions throw away every channel's data, not only the empty one's.

This is exactly the trap the crate's own sibling function in analysis.rs […]

**Expected.** Per `WaveformPeaks`'s own doc ("one entry per bucket per channel") and the design already implemented one file over in `analyze_loudness`, the fast-path check should be `buffer.channels().iter().all(|samples| samples.is_empty())` rather than `buffer.frame_count() == 0`, so a non-empty later channel is still reduced even when channel 0 happens to be empty.

**Fix direction.** In `compute_peaks_cpu` and `GpuContext::compute_peaks`, replace the `buffer.frame_count() == 0` short-circuit with the same all-channels-empty check `analysis.rs`'s `analyze_loudness` already uses (`buffer.channels().iter().all(|c| c.is_empty())`), so a ragged buffer with an empty first channel still gets peaks computed for its non-empty channels.

**Written rule it breaks.** // `frame_count()` only describes the first channel, and `AudioBuffer::channels_mut` (comment in crates/auris-gpu/src/analysis.rs documenting the exact trap this code falls into)
