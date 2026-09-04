# Review findings: auris-gpu

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 5 verified findings: 5 medium.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-168 | medium | `crates/auris-gpu/src/analysis.rs:9` | auris-gpu's crate/module docs still claim no shipped code reports the true peak, but Session::analyze has surfaced it via the MCP analyze tool since commit […] |
| F-198 | medium | `crates/auris-gpu/src/waveform.rs:116` | compute_peaks/compute_peaks_cpu gate on channel-0-only frame_count()==0, zeroing all channels' waveform peaks when only channel 0 is empty, unlike […] |
| F-361 | medium | `crates/auris-gpu/src/lib.rs:13` | auris-gpu's crate and module docs falsely claim compute_peaks reruns on every zoom/scroll, when it actually runs once per source and is cached in […] |
| F-365 | medium | `crates/auris-gpu/src/analysis.rs:238` | analyze_loudness's doc says callers use it, but balance_levels and analyze both call analyze_loudness_cpu directly, skipping GPU entirely. |
| F-419 | medium | `crates/auris-gpu/src/context.rs:383` | read_back's unbounded device.poll(wait_indefinitely()) can hang the gpui UI thread forever during audio import if the GPU driver stalls silently. |

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

### F-361 · medium · auris-gpu's crate and module docs falsely claim compute_peaks reruns on every zoom/scroll, when it actually runs once per source and is cached in Session.waveforms.

`crates/auris-gpu/src/lib.rs:13` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** A developer reading crates/auris-gpu/src/lib.rs or waveform.rs to decide whether to optimize, cache, or debounce zoom/scroll waveform rendering will believe compute_peaks (and thus GPU work) reruns on every zoom/scroll event, when in fact it runs once per source at import/reload/record/render time and is cached in Session.waveforms; zoom/scroll only re-bins the cached 256-bucket array on the CPU in auris_gpui::ui::paint::waveform. This could lead someone to add unnecessary throttling/caching to compute_peaks or the GPU path, or to wrongly suspect the GPU crate when diagnosing zoom/scroll performance issues, wasting investigation time on the wrong code path.

**Trigger.** Read the doc's description of when the GPU reduction runs, then trace every call site of `waveform::compute_peaks` / `GpuContext::compute_peaks` across the workspace.

**Mechanism.** lib.rs says of `compute_peaks`: 'min/max/RMS per horizontal pixel of a clip, over files that routinely run to tens of millions of samples. Redrawn on every zoom and scroll.' waveform.rs's own module doc repeats the same claim almost verbatim: 'the whole reduction is redone whenever the user zooms, so it is worth moving off the CPU when a GPU is present.' Neither is true of the actual call graph: `compute_peaks` has exactly one caller in the workspace, `Session::install_source` (crates/auris-session/src/session/assets.rs:239), which always passes the fixed constant `WAVEFORM_BUCKET = 256` (assets.rs:30) and is only invoked once per audio source — on import (files.rs:513), asset reload (assets.rs:68), a finished recording (record.rs:951), or a singer render (singer.rs:968) — never from a zoom or scroll handler. The peaks it produces are cached forever in `Session.waveforms: HashMap<SourceId, Arc<WaveformPeaks>>` (session/mod.rs:326) and only ever read back (session/mod.rs:788), never recomputed. Zooming/scrolling instead re-bins the already-computed 256-sample buckets on the CPU in […]

**Expected.** The doc should describe the actual lifecycle: the reduction runs once per decoded audio source at import/record time and is cached; zoom and scroll re-bin the cached buckets on the CPU without touching the GPU again.

**Fix direction.** Edit the doc comment at crates/auris-gpu/src/lib.rs:13 and the module doc at crates/auris-gpu/src/waveform.rs:6-7 to state the real lifecycle: compute_peaks runs once per decoded source (via Session::install_source at import, asset reload, record-finish, or singer-render), the result is cached in Session.waveforms and never recomputed, and zoom/scroll re-bin the cached buckets on the CPU in auris_gpui::ui::paint::waveform without calling back into this crate.

**Written rule it breaks.** Every public item carries a doc comment (`#![warn(missing_docs)]` is on in each crate) — implicit requirement that doc comments be accurate, per CLAUDE.md's "Conventions" section on documentation.

**Verifier's correction.** The doc comment at crates/auris-gpu/src/lib.rs:13 and the near-identical module doc at crates/auris-gpu/src/waveform.rs:6-7 should describe the actual lifecycle: compute_peaks runs once per decoded audio source, at import, asset-reload, record-finish, or singer-render time via Session::install_source, with the result cached in Session.waveforms and never recomputed; zoom and scroll re-bin the cached 256-sample buckets on the CPU in auris_gpui::ui::paint::waveform without invoking this crate again.

### F-365 · medium · analyze_loudness's doc says callers use it, but balance_levels and analyze both call analyze_loudness_cpu directly, skipping GPU entirely.

`crates/auris-gpu/src/analysis.rs:238` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Nothing is computed wrong — analyze_loudness_cpu is a correct, exact loudness measurement — but every automatic mix-balancing pass (Session::balance_levels, run on every Session::compose) and every mix analysis report (Session::analyze) always takes the slow CPU path even when a GpuContext is available and already threaded through the same struct for waveform peaks. A user sees composing/analysis take longer than the GPU path the crate was built to provide, with no visible failure — the doc's "this is what callers use" is simply false for both real call sites.

