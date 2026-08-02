//! Measurement helpers shared by the unit tests.
//!
//! The tests assert on numbers — frequencies in Hz, levels in dB, sample offsets — so they need
//! a way to measure a rendered block rather than eyeball it.

use auris_core::param::gain_to_db;
use auris_core::{AudioBuffer, Instrument, NoteEvent, PrepareContext, ProcessContext};

/// Drives an instrument block by block, the way the engine will.
pub(crate) struct Rig {
    pub instrument: Box<dyn Instrument>,
    sample_rate: f64,
    block: usize,
    buffer: AudioBuffer,
    prefill: Option<f32>,
    playhead: u64,
}

impl Rig {
    /// Prepares `instrument` for `channels` channels of `block`-frame blocks.
    pub fn new(
        mut instrument: Box<dyn Instrument>,
        sample_rate: f64,
        block: usize,
        channels: usize,
    ) -> Self {
        instrument.prepare(&PrepareContext::new(sample_rate, block, channels));
        Self {
            instrument,
            sample_rate,
            block,
            buffer: AudioBuffer::new(channels, block, sample_rate),
            prefill: None,
            playhead: 0,
        }
    }

    /// Writes `value` into the output buffer before every block, so a test can prove the
    /// instrument overwrites rather than adds.
    pub fn prefill(&mut self, value: f32) {
        self.prefill = Some(value);
    }

    /// Sets a parameter by its stable key, panicking on a typo.
    pub fn set_param(&mut self, key: &str, value: f32) {
        assert!(
            self.instrument.set_param_by_key(key, value),
            "unknown parameter `{key}`"
        );
    }

    /// Renders `frames` frames, applying `events` at their offsets from the start of this call.
    ///
    /// Returns channel 0. Every channel is checked to be identical, which is how the tests
    /// verify that the instrument writes the whole buffer.
    pub fn render(&mut self, frames: usize, events: &[NoteEvent]) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        let mut rendered = 0usize;
        while rendered < frames {
            let block = self.block.min(frames - rendered);
            self.buffer.set_frame_count(block);
            if let Some(value) = self.prefill {
                for channel in self.buffer.iter_channels_mut() {
                    channel.fill(value);
                }
            }

            let start = rendered as u32;
            let end = (rendered + block) as u32;
            let block_events: Vec<NoteEvent> = events
                .iter()
                .filter(|event| (start..end).contains(&event.frame()))
                .map(|event| event.with_frame(event.frame() - start))
                .collect();

            let ctx = ProcessContext::realtime(self.sample_rate, block, self.playhead, 120.0, true);
            self.instrument
                .process(&block_events, &mut self.buffer, &ctx);

            for channel in 1..self.buffer.channel_count() {
                assert_eq!(
                    self.buffer.channel(channel),
                    self.buffer.channel(0),
                    "channel {channel} does not match channel 0"
                );
            }
            out.extend_from_slice(self.buffer.channel(0));
            self.playhead += block as u64;
            rendered += block;
        }
        out
    }
}

/// Largest absolute sample.
pub(crate) fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
}

/// Peak level in dBFS.
pub(crate) fn peak_db(samples: &[f32]) -> f32 {
    gain_to_db(peak(samples))
}

/// Root-mean-square level.
pub(crate) fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Largest step between consecutive samples, a proxy for how sharp the waveform's
/// discontinuities are.
pub(crate) fn max_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .fold(0.0f32, |acc, pair| acc.max((pair[1] - pair[0]).abs()))
}

/// Fundamental frequency measured from upward zero crossings.
///
/// Counting whole cycles between the first and the last crossing measures the period directly,
/// with none of the bin-width error a short DFT would have. It assumes a signal with one
/// crossing pair per cycle, which is true of the tones the tests measure.
pub(crate) fn zero_crossing_hz(samples: &[f32], sample_rate: f64) -> f64 {
    let mut first: Option<f64> = None;
    let mut last = 0.0f64;
    let mut cycles = 0u32;
    for index in 1..samples.len() {
        let (previous, current) = (samples[index - 1], samples[index]);
        if previous <= 0.0 && current > 0.0 {
            // Interpolate the crossing so the estimate is not quantised to whole samples.
            let fraction = f64::from(-previous) / f64::from(current - previous);
            let position = (index - 1) as f64 + fraction;
            match first {
                None => first = Some(position),
                Some(_) => {
                    last = position;
                    cycles += 1;
                }
            }
        }
    }
    match first {
        Some(start) if cycles > 0 && last > start => {
            f64::from(cycles) * sample_rate / (last - start)
        }
        _ => 0.0,
    }
}

