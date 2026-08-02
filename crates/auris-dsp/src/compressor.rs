//! `auris.fx.compressor` — a soft-knee feed-forward compressor.

use auris_core::AudioBuffer;
use auris_core::param::{
    ParamDescriptor, ParamId, ParamUnit, ParamValueCurve, db_to_gain, gain_to_db,
};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::MILLISECONDS_PER_SECOND;
use crate::bank::ParamBank;
use crate::smooth::one_pole_coefficient;

const P_THRESHOLD_DB: u32 = 0;
const P_RATIO: u32 = 1;
const P_ATTACK_MS: u32 = 2;
const P_RELEASE_MS: u32 = 3;
const P_KNEE_DB: u32 = 4;
const P_MAKEUP_DB: u32 = 5;

/// A feed-forward compressor with a quadratic soft knee.
///
/// Detection runs on the largest absolute sample across **all** channels, so the same gain is
/// applied everywhere and a loud left channel cannot pull the stereo image sideways.
pub struct Compressor {
    params: ParamBank,
    sample_rate: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    /// Current gain in dB, always at or below zero.
    gain_db: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    /// A new compressor at its default settings.
    pub fn new() -> Self {
        let descriptors = vec![
            ParamDescriptor::decibels(
                P_THRESHOLD_DB,
                "threshold_db",
                "Threshold",
                -60.0,
                0.0,
                -18.0,
            ),
            // A power curve gives the musically interesting 1:1..4:1 region most of the knob.
            ParamDescriptor::new(P_RATIO, "ratio", "Ratio", 1.0, 20.0, 4.0)
                .with_unit(ParamUnit::Ratio)
                .with_curve(ParamValueCurve::Power(2.0)),
            ParamDescriptor::new(P_ATTACK_MS, "attack_ms", "Attack", 0.1, 100.0, 10.0)
                .with_unit(ParamUnit::Milliseconds)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::new(P_RELEASE_MS, "release_ms", "Release", 5.0, 2_000.0, 120.0)
                .with_unit(ParamUnit::Milliseconds)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::decibels(P_KNEE_DB, "knee_db", "Knee", 0.0, 24.0, 6.0),
            ParamDescriptor::decibels(P_MAKEUP_DB, "makeup_db", "Makeup", -12.0, 24.0, 0.0),
        ];
        let mut plugin = Self {
            params: ParamBank::new(descriptors),
            sample_rate: 48_000.0,
            attack_coefficient: 0.0,
            release_coefficient: 0.0,
            gain_db: 0.0,
        };
        plugin.recompute_coefficients();
        plugin
    }

    /// Gain the compressor is currently applying, in dB.
    ///
    /// Zero when it is not working, negative while it reduces: `-6.0` means the signal is being
    /// pulled down by 6 dB. Makeup gain is deliberately excluded so the value reads as a
    /// gain-reduction meter.
    pub fn gain_reduction_db(&self) -> f32 {
        self.gain_db
    }

    fn recompute_coefficients(&mut self) {
        self.attack_coefficient = one_pole_coefficient(
            self.params.at(P_ATTACK_MS) / MILLISECONDS_PER_SECOND,
            self.sample_rate,
        );
        self.release_coefficient = one_pole_coefficient(
            self.params.at(P_RELEASE_MS) / MILLISECONDS_PER_SECOND,
            self.sample_rate,
        );
    }

    /// Static gain curve: the dB change applied to a signal sitting `over` dB above threshold.
    ///
    /// Above the knee this is the textbook `over * (1/ratio - 1)`. Inside the knee it is the
    /// quadratic that matches both the value and the slope at each end, which is the standard
    /// interpolation from Giannoulis, Massberg & Reiss, *Digital Dynamic Range Compressor
    /// Design* (JAES 2012).
    #[inline]
    fn static_gain_db(over: f32, slope: f32, knee_db: f32) -> f32 {
        let half_knee = knee_db * 0.5;
        if over <= -half_knee {
            0.0
        } else if over >= half_knee {
            slope * over
        } else {
            let x = over + half_knee;
            slope * x * x / (2.0 * knee_db)
        }
    }
}

impl Parameterized for Compressor {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if self.params.set(id, value) && (id.raw() == P_ATTACK_MS || id.raw() == P_RELEASE_MS) {
            self.recompute_coefficients();
        }
    }
}

