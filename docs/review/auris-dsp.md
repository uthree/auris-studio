# Review findings: auris-dsp

Part of the [whole-repository adversarial review](README.md) of commit `52d1702`. 11 verified findings: 4 high, 5 medium, 2 low.

Each entry survived an independent skeptic and an independent reproducer (and a tie-breaker when they disagreed); "executed reproduction" means the reproducer ran a test, a binary or a scratch program and observed the behaviour, "traced" means it followed the call path with concrete values.

| ID | Severity | Location | Finding |
|---|---|---|---|
| F-057 | high | `crates/auris-dsp/src/gain.rs:112` | GainPan ramps gain/pan via SmoothedValue but applies phase-invert and width raw per-block, causing an audible step/pop on toggle. |
| F-075 | high | `crates/auris-dsp/src/reverb.rs:324` | Reverb reads mix/width/damping/room-size/pre-delay once per block instead of ramping them, causing zipper noise and pre-delay read-head jumps on parameter […] |
| F-086 | high | `crates/auris-dsp/src/compressor.rs:241` | Compressor makeup gain is added unsmoothed to the envelope-filtered gain each frame, causing a zipper-noise click whenever makeup_db changes between blocks. |
| F-087 | high | `crates/auris-dsp/src/eq.rs:335` | Re-enabling a disabled EQ band resumes its biquad from stale, frozen s1/s2 state, producing an audible click/thump instead of a clean rejoin. |
| F-040 | medium | `crates/auris-dsp/src/limiter.rs:173` | Limiter's look-ahead delay line doesn't sanitise samples like chorus/delay do, so its own NaN-proof-ceiling doc claim is false in isolation, though the […] |
| F-128 | medium | `crates/auris-dsp/src/eq.rs:409` | Disabling then re-enabling a resonant EQ band replays its frozen, stale filter memory as an audible thump instead of resuming cleanly. |
| F-147 | medium | `crates/auris-dsp/src/delay.rs:165` | delay.rs damping_alpha doc claims exact -3dB cutoff, but it's 5.6% off at the plugin's 6kHz default and unreachable below Nyquist above ~12kHz. |
| F-152 | medium | `crates/auris-dsp/src/spectrum.rs:166` | SpectrumAnalyzer::magnitudes applies the interior-bin 4/size mirror-recovery scale to the self-mirrored DC and Nyquist bins, reading them +6.02 dB hot; […] |
| F-179 | medium | `crates/auris-dsp/src/envelope.rs:110` | EnvelopeFollower::process (envelope.rs:110) omits the crate's mandatory settled() denormal/NaN flush that every other recirculating filter state uses, though […] |
| F-262 | low | `crates/auris-dsp/src/stretch.rs:129` | window_frames()'s doc claims the returned length is "always even" but the function never enforces it; only its sole caller's `& !1` mask makes that true today. |
| F-276 | low | `crates/auris-dsp/src/gain.rs:148` | GainPan reads the width parameter once per block unsmoothed, causing an audible zipper-noise step on the side channel when width is automated, unlike gain and […] |

### F-057 · high · GainPan ramps gain/pan via SmoothedValue but applies phase-invert and width raw per-block, causing an audible step/pop on toggle.

`crates/auris-dsp/src/gain.rs:112` · realtime · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Toggling Phase Invert or animating Width during playback (via a knob or an automation lane) produces an audible pop/click — a full-amplitude instantaneous step in the output at the block boundary, exactly the zipper-noise artifact the file's own SmoothedValue mechanism exists to prevent, but it only covers gain and pan.

**Trigger.** Prepare a `GainPan`, process one block of nonzero, non-trivial audio with `invert=false` (default), then call `set_param_by_key("invert", 1.0)` between blocks (an ordinary knob toggle or automation event) and process the next block continuing the same signal. Every sample's polarity flips instantly at the block boundary — output jumps from `+x` to `-x`, a discontinuity of up to `2x` the signal's amplitude. The same argument applies to animating `width` from e.g. 1.0 to 0.0 while a stereo signal has a nonzero side component (line 148).

