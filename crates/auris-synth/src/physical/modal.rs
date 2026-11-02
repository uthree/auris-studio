//! Reduced modal models of stiff strings, bars and bells.

use std::f32::consts::{PI, TAU};

use super::{Model, Quality, Settings};

#[path = "piano.rs"]
pub(super) mod piano;
use piano::Piano;
pub(super) use piano::{PianoVoicing, VOICING};

const MODES: usize = 32;
/// Modes a struck bar or bell keeps at [`Quality::Draft`].
const DRAFT_MODES: usize = 8;
/// Middle C, where the tuned bar's fundamental decays in exactly the Resonance Decay time.
const MALLET_REFERENCE_HZ: f32 = 261.63;

#[derive(Clone, Copy, Debug, Default)]
struct Mode {
    real: f32,
    imag: f32,
    cosine: f32,
    sine: f32,
    radius: f32,
    pickup: f32,
    ratio: f32,
}

/// A damped modal expansion, excited by an initial displacement or velocity.
#[derive(Clone, Debug)]
pub(super) struct Modal {
    modes: [Mode; MODES],
    fundamental: f32,
    piano: Option<Box<Piano>>,
}

impl Default for Modal {
    fn default() -> Self {
        Self {
            modes: [Mode::default(); MODES],
            fundamental: MALLET_REFERENCE_HZ,
            piano: None,
        }
    }
}

impl Modal {
    pub(super) fn motion(&self, points: &mut [f32], modes: &mut [f32], model: Model) -> f32 {
        if let Some(piano) = &self.piano {
            piano.motion(points, modes);
            return piano.contact();
        }
        let last = points.len().saturating_sub(1).max(1) as f32;
        for (index, point) in points.iter_mut().enumerate() {
            let x = index as f32 / last;
            *point = self
                .modes
                .iter()
                .take(16)
                .enumerate()
                .map(|(n, mode)| {
                    let basis = if model == Model::Bell {
                        (TAU * (n + 1) as f32 * x).cos()
                    } else {
                        (PI * (n as f32 + 0.5) * x).cos()
                    };
                    mode.real * basis
                })
                .sum();
        }
        for (mode, magnitude) in self.modes.iter().zip(modes) {
            *magnitude = mode.real.hypot(mode.imag);
        }
        0.0
    }
    pub(super) fn prepare(&mut self, model: Model, voicing: PianoVoicing) {
        self.piano = (model == Model::Piano).then(|| {
            let mut piano = Box::new(Piano::default());
            piano.set_voicing(voicing);
            piano
        });
    }

    #[cfg(feature = "piano-calibration")]
    pub(super) fn set_voicing(&mut self, voicing: PianoVoicing) {
        if let Some(piano) = &mut self.piano {
            piano.set_voicing(voicing);
        }
    }

    /// Lowers a piano damper; `false` when no damper stops this note.
    pub(super) fn damp(&mut self, rate: f32, settings: Settings) -> bool {
        self.piano
            .as_mut()
            .is_some_and(|piano| piano.damp(rate, settings))
    }

    /// Lifts a lowered piano damper, letting a decaying note ring on.
    pub(super) fn lift(&mut self, rate: f32, settings: Settings) {
        if let Some(piano) = &mut self.piano {
            piano.lift(rate, settings);
        }
    }

    pub(super) fn is_damped(&self) -> bool {
        self.piano.as_ref().is_some_and(|piano| piano.is_damped())
    }

    /// Frames a struck piano note may sound before its voice is released.
    pub(super) fn lifetime(&self, rate: f32, settings: Settings) -> Option<usize> {
        self.piano
            .as_ref()
            .map(|piano| piano.lifetime(rate, settings))
    }

