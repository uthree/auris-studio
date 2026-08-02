//! `auris.fx.distortion` — four waveshapers behind one drive control.

use std::borrow::Cow;

use auris_core::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId, db_to_gain};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::bank::ParamBank;

const P_DRIVE_DB: u32 = 0;
const P_MODE: u32 = 1;
const P_BITS: u32 = 2;
const P_OUTPUT_DB: u32 = 3;
const P_MIX: u32 = 4;

/// Labels for the mode parameter, in value order.
static MODE_CHOICES: [Cow<'static, str>; 4] = [
    Cow::Borrowed("Soft (tanh)"),
    Cow::Borrowed("Hard clip"),
    Cow::Borrowed("Fold"),
    Cow::Borrowed("Bitcrush"),
];

/// Which waveshaper the distortion runs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DistortionMode {
    /// `tanh`: a smooth saturation that adds mostly odd harmonics.
    Soft,
    /// Clamp to +/-1: abrupt, bright, harmonically rich.
    HardClip,
    /// Reflect back into +/-1 instead of clamping, folding the waveform on itself.
    Fold,
    /// Quantise to a chosen bit depth. The chiptune option.
    Bitcrush,
}

impl DistortionMode {
    /// Maps the parameter's numeric value onto a mode, falling back to [`Self::Soft`].
    pub fn from_value(value: f32) -> Self {
        match value.round() as i32 {
            1 => DistortionMode::HardClip,
            2 => DistortionMode::Fold,
            3 => DistortionMode::Bitcrush,
            _ => DistortionMode::Soft,
        }
    }

    /// Shapes one driven sample. `levels` is only read by [`Self::Bitcrush`].
    #[inline]
    pub fn shape(self, input: f32, levels: f32) -> f32 {
        match self {
            DistortionMode::Soft => input.tanh(),
            DistortionMode::HardClip => input.clamp(-1.0, 1.0),
            DistortionMode::Fold => fold(input),
            DistortionMode::Bitcrush => {
                let clamped = input.clamp(-1.0, 1.0);
                (clamped * levels).round() / levels
            }
        }
    }
}

/// Triangle wave-folder: identity on `[-1, 1]`, then mirrored with period 4.
///
/// `4 * |t/4 - round(t/4)| - 1` is the unit triangle wave; feeding the shifted input through it
/// gives `f(0) = 0`, `f(1) = 1`, `f(2) = 0`, `f(3) = -1` — a fold rather than a clip, which is
/// what produces the wave-folder's characteristic inharmonic spray.
#[inline]
fn fold(input: f32) -> f32 {
    if !input.is_finite() {
        return 0.0;
    }
    let phase = (input + 1.0) * 0.25;
    4.0 * (phase - (phase + 0.5).floor()).abs() - 1.0
}

/// Drive, waveshape, output trim and dry/wet mix.
pub struct Distortion {
    params: ParamBank,
}

impl Default for Distortion {
    fn default() -> Self {
        Self::new()
    }
}

impl Distortion {
    /// A new distortion at its default settings.
    pub fn new() -> Self {
        let descriptors = vec![
            ParamDescriptor::decibels(P_DRIVE_DB, "drive_db", "Drive", 0.0, 48.0, 12.0),
            ParamDescriptor::new(P_MODE, "mode", "Mode", 0.0, 3.0, 0.0).with_choices(&MODE_CHOICES),
            ParamDescriptor::new(P_BITS, "bits", "Bit Depth", 1.0, 16.0, 8.0).with_steps(16),
            ParamDescriptor::decibels(P_OUTPUT_DB, "output_db", "Output", -24.0, 12.0, 0.0),
            ParamDescriptor::percent(P_MIX, "mix", "Mix", 1.0),
        ];
        Self {
            params: ParamBank::new(descriptors),
        }
    }

    /// The waveshaper currently selected.
    pub fn mode(&self) -> DistortionMode {
        DistortionMode::from_value(self.params.at(P_MODE))
    }
}

impl Parameterized for Distortion {
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

impl Effect for Distortion {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            "auris.fx.distortion",
            "Distortion",
            "Saturation, hard clipping, wave folding and bitcrushing",
            PluginCategory::Distortion,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count();
        if frames == 0 {
            return;
        }

        let drive = db_to_gain(self.params.at(P_DRIVE_DB));
        let output = db_to_gain(self.params.at(P_OUTPUT_DB));
        let mix = self.params.at(P_MIX);
        let mode = self.mode();
        // A signed n-bit sample has 2^(n-1) steps per side of zero.
        let levels = (1u32 << (self.params.at(P_BITS).round().clamp(1.0, 16.0) as u32 - 1)) as f32;

