//! Reduced membrane and plate modes. Spatial bases are prepared once, outside the callback.

use std::f32::consts::PI;

use auris_core::motion::{MOTION_POINTS, MotionVoice};

pub(super) const MODES: usize = 32;
/// Time constant of the pitch glide after a strike, fitted by eye to the captured kick and toms,
/// which fall by half their raise in about forty milliseconds.
const GLIDE_SECONDS: f32 = 0.06;

// First circular-membrane eigenvalues (zeros of J_m), with their angular orders.
// The membrane edge is fixed; these ratios are unrelated to the MIDI harmonic series.
const HEAD: [(usize, f32); 16] = [
    (0, 2.404_826),
    (1, 3.831_706),
    (2, 5.135_622),
    (0, 5.520_078),
    (3, 6.380_162),
    (1, 7.015_587),
    (4, 7.588_342),
    (2, 8.417_244),
    (0, 8.653_728),
    (5, 8.771_483),
    (3, 9.761_023),
    (1, 10.173_468),
    (6, 9.936_11),
    (4, 11.064_71),
    (2, 11.619_841),
    (0, 11.791_534),
];

#[derive(Clone, Copy, Debug, Default)]
struct Mode {
    displacement: f32,
    quadrature: f32,
    cosine: f32,
    sine: f32,
    radius: f32,
    loss: f32,
    /// Phase advance per sample, kept for the glide's extra rotation.
    angle: f32,
    frequency: f32,
    pickup: f32,
    strike: [f32; 8],
}

#[derive(Clone, Debug)]
pub(super) struct Resonators {
    modes: [Mode; MODES],
    count: usize,
    /// Fractional pitch raise from tension modulation, decaying towards the settled tuning.
    glide: f32,
    glide_loss: f32,
}

/// The same spatial basis drives strike weights, microphone weights and the display.
#[derive(Clone, Debug)]
pub(super) struct Projection {
    shapes: [[f32; MOTION_POINTS]; MODES],
    strike: [[f32; 8]; MODES],
    pickup: [f32; MODES],
    metal: bool,
}

impl Projection {
    pub(super) fn new(metal: bool) -> Self {
        let basis = |mode: usize, x: f32, y: f32| {
            let radius = x.hypot(y);
            if radius > 1.0 || (!metal && radius >= 1.0) {
                return 0.0;
            }
            let angle = y.atan2(x);
            if metal {
                // Reduced polar bending basis. Unlike a drum head, the plate rim can move.
                // This compact approximation does not solve the full free-plate eigenproblem.
                let radial = 1 + mode / 8;
                let angular = mode % 8;
                (PI * radial as f32 * radius).cos()
                    * radius.powi(angular as i32)
                    * (angular as f32 * angle).cos()
            } else if let Some(&(order, zero)) = HEAD.get(mode) {
                bessel(order, zero * radius) * (order as f32 * angle).cos()
            } else {
                0.0
            }
        };
        Self {
            shapes: std::array::from_fn(|mode| {
                std::array::from_fn(|index| {
                    let x = (index % 8) as f32 / 3.5 - 1.0;
                    let y = (index / 8) as f32 / 3.5 - 1.0;
                    basis(mode, x, y)
                })
            }),
            strike: std::array::from_fn(|mode| {
                std::array::from_fn(|index| basis(mode, index as f32 / 7.5, 0.0))
            }),
            // An off-centre pickup avoids suppressing every angular mode.
            pickup: std::array::from_fn(|mode| basis(mode, 0.61, 0.23)),
            metal,
        }
    }

    pub(super) fn observe(&self, resonators: &Resonators, level: f32, voice: &mut MotionVoice) {
        for (index, mode) in resonators.modes.iter().take(resonators.count).enumerate() {
            for (point, shape) in voice.points.iter_mut().zip(&self.shapes[index]) {
                *point += shape * mode.displacement * level;
            }
            if let Some(magnitude) = voice.modes.get_mut(index) {
                *magnitude = mode.displacement.hypot(mode.quadrature) * level;
            }
        }
    }
}

impl Resonators {
    /// `t30` gives each mode the seconds its frequency takes to fall 30 dB.
    pub(super) fn prepared(
        projection: &Projection,
        rate: f32,
        fundamental: f32,
        t30: impl Fn(f32) -> f32,
    ) -> Self {
        let count = if projection.metal { MODES } else { HEAD.len() };
        let mut modes = [Mode::default(); MODES];
        for (index, mode) in modes.iter_mut().take(count).enumerate() {
            let ratio = if projection.metal {
                // A compact free-plate spectrum; increasing stiffness spreads high modes.
                let n = index as f32;
                1.0 + 0.29 * n + 0.021 * n * n
            } else {
                HEAD[index].1 / HEAD[0].1
            };
            let frequency = fundamental * ratio;
            if frequency >= rate * 0.45 {
                continue;
            }
            let angle = 2.0 * PI * frequency / rate;
            *mode = Mode {
                cosine: angle.cos(),
                sine: angle.sin(),
                // Natural-log decrement per sample of a fall of 30 dB.
                loss: 1.5 * std::f32::consts::LN_10 / (t30(frequency).max(1e-3) * rate),
                angle,
                frequency,
                pickup: projection.pickup[index],
                strike: projection.strike[index],
                ..Default::default()
            };
        }
        Self {
            modes,
            count,
            glide: 0.0,
            // The raise follows the square of the head's displacement, so it settles at about
            // twice the rate of the fundamental's own decay; the captured kick and toms settle
            // within a few tens of milliseconds whatever their ring.
            glide_loss: (-1.0 / (GLIDE_SECONDS * rate)).exp(),
        }
    }