**Trigger.** Any non-headless Session (default SessionOptions, gpu: true) with a working GPU adapter calls Session::balance_levels() or Session::analyze() on a normal project.

**Mechanism.** analysis.rs:236-239 documents the GPU-preferring wrapper as: "Measures a buffer's loudness, preferring the GPU. This is what callers use. A `None` context, or any GPU failure, transparently runs `analyze_loudness_cpu` instead." A workspace-wide grep for `analyze_loudness(` and `GpuContext::analyze_loudness` finds zero callers anywhere outside this crate's own tests. The two real production call sites -- `auris-session/src/session/levels.rs:257,269` (`faders_lift_db`/`master_gain_db`, run on every `Session::balance_levels()`) and `auris-session/src/session/analysis.rs:87,103` (`Session::analyze()`, the MCP/LLM-facing mix report) -- both call `analyze_loudness_cpu(&mix)` directly, never `analyze_loudness`. This isn't a case of the GPU simply being unavailable: `Session` already holds `gpu: Option<Arc<GpuContext>>` (session/mod.rs:271, populated whenever `SessionOptions::gpu` is true, which is the default at mod.rs:157), and the sibling waveform reduction in the very same struct already threads it through correctly -- `assets.rs:239`: `compute_peaks(self.gpu.as_deref(), &buffer, […]

**Expected.** levels.rs and analysis.rs should call `analyze_loudness(self.gpu.as_deref(), &mix)` the same way assets.rs already calls `compute_peaks(self.gpu.as_deref(), ...)`, so whole-mix loudness measurement can actually take the GPU path when available; failing that, the doc comment should not claim callers use the wrapper when none do.

**Fix direction.** In crates/auris-session/src/session/levels.rs (lines 257, 269, 430, 604) and session/analysis.rs (lines 87, 103), replace the direct `analyze_loudness_cpu(&mix)` calls with `analyze_loudness(self.gpu.as_deref(), &mix)`, mirroring the existing `compute_peaks(self.gpu.as_deref(), ...)` pattern in assets.rs:239. Alternatively, if GPU loudness is intentionally not wired up yet, rewrite the doc comment on analyze_loudness to stop claiming callers use it.

**Written rule it breaks.** Doc comment at crates/auris-gpu/src/analysis.rs:236-238: "Measures a buffer's loudness, preferring the GPU. This is what callers use. A `None` context, or any GPU failure, transparently runs `analyze_loudness_cpu` instead."

### F-419 · medium · read_back's unbounded device.poll(wait_indefinitely()) can hang the gpui UI thread forever during audio import if the GPU driver stalls silently.

`crates/auris-gpu/src/context.rs:383` · platform · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** If a GPU driver ever stops making forward progress on a submitted buffer-mapping operation without surfacing an error or a device-lost signal through wgpu, `read_back`'s `device.poll(wgpu::PollType::wait_indefinitely())` blocks forever. Because `compute_peaks` (which calls `read_back`) is invoked synchronously from `Session::install_source`, itself reached from `import_audio`, this hang would freeze the gpui UI thread that imported the audio — the whole application becomes unresponsive with no way to cancel, on a common user action (importing a file). `analyze_loudness`, the other GPU kernel that shares this same unbounded poll, is not currently wired to any caller ("Nothing calls it yet" per the crate's own lib.rs doc), so today's actual blast radius is narrower than the claim's broadest framing.

**Trigger.** A dispatch is in flight when the GPU stops making progress in a way the driver never reports back through wgpu as a poll error or device-lost callback (e.g. an external/removable GPU disconnected mid-dispatch, a laptop GPU reset across sleep/resume, or a hung driver on a platform without OS-level watchdog recovery).

**Mechanism.** `read_back` calls `self.device.poll(wgpu::PollType::wait_indefinitely())` and then `receiver.try_recv()`. `wait_indefinitely()` builds `PollType::Wait { submission_index: None, timeout: None }`; wgpu-types 30.0.0's own doc on that `timeout` field states: 'If not specified, will wait indefinitely (or until an error is detected)' — i.e. the call only returns early if the driver actively surfaces an error through wgpu; there is no bound otherwise. `Device::poll`'s guaranteed 'blocks until submission completed and callbacks invoked' behavior (verified against wgpu-30.0.0 source) makes the `try_recv()` immediately after safe, but does nothing to bound the poll itself.

**Expected.** `read_back` should poll with a bounded `PollType::Wait { timeout: Some(duration), .. }` (or loop with a deadline) so an unresponsive driver still yields `None` and falls back to the CPU path, consistent with the rest of this module's 'nothing here panics or blocks forever' intent.

**Fix direction.** Replace `wgpu::PollType::wait_indefinitely()` with a bounded `PollType::Wait { timeout: Some(duration), .. }` (a few seconds is ample for an offline reduction), and treat a timeout the same as any other poll error: log at debug and return `None` so the CPU fallback path in the caller takes over, per the crate's own documented contract that "every kernel returns `None` rather than panicking when wgpu reports a problem."

**Written rule it breaks.** Everything in this crate is an *optimisation* ... every kernel returns `None` rather than panicking when wgpu reports a problem, and every public entry point has a CPU implementation that produces the same numbers. A build with no working GPU behaves identically, only slower. (crates/auris-gpu/src/lib.rs)
