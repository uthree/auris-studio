//! Pluck contact, calibrated loss and allpass tuning for the guitar delay loop.

use std::f32::consts::{PI, TAU};

use super::super::Settings;
use super::Delay;
use auris_dsp::{Biquad, BiquadCoefficients};

#[derive(Clone, Debug, Default)]
pub(super) struct Pluck {
    rate: f32,
    frequency: f32,
    target: f32,
    period: f32,
    delay: usize,
    coefficient: f32,
    previous_input: f32,
    previous_output: f32,
    previous_motion: f32,
    previous_pickup: f32,
    pickup_filter: Biquad,
    pickup_position: f32,
    pickup_step: f32,
    filtered: f32,
    pole: f32,
    gain: f32,
    settings: Option<Settings>,
    tick: u32,
}

impl Pluck {
    pub(super) fn period(&self) -> f32 {
        self.period
    }
    pub(super) fn excite(
        &mut self,
        line: &mut Delay,
        frequency: f32,
        velocity: f32,
        rate: f32,
        settings: Settings,
    ) {
        self.rate = rate;
        self.frequency = frequency;
        self.settings = Some(settings);
        self.target = rate / frequency;
        self.period = self.target;
        self.pickup_filter.reset();
        self.pickup_position = settings.pickup_position;
        self.pickup_step = 1.0 - (-1.0 / (rate * 0.005)).exp();
        self.previous_pickup = 0.0;
        self.configure();
        // A finite-width contact averages nearby triangular displacements. Hardness changes
        // the release spectrum, independently of the string's subsequent decay filter.
        let width = 0.004 + 0.06 * (1.0 - settings.hardness * (0.4 + 0.6 * velocity));
        for index in 0..self.period.ceil() as usize {
            let phase = index as f32 / self.period;
            let shape = (-2..=2)
                .map(|tap| {
                    let phase = (phase + tap as f32 * width * 0.5).rem_euclid(1.0);
                    if phase < settings.position {
                        phase / settings.position - 0.5
                    } else {
                        (1.0 - phase) / (1.0 - settings.position) - 0.5
                    }
                })
                .sum::<f32>()
                / 5.0;
            line.write(velocity * shape);
        }
        let incoming = line.read(self.delay as f32);
        self.previous_input = incoming;
        self.previous_output = incoming;
        self.previous_motion = incoming;
        self.filtered = incoming;
        if settings.electric {
            // Seed the differentiated observation from the same initialized wave history.
            // Starting it at zero would turn the initial displacement into a rate-scaled spike.
            self.previous_pickup = (incoming
                - line.read(self.period * settings.pickup_position - 1.0))
                / (2.0 * (PI * settings.pickup_position).sin()).max(0.2)
                * 0.7;
        }
        self.tick = 0;
    }

    fn configure(&mut self) {
        let Some(settings) = self.settings else {
            return;
        };
        if settings.electric {
            self.pickup_filter
                .set_coefficients(BiquadCoefficients::lowpass(
                    f64::from(self.rate),
                    settings.tone.min(self.rate * 0.4),
                    settings.pickup_q,
                ));
        }
        let frequency = self.rate / self.period;
        let omega = TAU * frequency / self.rate;
        // Split the requested fundamental loss between a scalar and a lowpass. The latter
        // determines upper-partial decay; no feedback gain exceeds unity, even at DC.
        let exponent = 6.907_755 * (1.0 + 3.0 * settings.damping) / (settings.decay * frequency);
        let share = 0.1 + 0.75 * settings.damping;
        let magnitude_squared = (-2.0 * exponent * share).exp();
        let a =
            (1.0 - magnitude_squared) / (2.0 * magnitude_squared * (1.0 - omega.cos()).max(1e-7));
        self.pole = (2.0 * a / (2.0 * a + 1.0 + (4.0 * a + 1.0).sqrt())).min(0.99);
        self.gain = (-exponent * (1.0 - share)).exp();
        let phase_delay = (self.pole * omega.sin()).atan2(1.0 - self.pole * omega.cos()) / omega;
        let delay = (self.period - phase_delay).max(3.5);
        self.delay = (delay - 0.5).floor() as usize;
        let fraction = delay - self.delay as f32;
        // Exact fundamental phase, with fractional delay in 0.5..1.5 samples. This avoids
        // a pole close to the unit circle and introduces no interpolation gain loss.
        self.coefficient =
            (0.5 * omega * (1.0 - fraction)).sin() / (0.5 * omega * (1.0 + fraction)).sin();
    }

    pub(super) fn update(&mut self, settings: Settings) {
        self.settings = Some(settings);
        self.configure();
    }

    pub(super) fn retune(&mut self, ratio: f32) {
        self.frequency = (self.frequency * ratio).clamp(8.0, self.rate * 0.2);
        self.target = self.rate / self.frequency;
    }

    pub(super) fn next(&mut self, line: &mut Delay) -> f32 {
        if (self.period - self.target).abs() > 0.001 {
            self.period += (self.target - self.period) * (1.0 / (self.rate * 0.005)).min(1.0);
            if self.tick.is_multiple_of(8) {
                self.configure();
            }
        }
        self.tick = self.tick.wrapping_add(1);
        let input = line.read(self.delay as f32);
        let motion = self.coefficient * (input - self.previous_output) + self.previous_input;
        self.previous_input = input;
        self.previous_output = motion;
        self.filtered = (1.0 - self.pole) * motion + self.pole * self.filtered;
        line.write(self.filtered * self.gain);
        // Bridge radiation follows motion velocity. A magnetic pickup observes displacement
        // with a position-dependent comb, and bypasses the acoustic body in the instrument.
        let acoustic = (motion - self.previous_motion) * self.period / TAU * 0.7;
        self.previous_motion = motion;
        let Some(settings) = self.settings else {
            return acoustic;
        };
        if settings.electric {
            self.pickup_position +=
                (settings.pickup_position - self.pickup_position) * self.pickup_step;
            let displacement = (motion - line.read(self.period * self.pickup_position))
                / (2.0 * (PI * self.pickup_position).sin()).max(0.2)
                * 0.7;
            // Induced voltage follows motion velocity. The LC approximation then limits
            // upper partials without feeding pickup position or tone back into the string.
            // Below the standard guitar range, keep the voltage reference at the open E2
            // period rather than amplifying a differentiated transient by an unbounded period.
            let voltage = (displacement - self.previous_pickup)
                * self.period.min(self.rate / 82.406_89)
                / TAU;
            self.previous_pickup = displacement;
            return self.pickup_filter.process_sample(voltage);
        }
        let electric = (motion - line.read(self.period * settings.position))
            / (2.0 * (PI * settings.position).sin()).max(0.2)
            * 0.7;
        acoustic + settings.pickup * (electric - acoustic)
    }
}
