//! `auris.fx.delay` — a feedback delay with damping and a ping-pong mode.

use auris_core::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId, ParamUnit, ParamValueCurve};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::MILLISECONDS_PER_SECOND;
use crate::bank::ParamBank;
use crate::delay_line::DelayLine;
use crate::smooth::SmoothedValue;

const P_TIME_MS: u32 = 0;
const P_FEEDBACK: u32 = 1;
const P_DAMPING_HZ: u32 = 2;
const P_MIX: u32 = 3;
const P_PING_PONG: u32 = 4;

/// Longest delay the plugin offers, and therefore the length of every delay line.
const MAX_TIME_MS: f32 = 2_000.0;
const MIN_TIME_MS: f32 = 1.0;

/// Ramp time for the delay length. Moving the read head gradually is what gives a delay its
/// familiar tape-style pitch slide instead of a click.
const TIME_SMOOTHING_SECONDS: f32 = 0.050;
/// Ramp time for the dry/wet balance.
const MIX_SMOOTHING_SECONDS: f32 = 0.020;

/// Longest tail the plugin will ask the offline renderer to keep rendering, in seconds.
///
/// At 95 % feedback the true -60 dB point is well over a minute away, which nobody wants
/// appended to an export.
const MAX_TAIL_SECONDS: f32 = 10.0;

/// A stereo feedback delay.
///
/// The damping filter sits **inside** the feedback loop only, so the first repeat is a bit-exact
/// copy of the input and each subsequent one is progressively darker — the behaviour of an
/// analogue bucket-brigade line.
pub struct Delay {
    params: ParamBank,
    sample_rate: f32,
    lines: Vec<DelayLine>,
    damping_state: Vec<f32>,
    time: SmoothedValue,
    mix: SmoothedValue,
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

impl Delay {
    /// A new delay at its default settings.
    pub fn new() -> Self {
        let sample_rate = 48_000.0;
        let descriptors = vec![
            ParamDescriptor::new(
                P_TIME_MS,
                "time_ms",
                "Time",
                MIN_TIME_MS,
                MAX_TIME_MS,
                350.0,
            )
            .with_unit(ParamUnit::Milliseconds)
            .with_curve(ParamValueCurve::Logarithmic),
            // 0.95 is the highest feedback that still decays; above it the loop runs away.
            ParamDescriptor::new(P_FEEDBACK, "feedback", "Feedback", 0.0, 0.95, 0.35)
                .with_unit(ParamUnit::Percent),
            ParamDescriptor::hertz(
                P_DAMPING_HZ,
                "damping_hz",
                "Damping",
                200.0,
                20_000.0,
                6_000.0,
            ),
            ParamDescriptor::percent(P_MIX, "mix", "Mix", 0.3),
            ParamDescriptor::toggle(P_PING_PONG, "ping_pong", "Ping-Pong", false),
        ];
        let mut plugin = Self {
            params: ParamBank::new(descriptors),
            sample_rate,
            lines: Vec::new(),
            damping_state: Vec::new(),
            time: SmoothedValue::new(0.0, TIME_SMOOTHING_SECONDS, sample_rate),
            mix: SmoothedValue::new(0.0, MIX_SMOOTHING_SECONDS, sample_rate),
        };
        plugin.snap_to_params();
        plugin
    }

    fn delay_in_samples(&self) -> f32 {
        self.params.at(P_TIME_MS) * self.sample_rate / MILLISECONDS_PER_SECOND
    }

    fn snap_to_params(&mut self) {
        self.time.snap_to(self.delay_in_samples());
        self.mix.snap_to(self.params.at(P_MIX));
    }

    /// One-pole low-pass coefficient for the damping filter, as `y += a * (x - y)`.
    ///
    /// `a = 1 - exp(-2 pi fc / fs)` is the impulse-invariant mapping of a first-order RC
    /// section, so the cutoff really is the -3 dB point rather than a rough approximation.
    fn damping_alpha(&self) -> f32 {
        let cutoff = self.params.at(P_DAMPING_HZ);
        if self.sample_rate <= 0.0 {
            return 1.0;
        }
        let a = 1.0 - (-std::f32::consts::TAU * cutoff / self.sample_rate).exp();
        a.clamp(0.0, 1.0)
    }
}

impl Parameterized for Delay {
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

impl Effect for Delay {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            "auris.fx.delay",
            "Delay",
            "Feedback delay with damping and a ping-pong mode",
            PluginCategory::Delay,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate as f32;
        let channels = ctx.channel_count.max(1);
        let capacity =
            (MAX_TIME_MS * self.sample_rate / MILLISECONDS_PER_SECOND).ceil() as usize + 2;

        self.lines.clear();
        self.lines
            .resize_with(channels, || DelayLine::new(capacity));
        self.damping_state.clear();
        self.damping_state.resize(channels, 0.0);

        self.time.set_time(TIME_SMOOTHING_SECONDS, self.sample_rate);
        self.mix.set_time(MIX_SMOOTHING_SECONDS, self.sample_rate);
        self.snap_to_params();
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.reset();
        }
        self.damping_state.fill(0.0);
        self.snap_to_params();
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count();
        let channels = buffer.channel_count().min(self.lines.len());
        if frames == 0 || channels == 0 {
            return;
        }

