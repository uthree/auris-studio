//! Fractional-delay string loops with pluck and nonlinear bow excitation.

use super::{Model, Settings};

#[path = "pluck.rs"]
mod pluck;
use pluck::Pluck;

#[path = "bow.rs"]
mod bow;
use bow::Bow;

#[derive(Clone, Debug, Default)]
struct Delay {
    samples: Vec<f32>,
    cursor: usize,
}

impl Delay {
    fn prepare(&mut self, capacity: usize) {
        self.samples = vec![0.0; capacity];
        self.cursor = 0;
    }

    fn clear(&mut self) {
        self.samples.fill(0.0);
        self.cursor = 0;
    }

    fn read(&self, delay: f32) -> f32 {
        let length = self.samples.len();
        if length < 4 {
            return 0.0;
        }
        let delay = delay.clamp(2.0, (length - 2) as f32);
        let integral = delay as usize;
        let fraction = delay - integral as f32;
        let first = (self.cursor + length - integral) % length;
        let second = (first + length - 1) % length;
        let a = self.samples.get(first).copied().unwrap_or(0.0);
        let b = self.samples.get(second).copied().unwrap_or(0.0);
        a + fraction * (b - a)
    }

    fn write(&mut self, value: f32) {
        if let Some(sample) = self.samples.get_mut(self.cursor) {
            *sample = value;
            self.cursor = (self.cursor + 1) % self.samples.len();
        }
    }
}

/// Two travelling-wave delay lines meeting at a bow/string scattering junction.
#[derive(Clone, Debug, Default)]
pub(super) struct StringModel {
    bridge: Delay,
    neck: Delay,
    period: f32,
    position: f32,
    position_target: f32,
    period_target: f32,
    pole: f32,
    filtered: f32,
    loss: f32,
    bowed: bool,
    bow: Bow,
    velocity: f32,
    rate: f32,
    guitar: bool,
    pluck: Pluck,
}

