//! Waveform generation.
//!
//! Every built-in instrument builds its tone out of [`Oscillator`]s. The oscillator keeps a
//! normalised phase in `0..1` and advances it by a fixed increment per sample, which makes the
//! frequency a single multiply away and lets a caller modulate it per sample without any state
//! to unwind.
//!
//! # Band limiting
//!
//! A square or saw computed straight from the phase has a step discontinuity in it. Sampling a
//! step folds every harmonic above Nyquist back into the audible band, and because those
//! harmonics are not related to the fundamental by an integer ratio the result is an
//! inharmonic shimmer that gets worse the higher the note. [PolyBLEP][blep] fixes the first
//! order of that error by replacing the two samples nearest each discontinuity with a
//! polynomial approximation of a band-limited step. It costs a handful of operations per
//! sample and removes most of the audible aliasing, which is the right trade for an instrument
//! that plays several voices at once.
//!
//! Sine needs no correction, and triangle needs little: its harmonics fall off at 12 dB per
//! octave (amplitude `1/n²`) rather than the saw's 6 dB per octave, so its aliases land at
//! least 30 dB below the fundamental even at the top of the keyboard.
//!
//! [blep]: https://www.experimentalscene.com/articles/MinBLEPs.php

use std::borrow::Cow;
use std::f32::consts::TAU;

/// Shapes an [`Oscillator`] can produce.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Waveform {
    /// A pure tone; the only harmonic is the fundamental.
    Sine,
    /// A pulse wave with adjustable width; band-limited at both edges.
    #[default]
    Square,
    /// A band-limited sawtooth: every harmonic, falling at 6 dB per octave.
    Saw,
    /// A triangle: odd harmonics only, falling at 12 dB per octave.
    Triangle,
    /// The 15-bit linear-feedback shift register of the NES noise channel.
    Noise,
}

impl Waveform {
    /// Every waveform, in parameter order.
    pub const ALL: [Waveform; 5] = [
        Waveform::Sine,
        Waveform::Square,
        Waveform::Saw,
        Waveform::Triangle,
        Waveform::Noise,
    ];

    /// Labels for a [`ParamUnit::Choice`](auris_core::ParamUnit::Choice) parameter, in the same
    /// order as [`Waveform::ALL`].
    pub const CHOICES: &'static [Cow<'static, str>] = &[
        Cow::Borrowed("Sine"),
        Cow::Borrowed("Square"),
        Cow::Borrowed("Saw"),
        Cow::Borrowed("Triangle"),
        Cow::Borrowed("Noise"),
    ];

    /// Position of this waveform in [`Waveform::ALL`].
    pub fn index(self) -> u32 {
        match self {
            Waveform::Sine => 0,
            Waveform::Square => 1,
            Waveform::Saw => 2,
            Waveform::Triangle => 3,
            Waveform::Noise => 4,
        }
    }

    /// Waveform at `index`, clamping out-of-range values to the ends of [`Waveform::ALL`].
    pub fn from_index(index: u32) -> Waveform {
        match index {
            0 => Waveform::Sine,
            1 => Waveform::Square,
            2 => Waveform::Saw,
            3 => Waveform::Triangle,
            _ => Waveform::Noise,
        }
    }

    /// Waveform for a parameter value, rounding to the nearest choice.
    pub fn from_param(value: f32) -> Waveform {
        if value.is_finite() {
            Waveform::from_index(value.round().clamp(0.0, 4.0) as u32)
        } else {
            Waveform::Square
        }
    }

    /// Name shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Waveform::Sine => "Sine",
            Waveform::Square => "Square",
            Waveform::Saw => "Saw",
            Waveform::Triangle => "Triangle",
            Waveform::Noise => "Noise",
        }
    }
}

/// Shifts of the noise register per nominal oscillator cycle.
///
/// The NES clocks its shift register far above the pitch the noise channel is perceived at. At
/// 32 shifts per cycle an A440 note runs the register at 14 kHz, so its 32767-step sequence
/// only repeats every 2.3 seconds — long enough to read as noise instead of as a buzz — while
/// still tracking the note, which is what makes the channel usable for both hats and rumble.
const NOISE_CLOCK_RATIO: f32 = 32.0;

/// Largest phase increment, i.e. one cycle every two samples. Clamping here keeps the phase
/// wrap, the PolyBLEP window and the noise clock loop bounded no matter what frequency a
/// modulator asks for.
const MAX_INCREMENT: f32 = 0.5;

/// Upper bound on noise register shifts per sample, from `NOISE_CLOCK_RATIO * MAX_INCREMENT`
/// rounded up. The clocking loop is capped at this so it can never run unbounded.
const MAX_NOISE_STEPS: u32 = 17;

