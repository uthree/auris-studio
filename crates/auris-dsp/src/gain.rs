//! `auris.fx.gain` — level, stereo position and stereo width.

use auris_core::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId, ParamUnit, db_to_gain, pan_gains};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::bank::ParamBank;
use crate::smooth::SmoothedValue;

const P_GAIN_DB: u32 = 0;
const P_PAN: u32 = 1;
const P_WIDTH: u32 = 2;
const P_INVERT: u32 = 3;

/// Ramp time for gain and pan moves. Long enough to remove the step discontinuity that causes
/// zipper noise, short enough that a fader still feels immediate.
const SMOOTHING_SECONDS: f32 = 0.020;

/// Compensation applied to [`pan_gains`], whose constant-power law is `1/sqrt(2)` per side at
/// centre. A utility plugin has to be transparent at its defaults, so centre pan is normalised
/// to unity here; the trade-off is the usual +3 dB at the extremes.
const CENTRE_COMPENSATION: f32 = std::f32::consts::SQRT_2;

/// Gain, pan, stereo width and polarity.
pub struct GainPan {
    params: ParamBank,
    gain: SmoothedValue,
    left: SmoothedValue,
    right: SmoothedValue,
    polarity: SmoothedValue,
    width: SmoothedValue,
    sample_rate: f32,
}

impl Default for GainPan {
    fn default() -> Self {
        Self::new()
    }
}

impl GainPan {
    /// A new instance at its default settings (unity gain, centred, unchanged width).
    pub fn new() -> Self {
        let sample_rate = 48_000.0;
        let descriptors = vec![
            ParamDescriptor::decibels(P_GAIN_DB, "gain_db", "Gain", -60.0, 12.0, 0.0),
            ParamDescriptor::new(P_PAN, "pan", "Pan", -1.0, 1.0, 0.0).with_unit(ParamUnit::Pan),
            ParamDescriptor::new(P_WIDTH, "width", "Width", 0.0, 2.0, 1.0),
            ParamDescriptor::toggle(P_INVERT, "invert", "Phase Invert", false),
        ];
        let mut plugin = Self {
            params: ParamBank::new(descriptors),
            gain: SmoothedValue::new(1.0, SMOOTHING_SECONDS, sample_rate),
            left: SmoothedValue::new(1.0, SMOOTHING_SECONDS, sample_rate),
            right: SmoothedValue::new(1.0, SMOOTHING_SECONDS, sample_rate),
            polarity: SmoothedValue::new(1.0, SMOOTHING_SECONDS, sample_rate),
            width: SmoothedValue::new(1.0, SMOOTHING_SECONDS, sample_rate),
            sample_rate,
        };
        plugin.snap_to_params();
        plugin
    }

    fn snap_to_params(&mut self) {
        self.gain.snap_to(db_to_gain(self.params.at(P_GAIN_DB)));
        let (left, right) = pan_gains(self.params.at(P_PAN));
        self.left.snap_to(left * CENTRE_COMPENSATION);
        self.right.snap_to(right * CENTRE_COMPENSATION);
        self.polarity.snap_to(if self.params.flag(P_INVERT) {
            -1.0
        } else {
            1.0
        });
        self.width.snap_to(self.params.at(P_WIDTH));
    }
}

impl Parameterized for GainPan {
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

impl Effect for GainPan {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            "auris.fx.gain",
            "Gain & Pan",
            "Level, stereo position, stereo width and polarity",
            PluginCategory::Utility,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate as f32;
        for smoother in [
            &mut self.gain,
            &mut self.left,
            &mut self.right,
            &mut self.polarity,
            &mut self.width,
        ] {
            smoother.set_time(SMOOTHING_SECONDS, self.sample_rate);
        }
        self.snap_to_params();
    }