**Mechanism.** In `process()`, `gain_db`, `pan` are pushed into `self.gain`/`self.left`/`self.right` (`SmoothedValue`s) via `set_target()` and only reach the signal through `next_value()`, which ramps them sample-by-sample (lines 118-121). `polarity` (lines 112-116, from `P_INVERT`) and `width` (line 117, from `P_WIDTH`) are instead read straight out of `ParamBank::at`/`flag` and multiplied into the signal directly every sample (lines 126, 145-148, 158) with no smoother of any kind — there is no `SmoothedValue` field for either in the `GainPan` struct at all.

**Expected.** `invert` and `width` should ramp through their own `SmoothedValue` (as `gain`/`left`/`right` already do in this same struct) so that a step in either parameter does not appear as a step in the output.

**Fix direction.** Add SmoothedValue fields for invert (as a target of +1.0/-1.0) and width to the GainPan struct, set_target() them alongside gain/pan in process(), and consume them per-sample via next_value() instead of reading self.params.flag(P_INVERT)/self.params.at(P_WIDTH) directly; initialize and snap them in new()/prepare()/reset() the same way gain/left/right already are.

**Written rule it breaks.** Ramp time for gain and pan moves... to remove the step discontinuity that causes zipper noise (gain.rs doc comment on SMOOTHING_SECONDS)

### F-075 · high · Reverb reads mix/width/damping/room-size/pre-delay once per block instead of ramping them, causing zipper noise and pre-delay read-head jumps on parameter changes, unlike delay.rs/chorus.rs.

`crates/auris-dsp/src/reverb.rs:324` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Automating or manually turning any Reverb knob (mix, width, damping, room size, or pre-delay) while audio is playing produces an audible click/zipper artifact on the block boundary; worse, moving pre-delay causes the pre-delay line's read head to jump discontinuously to a new position, producing a sample-repeat or skip glitch rather than a smooth crossfade — exactly the failure mode delay.rs's own SmoothedValue machinery exists to prevent, but present here in the sibling effect.

**Trigger.** Automate (or live-turn) Reverb's Pre-Delay, Mix, Width, Damping or Room Size while a track is playing. `apply_automation`/`drive_automation` calls `Reverb::set_param` once per rendered segment; the very next `process()` call uses the new value from its first sample with no ramp.

**Mechanism.** `Effect::process` (reverb.rs:307-325) reads every control straight from the param bank once per call — `let width = self.params.at(P_WIDTH); let mix = self.params.at(P_MIX); ... let pre_delay_samples = (self.params.at(P_PRE_DELAY_MS) * self.sample_rate / MILLISECONDS_PER_SECOND).max(1.0);` — and applies the resulting `wet1`/`wet2`/`dry`/`pre_delay_samples` uniformly across the whole block with no interpolation state at all (reverb.rs has no `use crate::smooth::SmoothedValue;`, unlike delay.rs and chorus.rs in the same crate). auris-engine's own automation contract (crates/auris-engine/src/graph/mod.rs:692-695) states this is deliberate on the engine's side: 'Called once per rendered segment rather than once per sample... a segment-rate write comes out as a continuous slope. It is the honest rate for a plugin parameter, which has nowhere finer to put one', and automation.rs:121-122 says a plugin parameter is 'written the same way either way, because there is nothing between two values of it' — i.e. the ENGINE relies on the effect itself turning a block-rate value change into a slope. […]

**Expected.** Per the engine's own automation contract (graph/mod.rs:692-695) and the pattern every other time-based effect in this crate follows (Delay's `time`/`mix` SmoothedValues, Chorus's `depth`/`mix` SmoothedValues, GainPan's `gain`/`left`/`right` SmoothedValues), Reverb should carry `SmoothedValue`s for mix, width, damping, room-size/feedback and pre-delay and ramp them per-sample the same way, instead of applying a per-block value with no interpolation.

**Fix direction.** Add `use crate::smooth::SmoothedValue;` to reverb.rs and give `Reverb` smoothed fields for mix, width, damping, room_size and pre_delay_ms (mirroring delay.rs's `time`/`mix` and chorus.rs's `depth`/`mix` pattern), initialized in `prepare` with per-parameter smoothing constants and advanced once per frame inside the `for frame in 0..frames` loop instead of computed once per block.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs" — and Reverb's sibling effects (delay.rs, chorus.rs) establish the project's own convention of ramping user-facing controls via SmoothedValue to avoid zipper/read-head-jump artifacts, a convention Reverb silently omits.

