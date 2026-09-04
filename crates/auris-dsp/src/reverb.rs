//! `auris.fx.reverb` — a Freeverb-style Schroeder reverberator.

use auris_core::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::MILLISECONDS_PER_SECOND;
use crate::bank::ParamBank;
use crate::delay_line::DelayLine;
use crate::smooth::SmoothedValue;

const P_ROOM_SIZE: u32 = 0;
const P_DAMPING: u32 = 1;
const P_WIDTH: u32 = 2;
const P_MIX: u32 = 3;
const P_PRE_DELAY_MS: u32 = 4;

/// Comb delay lengths in samples, from Jezar at Dreampoint's public-domain *Freeverb*.
///
/// They are mutually prime-ish and spread over roughly 25 to 37 ms, which is what stops the
/// parallel bank from ringing on a single pitch. Given at the rate they were tuned for.
const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];

/// All-pass delay lengths in samples, same source.
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];

/// Offset added to every delay on the right channel, which decorrelates the two sides.
const STEREO_SPREAD: usize = 23;

/// Sample rate the tunings above were chosen at; everything is scaled from here.
const TUNING_SAMPLE_RATE: f64 = 44_100.0;

/// Input attenuation. The comb bank has a large DC gain, so the dry signal has to be scaled
/// well down before entering it.
const FIXED_GAIN: f32 = 0.015;

/// Damping, room size and wet level are all mapped from 0..1 with Jezar's original constants.
const SCALE_DAMP: f32 = 0.4;
const SCALE_ROOM: f32 = 0.28;
const OFFSET_ROOM: f32 = 0.7;
const SCALE_WET: f32 = 3.0;

/// All-pass feedback, fixed in the original design.
const ALLPASS_FEEDBACK: f32 = 0.5;

/// Longest pre-delay offered, in milliseconds.
const MAX_PRE_DELAY_MS: f32 = 200.0;

/// Ramp time for controls that change the reverb network or dry/wet balance.
const CONTROL_SMOOTHING_SECONDS: f32 = 0.020;
/// Ramp time for the pre-delay read head, matching the tape-style glide of [`crate::Delay`].
const PRE_DELAY_SMOOTHING_SECONDS: f32 = 0.050;

/// Ceiling on the reported tail so a maximum-size room does not add a minute to every export.
const MAX_TAIL_SECONDS: f32 = 10.0;

/// A low-pass damped feedback comb filter.
struct Comb {
    buffer: Vec<f32>,
    index: usize,
    filter_store: f32,
}

impl Comb {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            index: 0,
            filter_store: 0.0,
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
        self.filter_store = 0.0;
    }

    /// `damp` is the low-pass coefficient inside the feedback path; `feedback` sets the decay.
    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        // The read is settled — see `crate::settled` — so a non-finite slot (one bad input
        // sample, a lap ago) re-enters neither the feedback path nor the wet sum. The slot
        // itself heals when this lap's write reaches it.
        let output = crate::settled(self.buffer[self.index]);
        self.filter_store = crate::settled(output * (1.0 - damp) + self.filter_store * damp);
        self.buffer[self.index] = input + self.filter_store * feedback;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
        output
    }
}

/// A Schroeder all-pass section: flat magnitude, dense phase.
struct Allpass {
    buffer: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            index: 0,
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        // Settled for the same reason the comb's read is: this slot feeds its own next value.
        let buffered = crate::settled(self.buffer[self.index]);
        let output = buffered - input;
        self.buffer[self.index] = input + buffered * ALLPASS_FEEDBACK;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
        output
    }
}

/// One channel's network: eight parallel combs feeding four all-passes in series.
struct ReverbChannel {
    combs: [Comb; 8],
    allpasses: [Allpass; 4],
}

impl ReverbChannel {
    fn new(sample_rate: f64, offset: usize) -> Self {
        Self {
            combs: std::array::from_fn(|i| Comb::new(scaled(COMB_TUNING[i], sample_rate, offset))),
            allpasses: std::array::from_fn(|i| {
                Allpass::new(scaled(ALLPASS_TUNING[i], sample_rate, offset))
            }),
        }
    }

