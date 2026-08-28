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
use crate::smooth::{SmoothedValue, one_pole_coefficient};

const P_THRESHOLD_DB: u32 = 0;
const P_RATIO: u32 = 1;
const P_ATTACK_MS: u32 = 2;
const P_RELEASE_MS: u32 = 3;
const P_KNEE_DB: u32 = 4;
const P_MAKEUP_DB: u32 = 5;
const P_MIX: u32 = 6;

/// Ramp time for the dry/wet balance.
const MIX_SMOOTHING_SECONDS: f32 = 0.020;

/// A feed-forward compressor with a quadratic soft knee.
///
/// Detection runs on the largest absolute sample across **all** channels, so the same gain is
/// applied everywhere and a loud left channel cannot pull the stereo image sideways.
///
/// It reads a sidechain where the slot names one, and then that is what it listens to: the audio
/// passing through is turned down by what the *key* is doing. A bass keyed from the kick drum is
/// the ordinary use, and the reason the two signals have to be kept apart — the bass never gets
/// quieter for being loud, only for the kick being loud.
///
/// The mix knob blends the compressed signal against the dry one — parallel compression without
/// a second bus. Makeup gain sits on the wet side of the blend, so at half mix the untouched
/// transients ride over a floor that has been squeezed *and* brought up.
pub struct Compressor {
    params: ParamBank,
    sample_rate: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    /// Current gain in dB, always at or below zero.
    gain_db: f32,
    mix: SmoothedValue,
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
            // Fully wet by default: a compressor is expected to compress, and every project
            // saved before the knob existed loads to exactly the sound it had.
            ParamDescriptor::percent(P_MIX, "mix", "Mix", 1.0),
        ];
        let sample_rate = 48_000.0;
        let mut plugin = Self {
            params: ParamBank::new(descriptors),
            sample_rate,
            attack_coefficient: 0.0,
            release_coefficient: 0.0,
            gain_db: 0.0,
            mix: SmoothedValue::new(1.0, MIX_SMOOTHING_SECONDS, sample_rate),
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
            "Soft-knee compressor, stereo-linked and keyable from another track",
            PluginCategory::Dynamics,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate as f32;
        self.recompute_coefficients();
        self.mix.set_time(MIX_SMOOTHING_SECONDS, self.sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.gain_db = 0.0;
        self.mix.snap_to(self.params.at(P_MIX));
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        self.run(buffer, None);
    }

    fn wants_sidechain(&self) -> bool {
        true
    }

    fn process_with_sidechain(
        &mut self,
        buffer: &mut AudioBuffer,
        sidechain: &AudioBuffer,
        _ctx: &ProcessContext,
    ) {
        self.run(buffer, Some(sidechain));
    }
}

impl Compressor {
    /// One block, detecting from `key` where there is one and from the audio itself where there
    /// is not.
    ///
    /// The two are the same loop on purpose. A keyed compressor differs from an ordinary one in
    /// exactly one line — where the level comes from — and everything after it, the knee, the two
    /// time constants and the makeup, has to stay identical or a keyed compressor is a second
    /// compressor that happens to share a name.
    fn run(&mut self, buffer: &mut AudioBuffer, key: Option<&AudioBuffer>) {
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
        self.mix.set_target(self.params.at(P_MIX));

        for frame in 0..frames {
            // The immutable borrow ends on this line, which is what lets the unkeyed case read
            // the same buffer it is about to write to.
            let level = match key {
                Some(key) => peak_at(key, frame),
                None => peak_at(buffer, frame),
            };

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
            // Settled because the gain recirculates: `peak_at` keeps the level finite, but a
            // NaN that found any other way in would otherwise sit in this line for ever.
            self.gain_db = crate::settled(target_db + (self.gain_db - target_db) * coefficient);

            let gain = db_to_gain(self.gain_db + makeup_db);
            // One scalar per frame: `dry * (1 - mix) + dry * gain * mix`, factored so the dry
            // path multiplies by exactly 1.0 when the mix sits at zero.
            let blend = 1.0 + self.mix.next_value() * (gain - 1.0);
            for channel in buffer.channels_mut() {
                channel[frame] *= blend;
            }
        }
    }
}

