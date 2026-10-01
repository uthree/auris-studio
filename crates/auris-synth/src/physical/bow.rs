//! Stateful static/sliding friction with a smooth, independently controlled bow speed.

use super::super::Settings;

#[derive(Clone, Debug, Default)]
pub(super) struct Bow {
    velocity: f32,
    motion: f32,
    ramp: f32,
    rate: f32,
    speed: f32,
    hardness: f32,
    sticking: bool,
}

impl Bow {
    pub(super) fn excite(&mut self, velocity: f32, rate: f32, settings: Settings) {
        self.velocity = velocity;
        self.motion = 0.0;
        self.rate = rate;
        self.sticking = true;
        self.update(settings);
    }

    pub(super) fn update(&mut self, settings: Settings) {
        self.ramp = (1.0 / (self.rate * settings.bow_response)).min(1.0);
        self.speed = settings.bow_speed;
        self.hardness = settings.hardness;
    }

    pub(super) fn set_velocity(&mut self, velocity: f32) {
        self.velocity = velocity;
    }

    pub(super) fn next(&mut self, incoming: f32, expression: f32, pressure: f32) -> f32 {
        let target = self.velocity * 0.12 * self.speed * expression;
        self.motion += (target - self.motion) * self.ramp;
        let difference = self.motion - incoming;
        let static_limit = self.velocity * (0.008 + 0.09 * pressure.clamp(0.0, 1.0));
        if difference.abs() > static_limit {
            self.sticking = false;
        } else if difference.abs() < static_limit * 0.45 {
            self.sticking = true;
        }
        if self.sticking {
            return difference;
        }
        let slip_scale = (self.velocity * 0.04).max(0.001);
        let relative = difference / slip_scale;
        let dynamic_limit =
            static_limit * (0.3 + 0.35 * self.hardness) / (1.0 + relative * relative * 0.15);
        // Never reverse relative slip: |force| <= |difference| makes the junction passive
        // relative to bow motion, even when automation crosses the static/sliding boundary.
        difference.signum() * dynamic_limit.min(difference.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friction_has_hysteresis_and_never_reverses_relative_slip() {
        let mut bow = Bow {
            velocity: 0.8,
            hardness: 0.7,
            ..Bow::default()
        };
        let force = bow.next(-0.2, 0.0, 0.5);
        assert!(!bow.sticking);
        assert!((0.0..0.2).contains(&force));
        bow.next(-0.03, 0.0, 0.5);
        assert!(!bow.sticking);
        assert_eq!(bow.next(-0.005, 0.0, 0.5), 0.005);
        assert!(bow.sticking);
        for pressure in [0.0, 0.5, 1.0] {
            for incoming in [-1.0, -0.01, 0.0, 0.01, 1.0] {
                let force = bow.next(incoming, 0.0, pressure);
                assert!(force.abs() <= incoming.abs() + 1e-7);
                assert!(force * -incoming >= 0.0);
            }
        }
    }
}