/// A phase-accumulating oscillator with band-limited square and saw shapes.
#[derive(Clone, Debug)]
pub struct Oscillator {
    phase: f32,
    increment: f32,
    inv_sample_rate: f32,
    pulse_width: f32,
    noise_phase: f32,
    lfsr: u16,
    seed: u16,
}

impl Default for Oscillator {
    fn default() -> Self {
        Oscillator::new()
    }
}

impl Oscillator {
    /// Sample rate assumed until [`Oscillator::set_sample_rate`] is called.
    pub const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;

    /// A oscillator at 48 kHz with the default noise seed.
    pub fn new() -> Self {
        Oscillator::with_seed(1)
    }

    /// An oscillator whose noise register starts at `seed`.
    ///
    /// Only the low 15 bits are used and the value is forced odd, because a zero register is
    /// the one state the feedback cannot leave.
    pub fn with_seed(seed: u16) -> Self {
        let seed = (seed & 0x7fff) | 1;
        Self {
            phase: 0.0,
            increment: 0.0,
            inv_sample_rate: 1.0 / Self::DEFAULT_SAMPLE_RATE,
            pulse_width: 0.5,
            noise_phase: 0.0,
            lfsr: seed,
            seed,
        }
    }

    /// A reproducible noise seed for voice `index`.
    ///
    /// All 32767 non-zero states belong to the same cycle, so any of them is a valid entry
    /// point; multiplying by a large odd constant puts consecutive voices far apart in that
    /// cycle, which stops stacked voices from correlating into a tone.
    pub fn seed_for_voice(index: usize) -> u16 {
        (((index as u16).wrapping_mul(0x5ded) ^ 0x1f35) & 0x7fff) | 1
    }