    fn reset(&mut self) {
        for comb in &mut self.combs {
            comb.reset();
        }
        for allpass in &mut self.allpasses {
            allpass.reset();
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let mut wet = 0.0;
        for comb in &mut self.combs {
            wet += comb.process(input, feedback, damp);
        }
        for allpass in &mut self.allpasses {
            wet = allpass.process(wet);
        }
        wet
    }
}

fn scaled(tuning: usize, sample_rate: f64, offset: usize) -> usize {
    let length = (tuning as f64 * sample_rate / TUNING_SAMPLE_RATE).round() as usize;
    (length + offset).max(1)
}

/// Cutoff of the comb damping filter, in Hz, for a 0..1 damping setting.
///
/// Jezar's original mapping was `damping * SCALE_DAMP` used directly as the filter's pole, which
/// pins the corner to a fixed *fraction* of the sample rate. Reading that pole back as a
/// frequency at the rate the reverb was tuned at recovers the frequency it was always meant to
/// be: `fc = -ln(damping * SCALE_DAMP) * TUNING_SAMPLE_RATE / TAU`, i.e. 6432 Hz at damping 1.0
/// and rising past Nyquist as the control opens. Keeping this curve rather than inventing a new
/// range means a project authored at 44.1 kHz still renders exactly as it did.
///
/// Only defined for `damping > 0`; at zero the original mapping means "no filtering at all",
/// which no finite cutoff expresses.
fn damping_cutoff_hz(damping: f32) -> f32 {
    -(damping * SCALE_DAMP).ln() * TUNING_SAMPLE_RATE as f32 / std::f32::consts::TAU
}

/// Retention coefficient of the comb damping filter at `sample_rate`.
///
/// The filter is `y = x * (1 - a) + y * a`, so `a` is the pole itself and the impulse-invariant
/// mapping is `exp(-TAU * fc / fs)` with no `1 -` — the mirror image of [`crate::delay`]'s
/// `damping_alpha`, which smooths towards its input instead of retaining its state. Deriving
/// `a` from a frequency is what keeps the tail's brightness the same whether the graph was built
/// at the device rate for preview or at the project rate for export.
fn damping_coefficient(damping: f32, sample_rate: f32) -> f32 {
    if damping <= 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return 0.0;
    }
    let cutoff = damping_cutoff_hz(damping);
    (-std::f32::consts::TAU * cutoff / sample_rate)
        .exp()
        .clamp(0.0, 1.0)
}