impl Effect for Compressor {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            "auris.fx.compressor",
            "Compressor",
            "Soft-knee compressor with stereo-linked peak detection",
            PluginCategory::Dynamics,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate as f32;
        self.recompute_coefficients();
        self.reset();
    }

    fn reset(&mut self) {
        self.gain_db = 0.0;
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count();
        if frames == 0 {
            return;
        }

        let threshold_db = self.params.at(P_THRESHOLD_DB);
        let ratio = self.params.at(P_RATIO).max(1.0);
        let knee_db = self.params.at(P_KNEE_DB);
        let makeup_db = self.params.at(P_MAKEUP_DB);
        // Negative: how much of the excess above threshold is removed.
        let slope = 1.0 / ratio - 1.0;

        for frame in 0..frames {
            let mut level = 0.0f32;
            for channel in buffer.channels() {
                level = level.max(channel[frame].abs());
            }

            let over = gain_to_db(level) - threshold_db;
            let target_db = if knee_db > 0.0 {
                Self::static_gain_db(over, slope, knee_db)
            } else if over > 0.0 {
                slope * over
            } else {
                0.0
            };

            // Moving further down is an attack, coming back up is a release.
            let coefficient = if target_db < self.gain_db {
                self.attack_coefficient
            } else {
                self.release_coefficient
            };
            self.gain_db = target_db + (self.gain_db - target_db) * coefficient;

            let gain = db_to_gain(self.gain_db + makeup_db);
            for channel in buffer.channels_mut() {
                channel[frame] *= gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn prepared() -> Compressor {
        let mut plugin = Compressor::new();
        plugin.prepare(&PrepareContext::new(SR, 4_096, 2));
        plugin
    }

    fn context(frames: usize) -> ProcessContext {
        ProcessContext::realtime(SR, frames, 0, 120.0, true)
    }

    /// A constant-amplitude block: `|x|` never varies, so the detector converges exactly.
    fn constant(frames: usize, amplitude: f32) -> AudioBuffer {
        AudioBuffer::from_planar(vec![vec![amplitude; frames]; 2], SR).unwrap()
    }

    #[test]
    fn four_to_one_above_threshold_hits_the_arithmetic_answer() {
        let mut plugin = prepared();
        plugin.set_param_by_key("threshold_db", -20.0);
        plugin.set_param_by_key("ratio", 4.0);
        plugin.set_param_by_key("knee_db", 0.0);
        plugin.set_param_by_key("attack_ms", 1.0);
        plugin.set_param_by_key("release_ms", 200.0);
        plugin.set_param_by_key("makeup_db", 0.0);

        // -8 dBFS is 12 dB over the threshold; at 4:1 only 3 dB of that survives, so the
        // output must settle at -20 + 3 = -17 dBFS, i.e. 9 dB of reduction.
        let frames = 24_000;
        let mut buffer = constant(frames, db_to_gain(-8.0));
        plugin.process(&mut buffer, &context(frames));

        let settled = buffer.slice(frames - 1_000, 1_000);
        let out_db = gain_to_db(settled.peak());
        assert!(
            (out_db + 17.0).abs() < 0.05,
            "output settled at {out_db} dB"
        );
        assert!(
            (plugin.gain_reduction_db() + 9.0).abs() < 0.05,
            "meter read {} dB",
            plugin.gain_reduction_db()
        );
    }

    #[test]
    fn makeup_gain_adds_on_top_of_the_reduction() {
        let mut plugin = prepared();
        plugin.set_param_by_key("threshold_db", -20.0);
        plugin.set_param_by_key("ratio", 4.0);
        plugin.set_param_by_key("knee_db", 0.0);
        plugin.set_param_by_key("attack_ms", 1.0);
        plugin.set_param_by_key("makeup_db", 6.0);

        let frames = 24_000;
        let mut buffer = constant(frames, db_to_gain(-8.0));
        plugin.process(&mut buffer, &context(frames));
        let out_db = gain_to_db(buffer.slice(frames - 1_000, 1_000).peak());
        assert!(
            (out_db + 11.0).abs() < 0.05,
            "output settled at {out_db} dB"
        );
    }

    #[test]
    fn signal_below_threshold_passes_untouched() {
        let mut plugin = prepared();
        plugin.set_param_by_key("threshold_db", -20.0);
        plugin.set_param_by_key("knee_db", 0.0);
        let frames = 4_800;
        let input = db_to_gain(-30.0);
        let mut buffer = constant(frames, input);
        plugin.process(&mut buffer, &context(frames));
        assert!((buffer.peak() - input).abs() < 1e-6);
        assert_eq!(plugin.gain_reduction_db(), 0.0);
    }

    #[test]
    fn the_soft_knee_starts_working_below_the_threshold() {
        let mut plugin = prepared();
        plugin.set_param_by_key("threshold_db", -20.0);
        plugin.set_param_by_key("ratio", 4.0);
        plugin.set_param_by_key("knee_db", 12.0);
        plugin.set_param_by_key("attack_ms", 1.0);

        // 3 dB below threshold is inside a 12 dB knee, so a little reduction is expected.
        let frames = 24_000;
        let mut buffer = constant(frames, db_to_gain(-23.0));
        plugin.process(&mut buffer, &context(frames));
        let reduction = plugin.gain_reduction_db();
        // Knee curve at over = -3, slope = -0.75: -0.75 * 3^2 / 24 = -0.28 dB.
        assert!(
            (reduction + 0.28).abs() < 0.05,
            "knee produced {reduction} dB"
        );
    }

    #[test]
    fn detection_is_linked_so_the_image_does_not_shift() {
        let mut plugin = prepared();
        plugin.set_param_by_key("threshold_db", -20.0);
        plugin.set_param_by_key("ratio", 4.0);
        plugin.set_param_by_key("knee_db", 0.0);
        plugin.set_param_by_key("attack_ms", 1.0);

        let frames = 24_000;
        let loud = db_to_gain(-8.0);
        let quiet = db_to_gain(-40.0);
        let mut buffer =
            AudioBuffer::from_planar(vec![vec![loud; frames], vec![quiet; frames]], SR).unwrap();
        plugin.process(&mut buffer, &context(frames));

        let tail = buffer.slice(frames - 1_000, 1_000);
        // Both channels must be scaled by the same 9 dB, preserving their 32 dB difference.
        let difference = gain_to_db(tail.channel_peak(0)) - gain_to_db(tail.channel_peak(1));
        assert!(
            (difference - 32.0).abs() < 0.05,
            "difference {difference} dB"
        );
        assert!((gain_to_db(tail.channel_peak(1)) + 49.0).abs() < 0.05);
    }

    #[test]
    fn silence_stays_silent_and_output_is_finite() {
        let mut plugin = prepared();
        plugin.set_param_by_key("makeup_db", 24.0);
        let mut buffer = AudioBuffer::stereo(512, SR);
        plugin.process(&mut buffer, &context(512));
        assert_eq!(buffer.peak(), 0.0);
    }
}