    /// Sets the rate the oscillator is being rendered at. Non-positive rates are ignored.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if sample_rate > 0.0 {
            self.inv_sample_rate = 1.0 / sample_rate;
        }
    }

    /// Sets the frequency in Hz, clamped to the range `0..=Nyquist`.
    pub fn set_frequency(&mut self, hz: f32) {
        let increment = hz * self.inv_sample_rate;
        self.increment = if increment.is_finite() {
            increment.clamp(0.0, MAX_INCREMENT)
        } else {
            0.0
        };
    }

    /// Frequency in Hz that the oscillator is currently running at.
    pub fn frequency(&self) -> f32 {
        self.increment / self.inv_sample_rate
    }

    /// Sets the duty cycle of [`Waveform::Square`], clamped to `0.01..=0.99`.
    ///
    /// The ends are excluded so that a pulse always has two edges to correct. How far apart those
    /// edges have to *stay* is a question about the note being played rather than about the knob,
    /// and is answered separately, per note, where the waveform is evaluated.
    pub fn set_pulse_width(&mut self, width: f32) {
        if width.is_finite() {
            self.pulse_width = width.clamp(0.01, 0.99);
        }
    }

    /// Current duty cycle, as it was asked for.
    pub fn pulse_width(&self) -> f32 {
        self.pulse_width
    }

    /// The duty cycle the band-limited square can actually sound at this frequency.
    ///
    /// A PolyBLEP correction spans one sample either side of the edge it smooths, so the two edges
    /// of a pulse need `2 * dt` of phase between them to be corrected separately. Closer than that
    /// and the corrections overlap and subtract from each other: at C8 with the duty cycle at the
    /// 0.05 the interface allows, the sum never rose above -0.18 anywhere in the cycle — the pulse
    /// did not come out narrow, it did not come out at all, and what was left was a DC offset.
    ///
    /// So the width closes toward a square as the note climbs, which is a real change of tone and
    /// the honest one: a pulse thinner than two samples is not a shape this sample rate can carry,
    /// and the choice is which way to be wrong about it. Above `dt` of 0.25 there is no room for
    /// two separated edges at all and every duty cycle sounds as 0.5.
    ///
    /// The knob keeps whatever it was set to — [`Self::pulse_width`] still reports it — so the
    /// same patch played an octave down is narrow again.
    fn sounding_pulse_width(&self) -> f32 {
        let margin = 2.0 * self.increment;
        if margin * 2.0 >= 1.0 {
            return 0.5;
        }
        self.pulse_width.clamp(margin, 1.0 - margin)
    }

    /// Current phase, in cycles (`0..1`).
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Moves the phase, wrapping into `0..1`.
    pub fn set_phase(&mut self, phase: f32) {
        self.phase = if phase.is_finite() {
            phase - phase.floor()
        } else {
            0.0
        };
    }

    /// Returns the oscillator to its start-of-note state: phase zero, noise register reseeded.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.noise_phase = 0.0;
        self.lfsr = self.seed;
    }

    /// Produces the next sample and advances the phase.
    pub fn next(&mut self, waveform: Waveform) -> f32 {
        let sample = self.evaluate(self.phase, waveform, true);
        self.advance();
        sample
    }

    /// Produces the next sample with the phase shifted by `offset` cycles.
    ///
    /// This is the phase-modulation input used by the FM instrument: a modulator scaled to
    /// cycles is added to the read position without disturbing the accumulator, so the carrier
    /// keeps its own pitch exactly.
    pub fn next_modulated(&mut self, waveform: Waveform, offset: f32) -> f32 {
        let phase = if offset.is_finite() {
            let shifted = self.phase + offset;
            shifted - shifted.floor()
        } else {
            self.phase
        };
        let sample = self.evaluate(phase, waveform, true);
        self.advance();
        sample
    }

    /// Produces the next sample with band limiting switched off.
    ///
    /// Only useful for measuring what PolyBLEP buys: this is the naive waveform, aliases and
    /// all. Instruments should call [`Oscillator::next`].
    pub fn next_naive(&mut self, waveform: Waveform) -> f32 {
        let sample = self.evaluate(self.phase, waveform, false);
        self.advance();
        sample
    }

    fn evaluate(&self, phase: f32, waveform: Waveform, band_limited: bool) -> f32 {
        let dt = self.increment;
        match waveform {
            Waveform::Sine => (phase * TAU).sin(),
            Waveform::Square => {
                // The width the two edges can actually be placed at, not the one the knob holds:
                // the naive step and the corrections have to agree about where the falling edge
                // is, or the correction lands beside the edge rather than on it.
                let width = if band_limited {
                    self.sounding_pulse_width()
                } else {
                    self.pulse_width
                };
                let naive = if phase < width { 1.0 } else { -1.0 };
                if band_limited {
                    // Rising edge at phase 0, falling edge at the pulse width.
                    let mut fall = phase - width;
                    if fall < 0.0 {
                        fall += 1.0;
                    }
                    naive + poly_blep(phase, dt) - poly_blep(fall, dt)
                } else {
                    naive
                }
            }
            Waveform::Saw => {
                let naive = 2.0 * phase - 1.0;
                if band_limited {
                    naive - poly_blep(phase, dt)
                } else {
                    naive
                }
            }
            // Starts at -1, peaks at +1 half a cycle later; continuous, so no correction.
            Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            // The NES inverts bit 0 on the way out, so a cleared bit is the positive level.
            Waveform::Noise => {
                if self.lfsr & 1 == 0 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }

    fn advance(&mut self) {
        self.phase += self.increment;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }

        self.noise_phase += self.increment * NOISE_CLOCK_RATIO;
        if self.noise_phase >= 1.0 {
            let whole = self.noise_phase.floor();
            self.noise_phase -= whole;
            let steps = (whole as u32).min(MAX_NOISE_STEPS);
            for _ in 0..steps {
                self.clock_noise();
            }
        }
    }

    /// One shift of the NES noise register: bit 0 XOR bit 1 feeds back into bit 14.
    fn clock_noise(&mut self) {
        let feedback = (self.lfsr ^ (self.lfsr >> 1)) & 1;
        self.lfsr = (self.lfsr >> 1) | (feedback << 14);
    }
}

