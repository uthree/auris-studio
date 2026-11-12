//! Eight cylindrical sections joined by passive pressure-wave scattering.

const SECTIONS: usize = 8;
const DELAY_CAPACITY: usize = 64;
const SPEED_OF_SOUND: f32 = 343.0;

// Area profiles in square centimetres, ordered from glottis to lips: /u/, /a/, /i/.
// Bounded fitting to real sustained vowels, sung through the self-oscillating folds: see
// tools/eval/references/choir-mel-calibration.json. No recordings enter the instrument.
pub(super) const AREAS: [[f32; SECTIONS]; 3] = [
    [
        4.0, 7.646_59, 4.079791, 1.436472, 2.420324, 2.747955, 3.139518, 0.847230,
    ],
    [
        0.55, 0.601855, 0.466565, 2.362_71, 6.715187, 1.722294, 3.405806, 7.290531,
    ],
    [
        4.5, 6.989215, 6.374314, 3.012531, 1.224_3, 0.693821, 1.252482, 2.004086,
    ],
];
/// Cross-section of the tube just above the folds, square centimetres. The epilarynx is short
/// and narrow in every vowel, and its inertance is what lets the folds start against a wide
/// pharynx, so it is a tube of its own in front of the eight the vowels shape.
const EPILARYNX: f32 = 0.5;
/// Length of the epilarynx, metres; the vowel's sections share what remains of the tract.
const EPILARYNX_LENGTH: f32 = 0.02;

pub(super) const RADIATION_HZ: f32 = 198.2515;
// Every vowel as loud as "ah" was before the folds, from C3 to G4, so that automating the
// vowel does not ride the level; set by measurement, independently of mel loss.
pub(super) const OUTPUT_GAINS: [f32; 3] = [1.445, 0.933, 1.096];

/// First resonance of the vowel profiles in a 17 cm tract, at vowel 0, ½, 1, 1½ and 2.
/// Measured from the tract's impulse response; a test holds these to the profiles.
pub(super) const FIRST_FORMANT: [f32; 5] = [360.0, 565.0, 730.0, 495.0, 310.0];
const FORMANT_LENGTH: f32 = 0.17;
/// How far above the note a singer keeps the first resonance. Below it the tract's pressure
/// is in phase with the flow and helps the folds; past it the tract wins and they stall.
pub(super) const FORMANT_MARGIN: f32 = 1.1;
/// The most a singer shortens the tract — lips spread, jaw dropped — to keep that margin.
const SHORTEST: f32 = 0.7;

/// First resonance of `vowel` in a tract `length` metres long.
pub(super) fn first_formant(vowel: f32, length: f32) -> f32 {
    let at = (vowel.clamp(0.0, 2.0) * 2.0).min(3.999);
    let low = at as usize;
    let blend = at - low as f32;
    let hz = FIRST_FORMANT[low] + (FIRST_FORMANT[low + 1] - FIRST_FORMANT[low]) * blend;
    hz * FORMANT_LENGTH / length
}

/// The vowel and length a singer actually sings `hertz` with: opened towards "ah", and then
/// shortened, until the first resonance clears the note — what sopranos do on top notes.
pub(super) fn tuned(vowel: f32, length: f32, hertz: f32) -> (f32, f32) {
    let need = FORMANT_MARGIN * hertz;
    if first_formant(vowel, length) >= need {
        return (vowel, length);
    }
    // Walk towards "ah" (vowel 1), where the first resonance is highest.
    let steps = 32;
    for step in 1..=steps {
        let candidate = vowel + (1.0 - vowel) * step as f32 / steps as f32;
        if first_formant(candidate, length) >= need {
            return (candidate, length);
        }
    }
    let shortened = (length * first_formant(1.0, length) / need).max(length * SHORTEST);
    (1.0, shortened)
}

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
    epilarynx_forward: Delay,
    epilarynx_backward: Delay,
    epilarynx_delay: f32,
    reflections: [f32; SECTIONS - 1],
    targets: [f32; SECTIONS - 1],
    delay: f32,
    target_delay: f32,
    smoothing: f32,
    loss: f32,
    /// Cross-section of the first vowel section, square centimetres, where the epilarynx opens.
    glottal_area: f32,
    target_glottal_area: f32,
    output_gain: f32,
    target_output_gain: f32,
    radiation_memory: f32,
    radiation_output: f32,
    radiation_pole: f32,
    /// The profile being sung, glottis to lips, kept for the display.
    areas: [f32; SECTIONS],
}

impl Tract {
    pub(super) fn new(sample_rate: f32) -> Self {
        Self {
            forward: std::array::from_fn(|_| Delay::new()),
            backward: std::array::from_fn(|_| Delay::new()),
            epilarynx_forward: Delay::new(),
            epilarynx_backward: Delay::new(),
            epilarynx_delay: (EPILARYNX_LENGTH * sample_rate / SPEED_OF_SOUND).max(1.0),
            reflections: [0.0; SECTIONS - 1],
            targets: [0.0; SECTIONS - 1],
            delay: 1.0,
            target_delay: 1.0,
            smoothing: 1.0 - (-1.0 / (0.02 * sample_rate)).exp(),
            loss: (-35.0 / sample_rate).exp(),
            glottal_area: 1.0,
            target_glottal_area: 1.0,
            output_gain: 1.0,
            target_output_gain: 1.0,
            radiation_memory: 0.0,
            radiation_output: 0.0,
            radiation_pole: (-std::f32::consts::TAU * RADIATION_HZ / sample_rate).exp(),
            areas: AREAS[1],
        }
    }

