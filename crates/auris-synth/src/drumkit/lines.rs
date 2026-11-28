//! A cymbal's dense modes: the hundreds of lines a recorded cymbal's tail is made of.
//!
//! The reduced plate in `model` resolves 32 modes, which is what the motion display can show; a
//! real crash or ride rings on hundreds, a few hertz apart, standing 20 to 35 dB above the noise
//! between them. Band-limited noise of the same level and decay sounds like hiss rather than
//! metal, so each band's measured share of line energy is carried by decaying sinusoids here and
//! only the rest by the diffuse field. Every line falls at the measured rate of its own frequency.
//!
//! The lines are laid out band by band in whole SIMD lanes so the per-sample loop vectorises.

use super::BANDS;
use super::acoustics::BAND_HZ;

/// The most lines one hit models.
pub(super) const MAX_LINES: usize = 384;
/// Lines advanced together; every band holds a whole number of them.
pub(super) const LANES: usize = 8;
/// A cymbal is not quite round, so each of its modes is a pair a few hertz apart, and the pair
/// beats: the slow swell and fade a recorded cymbal's tail is full of. Hertz between partners.
const SPLIT_HZ: (f32, f32) = (0.4, 4.0);

#[derive(Clone, Debug)]
pub(super) struct Lines {
    real: [f32; MAX_LINES],
    imag: [f32; MAX_LINES],
    /// Per-sample rotation with the loss folded in, set at each strike.
    cosine: [f32; MAX_LINES],
    sine: [f32; MAX_LINES],
    /// The bare rotation and the natural-log decrement per sample of a fall of 30 dB.
    turn_cosine: [f32; MAX_LINES],
    turn_sine: [f32; MAX_LINES],
    loss: [f32; MAX_LINES],
    /// Initial amplitude of each line, sign included: the mode shape at the strike point.
    amplitude: [f32; MAX_LINES],
    /// One past each band's last line.
    ends: [usize; BANDS],
}

/// A small deterministic generator for laying out lines and jittering strikes.
pub(super) struct Rng(pub(super) u32);

impl Rng {
    pub(super) fn uniform(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / 16_777_216.0
    }

    pub(super) fn gaussian(&mut self) -> f32 {
        let radius = (-2.0 * (1.0 - self.uniform()).ln()).sqrt();
        radius * (std::f32::consts::TAU * self.uniform()).cos()
    }
}

impl Lines {
    /// No lines at all: a membrane's field is all noise.
    pub(super) fn none() -> Self {
        Self {
            real: [0.0; MAX_LINES],
            imag: [0.0; MAX_LINES],
            cosine: [0.0; MAX_LINES],
            sine: [0.0; MAX_LINES],
            turn_cosine: [0.0; MAX_LINES],
            turn_sine: [0.0; MAX_LINES],
            loss: [0.0; MAX_LINES],
            amplitude: [0.0; MAX_LINES],
            ends: [0; BANDS],
        }
    }

    /// Lay out `counts` lines per band as split pairs, the pairs evenly spread with jitter so
    /// no two clump into a beat the cymbal does not have. `power` is the mean power the lines of each band carry together
    /// at the strike; `spread_db` the deviation of their levels; `t30` the seconds a line at a
    /// given frequency takes to fall 30 dB. `seed` fixes the layout: a cymbal keeps its modes.
    pub(super) fn prepared(
        rate: f32,
        counts: &[u16; BANDS],
        power: &[f32; BANDS],
        spread_db: f32,
        t30: impl Fn(f32) -> f32,
        seed: u32,
    ) -> Self {
        let mut lines = Self::none();
        let mut rng = Rng(seed | 1);
        let mut next = 0;
        for band in 0..BANDS {
            let count = usize::from(counts[band]).next_multiple_of(LANES);
            let low = BAND_HZ[band] * std::f32::consts::FRAC_1_SQRT_2;
            let high = (BAND_HZ[band] * std::f32::consts::SQRT_2).min(rate * 0.45);
            if count == 0 || power[band] <= 0.0 || high <= low || next + count > MAX_LINES {
                lines.ends[band] = next;
                continue;
            }
            let range = next..next + count;
            let mut total = 0.0;
            let pairs = count / 2;
            let mut hz = 0.0;
            for (slot, line) in range.clone().enumerate() {
                hz = if slot % 2 == 0 {
                    low + (high - low) * ((slot / 2) as f32 + rng.uniform()) / pairs as f32
                } else {
                    hz + SPLIT_HZ.0 * (SPLIT_HZ.1 / SPLIT_HZ.0).powf(rng.uniform())
                };
                let angle = std::f32::consts::TAU * hz / rate;
                lines.turn_cosine[line] = angle.cos();
                lines.turn_sine[line] = angle.sin();
                lines.loss[line] = 1.5 * std::f32::consts::LN_10 / (t30(hz).max(1e-3) * rate);
                let sign = if rng.uniform() < 0.5 { -1.0 } else { 1.0 };
                let level = 10_f32.powf(spread_db * rng.gaussian() / 20.0);
                lines.amplitude[line] = sign * level;
                total += level * level * 0.5;
            }
            let scale = (power[band] / total).sqrt();
            for line in range {
                lines.amplitude[line] *= scale;
            }
            next += count;
            lines.ends[band] = next;
        }
        lines
    }