/// Correction for a step discontinuity of height 2 at phase 0.
///
/// `t` is the distance from the discontinuity in cycles and `dt` the phase increment, so the
/// two branches cover the sample just after and the sample just before the edge. Outside that
/// two-sample window the correction is zero and the naive waveform is already correct.
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// Radians-to-cycles factor, for turning an FM modulation index into a phase offset.
pub(crate) const RADIANS_TO_CYCLES: f32 = 1.0 / TAU;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{goertzel, max_step, rms, zero_crossing_hz};

    fn render(
        waveform: Waveform,
        hz: f32,
        sample_rate: f32,
        frames: usize,
        naive: bool,
    ) -> Vec<f32> {
        let mut osc = Oscillator::new();
        osc.set_sample_rate(sample_rate);
        osc.set_frequency(hz);
        (0..frames)
            .map(|_| {
                if naive {
                    osc.next_naive(waveform)
                } else {
                    osc.next(waveform)
                }
            })
            .collect()
    }

    #[test]
    fn sine_has_the_expected_level_and_rate() {
        let samples = render(Waveform::Sine, 440.0, 48_000.0, 48_000, false);
        // A full-scale sine has an RMS of 1/sqrt(2).
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (rms(&samples) - expected).abs() < 1e-3,
            "rms {}",
            rms(&samples)
        );
        let measured = zero_crossing_hz(&samples, 48_000.0);
        assert!((measured - 440.0).abs() < 0.5, "measured {measured} Hz");
    }

    #[test]
    fn square_duty_cycle_sets_the_dc_offset() {
        let mut osc = Oscillator::new();
        osc.set_sample_rate(48_000.0);
        osc.set_frequency(100.0);
        osc.set_pulse_width(0.25);
        let samples: Vec<f32> = (0..48_000)
            .map(|_| osc.next_naive(Waveform::Square))
            .collect();
        // A pulse that is high for a quarter of each cycle averages 0.25 - 0.75 = -0.5.
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        assert!((mean + 0.5).abs() < 1e-2, "mean {mean}");
    }

    #[test]
    fn triangle_and_saw_are_symmetric_around_zero() {
        for waveform in [Waveform::Triangle, Waveform::Saw] {
            let samples = render(waveform, 200.0, 48_000.0, 48_000, false);
            let mean = samples.iter().sum::<f32>() / samples.len() as f32;
            assert!(mean.abs() < 1e-2, "{waveform:?} mean {mean}");
        }
    }

    #[test]
    fn polyblep_saw_aliases_less_than_the_naive_one() {
        // At 5 kHz on a 48 kHz clock the 9th harmonic sits at 45 kHz and folds back to 3 kHz,
        // which is not a harmonic of 5 kHz — so any energy measured there is aliasing.
        let sample_rate = 48_000.0;
        let fundamental = 5_000.0;
        let limited = render(Waveform::Saw, fundamental, sample_rate, 16_384, false);
        let naive = render(Waveform::Saw, fundamental, sample_rate, 16_384, true);

        let alias_limited = goertzel(&limited, sample_rate as f64, 3_000.0);
        let alias_naive = goertzel(&naive, sample_rate as f64, 3_000.0);
        assert!(
            alias_limited < alias_naive * 0.5,
            "alias energy: polyblep {alias_limited:.5} vs naive {alias_naive:.5}"
        );

        // The wanted signal must survive: the fundamental stays within 1 dB.
        let f_limited = goertzel(&limited, sample_rate as f64, fundamental as f64);
        let f_naive = goertzel(&naive, sample_rate as f64, fundamental as f64);
        let ratio_db = 20.0 * (f_limited / f_naive).log10();
        assert!(
            ratio_db.abs() < 1.0,
            "fundamental changed by {ratio_db:.2} dB"
        );

        // The step at the wrap is spread over two samples instead of landing in one.
        assert!(
            max_step(&limited) < max_step(&naive),
            "max step: polyblep {} vs naive {}",
            max_step(&limited),
            max_step(&naive)
        );
    }

    #[test]
    fn polyblep_square_aliases_less_than_the_naive_one() {
        let sample_rate = 48_000.0;
        let limited = render(Waveform::Square, 5_000.0, sample_rate, 16_384, false);
        let naive = render(Waveform::Square, 5_000.0, sample_rate, 16_384, true);
        // A square has only odd harmonics; the 9th lands at 45 kHz and folds back to 3 kHz.
        let alias_limited = goertzel(&limited, sample_rate as f64, 3_000.0);
        let alias_naive = goertzel(&naive, sample_rate as f64, 3_000.0);
        assert!(
            alias_limited < alias_naive * 0.6,
            "alias energy: polyblep {alias_limited:.5} vs naive {alias_naive:.5}"
        );
    }

    #[test]
    fn noise_register_walks_all_32767_states() {
        let mut osc = Oscillator::with_seed(1);
        let mut steps = 0u32;
        loop {
            osc.clock_noise();
            steps += 1;
            if osc.lfsr == 1 || steps > 40_000 {
                break;
            }
        }
        assert_eq!(steps, 32_767, "LFSR period");
    }

    #[test]
    fn noise_is_bipolar_and_reproducible() {
        let first = render(Waveform::Noise, 440.0, 48_000.0, 4_096, false);
        let second = render(Waveform::Noise, 440.0, 48_000.0, 4_096, false);
        assert_eq!(first, second, "same seed must give the same noise");
        assert!(first.iter().all(|s| *s == 1.0 || *s == -1.0));
        // Over 4096 samples both levels should be reasonably balanced.
        let mean = first.iter().sum::<f32>() / first.len() as f32;
        assert!(mean.abs() < 0.2, "mean {mean}");
    }

    #[test]
    fn different_voice_seeds_decorrelate() {
        let mut a = Oscillator::with_seed(Oscillator::seed_for_voice(0));
        let mut b = Oscillator::with_seed(Oscillator::seed_for_voice(1));
        a.set_sample_rate(48_000.0);
        b.set_sample_rate(48_000.0);
        a.set_frequency(440.0);
        b.set_frequency(440.0);
        let mut correlation = 0.0f32;
        for _ in 0..8_192 {
            correlation += a.next(Waveform::Noise) * b.next(Waveform::Noise);
        }
        correlation /= 8_192.0;
        assert!(correlation.abs() < 0.2, "correlation {correlation}");
    }

    #[test]
    fn frequency_is_clamped_to_nyquist() {
        let mut osc = Oscillator::new();
        osc.set_sample_rate(48_000.0);
        osc.set_frequency(96_000.0);
        assert!((osc.frequency() - 24_000.0).abs() < 1e-1);
        osc.set_frequency(f32::NAN);
        assert_eq!(osc.frequency(), 0.0);
    }

    #[test]
    fn every_waveform_stays_finite_and_bounded() {
        for waveform in Waveform::ALL {
            for hz in [0.0, 1.0, 440.0, 12_000.0, 40_000.0] {
                let samples = render(waveform, hz, 48_000.0, 2_048, false);
                assert!(
                    samples.iter().all(|s| s.is_finite() && s.abs() <= 2.0),
                    "{waveform:?} at {hz} Hz left the safe range"
                );
            }
        }
    }

    #[test]
    fn waveform_indices_round_trip() {
        for waveform in Waveform::ALL {
            assert_eq!(Waveform::from_index(waveform.index()), waveform);
            assert_eq!(
                Waveform::CHOICES[waveform.index() as usize].as_ref(),
                waveform.label()
            );
        }
        assert_eq!(Waveform::from_param(f32::NAN), Waveform::Square);
    }

    /// The peak and trough of one cycle of a band-limited square.
    fn square_extremes(hz: f32, pulse_width: f32) -> (f32, f32) {
        let mut osc = Oscillator::new();
        osc.set_sample_rate(48_000.0);
        osc.set_frequency(hz);
        osc.set_pulse_width(pulse_width);
        osc.reset();
        let period = (48_000.0 / hz).ceil() as usize;
        let mut high = f32::NEG_INFINITY;
        let mut low = f32::INFINITY;
        for _ in 0..period {
            let sample = osc.next(Waveform::Square);
            high = high.max(sample);
            low = low.min(sample);
        }
        (high, low)
    }

    #[test]
    fn a_narrow_pulse_high_up_the_keyboard_still_has_a_pulse_in_it() {
        // The correction spans a sample either side of each edge, so two edges closer together
        // than that subtract from each other instead of smoothing one apiece. At C8 with the duty
        // cycle at the 0.05 the interface offers, that used to leave a waveform whose *highest*
        // sample in the whole cycle was -0.18: not a narrow pulse, no pulse, and a DC offset
        // where the note should have been.
        for hz in [440.0, 1_760.0, 4_186.0] {
            let (high, low) = square_extremes(hz, 0.05);
            assert!(
                high > 0.9,
                "at {hz} Hz the pulse never rises: highest sample {high}"
            );
            assert!(low < -0.9, "at {hz} Hz the trough is missing: {low}");
        }
    }

    #[test]
    fn the_duty_cycle_a_note_can_sound_closes_toward_square_as_it_climbs() {
        // What the fix trades away, pinned so it is a decision rather than a surprise: the knob
        // keeps its value, and the width the oscillator can place widens with the note until
        // there is only room for a square.
        let mut osc = Oscillator::new();
        osc.set_sample_rate(48_000.0);
        osc.set_pulse_width(0.05);

        osc.set_frequency(440.0);
        assert_eq!(osc.pulse_width(), 0.05);
        assert_eq!(osc.sounding_pulse_width(), 0.05, "440 Hz has room to spare");

        // 4186 Hz is dt = 0.0872, so the edges are held 0.1744 apart.
        osc.set_frequency(4_186.0);
        assert_eq!(osc.pulse_width(), 0.05, "the knob is untouched");
        assert!((osc.sounding_pulse_width() - 0.1744).abs() < 1e-3);

        // Past a quarter of the sample rate there is no room for two separated edges at all.
        osc.set_frequency(20_000.0);
        assert_eq!(osc.sounding_pulse_width(), 0.5);

        // And the widening is symmetric: a pulse set wide closes from the other side.
        osc.set_pulse_width(0.95);
        osc.set_frequency(4_186.0);
        assert!((osc.sounding_pulse_width() - 0.8256).abs() < 1e-3);
    }
}