    #[cfg(any(not(feature = "choir-calibration"), test))]
    pub(super) fn configure(&mut self, vowel: f32, length: f32, sample_rate: f32) {
        self.configure_profile(
            vowel,
            length,
            sample_rate,
            &AREAS,
            RADIATION_HZ,
            &OUTPUT_GAINS,
        );
    }

    pub(super) fn configure_profile(
        &mut self,
        vowel: f32,
        length: f32,
        sample_rate: f32,
        profiles: &[[f32; SECTIONS]; 3],
        radiation_hz: f32,
        output_gains: &[f32; 3],
    ) {
        let first = (vowel as usize).min(1);
        let blend = (vowel - first as f32).clamp(0.0, 1.0);
        let areas: [f32; SECTIONS] = std::array::from_fn(|i| {
            profiles[first][i] + (profiles[first + 1][i] - profiles[first][i]) * blend
        });
        self.radiation_pole = (-std::f32::consts::TAU * radiation_hz / sample_rate).exp();
        self.areas = areas;
        for (target, pair) in self.targets.iter_mut().zip(areas.windows(2)) {
            *target = (pair[0] - pair[1]) / (pair[0] + pair[1]);
        }
        // The folds feed volume flow into the impedance of the first tube, whereas the delay
        // lines carry pressure; the mouth is heard as radiated flow again. Reading lip pressure
        // directly would make a narrow /u/ aperture much louder than /a/.
        self.target_glottal_area = areas[0];
        let normalization =
            output_gains[first] + (output_gains[first + 1] - output_gains[first]) * blend;
        self.target_output_gain = areas[SECTIONS - 1] * normalization;
        self.target_delay = ((length - EPILARYNX_LENGTH) * sample_rate
            / (SPEED_OF_SOUND * SECTIONS as f32))
            .clamp(1.0, (DELAY_CAPACITY - 2) as f32);
    }

    pub(super) fn reset(&mut self) {
        for delay in self.forward.iter_mut().chain(&mut self.backward) {
            delay.reset();
        }
        self.epilarynx_forward.reset();
        self.epilarynx_backward.reset();
        self.reflections = self.targets;
        self.delay = self.target_delay;
        self.glottal_area = self.target_glottal_area;
        self.output_gain = self.target_output_gain;
        self.radiation_memory = 0.0;
        self.radiation_output = 0.0;
    }

    /// Pressure along the tract, glottis first, and each tube's cross-section relative to the
    /// widest. Both waves are read where they are inside each tube: the one travelling to the
    /// lips has come `s` of the way through it, the one travelling back `1 − s`.
    pub(super) fn motion(&self, points: &mut [f32], areas: &mut [f32]) {
        let last = points.len().saturating_sub(1).max(1) as f32;
        let span = (self.delay - 1.0).max(0.0);
        for (index, point) in points.iter_mut().enumerate() {
            let along = index as f32 / last * SECTIONS as f32;
            let section = (along as usize).min(SECTIONS - 1);
            let s = (along - section as f32).clamp(0.0, 1.0);
            *point = self.forward[section].read(1.0 + s * span)
                + self.backward[section].read(1.0 + (1.0 - s) * span);
        }
        let widest = self.areas.iter().fold(0.0_f32, |a, b| a.max(*b)).max(1e-6);
        for (out, area) in areas.iter_mut().zip(self.areas) {
            *out = area / widest;
        }
    }

    /// The pressure wave arriving back at the glottis, which the folds' flow is solved against.
    pub(super) fn incoming(&self) -> f32 {
        self.epilarynx_backward.read(self.epilarynx_delay)
    }

    /// Acoustic impedance of the first tube, in SI units.
    pub(super) fn glottal_impedance(&self) -> f64 {
        super::folds::impedance(f64::from(EPILARYNX) * 1e-4)
    }

    /// Advances one sample. `glottal` is the pressure wave leaving the glottis: the incoming
    /// wave reflected from the closed end, plus what the folds' flow adds to it.
    pub(super) fn next(&mut self, glottal: f32) -> f32 {
        self.delay += (self.target_delay - self.delay) * self.smoothing;
        self.glottal_area += (self.target_glottal_area - self.glottal_area) * self.smoothing;
        self.output_gain += (self.target_output_gain - self.output_gain) * self.smoothing;
        let right: [f32; SECTIONS] = std::array::from_fn(|i| self.forward[i].read(self.delay));
        let left: [f32; SECTIONS] = std::array::from_fn(|i| self.backward[i].read(self.delay));
        let rising = self.epilarynx_forward.read(self.epilarynx_delay);
        self.epilarynx_forward.push(glottal);
        let into = (EPILARYNX - self.glottal_area) / (EPILARYNX + self.glottal_area);
        let (forward, backward) = scatter(rising, left[0], into);
        self.forward[0].push(forward * self.loss);
        self.epilarynx_backward.push(backward * self.loss);
        // Open lips reflect with the opposite sign, and lose energy to the room.
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
                .map(|i| {
                    let back = tract.incoming();
                    tract.next(if i == 0 { 1.0 } else { 0.75 * back })
                })
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