    /// `damping` multiplies every mode's loss and `decay` divides it; `glide` is the fractional
    /// pitch raise the strike starts with.
    pub(super) fn strike(
        &mut self,
        hardness: f32,
        position: f32,
        damping: f32,
        decay: f32,
        glide: f32,
    ) {
        let position = position * 7.0;
        let left = (position as usize).min(6);
        let fraction = position - left as f32;
        let mut weights = [0.0; MODES];
        let mut energy = 0.0;
        for (mode, weight) in self.modes.iter_mut().zip(&mut weights).take(self.count) {
            let spatial = mode.strike[left] * (1.0 - fraction) + mode.strike[left + 1] * fraction;
            // A finite soft contact suppresses the upper modes before it leaves the body.
            let contact = mode.frequency * (0.000_02 + (1.0 - hardness) * 0.000_6);
            *weight = spatial / (1.0 + contact * contact);
            energy += (*weight * mode.pickup).powi(2);
            mode.radius = (-mode.loss * damping / decay).exp();
        }
        // Fix total strike energy, while position and hardness redistribute it between modes.
        let scale = 0.65 / energy.sqrt().max(0.1);
        for (mode, weight) in self.modes.iter_mut().zip(weights).take(self.count) {
            mode.displacement = 0.0;
            mode.quadrature = weight * scale;
        }
        self.glide = glide.clamp(0.0, 1.0);
    }

    pub(super) fn soften_ride_strike(&mut self) {
        // The axisymmetric modes at 2.65 and 12.36 kHz dominate this reduced plate.
        // Attenuate their excitation after energy normalization so the removed energy
        // cannot boost the other modes. Keep their frequencies and decay unchanged.
        for (index, gain) in [(0, 0.6), (8, 0.8)] {
            self.modes[index].quadrature *= gain;
        }
    }

    pub(super) fn next(&mut self) -> f32 {
        let mut output = 0.0;
        let gliding = self.glide > 1e-5;
        for mode in self.modes.iter_mut().take(self.count) {
            let mut real = mode.displacement * mode.cosine - mode.quadrature * mode.sine;
            let mut imag = mode.displacement * mode.sine + mode.quadrature * mode.cosine;
            if gliding {
                // The extra advance is a small angle: second order keeps the rotation unitary
                // to well below the mode's own loss.
                let extra = mode.angle * self.glide;
                let (cosine, sine) = (1.0 - extra * extra * 0.5, extra);
                (real, imag) = (real * cosine - imag * sine, real * sine + imag * cosine);
            }
            mode.displacement = real * mode.radius;
            mode.quadrature = imag * mode.radius;
            output += mode.displacement * mode.pickup;
        }
        self.glide *= self.glide_loss;
        output
    }
}

// Only used while preparing the spatial bases. f64 keeps cancellation near the higher roots
// from corrupting the fixed-edge boundary; the series is bounded and never enters process.
fn bessel(order: usize, x: f32) -> f32 {
    let half = f64::from(x) * 0.5;
    let mut term = (1..=order).fold(1.0, |term, n| term * half / n as f64);
    let mut sum = term;
    for k in 1..40 {
        term *= -half * half / (k * (k + order)) as f64;
        sum += term;
    }
    sum as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membrane_modes_have_fixed_edges_and_inharmonic_frequencies() {
        for &(order, zero) in &HEAD {
            assert!(bessel(order, zero).abs() < 2e-6);
        }
        let projection = Projection::new(false);
        let resonators = Resonators::prepared(&projection, 48_000.0, 100.0, |_| 0.5);
        assert!((resonators.modes[1].frequency / 100.0 - 1.5933).abs() < 0.001);
        assert!(
            projection
                .shapes
                .iter()
                .all(|shape| shape[0] == 0.0 && shape[63] == 0.0)
        );
    }

    #[test]
    fn every_prepared_mode_is_stable_and_below_nyquist() {
        for rate in [8_000.0, 22_050.0, 48_000.0, 96_000.0, 192_000.0] {
            for metal in [false, true] {
                let projection = Projection::new(metal);
                let mut resonators = Resonators::prepared(&projection, rate, 900.0, |_| 2.4);
                resonators.strike(1.0, 0.98, 0.4, 2.0, 0.3);
                for mode in &resonators.modes {
                    assert!(mode.frequency < rate * 0.5);
                    assert!(mode.radius <= 1.0);
                }
                for _ in 0..rate as usize {
                    assert!(resonators.next().is_finite());
                }
            }
        }
    }
}
