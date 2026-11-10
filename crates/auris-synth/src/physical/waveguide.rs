//! Travelling waves: delay lines read between samples, and the small parts around them.
//!
//! The struck and plucked instruments are sums of modes because nothing in them depends on
//! where a wave is at a given instant. A bow, a reed and an air jet do: each acts on the wave
//! that is passing it *now*, and what it does depends on what came back from the far end a
//! moment ago. Those instruments are therefore kept as what they are, a wave going out and a
//! wave coming back, in delay lines.

/// A delay line whose length may be changed while it sounds.
#[derive(Clone, Debug, Default)]
pub(crate) struct Line {
    samples: Vec<f32>,
    mask: usize,
    cursor: usize,
}

impl Line {
    /// Allocates room for delays up to `longest` samples.
    pub(crate) fn prepare(&mut self, longest: usize) {
        let size = (longest + 8).next_power_of_two().max(16);
        self.samples = vec![0.0; size];
        self.mask = size - 1;
        self.cursor = 0;
    }

    /// Empties the line.
    pub(crate) fn clear(&mut self) {
        self.samples.fill(0.0);
        self.cursor = 0;
    }

    /// Longest delay that can be read.
    pub(crate) fn reach(&self) -> f32 {
        self.samples.len().saturating_sub(4) as f32
    }

    /// Puts a sample in.
    pub(crate) fn write(&mut self, value: f32) {
        if let Some(slot) = self.samples.get_mut(self.cursor) {
            *slot = value;
        }
        self.cursor = (self.cursor + 1) & self.mask;
    }

    fn at(&self, back: usize) -> f32 {
        self.samples
            .get(self.cursor.wrapping_sub(back) & self.mask)
            .copied()
            .unwrap_or(0.0)
    }

    /// The sample written `delay` writes ago, a whole number of them; one is the last written.
    pub(crate) fn tap(&self, delay: usize) -> f32 {
        self.at(delay.max(1))
    }

    /// The value `delay` samples back, between samples: a cubic through the four nearest.
    ///
    /// A straight line between two samples is a low-pass whose cut depends on where between
    /// them the reading falls, so a vibrato would sweep the top of the spectrum in and out. A
    /// cubic is flat to a fifth of the sample rate wherever it reads.
    pub(crate) fn read(&self, delay: f32) -> f32 {
        let delay = delay.clamp(2.0, self.reach().max(2.0));
        let whole = delay as usize;
        let f = delay - whole as f32;
        let (older, old, new, newer) = (
            self.at(whole + 2),
            self.at(whole + 1),
            self.at(whole),
            self.at(whole - 1),
        );
        // Lagrange weights for a point `f` of the way from `new` toward `old`.
        let weight_newer = -f * (f - 1.0) * (f - 2.0) / 6.0;
        let weight_new = (f + 1.0) * (f - 1.0) * (f - 2.0) / 2.0;
        let weight_old = -(f + 1.0) * f * (f - 2.0) / 2.0;
        let weight_older = (f + 1.0) * f * (f - 1.0) / 6.0;
        newer * weight_newer + new * weight_new + old * weight_old + older * weight_older
    }
}

/// Taps of the half-band filter at odd distances 1, 3, 5, … 31 from its centre, whose own tap
/// is one half. Every even distance but zero is zero.
///
/// A 63-tap Kaiser-windowed (β = 8) half-band low-pass: flat within 0.003 dB to 0.21 of the
/// doubled rate and at least 70 dB down from 0.29, so what an oversampled loop produces above
/// the host's band edge is removed rather than folded back onto it.
const ODD_TAPS: [f32; 16] = [
    3.170_729e-1,
    -1.024_425_2e-1,
    5.772_404_8e-2,
    -3.748_938_4e-2,
    2.563_768_7e-2,
    -1.780_47e-2,
    1.231_558_4e-2,
    -8.376_211e-3,
    5.543_647_4e-3,
    -3.534_414_5e-3,
    2.145_908_7e-3,
    -1.222_076_1e-3,
    6.381_064e-4,
    -2.935_601e-4,
    1.090_362_4e-4,
    -2.401_525_4e-5,
];
const TAPS: usize = 63;
const CENTRE: usize = TAPS / 2;
const RING: usize = 64;

/// Two-to-one decimation for a loop run at twice the host rate. Fixed-size, so resetting it
/// for a new note never allocates. Its delay is 15.5 host samples, the same for every note.
#[derive(Clone, Debug)]
pub(crate) struct Decimator {
    /// Every input written twice, `RING` apart, so the newest `TAPS` are always contiguous.
    history: [f32; 2 * RING],
    cursor: usize,
}

impl Default for Decimator {
    fn default() -> Self {
        Self {
            history: [0.0; 2 * RING],
            cursor: 0,
        }
    }
}

impl Decimator {
    /// Forgets what has passed through.
    pub(crate) fn reset(&mut self) {
        self.history.fill(0.0);
        self.cursor = 0;
    }

    fn push(&mut self, sample: f32) {
        self.history[self.cursor] = sample;
        self.history[self.cursor + RING] = sample;
        self.cursor = (self.cursor + 1) % RING;
    }

    /// Takes two internal samples, oldest first, and returns one host sample.
    pub(crate) fn process(&mut self, first: f32, second: f32) -> f32 {
        self.push(first);
        self.push(second);
        let start = self.cursor + RING - TAPS;
        let window = &self.history[start..start + TAPS];
        let mut sum = 0.5 * window[CENTRE];
        for (index, tap) in ODD_TAPS.iter().enumerate() {
            let distance = 2 * index + 1;
            sum += tap * (window[CENTRE - distance] + window[CENTRE + distance]);
        }
        sum
    }
}

