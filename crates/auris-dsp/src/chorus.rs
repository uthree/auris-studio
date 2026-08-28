//! `auris.fx.chorus` — a modulated delay with a quadrature stereo spread.

use auris_core::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{
    Effect, Parameterized, PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};

use crate::MILLISECONDS_PER_SECOND;
use crate::bank::ParamBank;
use crate::delay_line::DelayLine;
use crate::smooth::SmoothedValue;

const P_RATE_HZ: u32 = 0;
const P_DEPTH_MS: u32 = 1;
const P_MIX: u32 = 2;

/// Centre of the modulated tap, in milliseconds.
///
/// Far enough out that the wet tap reads a distinctly *earlier* copy of the signal — much
/// under 10 ms and the effect collapses toward a flanger's comb — and near enough in that the
/// doubling stays fused with the dry voice instead of separating into a slapback.
const BASE_DELAY_MS: f32 = 15.0;

/// Furthest the tap may swing from the centre, in milliseconds.
///
/// Kept below `BASE_DELAY_MS` so the tap can never reach the write head, whatever the
/// parameters say.
const MAX_DEPTH_MS: f32 = 8.0;

/// Ramp time for depth changes. The tap position is what depth moves, so a step here is a
/// click in the pitch of the wet voice, exactly as it is for the delay's time knob.
const DEPTH_SMOOTHING_SECONDS: f32 = 0.050;
/// Ramp time for the dry/wet balance.
const MIX_SMOOTHING_SECONDS: f32 = 0.020;

/// Phase lead of each channel over the one before it, in radians.
///
/// A quarter turn: when the left tap is at the centre of its swing the right tap is at an
/// extreme, so the two sides never rise and fall in step and the image widens without either
/// side simply inverting the other — a mono sum keeps both voices.
const CHANNEL_PHASE_OFFSET: f32 = std::f32::consts::FRAC_PI_2;

/// A single modulated tap per channel, swept by one shared sine.
///
/// The wet voice is the input read `BASE_DELAY_MS` ago, with the read position swung by up
/// to `depth` milliseconds either side at `rate`. Moving the read head is a slow resampling,
/// so the wet voice is detuned by a few cents that rise and fall with the sweep — beat against
/// the dry copy, that detune is the chorus. Every channel shares the one oscillator and takes
/// its own phase offset, which is what keeps the left and right sweeps locked but never equal.
pub struct Chorus {
    params: ParamBank,
    sample_rate: f32,
    lines: Vec<DelayLine>,
    /// Phase of the shared LFO, in radians, kept within one turn between blocks.
    phase: f32,
    /// Modulation depth, smoothed in *samples* so a depth change glides the tap.
    depth: SmoothedValue,
    mix: SmoothedValue,
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

impl Chorus {
    /// A new chorus at its default settings.
    pub fn new() -> Self {
        let sample_rate = 48_000.0;
        let descriptors = vec![
            ParamDescriptor::hertz(P_RATE_HZ, "rate_hz", "Rate", 0.05, 8.0, 0.8),
            ParamDescriptor::new(P_DEPTH_MS, "depth_ms", "Depth", 0.0, MAX_DEPTH_MS, 2.0)
                .with_unit(ParamUnit::Milliseconds),
            ParamDescriptor::percent(P_MIX, "mix", "Mix", 0.5),
        ];
        let mut plugin = Self {
            params: ParamBank::new(descriptors),
            sample_rate,
            lines: Vec::new(),
            phase: 0.0,
            depth: SmoothedValue::new(0.0, DEPTH_SMOOTHING_SECONDS, sample_rate),
            mix: SmoothedValue::new(0.0, MIX_SMOOTHING_SECONDS, sample_rate),
        };
        plugin.snap_to_params();
        plugin
    }

    fn depth_in_samples(&self) -> f32 {
        self.params.at(P_DEPTH_MS) * self.sample_rate / MILLISECONDS_PER_SECOND
    }