### F-086 · high · Compressor makeup gain is added unsmoothed to the envelope-filtered gain each frame, causing a zipper-noise click whenever makeup_db changes between blocks.

`crates/auris-dsp/src/compressor.rs:241` · realtime · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Automating or live-tweaking the compressor's makeup-gain parameter (e.g. via automation lanes, a plugin UI drag, or the MCP/agent toolbox calling set_param) produces an audible click/zipper-noise artifact at the block boundary where the value changes, because the new makeup gain is applied bit-for-bit on the very first sample of the next block instead of being ramped in like every other continuous parameter in the DSP chain.

**Trigger.** Prepare a `Compressor`, process a block with `makeup_db = 0.0` and the input at or under threshold (so `self.gain_db` settles near `0.0`), then call `set_param_by_key("makeup_db", 24.0)` between blocks and process the next block with the same steady input. The first output sample of the new block jumps by ~24 dB (~16x amplitude) relative to the previous block's trailing samples, with no ramp.

**Mechanism.** `self.gain_db` is the one value in this plugin that is smoothed, via the attack/release one-pole at lines 231-239. `makeup_db` is read raw from `ParamBank` at line 209 (`self.params.at(P_MAKEUP_DB)`) and added directly to the already-smoothed `gain_db` at line 241 (`let gain = db_to_gain(self.gain_db + makeup_db);`) before being applied to the audio at line 246 — it never passes through the gain envelope or through `self.mix` (which itself is a `SmoothedValue`, `MIX_SMOOTHING_SECONDS`).

**Expected.** `makeup_db` should be applied through a `SmoothedValue`, the same mechanism `mix` already uses in this file, so a step in the knob does not produce a step in the output.

**Fix direction.** Give makeup gain the same smoothing every other continuous parameter gets: add a `SmoothedValue` (or fold it into the existing one-pole) for makeup gain, call `.set_target(makeup_db)` once per block, and use `.next_value()` per frame instead of reading `self.params.at(P_MAKEUP_DB)` once and adding it raw to `self.gain_db` at line 241.

**Written rule it breaks.** Applying it as a step produces an audible click at the block boundary ('zipper noise'), so continuous parameters are moved [through smoothing] — crates/auris-dsp/src/smooth.rs module doc

### F-087 · high · Re-enabling a disabled EQ band resumes its biquad from stale, frozen s1/s2 state, producing an audible click/thump instead of a clean rejoin.

`crates/auris-dsp/src/eq.rs:335` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** Toggling an EQ band off and back on during playback (or automation muting/unmuting a band) produces an audible glitch: a click, thump, or a burst of the pre-disable filter energy, because the biquad's internal state (s1/s2) was frozen mid-signal rather than cleared, and the recursive filter resumes from that stale state using new coefficients on re-enable. With a high-Q/high-gain band this can be a sharp, surprising artifact rather than the clean "band skipped, then rejoins" behavior a user expects.

**Trigger.** Enable a resonant band (e.g. `p1_enabled=1`, high `p1_gain`/`p1_q`) and run a loud signal through it so `s1`/`s2` build up a large nonzero value; set `p1_enabled` to `0` mid-stream (state now frozen); process a few more blocks of any signal; then set `p1_enabled` back to `1` and process the next block.

**Mechanism.** `recompute_band` (lines 330-342), the only code path `set_param` runs when a band's `enabled` flag changes, updates `self.enabled[band]` and `self.coefficients[band]` but never touches the corresponding `Biquad`'s `s1`/`s2` state inside `self.filters`. In `process()` (lines 408-412), `filter.process_block(samples)` is skipped entirely for a disabled band, so its `s1`/`s2` freeze at whatever they held the instant it was turned off. `Biquad::set_coefficients` (biquad.rs:301-304) only replaces the coefficients and likewise never clears `s1`/`s2`.

**Expected.** A transition from disabled to enabled should reset the band's filter state (e.g. `filter.reset()` for that band) so resuming filtering starts clean rather than replaying old history.