        let feedback = self.params.at(P_FEEDBACK);
        let alpha = self.damping_alpha();
        self.time.set_target(self.delay_in_samples());
        self.mix.set_target(self.params.at(P_MIX));

        let ping_pong = self.params.flag(P_PING_PONG) && channels >= 2;
        let time_at_block_start = self.time;
        let mix_at_block_start = self.mix;

        // Ping-pong only has a meaning for the stereo pair; anything beyond it falls through to
        // the plain per-channel path below rather than being left undelayed.
        let first_plain_channel = if ping_pong { 2 } else { 0 };

        if ping_pong {
            let (left_lines, right_lines) = self.lines.split_at_mut(1);
            let (left_channels, right_channels) = buffer.channels_mut().split_at_mut(1);
            let left_line = &mut left_lines[0];
            let right_line = &mut right_lines[0];
            let mut left_damp = self.damping_state[0];
            let mut right_damp = self.damping_state[1];

            let left = &mut left_channels[0][..frames];
            let right = &mut right_channels[0][..frames];
            for (left_sample, right_sample) in left.iter_mut().zip(right.iter_mut()) {
                let delay = self.time.next_value();
                let mix = self.mix.next_value();
                let delayed_left = left_line.read(delay);
                let delayed_right = right_line.read(delay);

                left_damp += alpha * (delayed_right - left_damp);
                right_damp += alpha * (delayed_left - right_damp);
                // The crossed feedback is what makes repeats alternate between the speakers.
                left_line.write(*left_sample + feedback * left_damp);
                right_line.write(*right_sample + feedback * right_damp);

                *left_sample = *left_sample * (1.0 - mix) + delayed_left * mix;
                *right_sample = *right_sample * (1.0 - mix) + delayed_right * mix;
            }
            self.damping_state[0] = left_damp;
            self.damping_state[1] = right_damp;
        }

        for channel in first_plain_channel..channels {
            // Replay the identical ramp on every channel.
            let mut time = time_at_block_start;
            let mut mix = mix_at_block_start;
            let mut damp = self.damping_state[channel];
            let line = &mut self.lines[channel];

            for sample in buffer.channel_mut(channel)[..frames].iter_mut() {
                let delay = time.next_value();
                let wet_amount = mix.next_value();
                let delayed = line.read(delay);
                damp += alpha * (delayed - damp);
                line.write(*sample + feedback * damp);
                *sample = *sample * (1.0 - wet_amount) + delayed * wet_amount;
            }

            self.damping_state[channel] = damp;
            self.time = time;
            self.mix = mix;
        }
    }

    fn tail_frames(&self) -> usize {
        let delay_samples = self.delay_in_samples().max(1.0);
        let feedback = self.params.at(P_FEEDBACK);
        // Each trip round the loop scales the signal by `feedback`; count the trips it takes to
        // fall 60 dB (a factor of 1000).
        let repeats = if feedback <= 1.0e-4 {
            1.0
        } else {
            ((0.001f32).ln() / feedback.ln()).max(1.0)
        };
        let frames = delay_samples * repeats;
        (frames).clamp(0.0, MAX_TAIL_SECONDS * self.sample_rate) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn context(frames: usize) -> ProcessContext {
        ProcessContext::realtime(SR, frames, 0, 120.0, true)
    }

    /// A fully wet, feedback-free delay of `time_ms`, ready to process.
    fn dry_tap(time_ms: f32) -> Delay {
        let mut plugin = Delay::new();
        plugin.set_param_by_key("time_ms", time_ms);
        plugin.set_param_by_key("feedback", 0.0);
        plugin.set_param_by_key("mix", 1.0);
        plugin.prepare(&PrepareContext::new(SR, 8_192, 2));
        plugin
    }

    fn impulse(frames: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::stereo(frames, SR);
        buffer.channel_mut(0)[0] = 1.0;
        buffer.channel_mut(1)[0] = 1.0;
        buffer
    }

    #[test]
    fn an_impulse_reappears_at_the_requested_sample() {
        for time_ms in [1.0f32, 10.0, 33.0, 120.0] {
            let expected = (time_ms * SR as f32 / 1_000.0).round() as usize;
            let mut plugin = dry_tap(time_ms);
            let mut buffer = impulse(8_192);
            plugin.process(&mut buffer, &context(8_192));

            let channel = buffer.channel(0);
            assert!(
                (channel[expected] - 1.0).abs() < 1e-6,
                "{time_ms} ms: sample {expected} was {}",
                channel[expected]
            );
            assert_eq!(channel[expected - 1], 0.0);
            assert_eq!(channel[expected + 1], 0.0);
        }
    }

    #[test]
    fn a_fractional_delay_peaks_at_the_rounded_sample() {
        // 10.3 ms is 494.4 samples at 48 kHz; the energy splits over 494 and 495.
        let mut plugin = dry_tap(10.3);
        let mut buffer = impulse(2_048);
        plugin.process(&mut buffer, &context(2_048));

        let channel = buffer.channel(0);
        let peak_index = channel
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak_index, 494);
        assert!((channel[494] - 0.6).abs() < 1e-3, "got {}", channel[494]);
        assert!((channel[495] - 0.4).abs() < 1e-3, "got {}", channel[495]);
    }