    fn base_in_samples(&self) -> f32 {
        BASE_DELAY_MS * self.sample_rate / MILLISECONDS_PER_SECOND
    }

    fn snap_to_params(&mut self) {
        self.depth.snap_to(self.depth_in_samples());
        self.mix.snap_to(self.params.at(P_MIX));
    }
}

impl Parameterized for Chorus {
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

impl Effect for Chorus {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            "auris.fx.chorus",
            "Chorus",
            "Modulated delay that doubles and widens a voice",
            PluginCategory::Modulation,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = ctx.sample_rate as f32;
        let channels = ctx.channel_count.max(1);
        let capacity = ((BASE_DELAY_MS + MAX_DEPTH_MS) * self.sample_rate / MILLISECONDS_PER_SECOND)
            .ceil() as usize
            + 2;

        self.lines.clear();
        self.lines
            .resize_with(channels, || DelayLine::new(capacity));
        self.phase = 0.0;

        self.depth
            .set_time(DEPTH_SMOOTHING_SECONDS, self.sample_rate);
        self.mix.set_time(MIX_SMOOTHING_SECONDS, self.sample_rate);
        self.snap_to_params();
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.reset();
        }
        self.phase = 0.0;
        self.snap_to_params();
    }

    fn process(&mut self, buffer: &mut AudioBuffer, _ctx: &ProcessContext) {
        let frames = buffer.frame_count();
        let channels = buffer.channel_count().min(self.lines.len());
        if frames == 0 || channels == 0 {
            return;
        }

        let increment = std::f32::consts::TAU * self.params.at(P_RATE_HZ) / self.sample_rate;
        let base = self.base_in_samples();
        self.depth.set_target(self.depth_in_samples());
        self.mix.set_target(self.params.at(P_MIX));

        // Every channel replays the identical ramps and phase walk from the block start; only
        // the phase *offset* differs per channel, so the sweeps stay locked to one oscillator.
        let depth_at_block_start = self.depth;
        let mix_at_block_start = self.mix;
        let phase_at_block_start = self.phase;

        for channel in 0..channels {
            let offset = channel as f32 * CHANNEL_PHASE_OFFSET;
            let mut depth = depth_at_block_start;
            let mut mix = mix_at_block_start;
            let mut phase = phase_at_block_start;
            let line = &mut self.lines[channel];

            for sample in buffer.channel_mut(channel)[..frames].iter_mut() {
                phase += increment;
                let delay = base + depth.next_value() * (phase + offset).sin();
                // Settled read: one non-finite input sample must not reach the wet mix, and the
                // slot it poisoned heals as soon as the ring overwrites it.
                let wet = crate::settled(line.read(delay));
                line.write(*sample);
                let wet_amount = mix.next_value();
                *sample = *sample * (1.0 - wet_amount) + wet * wet_amount;
            }

            self.depth = depth;
            self.mix = mix;
            self.phase = phase;
        }

        // Bounded phase keeps the sine's argument in the range where `f32` still resolves it.
        self.phase = self.phase.rem_euclid(std::f32::consts::TAU);
    }