/// A Freeverb-style reverberator.
pub struct Reverb {
    params: ParamBank,
    sample_rate: f32,
    room_size: SmoothedValue,
    damping: SmoothedValue,
    width: SmoothedValue,
    mix: SmoothedValue,
    pre_delay_ms: SmoothedValue,
    channels: Vec<ReverbChannel>,
    pre_delay: Vec<DelayLine>,
    /// Wet signal per channel for the current frame; sized in `prepare` so `process` never
    /// allocates while it gathers across channels.
    wet: Vec<f32>,
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Reverb {
    /// A new reverb at its default settings.
    pub fn new() -> Self {
        let descriptors = vec![
            ParamDescriptor::percent(P_ROOM_SIZE, "room_size", "Room Size", 0.5),
            ParamDescriptor::percent(P_DAMPING, "damping", "Damping", 0.5),
            ParamDescriptor::percent(P_WIDTH, "width", "Width", 1.0),
            ParamDescriptor::percent(P_MIX, "mix", "Mix", 0.3),
            ParamDescriptor::new(
                P_PRE_DELAY_MS,
                "pre_delay_ms",
                "Pre-Delay",
                0.0,
                MAX_PRE_DELAY_MS,
                0.0,
            )
            .with_unit(ParamUnit::Milliseconds),
        ];
        Self {
            params: ParamBank::new(descriptors),
            sample_rate: 48_000.0,
            room_size: SmoothedValue::new(0.5, CONTROL_SMOOTHING_SECONDS, 48_000.0),
            damping: SmoothedValue::new(
                damping_coefficient(0.5, 48_000.0),
                CONTROL_SMOOTHING_SECONDS,
                48_000.0,
            ),
            width: SmoothedValue::new(1.0, CONTROL_SMOOTHING_SECONDS, 48_000.0),
            mix: SmoothedValue::new(0.3, CONTROL_SMOOTHING_SECONDS, 48_000.0),
            pre_delay_ms: SmoothedValue::new(0.0, PRE_DELAY_SMOOTHING_SECONDS, 48_000.0),
            channels: Vec::new(),
            pre_delay: Vec::new(),
            wet: Vec::new(),
        }
    }

    fn snap_to_params(&mut self) {
        self.room_size.snap_to(self.params.at(P_ROOM_SIZE));
        self.damping.snap_to(damping_coefficient(
            self.params.at(P_DAMPING),
            self.sample_rate,
        ));
        self.width.snap_to(self.params.at(P_WIDTH));
        self.mix.snap_to(self.params.at(P_MIX));
        self.pre_delay_ms.snap_to(self.params.at(P_PRE_DELAY_MS));
    }

    /// Comb feedback for the current room size, in `OFFSET_ROOM ..= OFFSET_ROOM + SCALE_ROOM`.
    fn feedback(&self) -> f32 {
        self.params.at(P_ROOM_SIZE) * SCALE_ROOM + OFFSET_ROOM
    }
}

impl Parameterized for Reverb {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        self.params.set(id, value);
    }
}

impl Effect for Reverb {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            "auris.fx.reverb",
            "Reverb",
            "Schroeder reverb: eight damped combs into four all-passes per channel",
            PluginCategory::Reverb,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate as f32;
        let channel_count = ctx.channel_count.max(1);
        let spread = scaled(STEREO_SPREAD, ctx.sample_rate, 0);
        let pre_delay_capacity =
            (MAX_PRE_DELAY_MS * self.sample_rate / MILLISECONDS_PER_SECOND).ceil() as usize + 2;

        self.channels.clear();
        self.channels.reserve(channel_count);
        for channel in 0..channel_count {
            // Offsetting the right channel's delays is what turns one mono tail into a stereo
            // one; without it both sides would be identical and the reverb would collapse.
            self.channels
                .push(ReverbChannel::new(ctx.sample_rate, channel * spread));
        }

        self.pre_delay.clear();
        self.pre_delay
            .resize_with(channel_count, || DelayLine::new(pre_delay_capacity));
        self.wet.clear();
        self.wet.resize(channel_count, 0.0);