        for channel in buffer.channels_mut() {
            for sample in channel[..frames].iter_mut() {
                let wet = mode.shape(*sample * drive, levels) * output;
                *sample = *sample * (1.0 - mix) + wet * mix;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn prepared() -> Distortion {
        let mut plugin = Distortion::new();
        plugin.prepare(&PrepareContext::new(SR, 512, 2));
        plugin
    }

    fn context(frames: usize) -> ProcessContext {
        ProcessContext::realtime(SR, frames, 0, 120.0, true)
    }

    fn sine(frames: usize, amplitude: f32) -> AudioBuffer {
        let step = std::f32::consts::TAU * 220.0 / SR as f32;
        let samples: Vec<f32> = (0..frames)
            .map(|n| (step * n as f32).sin() * amplitude)
            .collect();
        AudioBuffer::from_planar(vec![samples.clone(), samples], SR).unwrap()
    }

    #[test]
    fn the_folder_is_a_triangle_through_the_expected_points() {
        for (input, expected) in [
            (0.0f32, 0.0f32),
            (0.5, 0.5),
            (1.0, 1.0),
            (1.5, 0.5),
            (2.0, 0.0),
            (3.0, -1.0),
            (5.0, 1.0),
            (-1.0, -1.0),
            (-2.0, 0.0),
        ] {
            assert!(
                (fold(input) - expected).abs() < 1e-6,
                "fold({input}) was {}, expected {expected}",
                fold(input)
            );
        }
    }

    #[test]
    fn hard_clip_stops_exactly_at_full_scale() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mode", 1.0);
        plugin.set_param_by_key("drive_db", 12.0);
        let mut buffer = sine(2_048, 0.5);
        plugin.process(&mut buffer, &context(2_048));
        // 0.5 driven by 12 dB is about 1.99, so the tops are flat at 1.0.
        assert!(
            (buffer.peak() - 1.0).abs() < 1e-6,
            "peak was {}",
            buffer.peak()
        );
    }

    #[test]
    fn soft_saturation_never_reaches_full_scale() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mode", 0.0);
        plugin.set_param_by_key("drive_db", 12.0);
        let mut buffer = sine(2_048, 0.5);
        plugin.process(&mut buffer, &context(2_048));
        // tanh(0.5 * 10^(12/20)) = tanh(1.9905) = 0.9631.
        let expected = (0.5 * db_to_gain(12.0)).tanh();
        assert!(
            (buffer.peak() - expected).abs() < 1e-3,
            "peak {} vs expected {expected}",
            buffer.peak()
        );
        assert!(buffer.peak() < 1.0);
    }

    #[test]
    fn bitcrush_quantises_to_the_requested_step_size() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mode", 3.0);
        plugin.set_param_by_key("drive_db", 0.0);
        plugin.set_param_by_key("bits", 4.0);
        let mut buffer = sine(2_048, 1.0);
        plugin.process(&mut buffer, &context(2_048));

        // 4 bits gives 8 steps per side, so every output must be a multiple of 1/8.
        let step = 1.0 / 8.0;
        let mut distinct = std::collections::BTreeSet::new();
        for sample in buffer.channel(0) {
            let quantised = (sample / step).round();
            assert!(
                (sample - quantised * step).abs() < 1e-6,
                "{sample} is not on the 1/8 grid"
            );
            distinct.insert(quantised as i32);
        }
        assert!(
            distinct.iter().all(|level| (-8..=8).contains(level)),
            "levels outside +/-8: {distinct:?}"
        );
        assert!(distinct.len() >= 15, "only {} levels used", distinct.len());
    }

    #[test]
    fn output_trim_scales_the_wet_signal() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mode", 1.0);
        plugin.set_param_by_key("drive_db", 24.0);
        plugin.set_param_by_key("output_db", -6.0206);
        let mut buffer = sine(2_048, 0.5);
        plugin.process(&mut buffer, &context(2_048));
        assert!(
            (buffer.peak() - 0.5).abs() < 1e-3,
            "peak was {}",
            buffer.peak()
        );
    }

    #[test]
    fn a_dry_mix_is_a_bit_exact_bypass() {
        let mut plugin = prepared();
        plugin.set_param_by_key("mix", 0.0);
        plugin.set_param_by_key("drive_db", 48.0);
        let input = sine(512, 0.5);
        let mut buffer = input.clone();
        plugin.process(&mut buffer, &context(512));
        assert_eq!(buffer.channel(0), input.channel(0));
    }

    #[test]
    fn silence_stays_silent_in_every_mode() {
        for mode in 0..4 {
            let mut plugin = prepared();
            plugin.set_param_by_key("mode", mode as f32);
            plugin.set_param_by_key("drive_db", 48.0);
            let mut buffer = AudioBuffer::stereo(256, SR);
            plugin.process(&mut buffer, &context(256));
            assert_eq!(buffer.peak(), 0.0, "mode {mode} broke silence");
        }
    }
}