    #[test]
    fn feedback_repeats_decay_by_the_feedback_amount() {
        let mut plugin = Delay::new();
        plugin.set_param_by_key("time_ms", 10.0);
        plugin.set_param_by_key("feedback", 0.5);
        plugin.set_param_by_key("mix", 1.0);
        // The damping filter sits in the feedback path; open it right up so the repeats are
        // scaled by the feedback amount alone.
        plugin.set_param_by_key("damping_hz", 20_000.0);
        plugin.prepare(&PrepareContext::new(SR, 8_192, 2));

        let mut buffer = impulse(4_096);
        plugin.process(&mut buffer, &context(4_096));
        let channel = buffer.channel(0);
        // First repeat is untouched by damping; later ones are only checked for decay.
        assert!((channel[480] - 1.0).abs() < 1e-6);
        let second: f32 = channel[955..=965].iter().map(|s| s.abs()).sum();
        assert!(
            (second - 0.5).abs() < 0.05,
            "second repeat summed to {second}"
        );
    }

    #[test]
    fn ping_pong_sends_the_first_repeat_to_the_other_side() {
        let mut plugin = Delay::new();
        plugin.set_param_by_key("time_ms", 10.0);
        plugin.set_param_by_key("feedback", 0.9);
        plugin.set_param_by_key("mix", 1.0);
        plugin.set_param_by_key("damping_hz", 20_000.0);
        plugin.set_param_by_key("ping_pong", 1.0);
        plugin.prepare(&PrepareContext::new(SR, 8_192, 2));

        let mut buffer = AudioBuffer::stereo(4_096, SR);
        buffer.channel_mut(0)[0] = 1.0;
        plugin.process(&mut buffer, &context(4_096));

        // Tap one comes back on the left (its own line), tap two crosses to the right.
        assert!((buffer.channel(0)[480] - 1.0).abs() < 1e-6);
        assert!(buffer.channel(1)[480].abs() < 1e-6);
        let crossed: f32 = buffer.channel(1)[955..=965].iter().map(|s| s.abs()).sum();
        assert!(crossed > 0.5, "right channel repeat summed to {crossed}");
    }

    #[test]
    fn ping_pong_still_delays_channels_beyond_the_stereo_pair() {
        let mut plugin = Delay::new();
        plugin.set_param_by_key("time_ms", 10.0);
        plugin.set_param_by_key("feedback", 0.0);
        plugin.set_param_by_key("mix", 1.0);
        plugin.set_param_by_key("ping_pong", 1.0);
        plugin.prepare(&PrepareContext::new(SR, 2_048, 4));

        let mut buffer = AudioBuffer::new(4, 2_048, SR);
        for channel in buffer.channels_mut() {
            channel[0] = 1.0;
        }
        plugin.process(&mut buffer, &context(2_048));
        for channel in 0..4 {
            assert!(
                (buffer.channel(channel)[480] - 1.0).abs() < 1e-6,
                "channel {channel} was {}",
                buffer.channel(channel)[480]
            );
        }
    }

    #[test]
    fn tail_scales_with_time_and_feedback() {
        let mut plugin = Delay::new();
        plugin.set_param_by_key("time_ms", 500.0);
        plugin.set_param_by_key("feedback", 0.0);
        plugin.prepare(&PrepareContext::new(SR, 512, 2));
        // One repeat of 500 ms is 24 000 frames.
        assert_eq!(plugin.tail_frames(), 24_000);

        plugin.set_param_by_key("feedback", 0.5);
        // ln(0.001)/ln(0.5) is 9.9658 repeats, so 239 179 frames give or take rounding.
        let tail = plugin.tail_frames();
        assert!((239_100..=239_260).contains(&tail), "tail was {tail}");

        plugin.set_param_by_key("feedback", 0.95);
        assert_eq!(plugin.tail_frames(), (10.0 * SR) as usize);
    }

    #[test]
    fn silence_stays_silent_and_a_signal_stays_finite() {
        let mut plugin = Delay::new();
        plugin.set_param_by_key("feedback", 0.95);
        plugin.prepare(&PrepareContext::new(SR, 4_096, 2));
        let mut buffer = AudioBuffer::stereo(4_096, SR);
        plugin.process(&mut buffer, &context(4_096));
        assert_eq!(buffer.peak(), 0.0);

        let step = std::f32::consts::TAU * 220.0 / SR as f32;
        for channel in buffer.channels_mut() {
            for (n, sample) in channel.iter_mut().enumerate() {
                *sample = (step * n as f32).sin() * 0.8;
            }
        }
        for _ in 0..40 {
            plugin.process(&mut buffer, &context(4_096));
            assert!(buffer.channel(0).iter().all(|s| s.is_finite()));
        }
    }
}