impl StringModel {
    pub(super) fn motion(&self, points: &mut [f32]) -> (f32, f32) {
        let period = if self.guitar {
            self.pluck.period()
        } else {
            self.period
        };
        let position = self.position;
        let last = points.len().saturating_sub(1).max(1) as f32;
        for (index, point) in points.iter_mut().enumerate() {
            let x = index as f32 / last;
            // Reconstruct a fixed-end spatial projection from opposing travelling waves.
            // Single-loop plucked models expose the odd projection of their wave history.
            // A period is a round trip: travelling the string once takes half a period.
            *point = if index == 0 || x == 1.0 {
                0.0
            } else if !self.bowed {
                self.bridge.read(x * period * 0.5) - self.bridge.read((1.0 - x * 0.5) * period)
            } else if x <= position {
                self.bridge.read((position - x) * period * 0.5)
                    - self.bridge.read((position + x) * period * 0.5)
            } else {
                self.neck.read((x - position) * period * 0.5)
                    - self.neck.read((2.0 - position - x) * period * 0.5)
            };
        }
        (position, if self.bowed { self.bow.motion() } else { 0.0 })
    }
    pub(super) fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        // MIDI 0 is 8.18 Hz. Bound the supported host rate before allocating the voice pool.
        let capacity = (rate / 8.0).ceil() as usize + 8;
        self.bridge.prepare(capacity);
        self.neck.prepare(capacity);
    }

    pub(super) fn excite(
        &mut self,
        model: Model,
        frequency: f32,
        velocity: f32,
        rate: f32,
        settings: Settings,
    ) {
        let frequency = frequency.clamp(8.0, rate * 0.2);
        self.bridge.clear();
        self.neck.clear();
        self.filtered = 0.0;
        self.bowed = model == Model::Violin;
        self.guitar = matches!(model, Model::Guitar | Model::ElectricGuitar);
        self.velocity = velocity;
        if self.guitar {
            self.position = settings.position;
            self.pluck
                .excite(&mut self.bridge, frequency, velocity, rate, settings);
            return;
        }
        self.pole = if self.bowed {
            (0.35 + 0.55 * settings.damping).powf(48_000.0 / rate)
        } else {
            (0.8 - 0.65 * settings.hardness * (0.4 + 0.6 * velocity) + settings.damping * 0.15)
                .clamp(0.05, 0.92)
        };
        self.position = settings.position;
        self.position_target = self.position;
        self.bow.excite(velocity, rate, settings);
        // Compensate the low-frequency phase delay of the bridge's one-pole loss filter.
        self.period = (rate / frequency - self.pole / (1.0 - self.pole)).max(6.0);
        self.period_target = self.period;
        self.loss = (-6.907_755 / (settings.decay * frequency)).exp();
        if !self.bowed {
            // A triangular initial displacement is a string pulled at one point and let go.
            // The pick width smooths its corner. No sampled noise burst is needed.
            let period = self.period.ceil() as usize;
            for index in 0..period {
                let phase = index as f32 / self.period;
                let triangle = if phase < settings.position {
                    phase / settings.position
                } else {
                    (1.0 - phase) / (1.0 - settings.position)
                };
                let displacement = triangle - 0.5;
                let blend = settings.hardness * (0.4 + velocity * 0.6);
                let rounded = (std::f32::consts::TAU * phase).cos() * -0.35;
                self.bridge
                    .write(velocity * (rounded + blend * (displacement - rounded)));
            }
        }
    }

    pub(super) fn retune(&mut self, ratio: f32) {
        if self.guitar {
            self.pluck.retune(ratio);
            return;
        }
        if self.bowed {
            let phase = self.pole / (1.0 - self.pole);
            self.period_target =
                ((self.period_target + phase) / ratio - phase).clamp(6.0, self.rate / 8.0 - phase);
        } else {
            self.period = (self.period / ratio).max(6.0);
        }
    }

    pub(super) fn update_loss(&mut self, settings: Settings) {
        if self.guitar {
            self.pluck.update(settings);
            return;
        }
        let old_phase = self.pole / (1.0 - self.pole);
        self.pole = if self.bowed {
            (0.35 + 0.55 * settings.damping).powf(48_000.0 / self.rate)
        } else {
            (0.8 - 0.65 * settings.hardness * (0.4 + 0.6 * self.velocity) + settings.damping * 0.15)
                .clamp(0.05, 0.92)
        };
        let new_phase = self.pole / (1.0 - self.pole);
        self.period = (self.period + old_phase - new_phase).max(6.0);
        self.period_target = (self.period_target + old_phase - new_phase).max(6.0);
        self.position_target = settings.position;
        self.bow.update(settings);
        self.loss = (-6.907_755 * (self.period + new_phase) / (settings.decay * self.rate)).exp();
    }

    pub(super) fn next(&mut self, bow: f32, pressure: f32) -> f32 {
        if self.guitar {
            return self.pluck.next(&mut self.bridge);
        }
        if !self.bowed {
            let incoming = self.bridge.read(self.period);
            self.filtered = (1.0 - self.pole) * incoming + self.pole * self.filtered;
            self.bridge.write(self.loss * self.filtered);
            return incoming;
        }
        self.period += (self.period_target - self.period) / (self.rate * 0.005);
        self.position += (self.position_target - self.position) / (self.rate * 0.015);
        let bridge = self.bridge.read(self.period * self.position);
        let neck = -self.neck.read(self.period * (1.0 - self.position));
        self.filtered = self.pole * self.filtered - (1.0 - self.pole) * bridge;
        let incoming = self.filtered * self.loss + neck;
        let force = self.bow.next(incoming, bow, pressure);
        self.neck.write(self.filtered * self.loss + force);
        self.bridge.write(neck + force);
        bridge
    }

    pub(super) fn set_velocity(&mut self, velocity: f32) {
        self.velocity = velocity;
        self.bow.set_velocity(velocity);
    }

    pub(super) fn glide_to(&mut self, frequency: f32) {
        let phase = self.pole / (1.0 - self.pole);
        self.period_target = (self.rate / frequency.clamp(8.0, self.rate * 0.2) - phase).max(6.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fundamental_motion_has_a_center_antinode_and_fixed_endpoints() {
        let mut string = StringModel {
            period: 128.0,
            ..Default::default()
        };
        string.bridge.prepare(256);
        for index in 0..256 {
            string
                .bridge
                .write((std::f32::consts::TAU * index as f32 / 128.).sin());
        }
        let mut points = [0.0; 65];
        string.motion(&mut points);
        assert_eq!(points[0], 0.0);
        assert_eq!(points[64], 0.0);
        assert!(points[32].abs() > 1.99);
        assert!((points[16] / points[32] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert!((points[16] - points[48]).abs() < 1e-5);
    }
}