        for smoother in [
            &mut self.room_size,
            &mut self.damping,
            &mut self.width,
            &mut self.mix,
        ] {
            smoother.set_time(CONTROL_SMOOTHING_SECONDS, self.sample_rate);
        }
        self.pre_delay_ms
            .set_time(PRE_DELAY_SMOOTHING_SECONDS, self.sample_rate);
        self.snap_to_params();
    }

    fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.reset();
        }
        for line in &mut self.pre_delay {
            line.reset();
        }
        self.wet.fill(0.0);
        self.snap_to_params();
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count();
        let channel_count = buffer.channel_count().min(self.channels.len());
        if frames == 0 || channel_count == 0 {
            return;
        }

        self.room_size.set_target(self.params.at(P_ROOM_SIZE));
        self.damping.set_target(damping_coefficient(
            self.params.at(P_DAMPING),
            self.sample_rate,
        ));
        self.width.set_target(self.params.at(P_WIDTH));
        self.mix.set_target(self.params.at(P_MIX));
        self.pre_delay_ms.set_target(self.params.at(P_PRE_DELAY_MS));

        for frame in 0..frames {
            let feedback = self.room_size.next_value() * SCALE_ROOM + OFFSET_ROOM;
            let damp = self.damping.next_value();
            let width = self.width.next_value();
            let mix = self.mix.next_value();
            let dry = 1.0 - mix;
            let wet_level = mix * SCALE_WET;
            // Freeverb's stereo spread: `wet1` keeps a channel's own tail, `wet2` bleeds the
            // other channel's in, so width 0 collapses the tail to mono and width 1 leaves it
            // wide.
            let wet1 = wet_level * (width * 0.5 + 0.5);
            let wet2 = wet_level * ((1.0 - width) * 0.5);
            let pre_delay_samples = (self.pre_delay_ms.next_value() * self.sample_rate
                / MILLISECONDS_PER_SECOND)
                .max(1.0);
            let mut input = 0.0;
            for channel in 0..channel_count {
                let sample = buffer.channel(channel)[frame];
                let line = &mut self.pre_delay[channel];
                input += line.read(pre_delay_samples);
                line.write(sample);
            }
            input *= FIXED_GAIN;

            for (channel, network) in self.channels.iter_mut().take(channel_count).enumerate() {
                self.wet[channel] = network.process(input, feedback, damp);
            }

            if channel_count == 2 {
                let (left, right) = (self.wet[0], self.wet[1]);
                let dry_left = buffer.channel(0)[frame];
                let dry_right = buffer.channel(1)[frame];
                buffer.channel_mut(0)[frame] = left * wet1 + right * wet2 + dry_left * dry;
                buffer.channel_mut(1)[frame] = right * wet1 + left * wet2 + dry_right * dry;
            } else {
                for channel in 0..channel_count {
                    let wet = self.wet[channel];
                    let sample = &mut buffer.channel_mut(channel)[frame];
                    *sample = *sample * dry + wet * wet_level;
                }
            }
        }
    }

    fn tail_frames(&self) -> usize {
        let feedback = self.feedback();
        let longest_tuning = COMB_TUNING.iter().copied().max().unwrap_or(1_617);
        let longest = scaled(longest_tuning, self.sample_rate as f64, 0) as f32;
        // Every trip round the longest comb costs -20*log10(feedback) dB; count trips to -60.
        let per_loop_db = -20.0 * feedback.log10();
        let loops = if per_loop_db > 1.0e-4 {
            60.0 / per_loop_db
        } else {
            0.0
        };
        let pre_delay = self.params.at(P_PRE_DELAY_MS) * self.sample_rate / MILLISECONDS_PER_SECOND;
        (longest * loops + pre_delay).clamp(0.0, MAX_TAIL_SECONDS * self.sample_rate) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn prepared() -> Reverb {
        let mut plugin = Reverb::new();
        plugin.prepare(&PrepareContext::new(SR, 4_096, 2));
        plugin
    }

    fn context(frames: usize) -> ProcessContext {
        ProcessContext::realtime(SR, frames, 0, 120.0, true)
    }

    fn impulse(frames: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::stereo(frames, SR);
        buffer.channel_mut(0)[0] = 1.0;
        buffer.channel_mut(1)[0] = 1.0;
        buffer
    }

    #[test]
    fn a_dry_mix_is_a_bit_exact_bypass() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mix", 0.0);
        plugin.prepare(&PrepareContext::new(SR, 4_096, 2));
        let input = impulse(1_024);
        let mut buffer = input.clone();
        plugin.process(&mut buffer, &context(1_024));
        assert_eq!(buffer.channel(0), input.channel(0));
    }

    #[test]
    fn every_live_control_moves_without_a_block_boundary_step() {
        let mut plugin = prepared();
        plugin.set_param_by_key("room_size", 1.0);
        plugin.set_param_by_key("damping", 1.0);
        plugin.set_param_by_key("width", 0.0);
        plugin.set_param_by_key("mix", 1.0);
        plugin.set_param_by_key("pre_delay_ms", MAX_PRE_DELAY_MS);

        let damping_start = plugin.damping.current();
        let damping_target = damping_coefficient(1.0, SR as f32);

        let mut buffer = AudioBuffer::stereo(1, SR);
        buffer.channel_mut(0)[0] = 1.0;
        buffer.channel_mut(1)[0] = 1.0;
        plugin.process(&mut buffer, &context(1));

        for (name, current, start, target) in [
            ("room size", plugin.room_size.current(), 0.5_f32, 1.0),
            (
                "damping",
                plugin.damping.current(),
                damping_start,
                damping_target,
            ),
            ("width", plugin.width.current(), 1.0, 0.0),
            ("mix", plugin.mix.current(), 0.3, 1.0),
            ("pre-delay", plugin.pre_delay_ms.current(), 0.0, 200.0),
        ] {
            assert!(
                current > start.min(target) && current < start.max(target),
                "{name} stepped or did not move: {start} -> {current} (target {target})"
            );
        }
        // Before the wet network can answer, the first sample exposes the mix ramp directly:
        // it stays near the previous 70%-dry setting instead of jumping to silence at mix 100%.
        assert!(buffer.channel(0)[0] > 0.69);
    }

    #[test]
    fn an_impulse_grows_a_decaying_tail() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mix", 1.0);
        let mut buffer = impulse(48_000);
        plugin.process(&mut buffer, &context(48_000));

        // Energy must appear well after the impulse, then die away.
        let early = buffer.slice(2_400, 4_800).channel_rms(0);
        let late = buffer.slice(38_400, 4_800).channel_rms(0);
        assert!(early > 1e-4, "no tail at 50 ms: {early}");
        assert!(late < early, "tail did not decay: {early} -> {late}");
        assert!(buffer.channel(0).iter().all(|s| s.is_finite()));
    }

    #[test]
    fn the_two_channels_decorrelate() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mix", 1.0);
        let mut buffer = impulse(24_000);
        plugin.process(&mut buffer, &context(24_000));

        let tail = buffer.slice(4_800, 12_000);
        // Compare the *difference* signal against the tail's own level. A bare sum of absolute
        // differences would pass for two channels that are identical to within rounding, which is
        // exactly the bug this test exists to catch (a missing stereo spread).
        let difference_rms = {
            let sum: f64 = tail
                .channel(0)
                .iter()
                .zip(tail.channel(1))
                .map(|(l, r)| ((l - r) as f64).powi(2))
                .sum();
            (sum / tail.frame_count() as f64).sqrt() as f32
        };
        let level = tail.channel_rms(0);
        assert!(level > 1e-4, "there was no tail to compare: {level}");
        assert!(
            difference_rms / level > 0.3,
            "channels were near-identical: difference {difference_rms} against level {level}"
        );
    }

    #[test]
    fn zero_width_makes_both_channels_identical() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mix", 1.0);
        plugin.set_param_by_key("width", 0.0);
        plugin.prepare(&PrepareContext::new(SR, 4_096, 2));
        let mut buffer = impulse(8_192);
        plugin.process(&mut buffer, &context(8_192));
        for (left, right) in buffer.channel(0).iter().zip(buffer.channel(1)) {
            assert!((left - right).abs() < 1e-6);
        }
    }

    #[test]
    fn the_tail_grows_with_the_room() {
        let mut plugin = prepared();
        plugin.set_param_by_key("room_size", 0.0);
        let small = plugin.tail_frames();
        plugin.set_param_by_key("room_size", 0.5);
        let medium = plugin.tail_frames();
        plugin.set_param_by_key("room_size", 1.0);
        let large = plugin.tail_frames();

        assert!(small < medium, "{small} !< {medium}");
        assert!(medium < large, "{medium} !< {large}");
        // The smallest room still rings for a few hundred milliseconds.
        assert!(
            (24_000..48_000).contains(&small),
            "smallest tail was {small} frames"
        );
        assert_eq!(large, (MAX_TAIL_SECONDS * SR as f32) as usize);
    }

    #[test]
    fn pre_delay_holds_the_tail_back() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mix", 1.0);
        plugin.set_param_by_key("pre_delay_ms", 100.0);
        plugin.prepare(&PrepareContext::new(SR, 4_096, 2));
        let mut buffer = impulse(24_000);
        plugin.process(&mut buffer, &context(24_000));
        // 100 ms is 4 800 frames; nothing may arrive before that.
        let before = buffer.slice(0, 4_700).peak();
        let after = buffer.slice(4_800, 4_800).peak();
        assert_eq!(before, 0.0);
        assert!(after > 1e-5, "nothing arrived after the pre-delay: {after}");
    }

    /// Decay of the reverb tail at `frequency`, in dB per second, rendered at `sample_rate`.
    ///
    /// The excitation is a raised-cosine-enveloped tone burst, so the render stays in a narrow
    /// band around `frequency` and a plain RMS of the tail measures that band directly. Both
    /// measurement windows sit well after the burst, where the tail has settled onto its
    /// slowest mode and the reading is a decay rate rather than a transient.
    fn tail_decay_db_per_second(sample_rate: f64, frequency: f32) -> f32 {
        const EARLY_SECONDS: f64 = 0.5;
        const LATE_SECONDS: f64 = 1.3;
        const WINDOW_SECONDS: f64 = 0.2;
        const BURST_SECONDS: f64 = 0.2;

        let mut plugin = Reverb::new();
        plugin.set_param_by_key("room_size", 0.9);
        plugin.set_param_by_key("damping", 1.0);
        plugin.set_param_by_key("mix", 1.0);
        plugin.prepare(&PrepareContext::new(sample_rate, 4_096, 2));

        let at = |t: f64| (t * sample_rate).round() as usize;
        let frames = at(LATE_SECONDS + WINDOW_SECONDS);
        let burst = at(BURST_SECONDS);
        let mut buffer = AudioBuffer::stereo(frames, sample_rate);
        let step = std::f32::consts::TAU * frequency / sample_rate as f32;
        for channel in buffer.channels_mut() {
            for (n, sample) in channel.iter_mut().enumerate().take(burst) {
                let envelope = 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / burst as f32).cos();
                *sample = (step * n as f32).sin() * envelope;
            }
        }
        plugin.process(
            &mut buffer,
            &ProcessContext::realtime(sample_rate, frames, 0, 120.0, true),
        );

        let window = at(WINDOW_SECONDS);
        let early = buffer.slice(at(EARLY_SECONDS), window).channel_rms(0);
        let late = buffer.slice(at(LATE_SECONDS), window).channel_rms(0);
        assert!(
            early > 1.0e-6 && late > 0.0,
            "{sample_rate} Hz: no {frequency} Hz tail to measure ({early} -> {late})"
        );
        20.0 * (late / early).log10() / (LATE_SECONDS - EARLY_SECONDS) as f32
    }

    #[test]
    fn the_damped_tail_decays_at_the_same_rate_at_every_sample_rate() {
        // The damping filter's corner has to be a frequency, not a fraction of fs. When its
        // coefficient is used as a bare z-plane pole the corner moves with the sample rate, so
        // a 6 kHz tail that dies away in a second at 44.1 kHz still rings at 96 kHz — which is
        // exactly the mismatch a user hears between a device-rate preview and a project-rate
        // export.
        let at_44k = tail_decay_db_per_second(44_100.0, 6_000.0);
        let at_96k = tail_decay_db_per_second(96_000.0, 6_000.0);
        assert!(
            (at_44k - at_96k).abs() < 3.0,
            "6 kHz tail decayed at {at_44k} dB/s at 44.1 kHz but {at_96k} dB/s at 96 kHz"
        );

        // A control well below the damping corner, where the comb tuning alone sets the decay.
        // It already agreed across rates, so a regression here means the comb bank moved.
        let low_at_44k = tail_decay_db_per_second(44_100.0, 400.0);
        let low_at_96k = tail_decay_db_per_second(96_000.0, 400.0);
        assert!(
            (low_at_44k - low_at_96k).abs() < 3.0,
            "400 Hz tail decayed at {low_at_44k} dB/s at 44.1 kHz but {low_at_96k} dB/s at 96 kHz"
        );

        // Both assertions above would also hold if the coefficient never reached the comb bank
        // at all, so pin the fact that damping is engaged: at full damping the band above the
        // corner has to die away far faster than the band below it, at either rate. Measured
        // separation is around 68 dB/s; the threshold only has to rule out "no filtering".
        for (rate, high, low) in [
            (44_100.0, at_44k, low_at_44k),
            (96_000.0, at_96k, low_at_96k),
        ] {
            assert!(
                low - high > 30.0,
                "{rate} Hz: damping barely engaged, 6 kHz decayed at {high} dB/s \
                 against {low} dB/s at 400 Hz"
            );
        }
    }

    #[test]
    fn the_damping_pole_is_unchanged_at_the_tuning_rate() {
        // Existing projects at 44.1 kHz must render exactly as they did, so the cutoff mapping
        // has to reproduce Jezar's `damping * SCALE_DAMP` pole at TUNING_SAMPLE_RATE.
        for damping in [0.1f32, 0.25, 0.5, 0.75, 1.0] {
            let coefficient = damping_coefficient(damping, TUNING_SAMPLE_RATE as f32);
            let legacy = damping * SCALE_DAMP;
            assert!(
                (coefficient - legacy).abs() < 1.0e-6,
                "damping {damping}: {coefficient} != {legacy}"
            );
        }
        // Damping 0 means "no filtering at all", which no finite cutoff can express.
        assert_eq!(damping_coefficient(0.0, 44_100.0), 0.0);
        assert_eq!(damping_coefficient(1.0, 0.0), 0.0);
        // The pole must fall as the rate rises, holding the corner at a fixed frequency.
        let corner = damping_cutoff_hz(1.0);
        assert!((corner - 6_431.6).abs() < 1.0, "corner was {corner} Hz");
        let at_96k = damping_coefficient(1.0, 96_000.0);
        let expected = (-std::f32::consts::TAU * corner / 96_000.0).exp();
        assert!(
            (at_96k - expected).abs() < 1.0e-6,
            "96 kHz pole was {at_96k}"
        );
    }

    #[test]
    fn silence_stays_silent() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mix", 1.0);
        plugin.set_param_by_key("room_size", 1.0);
        let mut buffer = AudioBuffer::stereo(4_096, SR);
        plugin.process(&mut buffer, &context(4_096));
        assert_eq!(buffer.peak(), 0.0);
    }

    #[test]
    fn a_non_finite_sample_does_not_silence_the_tail() {
        // A NaN reaching a comb used to poison its whole ring within one lap — for good, since
        // no arithmetic clears a NaN and `feedback * 0` never gets the chance — and the reverb
        // fell silent until reset. The reads are settled now, so the network heals and a later
        // impulse still grows a tail.
        let mut plugin = prepared();
        plugin.set_param_by_key("mix", 1.0);

        let mut poisoned = impulse(4_800);
        poisoned.channel_mut(0)[1] = f32::NAN;
        poisoned.channel_mut(1)[1] = f32::INFINITY;
        plugin.process(&mut poisoned, &context(4_800));
        // The poisoned frame itself passes through — the dry term is `NaN * 0.0`, and hiding a
        // source's own bad sample is the master sanitize's job — but it must not spread.
        let leaked = poisoned
            .channel(0)
            .iter()
            .enumerate()
            .filter(|(frame, sample)| *frame != 1 && !sample.is_finite())
            .count();
        assert_eq!(leaked, 0, "the poison spread beyond its own frame");

        let mut buffer = impulse(48_000);
        plugin.process(&mut buffer, &context(48_000));
        assert!(buffer.channel(0).iter().all(|s| s.is_finite()));
        let tail = buffer.slice(2_400, 4_800).channel_rms(0);
        assert!(tail > 1e-4, "the tail never came back: {tail}");
    }
}