    /// Start every line from the strike. `exponent` scales every loss (damping over decay);
    /// `jitter_db` varies each line's level from hit to hit, as a stick never lands twice alike.
    pub(super) fn strike(&mut self, exponent: f32, jitter_db: f32, rng: &mut Rng) {
        let count = self.ends[BANDS - 1];
        for line in 0..count {
            let radius = (-self.loss[line] * exponent).exp();
            self.cosine[line] = radius * self.turn_cosine[line];
            self.sine[line] = radius * self.turn_sine[line];
            self.real[line] = 0.0;
            let jitter = if jitter_db > 0.0 {
                10_f32.powf(jitter_db * rng.gaussian() / 20.0)
            } else {
                1.0
            };
            self.imag[line] = self.amplitude[line] * jitter;
        }
    }

    /// One sample: every band's lines, each band scaled by its `gain` (the bloom still to come).
    pub(super) fn next(&mut self, gain: &[f32; BANDS]) -> f32 {
        let mut output = 0.0;
        let mut start = 0;
        for (band, &end) in self.ends.iter().enumerate() {
            if end == start {
                continue;
            }
            let mut lanes = [0.0; LANES];
            let real = self.real[start..end].as_chunks_mut::<LANES>().0;
            let imag = self.imag[start..end].as_chunks_mut::<LANES>().0;
            let cosine = self.cosine[start..end].as_chunks::<LANES>().0;
            let sine = self.sine[start..end].as_chunks::<LANES>().0;
            for (((real, imag), cosine), sine) in real.iter_mut().zip(imag).zip(cosine).zip(sine) {
                for lane in 0..LANES {
                    let turned = real[lane] * cosine[lane] - imag[lane] * sine[lane];
                    imag[lane] = real[lane] * sine[lane] + imag[lane] * cosine[lane];
                    real[lane] = turned;
                    lanes[lane] += imag[lane];
                }
            }
            output += lanes.iter().sum::<f32>() * gain[band];
            start = end;
        }
        output
    }

    /// Whether `band` has lines; one that has none leaves its whole share to the noise.
    pub(super) fn carries(&self, band: usize) -> bool {
        let start = if band == 0 { 0 } else { self.ends[band - 1] };
        self.ends[band] > start
    }

    pub(super) fn is_empty(&self) -> bool {
        self.ends[BANDS - 1] == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_carry_their_power_and_fall_30_db_in_their_time() {
        let rate = 48_000.0;
        let mut counts = [0; BANDS];
        counts[6] = 24;
        let mut power = [0.0; BANDS];
        power[6] = 0.02;
        let mut lines = Lines::prepared(rate, &counts, &power, 6.0, |_| 1.0, 7);
        lines.strike(1.0, 0.0, &mut Rng(1));
        let gain = [1.0; BANDS];
        let audio: Vec<f32> = (0..rate as usize).map(|_| lines.next(&gain)).collect();
        let mean = |range: std::ops::Range<usize>| {
            audio[range.clone()].iter().map(|x| x * x).sum::<f32>() / range.len() as f32
        };
        let start = mean(0..2_400);
        // Uncorrelated lines add as powers, so the sum carries the power it was given.
        assert!((start / 0.02 - 1.0).abs() < 0.35, "{start}");
        let late = mean(46_800..48_000);
        // The windows' centres are 0.96 s apart: 29 dB of a 30 dB-per-second fall.
        let fall = 10.0 * (late / start).log10();
        assert!((-33.0..-25.0).contains(&fall), "{fall} dB, not 29");
        assert!(lines.ends[5] == 0 && lines.ends[6] == 24 && lines.ends[8] == 24);
    }

    #[test]
    fn a_band_above_nyquist_gets_no_lines_and_nothing_is_unstable() {
        let mut counts = [16; BANDS];
        counts[0] = 0;
        let power = [0.01; BANDS];
        for rate in [8_000.0, 22_050.0, 44_100.0, 96_000.0] {
            let mut lines = Lines::prepared(rate, &counts, &power, 9.0, |_| 8.0, 3);
            lines.strike(0.5, 2.0, &mut Rng(9));
            assert!(lines.ends[BANDS - 1] <= MAX_LINES);
            let gain = [1.0; BANDS];
            for _ in 0..rate as usize {
                assert!(lines.next(&gain).is_finite());
            }
        }
        assert!(Lines::none().is_empty());
    }
}