    pub(super) fn excite(
        &mut self,
        model: Model,
        frequency: f32,
        velocity: f32,
        rate: f32,
        quality: Quality,
        settings: Settings,
    ) {
        if let Some(piano) = &mut self.piano {
            piano.excite(frequency, velocity, rate, quality, settings);
            return;
        }
        let count = if quality == Quality::Draft {
            DRAFT_MODES
        } else {
            MODES
        };
        for (index, mode) in self.modes.iter_mut().enumerate() {
            let n = (index + 1) as f32;
            let ratio = match model {
                Model::Bell => {
                    const RATIOS: [f32; 8] = [1.0, 2.01, 2.74, 3.01, 4.07, 5.43, 6.79, 8.21];
                    RATIOS.get(index).copied().unwrap_or(0.0)
                }
                // A marimba bar is undercut until its first overtones sit two octaves, and three
                // octaves and a major third, above the fundamental: 4.00 and 9.9 times it on
                // every Iowa marimba note. Higher modes keep a free bar's quadratic spacing.
                Model::Mallet => match index {
                    0 => 1.0,
                    1 => 4.0,
                    2 => 9.9,
                    _ => 9.9 * ((n + 0.5) / 3.5).powi(2),
                },
                _ => n,
            };
            let hz = frequency * ratio;
            *mode = Mode::default();
            if ratio == 0.0 || index >= count || hz >= rate * 0.45 {
                continue;
            }
            mode.ratio = ratio;
            let angle = TAU * hz / rate;
            (mode.sine, mode.cosine) = angle.sin_cos();
            let hardness = (settings.hardness * (0.4 + 0.6 * velocity)).max(0.03);
            let contact = (PI * n * settings.position).sin();
            let amplitude = contact / (n * (1.0 + (n / (2.0 + 22.0 * hardness)).powi(2)));
            // A strike gives the string velocity. Mallet/bell contact is wider and therefore
            // progressively rejects short-wavelength modes as the beater becomes softer.
            mode.imag = amplitude * velocity;
            mode.pickup = match model {
                Model::Bell => 0.8,
                _ => 0.8 / n.sqrt(),
            };
        }
        self.fundamental = frequency;
        self.update_loss(model, rate, settings);
    }

    pub(super) fn update_loss(&mut self, model: Model, rate: f32, settings: Settings) {
        if let Some(piano) = &mut self.piano {
            piano.update_loss(rate, settings);
            return;
        }
        // Shorter, thinner bars and higher modes lose energy faster: on the Iowa marimba both
        // follow about the 0.6th power of frequency, 25 dB/s at C3 to 55 dB/s at B4.
        let register = (self.fundamental / MALLET_REFERENCE_HZ).max(0.01).powf(0.6);
        for (index, mode) in self.modes.iter_mut().enumerate() {
            let n = (index + 1) as f32;
            let loss = match model {
                Model::Bell => 1.0 + n * 0.09,
                Model::Mallet => register * mode.ratio.max(1.0).powf(0.6),
                _ => 1.0 + n * n * 0.025,
            };
            mode.radius = (-6.907_755 * loss * (1.0 + 6.0 * settings.damping)
                / (settings.decay * rate))
                .exp();
        }
    }

    pub(super) fn next(&mut self) -> f32 {
        if let Some(piano) = &mut self.piano {
            return piano.next();
        }
        let mut output = 0.0;
        for mode in &mut self.modes {
            let real = mode.radius * (mode.cosine * mode.real - mode.sine * mode.imag);
            mode.imag = mode.radius * (mode.sine * mode.real + mode.cosine * mode.imag);
            mode.real = real;
            output += real * mode.pickup;
        }
        output
    }

    /// The piano's soundboard knock for the last sample; it bypasses string radiation.
    pub(super) fn thump(&self) -> f32 {
        self.piano.as_ref().map_or(0.0, |piano| piano.thump())
    }

    pub(super) fn energy(&self) -> f32 {
        if let Some(piano) = &self.piano {
            return piano.energy();
        }
        self.modes
            .iter()
            .map(|mode| mode.real.abs() + mode.imag.abs())
            .sum()
    }

    pub(super) fn retune(&mut self, ratio: f32) {
        if let Some(piano) = &mut self.piano {
            piano.retune(ratio);
            return;
        }
        for mode in &mut self.modes {
            let angle = mode.sine.atan2(mode.cosine) * ratio;
            // Silence modes that a bend moved above the safe band instead of folding them.
            if angle >= PI * 0.9 {
                mode.real = 0.0;
                mode.imag = 0.0;
            }
            (mode.sine, mode.cosine) = angle.sin_cos();
        }
    }
}
