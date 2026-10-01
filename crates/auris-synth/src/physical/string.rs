//! Fractional-delay string loops with pluck and nonlinear bow excitation.

use super::{Model, Settings};

#[path = "pluck.rs"]
mod pluck;
use pluck::Pluck;

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
    pole: f32,
    filtered: f32,
    loss: f32,
    bowed: bool,
    bow_velocity: f32,
    velocity: f32,
    rate: f32,
    guitar: bool,
    pluck: Pluck,
}

impl StringModel {
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
        self.bridge.clear();
        self.neck.clear();
        self.filtered = 0.0;
        self.bowed = model == Model::Violin;
        self.guitar = model == Model::Guitar;
        self.velocity = velocity;
        if self.guitar {
            self.pluck
                .excite(&mut self.bridge, frequency, velocity, rate, settings);
            return;
        }
        self.pole = (0.8 - 0.65 * settings.hardness * (0.4 + 0.6 * velocity)
            + settings.damping * 0.15)
            .clamp(0.05, 0.92);
        self.position = settings.position;
        self.bow_velocity = velocity * 0.08;
        // Compensate the low-frequency phase delay of the bridge's one-pole loss filter.
        self.period = (rate / frequency - self.pole / (1.0 - self.pole)).max(6.0);
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
        self.period = (self.period / ratio).max(6.0);
    }

    pub(super) fn update_loss(&mut self, settings: Settings) {
        if self.guitar {
            self.pluck.update(settings);
            return;
        }
        let old_phase = self.pole / (1.0 - self.pole);
        self.pole = (0.8 - 0.65 * settings.hardness * (0.4 + 0.6 * self.velocity)
            + settings.damping * 0.15)
            .clamp(0.05, 0.92);
        let new_phase = self.pole / (1.0 - self.pole);
        self.period = (self.period + old_phase - new_phase).max(6.0);
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
        let bridge = self.bridge.read(self.period * self.position);
        let neck = -self.neck.read(self.period * (1.0 - self.position));
        self.filtered = self.pole * self.filtered - (1.0 - self.pole) * bridge;
        let incoming = self.filtered * self.loss + neck;
        let difference = self.bow_velocity * bow - incoming;
        // A bounded friction admittance: near zero relative velocity the bow sticks; as the
        // string slips, friction falls. Bounding it by unity keeps scattering passive.
        // Express slip in units of this note's bow speed. Without this normalisation a loud
        // attack jumps straight past the friction peak and can sound quieter than a soft one.
        let slope = (2.0 + 18.0 * pressure.clamp(0.0, 1.0)) * 0.04 / self.bow_velocity.max(0.001);
        let friction = (difference.abs() * slope + 0.75).powi(-4).min(1.0);
        let force = difference * friction;
        self.neck.write(self.filtered * self.loss + force);
        self.bridge.write(neck + force);
        bridge
    }
}