    fn tail_frames(&self) -> usize {
        // The wet tap reads at most this far back, so this is how long the doubling voice can
        // outlive its input.
        ((BASE_DELAY_MS + MAX_DEPTH_MS) * self.sample_rate / MILLISECONDS_PER_SECOND).ceil()
            as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn context(frames: usize) -> ProcessContext {
        ProcessContext::realtime(SR, frames, 0, 120.0, true)
    }

    /// A fully wet chorus with the given sweep, ready to process.
    fn wet(rate_hz: f32, depth_ms: f32) -> Chorus {
        let mut plugin = Chorus::new();
        plugin.set_param_by_key("rate_hz", rate_hz);
        plugin.set_param_by_key("depth_ms", depth_ms);
        plugin.set_param_by_key("mix", 1.0);
        plugin.prepare(&PrepareContext::new(SR, 4_096, 2));
        plugin
    }

    fn impulse(frames: usize) -> AudioBuffer {
        let mut buffer = AudioBuffer::stereo(frames, SR);
        buffer.channel_mut(0)[0] = 1.0;
        buffer.channel_mut(1)[0] = 1.0;
        buffer
    }

    fn peak_index(channel: &[f32]) -> usize {
        channel
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .map(|(index, _)| index)
            .unwrap()
    }

    #[test]
    fn zero_depth_is_a_pure_delay_at_the_base_time() {
        // 15 ms at 48 kHz is exactly 720 samples, so the interpolator has nothing to spread.
        let mut plugin = wet(1.0, 0.0);
        let mut buffer = impulse(4_096);
        plugin.process(&mut buffer, &context(4_096));
        for channel in 0..2 {
            let samples = buffer.channel(channel);
            assert!(
                (samples[720] - 1.0).abs() < 1e-6,
                "channel {channel}: sample 720 was {}",
                samples[720]
            );
            assert_eq!(samples[719], 0.0);
            assert_eq!(samples[721], 0.0);
        }
    }

    #[test]
    fn the_lfo_bends_the_tap_away_from_the_base() {
        // At 1 Hz and 4 ms depth the left tap has drifted about 18 samples late by the time the
        // impulse comes back; the right channel leads by a quarter turn, so its sweep is near
        // its 192-sample extreme and the impulse lands close to 15 ms + 4 ms.
        let mut plugin = wet(1.0, 4.0);
        let mut buffer = impulse(4_096);
        plugin.process(&mut buffer, &context(4_096));

        let left = peak_index(buffer.channel(0));
        assert!(
            (736..=742).contains(&left),
            "left peak was at {left}, expected the sweep to push it past 720"
        );
        let right = peak_index(buffer.channel(1));
        assert!(
            (905..=915).contains(&right),
            "right peak was at {right}, expected the quadrature offset near 912"
        );
    }

    #[test]
    fn mix_zero_is_bit_transparent() {
        let mut plugin = Chorus::new();
        plugin.set_param_by_key("mix", 0.0);
        plugin.prepare(&PrepareContext::new(SR, 1_024, 2));

        let step = std::f32::consts::TAU * 440.0 / SR as f32;
        let samples: Vec<f32> = (0..1_024).map(|n| (step * n as f32).sin() * 0.8).collect();
        let input = AudioBuffer::from_planar(vec![samples.clone(), samples], SR).unwrap();
        let mut buffer = input.clone();
        plugin.process(&mut buffer, &context(1_024));
        for channel in 0..2 {
            assert_eq!(buffer.channel(channel), input.channel(channel));
        }
    }

    #[test]
    fn a_non_finite_sample_does_not_stick_in_the_ring() {
        let mut plugin = wet(1.0, 2.0);
        let mut buffer = impulse(2_048);
        buffer.channel_mut(0)[0] = f32::NAN;
        buffer.channel_mut(1)[0] = f32::INFINITY;
        plugin.process(&mut buffer, &context(2_048));
        // The frame the NaN arrived on is the input's problem; every *wet* sample is settled.
        assert!(
            buffer
                .channels()
                .iter()
                .all(|channel| channel[1..].iter().all(|s| s.is_finite()))
        );

        // The poisoned slots have been overwritten by now; a fresh impulse comes back whole.
        let mut buffer = impulse(2_048);
        plugin.process(&mut buffer, &context(2_048));
        let total: f32 = buffer.channel(0).iter().map(|s| s.abs()).sum();
        assert!(
            (total - 1.0).abs() < 0.05,
            "the doubled impulse summed to {total}"
        );
    }

    #[test]
    fn the_tail_is_the_furthest_the_tap_reads_back() {
        let mut plugin = Chorus::new();
        plugin.prepare(&PrepareContext::new(SR, 512, 2));
        // (15 + 8) ms at 48 kHz.
        assert_eq!(plugin.tail_frames(), 1_104);
        assert_eq!(plugin.latency_frames(), 0);
    }
}