/// A seeded source of noise in `-1.0..1.0`: a player's breath, a bow's rosin.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Noise(u32);

impl Default for Noise {
    fn default() -> Self {
        Self(0x9E37_79B9)
    }
}

impl Noise {
    /// Starts the sequence `seed` names. Zero is taken for another number, since the
    /// generator cannot leave it.
    pub(crate) fn seed(&mut self, seed: u32) {
        self.0 = if seed == 0 { 0x9E37_79B9 } else { seed };
    }

    /// The next value.
    pub(crate) fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 * (2.0 / u32::MAX as f32) - 1.0
    }
}

/// Pole of a one-pole low-pass with its corner at `hz`.
pub(crate) fn lowpass_pole(hz: f32, rate: f32) -> f32 {
    (-std::f32::consts::TAU * hz.min(rate * 0.45) / rate).exp()
}

/// How much of a sinusoid turning `turn` radians a sample a one-pole low-pass with `pole`
/// lets through.
pub(crate) fn lowpass_gain(pole: f32, turn: f32) -> f32 {
    (1.0 - pole)
        / (1.0 - 2.0 * pole * turn.cos() + pole * pole)
            .max(1.0e-12)
            .sqrt()
}

/// Samples by which a one-pole low-pass with `pole` delays a sinusoid turning `turn` radians
/// a sample: its phase lag there over the frequency. Toward zero frequency this is
/// `pole / (1 - pole)`.
pub(crate) fn lowpass_delay(pole: f32, turn: f32) -> f32 {
    let turn = turn.max(1.0e-4);
    let (sine, cosine) = turn.sin_cos();
    (pole * sine).atan2(1.0 - pole * cosine) / turn
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn a_line_reads_back_what_was_written_however_long_ago() {
        let mut line = Line::default();
        line.prepare(100);
        for value in 0..200 {
            line.write(value as f32);
        }
        assert_eq!(line.tap(1), 199.0);
        assert_eq!(line.tap(50), 150.0);
        // A ramp is read exactly between its samples by any interpolator worth the name.
        assert!((line.read(10.25) - 189.75).abs() < 1.0e-3);
        assert!((line.read(64.5) - 135.5).abs() < 1.0e-3);
        line.clear();
        assert_eq!(line.read(10.0), 0.0);
    }

    #[test]
    fn a_reading_between_samples_keeps_the_top_of_the_band() {
        // A sine at an eighth of the sample rate, read half-way between samples: a straight
        // line would lose 0.7 dB of it, a cubic loses a tenth of that.
        let mut line = Line::default();
        line.prepare(256);
        let mut power = 0.0_f64;
        for n in 0..2_000 {
            line.write((TAU * n as f64 / 8.0).sin() as f32);
            if n >= 400 {
                power += f64::from(line.read(20.5)).powi(2);
            }
        }
        // A unit sine has a mean square of a half.
        let loss = 10.0 * (power / 1_600.0 / 0.5).log10();
        assert!(loss.abs() < 0.1, "{loss} dB");
    }

    /// Steady-state amplitude of a sine at `cycles` per internal sample through the decimator.
    fn gain(cycles: f64) -> f32 {
        let mut decimator = Decimator::default();
        let mut peak = 0.0_f32;
        for frame in 0..4_000 {
            let at = |n: usize| (TAU * cycles * n as f64).sin() as f32;
            let out = decimator.process(at(2 * frame), at(2 * frame + 1));
            if frame > 100 {
                peak = peak.max(out.abs());
            }
        }
        peak
    }

    #[test]
    fn the_decimator_passes_the_host_band_and_rejects_what_would_fold_into_it() {
        // At a 48 kHz host the internal rate is 96 kHz: 1 kHz, 18 kHz, then 30 and 40 kHz.
        assert!((gain(1.0 / 96.0) - 1.0).abs() < 1e-3);
        assert!((gain(18.0 / 96.0) - 1.0).abs() < 1e-3);
        assert!(gain(30.0 / 96.0) < 10.0_f32.powf(-70.0 / 20.0));
        assert!(gain(40.0 / 96.0) < 10.0_f32.powf(-70.0 / 20.0));
    }

    #[test]
    fn noise_is_seeded_and_centred() {
        let (mut a, mut b) = (Noise::default(), Noise::default());
        a.seed(7);
        b.seed(7);
        let values: Vec<f32> = (0..4_000).map(|_| a.next()).collect();
        assert!(
            values
                .iter()
                .zip((0..4_000).map(|_| b.next()))
                .all(|(x, y)| *x == y)
        );
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!(values.iter().all(|value| (-1.0..=1.0).contains(value)));
        let mut zero = Noise::default();
        zero.seed(0);
        assert_ne!(zero.next(), 0.0);
    }

    #[test]
    fn a_low_pass_delays_what_it_passes_by_its_pole_over_one_less() {
        let pole = lowpass_pole(4_000.0, 96_000.0);
        assert!((pole - (-TAU as f32 * 4_000.0 / 96_000.0).exp()).abs() < 1.0e-6);
        assert!((lowpass_delay(0.5, 1.0e-4) - 1.0).abs() < 1.0e-4);
        // Higher up it delays less, and never more.
        assert!(lowpass_delay(0.5, 0.5) < 0.95);
        assert!((lowpass_gain(0.5, 0.0) - 1.0).abs() < 1.0e-6);
        // At its corner a one-pole is three decibels down.
        let corner = TAU as f32 * 4_000.0 / 96_000.0;
        assert!((lowpass_gain(pole, corner) - 0.707).abs() < 0.02);
    }
}