**Fix direction.** In `recompute_band` (or in `set_param` when the enable flag specifically flips from true to false, or false to true), reset that band's `Biquad` state across all channels — e.g. `for ch in &mut self.filters { ch[band].reset(); }` — whenever `OFFSET_ENABLED` changes value, so a disabled band always resumes silent/settled instead of carrying stale s1/s2 forward.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-040 · medium · Limiter's look-ahead delay line doesn't sanitise samples like chorus/delay do, so its own NaN-proof-ceiling doc claim is false in isolation, though the engine's blanket master_scratch.sanitize() keeps it from reaching real playback or exported audio.

`crates/auris-dsp/src/limiter.rs:173` · dsp · confirmed (executed reproduction; reported independently 2×)

**What a user sees.** In the shipped app no user hears or exports a NaN: auris-engine's render_segment unconditionally sanitizes master_scratch after all processing and before the only path to audio output or file export. The observable harm is confined to the Limiter unit itself — its documented "provably cannot exceed its ceiling" guarantee is false when a non-finite sample enters it (e.g. under direct unit testing, or if the Limiter is ever used outside the engine's blanket sanitize, such as by a future frontend or a hosted CLAP graph).

**Trigger.** One non-finite sample entering the limiter (e.g. from a hosted CLAP plugin, a broken import, or numeric overflow upstream) — a scenario the crate's own doc comment for `crate::settled` explicitly calls out ("one non-finite sample from a broken import or a misbehaving plugin"). Concretely: feed the limiter a buffer where `channel[0][100] = f32::NAN` and process normally; the sample written back out at `channel[0][100 + lookahead]` is NaN.

