//! Finite hammer contact and a passive, coupled unison-string modal expansion.

use std::f32::consts::{PI, TAU};

use super::super::Settings;

const PARTIALS: usize = 64;

#[derive(Clone, Copy, Debug, Default)]
struct StringMode {
    real: f32,
    imag: f32,
    sine: f32,
    cosine: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Partial {
    strings: [StringMode; 3],
    radius: f32,
    drive: f32,
}

#[derive(Clone, Debug)]
pub(super) struct Piano {
    partials: [Partial; PARTIALS],
    active: usize,
    strings: usize,
    contact_frames: usize,
    age: usize,
    coupling: f32,
}

impl Default for Piano {
    fn default() -> Self {
        Self {
            partials: [Partial::default(); PARTIALS],
            active: 0,
            strings: 1,
            contact_frames: 2,
            age: 0,
            coupling: 0.0,
        }
    }
}

impl Piano {
    pub(super) fn excite(&mut self, frequency: f32, velocity: f32, rate: f32, settings: Settings) {
        self.strings = if frequency < 65.0 {
            1
        } else if frequency < 180.0 {
            2
        } else {
            3
        };
        let detune = match self.strings {
            1 => [0.0, 0.0, 0.0],
            2 => [-0.9, 0.9, 0.0],
            _ => [-1.8, 0.0, 1.8],
        };
        let hardness = settings.hardness * (0.4 + 0.6 * velocity);
        // Felt contact is longer for a soft strike. At high registers the string compliance
        // shortens contact; retaining hardness in that cap avoids a fixed treble attack.
        let seconds = (0.0002 + 0.0024 * (1.0 - hardness).powi(2))
            .min((0.1 + 0.28 * (1.0 - hardness)) / frequency);
        self.contact_frames = (seconds * rate).round().max(2.0) as usize;
        self.age = 0;
        self.coupling = 1.0 - (-0.6 / rate).exp();
        self.active = 0;
        for (index, partial) in self.partials.iter_mut().enumerate() {
            *partial = Partial::default();
            let n = (index + 1) as f32;
            let ratio =
                n * ((1.0 + settings.stiffness * n * n) / (1.0 + settings.stiffness)).sqrt();
            if frequency * ratio * 1.002 >= rate * 0.45 {
                continue;
            }
            self.active = index + 1;
            partial.drive = (PI * n * settings.position).sin() / n * velocity.powf(1.25);
            for (string, cents) in partial.strings.iter_mut().zip(detune).take(self.strings) {
                let angle = TAU * frequency * ratio * (cents / 1200.0_f32).exp2() / rate;
                (string.sine, string.cosine) = angle.sin_cos();
            }
        }
        self.update_loss(rate, settings);
    }

    pub(super) fn update_loss(&mut self, rate: f32, settings: Settings) {
        for (index, partial) in self.partials[..self.active].iter_mut().enumerate() {
            let n = (index + 1) as f32;
            partial.radius = (-6.907_755 * (1.0 + 0.025 * n * n) * (1.0 + 6.0 * settings.damping)
                / (rate * settings.decay))
                .exp();
        }
    }

    pub(super) fn next(&mut self) -> f32 {
        let force = if self.age < self.contact_frames {
            // A raised-cosine contact pulse integrates to unity. It excites the modes over
            // finite time rather than filling their initial amplitudes with an EQ curve.
            let phase = (self.age as f32 + 0.5) / self.contact_frames as f32;
            (1.0 - (TAU * phase).cos()) / self.contact_frames as f32
        } else {
            0.0
        };
        self.age = self.age.saturating_add(1);
        let mut output = 0.0;
        let normalization = 1.0 / self.strings as f32;
        for partial in &mut self.partials[..self.active] {
            if partial.radius == 0.0 {
                continue;
            }
            let strings = &mut partial.strings[..self.strings];
            let mut mean_real = 0.0;
            let mut mean_imag = 0.0;
            let mut strength = 0.0;
            for string in &mut *strings {
                string.imag += partial.drive * force;
                let real =
                    partial.radius * (string.cosine * string.real - string.sine * string.imag);
                string.imag =
                    partial.radius * (string.sine * string.real + string.cosine * string.imag);
                string.real = real;
                mean_real += real;
                mean_imag += string.imag;
                strength += real.abs() + string.imag.abs();
            }
            // Retire modes below -240 dB before coupling creates denormal arithmetic. This
            // threshold is far below PCM resolution even after summing the complete pool.
            if self.age > self.contact_frames && strength < 1e-12 {
                for string in strings {
                    string.real = 0.0;
                    string.imag = 0.0;
                }
                partial.radius = 0.0;
                continue;
            }
            mean_real *= normalization;
            mean_imag *= normalization;
            // The shared bridge contracts relative motion. This convex mixing is passive,
            // leaving the common component while damping differential unison components.
            for string in strings {
                string.real += self.coupling * (mean_real - string.real);
                string.imag += self.coupling * (mean_imag - string.imag);
            }
            output += mean_real * 0.85;
        }
        output
    }

