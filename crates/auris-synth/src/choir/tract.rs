//! Eight cylindrical sections joined by passive pressure-wave scattering.

const SECTIONS: usize = 8;
const DELAY_CAPACITY: usize = 64;
const SPEED_OF_SOUND: f32 = 343.0;

// Designed area profiles in square centimetres, ordered from glottis to lips.
// They describe rounded /u/, open /a/ and front /i/ tube configurations.
const AREAS: [[f32; SECTIONS]; 3] = [
    [4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 0.3],
    [0.55, 0.6, 0.75, 1.0, 3.2, 4.8, 5.4, 4.0],
    [4.5, 4.5, 4.5, 4.5, 0.5, 0.5, 0.5, 1.5],
];

fn scatter(right: f32, left: f32, reflection: f32) -> (f32, f32) {
    let scattered = reflection * (right - left);
    (right + scattered, left + scattered)
}

struct Delay {
    samples: [f32; DELAY_CAPACITY],
    write: usize,
}

impl Delay {
    fn new() -> Self {
        Self {
            samples: [0.0; DELAY_CAPACITY],
            write: 0,
        }
    }

    fn read(&self, delay: f32) -> f32 {
        let whole = delay as usize;
        let index = (self.write + DELAY_CAPACITY - whole) % DELAY_CAPACITY;
        let older = (index + DELAY_CAPACITY - 1) % DELAY_CAPACITY;
        self.samples[index] + (self.samples[older] - self.samples[index]) * delay.fract()
    }

    fn push(&mut self, value: f32) {
        self.samples[self.write] = value;
        self.write = (self.write + 1) % DELAY_CAPACITY;
    }

    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.write = 0;
    }
}

pub(super) struct Tract {
    forward: [Delay; SECTIONS],
    backward: [Delay; SECTIONS],
    reflections: [f32; SECTIONS - 1],
    targets: [f32; SECTIONS - 1],
    delay: f32,
    target_delay: f32,
    smoothing: f32,
    loss: f32,
    input_gain: f32,
    target_input_gain: f32,
    output_gain: f32,
    target_output_gain: f32,
    radiation_memory: f32,
    radiation_output: f32,
    radiation_pole: f32,
}

impl Tract {
    pub(super) fn new(sample_rate: f32) -> Self {
        Self {
            forward: std::array::from_fn(|_| Delay::new()),
            backward: std::array::from_fn(|_| Delay::new()),
            reflections: [0.0; SECTIONS - 1],
            targets: [0.0; SECTIONS - 1],
            delay: 1.0,
            target_delay: 1.0,
            smoothing: 1.0 - (-1.0 / (0.02 * sample_rate)).exp(),
            loss: (-35.0 / sample_rate).exp(),
            input_gain: 1.0,
            target_input_gain: 1.0,
            output_gain: 1.0,
            target_output_gain: 1.0,
            radiation_memory: 0.0,
            radiation_output: 0.0,
            radiation_pole: (-std::f32::consts::TAU * 80.0 / sample_rate).exp(),
        }
    }

    pub(super) fn configure(&mut self, vowel: f32, length: f32, sample_rate: f32) {
        let first = (vowel as usize).min(1);
        let blend = (vowel - first as f32).clamp(0.0, 1.0);
        let areas: [f32; SECTIONS] = std::array::from_fn(|i| {
            AREAS[first][i] + (AREAS[first + 1][i] - AREAS[first][i]) * blend
        });
        for (target, pair) in self.targets.iter_mut().zip(areas.windows(2)) {
            *target = (pair[0] - pair[1]) / (pair[0] + pair[1]);
        }
        // The source prescribes volume flow, whereas the delay lines carry pressure.
        // Convert at the glottis and back to radiated flow at the mouth. Reading lip
        // pressure directly would make a narrow /u/ aperture much louder than /a/.
        self.target_input_gain = 1.0 / areas[0];
        self.target_output_gain = areas[SECTIONS - 1];
        self.target_delay = (length * sample_rate / (SPEED_OF_SOUND * SECTIONS as f32))
            .clamp(1.0, (DELAY_CAPACITY - 2) as f32);
    }

    pub(super) fn reset(&mut self) {
        for delay in self.forward.iter_mut().chain(&mut self.backward) {
            delay.reset();
        }
        self.reflections = self.targets;
        self.delay = self.target_delay;
        self.input_gain = self.target_input_gain;
        self.output_gain = self.target_output_gain;
        self.radiation_memory = 0.0;
        self.radiation_output = 0.0;
    }

    pub(super) fn next(&mut self, excitation: f32) -> f32 {
        self.delay += (self.target_delay - self.delay) * self.smoothing;
        self.input_gain += (self.target_input_gain - self.input_gain) * self.smoothing;
        self.output_gain += (self.target_output_gain - self.output_gain) * self.smoothing;
        let right: [f32; SECTIONS] = std::array::from_fn(|i| self.forward[i].read(self.delay));
        let left: [f32; SECTIONS] = std::array::from_fn(|i| self.backward[i].read(self.delay));
        // Closed glottis and open lips reflect with opposite signs. Both lose energy.
        self.forward[0].push(excitation * self.input_gain + 0.75 * left[0]);
        self.backward[SECTIONS - 1].push(-0.85 * right[SECTIONS - 1]);
        for (i, reflection) in self.reflections.iter_mut().enumerate() {
            *reflection += (self.targets[i] - *reflection) * self.smoothing;
            let (forward, backward) = scatter(right[i], left[i + 1], *reflection);
            self.forward[i + 1].push(forward * self.loss);
            self.backward[i].push(backward * self.loss);
        }
        let lip = right[SECTIONS - 1] * self.output_gain;
        // A first-order lip-radiation/DC-rejection approximation.
        self.radiation_output =
            lip - self.radiation_memory + self.radiation_pole * self.radiation_output;
        self.radiation_memory = lip;
        self.radiation_output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::goertzel;

    #[test]
    fn a_pressure_junction_preserves_area_weighted_energy() {
        for (a, b, right, left) in [(0.3_f32, 4.0_f32, 0.7_f32, -0.2_f32), (5.0, 0.5, -0.3, 0.9)] {
            let reflection = (a - b) / (a + b);
            let (transmitted, reflected) = scatter(right, left, reflection);
            let incoming = a * right.powi(2) + b * left.powi(2);
            let outgoing = b * transmitted.powi(2) + a * reflected.powi(2);
            assert!((incoming - outgoing).abs() < 1e-6);
        }
    }

    #[test]
    fn a_uniform_tube_resonates_at_its_physical_quarter_wave_frequency() {
        for length in [0.14, 0.17, 0.205] {
            let mut tract = Tract::new(48_000.0);
            tract.configure(1.0, length, 48_000.0);
            tract.targets.fill(0.0);
            tract.reset();
            let impulse: Vec<_> = (0..8_192)
                .map(|i| tract.next(if i == 0 { 1.0 } else { 0.0 }))
                .collect();
            let peak = (350..700)
                .step_by(2)
                .max_by(|a, b| {
                    goertzel(&impulse, 48_000.0, f64::from(*a)).total_cmp(&goertzel(
                        &impulse,
                        48_000.0,
                        f64::from(*b),
                    ))
                })
                .unwrap();
            let expected = SPEED_OF_SOUND / (4.0 * length);
            assert!(
                (peak as f32 / expected - 1.0).abs() < 0.02,
                "length={length}, peak={peak}, expected={expected}"
            );
        }
    }
}