**Mechanism.** In `Effect::process` (lines 165-176): `let driven = channel[frame] * input_gain; let delayed = line.read(lookahead); line.write(driven); channel[frame] = (delayed * gain).clamp(-ceiling, ceiling);`. `driven` is written straight into the `DelayLine` ring buffer (`delay_line.rs::write`, line 75-78) with no finiteness check. `gain` itself stays correct/finite because it is derived from `required = if peak > ceiling { ceiling / peak } else { 1.0 }`, and `peak = peak.max((channel[frame] * input_gain).abs())` silently drops a NaN sample (Rust's `f32::max` returns the non-NaN operand), so the *gain curve* never sees the NaN. But the raw sample itself is written unsanitised into the delay line and, `lookahead` frames later, comes back out of `line.read(lookahead)` as NaN. `f32::clamp` returns NaN unchanged when `self` is NaN (it only compares `<`/`>`, both false for NaN), so `(delayed * gain).clamp(-ceiling, ceiling)` is a no-op and the NaN is written to `channel[frame]`, i.e. the output audio bus.

**Expected.** Per `crate::settled`'s own stated purpose ("Every feedback loop in the crate passes its state through this") and the Compressor's analogous handling of non-finite samples (`peak_at`, which folds NaN away and floors +Inf via `gain_to_db`), the sample written into the delay line should be sanitised (e.g. `crate::settled(driven)` or an explicit `is_finite()` check) before `line.write`, the way `Biquad::process_sample` sanitises its `input` before it ever reaches recirculating state.

**Fix direction.** In Limiter::process (limiter.rs:173), sanitise the sample before/after the delay line the same way chorus.rs and delay.rs do: write crate::settled(driven) into the line, or wrap the read as delayed = crate::settled(line.read(lookahead)), so a NaN/Inf input cannot resurface past the clamp.

**Written rule it breaks.** A brickwall limiter whose output provably cannot exceed its ceiling. (limiter.rs:28 doc comment)

**Verifier's correction.** In Limiter::process (crates/auris-dsp/src/limiter.rs:165-176), the sample written into the look-ahead DelayLine is not sanitised, so a non-finite (NaN) input sample resurfaces lookahead frames later via line.read and passes through f32::clamp unchanged (clamp is a no-op on NaN), landing in the Limiter's own output buffer -- falsifying its doc comment's claim to be "a brickwall limiter whose output provably cannot exceed its ceiling." This is a real, reproducible defect and an inconsistency with the crate's own convention: every other delay-line-based effect (chorus, delay) wraps its […]

### F-128 · medium · Disabling then re-enabling a resonant EQ band replays its frozen, stale filter memory as an audible thump instead of resuming cleanly.

`crates/auris-dsp/src/eq.rs:409` · dsp · confirmed (executed reproduction; reported independently 3×)

**What a user sees.** If a user disables an EQ band (especially a resonant, high-Q, high-gain bell) while loud audio was passing through it, then re-enables it later during playback — even after long stretches of silence or unrelated audio — the very first block processed on re-enable resumes from the stale, non-zero filter state (s1/s2) captured at the moment it was disabled. This produces an audible transient thump/spike unrelated to the current signal, instead of a clean resumption of filtering.

**Trigger.** Enable a resonant band (e.g. `p1_q = 18.0`, `p1_gain = 24.0`) and play a loud transient through it so `s1`/`s2` build up significant energy; disable the band (`p1_enabled = 0`) at that moment — the state is now frozen at that non-zero value indefinitely, however many silent or unrelated blocks pass, since the disabled band's `Biquad` is never touched. Re-enable the band later (`p1_enabled = 1`): the very next block's `filter.process_block` resumes from that stale `s1`/`s2`, injecting the old, now-unrelated energy into whatever is currently playing.

**Mechanism.** `Effect::process` (lines 402-414) only calls `filter.set_coefficients(...)` and `filter.process_block(samples)` `if enabled[band]`. When a band is disabled, its per-channel `Biquad` (with internal state `s1`/`s2`) is simply skipped for every subsequent block — it neither continues to run (so its state can't decay toward the new, silent/whatever input) nor is it reset. `Biquad::reset()` is only ever called from `Equalizer::reset()` (called from `prepare`), never from `set_param`/`recompute_band` when a band's enabled flag flips.

**Expected.** Toggling a band off should either continue running the filter (so state tracks the actual, if unfiltered, signal / decays naturally) or reset it (`filter.reset()`) on the enable transition, the way `Equalizer::reset()` clears every band's state on prepare. Nothing in `recompute_band`/`set_param` does either when only the enabled flag changes.

**Fix direction.** In `Equalizer::set_param`/`recompute_band`, detect the enabled-flag transition (false→true, or even any change) and call `self.filters[..][band].reset()` on the affected band across all channels before it resumes processing — mirroring what `Equalizer::reset()` already does for every band in `prepare`. Alternatively, always run `process_block` and only skip mixing a disabled band's output, so state decays naturally with the live signal instead of being frozen.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-147 · medium · delay.rs damping_alpha doc claims exact -3dB cutoff, but it's 5.6% off at the plugin's 6kHz default and unreachable below Nyquist above ~12kHz.

`crates/auris-dsp/src/delay.rs:165` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** A user (or a preset-design script) reading the doc comment on `damping_alpha` and trusting it to set the delay's damping cutoff precisely will get audibly wrong results: at the plugin's own 6kHz default the real -3dB point is ~6335Hz (5.6% high), at 10kHz it's ~11.9kHz (19% high), and at 15-20kHz the filter never reaches -3dB at all below Nyquist — so "set damping to 15kHz" does not roll off the top end the way the doc promises.

**Trigger.** Instantiate `auris.fx.delay` at its default settings (damping_hz = 6000.0) or set `damping_hz` anywhere from about 8 kHz to 20 kHz (both well within the parameter's declared 200-20,000 Hz range) and measure the filter's actual frequency response.

**Mechanism.** The doc comment at lines 162-165 reads: `a = 1 - exp(-2 pi fc / fs)` is the impulse-invariant mapping of a first-order RC section, so the cutoff really is the -3 dB point rather than a rough approximation.` `damping_alpha()` (line 166) implements exactly `a = 1 - exp(-TAU * fc / fs)` and this coefficient is then used in a one-pole recursion `y += a * (x - y)`. This mapping only approximates the analogue RC pole; the actual discrete-time -3 dB frequency of the resulting filter diverges from the nominal `fc` as `fc` grows relative to the sample rate, because the exponential-decay-per-sample model does not account for the frequency warping of the discrete recursion. I verified this by numerically solving `|H(e^jw)|^2 = 0.5` for the filter's transfer function `H(z) = a / (1 - (1-a) z^-1)` at fs = 48 kHz for several `fc` values in the parameter's valid 200-20,000 Hz range: fc=100 Hz -> actual -3dB point 100.0 Hz (correct); fc=1000 Hz -> 1001.4 Hz (0.14% off); fc=6000 Hz (the plugin's own default) -> 6335.4 Hz (5.6% off, 335 Hz); fc=10000 Hz -> 11895.1 Hz (19% off, ~1895 Hz); fc=15000 Hz […]

**Expected.** Either the doc comment should describe the mapping honestly as an approximation that only holds for `fc` well below the sample rate (mirroring how `reverb.rs`'s analogous `damping_coefficient` is documented without an 'exact -3dB' claim), or the coefficient should be derived via the bilinear-transform-corrected one-pole formula (pre-warping `fc` with `tan`) so the claim is actually true across the parameter's full range.

**Fix direction.** Replace the exactness claim with an accurate one: state that `a = 1-exp(-2*pi*fc/fs)` is the impulse-invariant RC mapping and is only a close approximation of the true digital -3dB point for fc << fs, degrading (and eventually becoming unreachable below Nyquist) as fc approaches fs/2 — or replace the coefficient with the exact bilinear-derived one-pole formula so the code matches the doc's claim.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-152 · medium · SpectrumAnalyzer::magnitudes applies the interior-bin 4/size mirror-recovery scale to the self-mirrored DC and Nyquist bins, reading them +6.02 dB hot; untested and currently masked by the only production caller's 30 Hz-18 kHz band range.

`crates/auris-dsp/src/spectrum.rs:166` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No musician-visible effect today: the only production caller (the analyser overlay in crates/auris-gpui/src/ui/analyser.rs) always uses LOW_HZ=30/HIGH_HZ=18000, which excludes the DC bin (0 Hz) and the Nyquist bin (sample_rate/2, always above 18 kHz for standard audio interfaces) before they reach the display. But any other caller of the public SpectrumAnalyzer::magnitudes API - a future test, a different UI panel, a wider band range, or an engine running at an unusually low sample rate (<=36 kHz) - would read the DC and Nyquist bins as +6.02 dB too loud, silently.

**Trigger.** Any DC-biased material (a common artifact of a poorly recorded or badly converted source - exactly the sort of low-frequency content an equalizer's analyzer overlay is meant to show accurately) or any signal with energy exactly at the Nyquist rate (e.g. the crate's own `DistortionMode::Bitcrush`, which deliberately aliases). Feed either through `SpectrumAnalyzer` and read bin 0 or bin `bin_count()-1`.

**Mechanism.** `scale = 4.0 / size` (line 166) is applied uniformly to every bin in `bin_count()` (`0..=size/2`, lines 167-176). The doc comment directly above it (lines 163-165) explains the derivation: 'the transform of a real signal splits each tone between a bin and its mirror ... so the scale ... is 4/size rather than 1/size' - i.e. the factor of 2 baked into 4/size (versus the naive 2/size for a single-sided spectrum) exists specifically to recover the energy that a mirrored bin pair (`k` and `N-k`) split between them. Bins 0 (DC) and `size/2` (Nyquist) are the two bins that map to themselves under that mirror (`N-k = k`), so they never actually have a partner to recover energy from, yet get the same double-counting correction as every interior bin. I reproduced the crate's exact `fft`+`magnitudes` arithmetic standalone at size=1024: a full-scale, Hann-windowed DC signal reports amplitude 2.0 (+6.02 dBFS) and a full-scale Nyquist-rate signal reports the same +6.02 dBFS, while an ordinary full-scale sine placed exactly on an interior bin (bin 64) reports ~0.0 dBFS as the module's own test […]

**Expected.** Bins 0 and `size/2` are self-mirrored and should use half the interior scale (i.e. `scale/2`, equivalently `2.0/size`) so a full-scale DC or Nyquist-rate signal reads ~0 dBFS the same way the module's existing test shows for an interior bin.

**Fix direction.** In SpectrumAnalyzer::magnitudes (spectrum.rs:166), use a per-bin scale: 2.0/size for bin 0 and bin size/2 (Nyquist, when in range), and 4.0/size for all other bins, since those two bins are self-mirrored and get none of the mirror-splitting the 4/size factor corrects for. Add a unit test asserting a full-scale DC or Nyquist tone reads ~0 dBFS, matching the existing pattern used for a_full_scale_sine_reads_about_zero_decibels_at_its_own_frequency.

**Written rule it breaks.** DSP code lives behind unit tests that assert on numbers (levels, frequencies, lengths) rather than on "it runs".

### F-179 · medium · EnvelopeFollower::process (envelope.rs:110) omits the crate's mandatory settled() denormal/NaN flush that every other recirculating filter state uses, though the type is currently unused by any shipped effect.

`crates/auris-dsp/src/envelope.rs:110` · dsp · confirmed (executed reproduction; reported independently 1×)

**What a user sees.** No user-observable effect today: grep confirms EnvelopeFollower is not instantiated by any shipped instrument or effect, so no audio thread currently runs this code. The moment a limiter, gate, or meter effect adopts it (which is the type's whole purpose), a track that goes to silence after a transient will decay `state` through the subnormal f32 range every sample until it underflows to exact zero — on affected CPUs that means an extended run of denormal-arithmetic slowdown on the realtime audio callback thread, i.e. the exact glitch class the crate's `settled` helper exists to prevent.

**Trigger.** Feed the follower a transient, e.g. `EnvelopeFollower::new(48_000.0, 0.001, 0.100, EnvelopeMode::Peak)` then `process(1.0)` a few times so `state` sits near 1.0, then feed digital silence (`process(0.0)`) continuously — an ordinary quiet passage or the decay after a note ends. With coefficient `exp(-1/(0.1*48000)) ≈ 0.999979`, `state` decays geometrically as `state *= coefficient` every sample (rectified is 0 for silent input in both Peak and RMS mode) and never snaps to exact zero. After roughly 400,000-800,000 samples (well inside a single song, ~10-15 seconds) the f32 value of `state` crosses under the true hardware subnormal boundary (~1.1755e-38) and keeps recirculating there — every […]

**Mechanism.** `EnvelopeFollower::process` (lines 93-112) sanitizes only the *input*: `let input = if input.is_finite() { input } else { 0.0 };` (line 98). The follower's own recirculating state is then updated with a bare one-pole step, `self.state = rectified + (self.state - rectified) * coefficient;` (line 110) — no call to `crate::settled`, no check against `crate::DENORMAL_FLOOR`, nothing. That is the one recirculating filter state left unprotected in the crate: `biquad.rs:326-327` wraps `s1`/`s2` in `crate::settled(...)`, `reverb.rs:82` wraps the comb's `filter_store`, `reverb.rs:114` wraps the allpass tap read, `delay.rs:275-276` and `delay.rs:299` wrap the damping state `left_damp`/`right_damp`/`damp`, and `compressor.rs:239` wraps `gain_db`. `crates/auris-dsp/src/lib.rs:37` states the crate-wide rule this file violates: "Every feedback loop in the crate passes its state through this [`settled`]." A grep of the whole workspace for FTZ/DAZ/MXCSR/flush-to-zero configuration (`grep -rniE "flush.to.zero|denormal|subnormal|ftz|daz|mxcsr"`) turns up nothing outside this same doc comment — the […]

**Expected.** `self.state`'s update on line 110 should be wrapped in `crate::settled(...)`, matching every other recirculating state in the crate (`biquad.rs:326-327`, `reverb.rs:82`, `delay.rs:275-276,299`, `compressor.rs:239`) and the invariant `crates/auris-dsp/src/lib.rs:34-38` documents for the whole crate: state below `DENORMAL_FLOOR` (1e-30, itself set ~8 orders of magnitude above the real subnormal boundary so the flush always intercepts state before it becomes genuinely subnormal) should be zeroed […]

**Fix direction.** Wrap the state update at envelope.rs:110 as `self.state = crate::settled(rectified + (self.state - rectified) * coefficient);`, matching every sibling recirculating-state site (biquad.rs, chorus.rs, compressor.rs, delay.rs, reverb.rs) and add a regression test asserting the state reaches exact 0.0 within a bounded number of silent samples after a transient.

**Written rule it breaks.** Every feedback loop in the crate passes its state through this [settled]. (crates/auris-dsp/src/lib.rs doc comment on `settled`)

### F-262 · low · window_frames()'s doc claims the returned length is "always even" but the function never enforces it; only its sole caller's `& !1` mask makes that true today.

`crates/auris-dsp/src/stretch.rs:129` · spec-mismatch · confirmed (traced through the code; reported independently 1×)

**What a user sees.** No observable effect today: the sole caller (time_stretch, stretch.rs:68) masks the result with `& !1` before using it, so no odd window length ever reaches the stretching algorithm. A future caller of window_frames that trusted its doc comment and skipped that mask would silently get an odd window length, which would throw off the exact half-Hann-overlap unity sum the surrounding code relies on.

**Trigger.** Call `window_frames(44_100.0)` (or `window_frames(22_050.0)`) directly: `WINDOW_SECONDS * 44_100.0 = 0.050 * 44_100.0 = 2205.0` exactly, `.round()` gives `2205.0`, and `.max(MIN_WINDOW)` leaves it at `2205` — an odd number, directly contradicting "always even".

**Mechanism.** The doc comment reads `/// A window length in samples, at least [MIN_WINDOW] and always even.` (line 129) but the body — `let rate = sample_rate.max(1.0); ((WINDOW_SECONDS * rate).round() as usize).max(MIN_WINDOW)` (lines 130-133) — has no bit-masking or rounding-to-even step at all; it only floors the result at `MIN_WINDOW`. The only place evenness is actually enforced is in the sole caller, `time_stretch`, which separately does `window_frames(input.sample_rate()).min(frames / 4) & !1` (line 68) — an external mask the function's own doc comment does not mention needing.

**Expected.** Either the doc comment should not claim evenness (only `time_stretch`'s external mask guarantees it), or `window_frames` should apply `& !1` internally so the guarantee it advertises is actually true of its own return value.

**Fix direction.** Either enforce the invariant inside window_frames itself (e.g. `.max(MIN_WINDOW)` followed by `& !1`, or round MIN_WINDOW/WINDOW_SECONDS handling so the result is always even) and drop the redundant mask at the call site, or weaken the doc comment to say the value is not guaranteed even and note that callers must mask it. The former is smaller and keeps the invariant where the name/doc claims it lives.

**Written rule it breaks.** /// A window length in samples, at least [`MIN_WINDOW`] and always even.

### F-276 · low · GainPan reads the width parameter once per block unsmoothed, causing an audible zipper-noise step on the side channel when width is automated, unlike gain and pan which ramp over SMOOTHING_SECONDS.

`crates/auris-dsp/src/gain.rs:148` · dsp · confirmed (traced through the code; reported independently 1×)

**What a user sees.** Automating or fast-moving the width parameter (e.g. an automation lane, MIDI/DAW automation, or a fast UI drag) on the `auris.fx.gain` effect produces an audible click/zipper-noise step in the side channel at the first sample of the block following the change, while an equally fast gain or pan move ramps smoothly over 20ms with no click. Static or slow manual width changes are inaudible since the discontinuity is masked by normal audio content.

**Trigger.** Automate or move the `width` knob while stereo content with a non-trivial side signal is playing (e.g. from 1.0 to 0.0 between two blocks).

**Mechanism.** `process` reads `let width = self.params.at(P_WIDTH);` once per block (line 117) and uses that raw, unsmoothed value directly in `let side = (right_in - left_in) * 0.5 * width;` (line 148) for every sample of the block, while `gain`, `left` and `right` are all routed through `SmoothedValue`s that ramp per sample specifically to avoid "the step discontinuity that causes zipper noise" (line 17-19's own doc comment).

**Expected.** Given the file's own stated purpose for `SMOOTHING_SECONDS` ("long enough to remove the step discontinuity that causes zipper noise"), `width` should be ramped through a `SmoothedValue` the same way `gain`/`left`/`right` are, rather than read as a raw per-block constant.

**Fix direction.** Add a fourth `SmoothedValue` for width (or reuse the existing smoothing machinery), call `self.width.set_target(self.params.at(P_WIDTH))` alongside the other `set_target` calls, and read `self.width.next_value()` inside the per-sample loop instead of the loop-invariant `width` local.

**Written rule it breaks.** // Ramp time for gain and pan moves. Long enough to remove the step discontinuity that causes zipper noise, short enough that a fader still feels immediate. (SMOOTHING_SECONDS doc comment, gain.rs:17-19); module doc comment (gain.rs:1) names "level, stereo position and stereo width" as peer controls of the same plugin.