    fn reset(&mut self) {
        self.snap_to_params();
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count();
        if frames == 0 {
            return;
        }

        self.polarity.set_target(if self.params.flag(P_INVERT) {
            -1.0
        } else {
            1.0
        });
        self.width.set_target(self.params.at(P_WIDTH));
        self.gain.set_target(db_to_gain(self.params.at(P_GAIN_DB)));
        let (left_gain, right_gain) = pan_gains(self.params.at(P_PAN));
        self.left.set_target(left_gain * CENTRE_COMPENSATION);
        self.right.set_target(right_gain * CENTRE_COMPENSATION);

        if buffer.channel_count() < 2 {
            let mut gain = self.gain;
            let mut polarity = self.polarity;
            for sample in buffer.channel_mut(0)[..frames].iter_mut() {
                *sample *= gain.next_value() * polarity.next_value();
            }
            self.gain = gain;
            self.polarity = polarity;
            return;
        }

        // Every channel must see the same gain curve, so the ramp state is captured before the
        // stereo pair and replayed for any channel beyond it.
        let gain_at_block_start = self.gain;
        let polarity_at_block_start = self.polarity;
        {
            let (head, tail) = buffer.channels_mut().split_at_mut(1);
            let left_channel = &mut head[0][..frames];
            let right_channel = &mut tail[0][..frames];
            for (left, right) in left_channel.iter_mut().zip(right_channel.iter_mut()) {
                let gain = self.gain.next_value();
                let left_pan = self.left.next_value();
                let right_pan = self.right.next_value();
                let polarity = self.polarity.next_value();
                let width = self.width.next_value();

                // Mid/side: width 0 collapses to mono, 1 is untouched, 2 doubles the sides.
                let left_in = *left * polarity;
                let right_in = *right * polarity;
                let mid = (left_in + right_in) * 0.5;
                let side = (right_in - left_in) * 0.5 * width;
                *left = (mid - side) * gain * left_pan;
                *right = (mid + side) * gain * right_pan;
            }
        }

        // Surround channels keep the level change but neither pan nor width apply to them.
        for channel in buffer.channels_mut().iter_mut().skip(2) {
            let mut gain = gain_at_block_start;
            let mut polarity = polarity_at_block_start;
            for sample in channel[..frames].iter_mut() {
                *sample *= gain.next_value() * polarity.next_value();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::param::gain_to_db;

    fn prepared() -> GainPan {
        let mut plugin = GainPan::new();
        plugin.prepare(&PrepareContext::new(48_000.0, 512, 2));
        plugin
    }

    fn context(frames: usize) -> ProcessContext {
        ProcessContext::realtime(48_000.0, frames, 0, 120.0, true)
    }

    fn tone(frames: usize) -> AudioBuffer {
        let step = std::f32::consts::TAU * 1_000.0 / 48_000.0;
        let samples: Vec<f32> = (0..frames).map(|n| (step * n as f32).sin()).collect();
        AudioBuffer::from_planar(vec![samples.clone(), samples], 48_000.0).unwrap()
    }

    #[test]
    fn defaults_are_bit_transparent() {
        let mut plugin = prepared();
        let input = tone(256);
        let mut buffer = input.clone();
        plugin.process(&mut buffer, &context(256));
        for channel in 0..2 {
            for (out, expected) in buffer.channel(channel).iter().zip(input.channel(channel)) {
                assert!((out - expected).abs() < 1e-6, "{out} != {expected}");
            }
        }
    }

    #[test]
    fn minus_six_db_halves_the_amplitude() {
        let mut plugin = prepared();
        plugin.set_param_by_key("gain_db", -6.0206);
        // The ramp is 20 ms; give it a full second to settle before measuring.
        let mut buffer = tone(48_000);
        plugin.process(&mut buffer, &context(48_000));
        let tail = buffer.slice(24_000, 24_000);
        assert!(
            (gain_to_db(tail.peak()) + 6.0206).abs() < 0.05,
            "peak was {} dB",
            gain_to_db(tail.peak())
        );
    }

    #[test]
    fn hard_left_silences_the_right_channel() {
        let mut plugin = prepared();
        plugin.set_param_by_key("pan", -1.0);
        let mut buffer = tone(48_000);
        plugin.process(&mut buffer, &context(48_000));
        let tail = buffer.slice(24_000, 24_000);
        assert!(
            tail.channel_peak(1) < 1e-5,
            "right was {}",
            tail.channel_peak(1)
        );
        // Constant power normalised to unity at centre puts +3 dB on the remaining side.
        assert!(
            (gain_to_db(tail.channel_peak(0)) - 3.0103).abs() < 0.05,
            "left was {} dB",
            gain_to_db(tail.channel_peak(0))
        );
    }

    #[test]
    fn zero_width_collapses_to_mono() {
        let mut plugin = prepared();
        plugin.set_param_by_key("width", 0.0);
        let left = vec![-1.0; 48_000];
        let right = vec![1.0; 48_000];
        let mut buffer = AudioBuffer::from_planar(vec![left, right], 48_000.0).unwrap();
        plugin.process(&mut buffer, &context(48_000));
        assert!(
            buffer.channel(0)[0].abs() > 0.9,
            "the width starts a ramp instead of stepping to zero"
        );
        // Perfectly anti-correlated input is pure side, so width 0 leaves nothing.
        assert!(
            buffer.channel(0)[47_999].abs() < 1e-6,
            "tail was {}",
            buffer.channel(0)[47_999]
        );
    }

    #[test]
    fn phase_invert_flips_the_sign() {
        let mut plugin = prepared();
        plugin.set_param_by_key("invert", 1.0);
        let input = vec![1.0; 48_000];
        let mut buffer = AudioBuffer::from_planar(vec![input.clone(), input], 48_000.0).unwrap();
        plugin.process(&mut buffer, &context(48_000));
        assert!(
            buffer.channel(0)[0] > 0.9,
            "polarity starts a ramp instead of stepping across zero"
        );
        assert!((buffer.channel(0)[47_999] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn silence_stays_silent() {
        let mut plugin = prepared();
        plugin.set_param_by_key("gain_db", 12.0);
        let mut buffer = AudioBuffer::stereo(128, 48_000.0);
        plugin.process(&mut buffer, &context(128));
        assert_eq!(buffer.peak(), 0.0);
    }
}