/// Amplitude of `samples` at `frequency`, by the Goertzel algorithm.
///
/// This is one bin of a DFT computed with a two-tap recurrence. It gives the tests a way to ask
/// "how much energy is at exactly this frequency" without pulling in an FFT.
pub(crate) fn goertzel(samples: &[f32], sample_rate: f64, frequency: f64) -> f64 {
    if samples.is_empty() || sample_rate <= 0.0 {
        return 0.0;
    }
    let omega = std::f64::consts::TAU * frequency / sample_rate;
    let coefficient = 2.0 * omega.cos();
    let (mut previous, mut older) = (0.0f64, 0.0f64);
    for sample in samples {
        let current = f64::from(*sample) + coefficient * previous - older;
        older = previous;
        previous = current;
    }
    let power = previous * previous + older * older - coefficient * previous * older;
    // 2/N turns the bin's power back into the amplitude of a sinusoid at that frequency.
    2.0 * power.max(0.0).sqrt() / samples.len() as f64
}

/// Mean power in a narrow band around `centre`, as an amplitude.
///
/// One DFT bin of a noise signal is a random draw with several dB of spread, which makes a
/// single [`goertzel`] call useless for comparing noise spectra. Averaging the power of a
/// spread of bins across the band trades frequency resolution for a stable estimate.
pub(crate) fn band_amplitude(samples: &[f32], sample_rate: f64, centre: f64) -> f64 {
    const BINS: usize = 15;
    let mut total = 0.0;
    for bin in 0..BINS {
        // Geometric spread over an eighth of an octave either side of the centre.
        let offset = 2.0 * bin as f64 / (BINS - 1) as f64 - 1.0;
        let frequency = centre * (offset / 8.0).exp2();
        let amplitude = goertzel(samples, sample_rate, frequency);
        total += amplitude * amplitude;
    }
    (total / BINS as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn sine(frequency: f64, sample_rate: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| (TAU * frequency * n as f64 / sample_rate).sin() as f32)
            .collect()
    }

    #[test]
    fn zero_crossings_measure_a_known_sine() {
        let samples = sine(440.0, 48_000.0, 48_000);
        let measured = zero_crossing_hz(&samples, 48_000.0);
        assert!((measured - 440.0).abs() < 0.1, "measured {measured}");
    }

    #[test]
    fn goertzel_recovers_a_known_amplitude() {
        let mut samples = sine(1_000.0, 48_000.0, 4_800);
        for sample in &mut samples {
            *sample *= 0.25;
        }
        let amplitude = goertzel(&samples, 48_000.0, 1_000.0);
        assert!((amplitude - 0.25).abs() < 0.01, "amplitude {amplitude}");
        let elsewhere = goertzel(&samples, 48_000.0, 3_000.0);
        assert!(elsewhere < 0.01, "leakage {elsewhere}");
    }

    #[test]
    fn band_amplitude_follows_the_signal() {
        let samples = sine(1_000.0, 48_000.0, 4_800);
        let inside = band_amplitude(&samples, 48_000.0, 1_000.0);
        let outside = band_amplitude(&samples, 48_000.0, 4_000.0);
        assert!(inside > outside * 10.0, "{inside} vs {outside}");
    }

    #[test]
    fn level_helpers_agree_with_the_maths() {
        let samples = sine(1_000.0, 48_000.0, 4_800);
        assert!((peak(&samples) - 1.0).abs() < 1e-3);
        assert!((rms(&samples) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert!(peak_db(&samples).abs() < 0.01);
        assert_eq!(peak_db(&[0.0; 16]), -120.0);
    }
}