/// The largest absolute sample across every channel at one frame, and zero past the end.
///
/// Past the end is the key running short, which the engine does not do — but a detector that
/// panicked on it would take the audio thread with it, and silence is the answer that leaves the
/// compressor open rather than closed.
fn peak_at(buffer: &AudioBuffer, frame: usize) -> f32 {
    buffer
        .channels()
        .iter()
        .filter_map(|channel| channel.get(frame))
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
        // An infinite sample reads as "as loud as a number gets" rather than poisoning the dB
        // arithmetic: `gain_to_db(f32::MAX)` is about 770 dB and finite, and the compressor
        // answers it by closing, which is the right answer to an infinitely loud input. A NaN
        // never survives the fold at all — `max` keeps the other operand.
        .min(f32::MAX)
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

    /// The settings the keyed tests share: no knee and a quick attack, so the answer is the
    /// static gain curve and nothing else.
    fn keyed() -> Compressor {
        let mut plugin = prepared();
        plugin.set_param_by_key("threshold_db", -20.0);
        plugin.set_param_by_key("ratio", 4.0);
        plugin.set_param_by_key("knee_db", 0.0);
        plugin.set_param_by_key("attack_ms", 1.0);
        plugin.set_param_by_key("release_ms", 200.0);
        plugin.set_param_by_key("makeup_db", 0.0);
        plugin
    }

    #[test]
    fn a_key_turns_the_audio_down_by_what_the_key_is_doing() {
        let mut plugin = keyed();
        // The audio sits 10 dB under the threshold and would be left alone on its own. The key
        // is 12 dB over it, which at 4:1 is 9 dB of reduction — so the output settles 9 dB below
        // where it came in, at -39 dBFS.
        let frames = 24_000;
        let mut buffer = constant(frames, db_to_gain(-30.0));
        let key = constant(frames, db_to_gain(-8.0));
        plugin.process_with_sidechain(&mut buffer, &key, &context(frames));

        let out_db = gain_to_db(buffer.slice(frames - 1_000, 1_000).peak());
        assert!(
            (out_db + 39.0).abs() < 0.05,
            "output settled at {out_db} dB"
        );
        assert!(
            (plugin.gain_reduction_db() + 9.0).abs() < 0.05,
            "meter read {} dB",
            plugin.gain_reduction_db()
        );
    }

    #[test]
    fn loud_audio_under_a_silent_key_is_left_alone() {
        // The other half of the same claim: what the compressor hears is the key, so audio well
        // over the threshold passes untouched while nothing is keying it.
        let mut plugin = keyed();
        let frames = 24_000;
        let input = db_to_gain(-2.0);
        let mut buffer = constant(frames, input);
        let key = constant(frames, 0.0);
        plugin.process_with_sidechain(&mut buffer, &key, &context(frames));

        assert!((buffer.peak() - input).abs() < 1e-6);
        assert_eq!(plugin.gain_reduction_db(), 0.0);
    }

    #[test]
    fn keying_a_compressor_from_its_own_input_is_the_unkeyed_compressor() {
        // The two paths are one loop with one line different, and this is what says so: handed
        // its own audio as the key, a keyed compressor has to produce the plain one's output to
        // the sample.
        let frames = 8_192;
        let ctx = context(frames);
        let signal: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.031).sin() * db_to_gain(-6.0))
            .collect();
        let buffer = AudioBuffer::from_planar(vec![signal.clone(), signal], SR).unwrap();

        let mut plain = keyed();
        let mut alone = buffer.clone();
        plain.process(&mut alone, &ctx);

        let mut keyed_by_itself = keyed();
        let mut through = buffer.clone();
        keyed_by_itself.process_with_sidechain(&mut through, &buffer, &ctx);

        for channel in 0..alone.channel_count() {
            assert_eq!(alone.channel(channel), through.channel(channel));
        }
    }

    #[test]
    fn a_key_that_runs_short_leaves_the_compressor_open() {
        // The engine hands over a key the length of the block; a shorter one is a fault, and the
        // answer to it has to be silence rather than a panic on the audio thread.
        let mut plugin = keyed();
        let frames = 4_800;
        let mut buffer = constant(frames, db_to_gain(-2.0));
        let key = constant(frames / 2, 1.0);
        plugin.process_with_sidechain(&mut buffer, &key, &context(frames));
        assert!(
            plugin.gain_reduction_db() < 0.0,
            "the frames it had still key"
        );
        assert!(buffer.peak().is_finite());
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
    fn half_mix_blends_the_two_paths_in_linear_gain() {
        let mut plugin = keyed();
        plugin.set_param_by_key("mix", 0.5);

        // Fully wet this input settles 9 dB down (the 4:1 arithmetic above). Half mix averages
        // the linear gains: (1 + 10^(-9/20)) / 2 = 0.6774, which is -3.38 dB — quieter than
        // dry, far from halfway down in decibels, because parallel blending is linear.
        let frames = 24_000;
        let mut buffer = constant(frames, db_to_gain(-8.0));
        plugin.process(&mut buffer, &context(frames));
        let out_db = gain_to_db(buffer.slice(frames - 1_000, 1_000).peak());
        assert!(
            (out_db - (-8.0 - 3.384)).abs() < 0.05,
            "output settled at {out_db} dB"
        );
        // The meter still reads the wet path's reduction; the blend is after it.
        assert!(
            (plugin.gain_reduction_db() + 9.0).abs() < 0.05,
            "meter read {} dB",
            plugin.gain_reduction_db()
        );
    }

    #[test]
    fn mix_zero_is_bit_transparent() {
        let mut plugin = keyed();
        plugin.set_param_by_key("mix", 0.0);
        plugin.set_param_by_key("makeup_db", 12.0);
        // The knob was turned before the transport rolled; without this the first 20 ms are
        // the ramp from the default, which is the glide a *live* turn is supposed to get.
        plugin.reset();

        let frames = 4_096;
        let signal: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.031).sin() * db_to_gain(-3.0))
            .collect();
        let input = AudioBuffer::from_planar(vec![signal.clone(), signal], SR).unwrap();
        let mut buffer = input.clone();
        plugin.process(&mut buffer, &context(frames));
        for channel in 0..2 {
            assert_eq!(buffer.channel(channel), input.channel(channel));
        }
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

    #[test]
    fn an_infinite_sample_closes_the_compressor_instead_of_breaking_it() {
        // `gain_to_db(inf)` is infinite, and infinity arithmetic lands the smoothed gain on
        // NaN — where it used to stay for ever, muting every later block. Read as "as loud as
        // a number gets", the burst just slams the gain down, and the release brings it back.
        let mut plugin = keyed();
        let frames = 24_000;

        let mut burst = constant(frames, 0.1);
        burst.channel_mut(0)[100] = f32::INFINITY;
        plugin.process(&mut burst, &context(frames));

        // -30 dB sits 10 dB under the threshold: once the release has run, it has to come
        // through untouched rather than through a latched NaN gain.
        let mut buffer = constant(frames, db_to_gain(-30.0));
        plugin.process(&mut buffer, &context(frames));
        assert!(buffer.channel(0).iter().all(|s| s.is_finite()));
        let out_db = gain_to_db(buffer.slice(frames - 1_000, 1_000).peak());
        assert!((out_db + 30.0).abs() < 0.1, "output settled at {out_db} dB");
    }
}