    pub(super) fn energy(&self) -> f32 {
        self.partials[..self.active]
            .iter()
            .flat_map(|partial| &partial.strings[..self.strings])
            .map(|string| string.real.abs() + string.imag.abs())
            .sum()
    }

    pub(super) fn retune(&mut self, ratio: f32) {
        for string in self.partials[..self.active]
            .iter_mut()
            .flat_map(|partial| &mut partial.strings[..self.strings])
        {
            let angle = string.sine.atan2(string.cosine) * ratio;
            if angle >= PI * 0.9 {
                string.real = 0.0;
                string.imag = 0.0;
            }
            (string.sine, string.cosine) = angle.sin_cos();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            hardness: 0.5,
            position: 0.14,
            decay: 4.0,
            damping: 0.12,
            stiffness: 0.0003,
            pickup: 0.0,
        }
    }

    #[test]
    fn contact_duration_and_string_count_follow_the_performance() {
        let mut piano = Piano::default();
        for (hz, strings) in [(55.0, 1), (110.0, 2), (440.0, 3)] {
            piano.excite(hz, 0.3, 48_000.0, settings());
            assert_eq!(piano.strings, strings);
            let soft = piano.contact_frames;
            piano.excite(
                hz,
                0.9,
                48_000.0,
                Settings {
                    hardness: 1.0,
                    ..settings()
                },
            );
            assert!(piano.contact_frames < soft);
        }
        piano.excite(65.4, 0.8, 48_000.0, settings());
        assert!(piano.active > 32);
    }

    #[test]
    fn unison_rotations_are_detuned_and_bridge_coupling_is_passive() {
        let mut piano = Piano::default();
        piano.excite(440.0, 0.8, 48_000.0, settings());
        let first = &piano.partials[0];
        assert!(first.strings[0].sine < first.strings[1].sine);
        assert!(first.strings[1].sine < first.strings[2].sine);
        for _ in 0..piano.contact_frames {
            piano.next();
        }
        let energy = |piano: &Piano| {
            piano.partials[..piano.active]
                .iter()
                .flat_map(|partial| &partial.strings[..piano.strings])
                .map(|string| string.real * string.real + string.imag * string.imag)
                .sum::<f32>()
        };
        let mut previous = energy(&piano);
        for _ in 0..24_000 {
            piano.next();
            let current = energy(&piano);
            assert!(current <= previous + 1e-6);
            previous = current;
        }
    }

    #[test]
    fn inaudible_partials_retire_before_entering_denormal_range() {
        let mut piano = Piano::default();
        piano.excite(
            130.8,
            0.8,
            48_000.0,
            Settings {
                decay: 1.0,
                ..settings()
            },
        );
        for _ in 0..48_000 {
            piano.next();
        }
        assert!(
            piano.partials[..piano.active]
                .iter()
                .any(|partial| partial.radius == 0.0)
        );
        assert!(piano.partials[0].radius > 0.0);
        for partial in &piano.partials[..piano.active] {
            for string in &partial.strings {
                assert!(string.real == 0.0 || string.real.abs() > f32::MIN_POSITIVE);
                assert!(string.imag == 0.0 || string.imag.abs() > f32::MIN_POSITIVE);
            }
        }
    }
}
