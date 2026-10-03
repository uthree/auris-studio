//! Band-limited, prescribed glottal flow for the vocal-tract input.

use std::f32::consts::{PI, TAU};

const TABLE_SIZE: usize = 1_024;
const MAX_HARMONICS: usize = 64;

pub(super) struct GlottalTables {
    tables: Vec<[f32; TABLE_SIZE]>,
}

impl GlottalTables {
    pub(super) fn new() -> Self {
        Self { tables: Vec::new() }
    }

    pub(super) fn prepare(&mut self) {
        if !self.tables.is_empty() {
            return;
        }
        // A Rosenberg-style pulse: gradual opening, faster closure, then a closed interval.
        // Fourier reconstruction removes harmonics above the selected table's cutoff.
        let pulse: [f32; TABLE_SIZE] = std::array::from_fn(|i| {
            let phase = i as f32 / TABLE_SIZE as f32;
            if phase < 0.45 {
                0.5 * (1.0 - (PI * phase / 0.45).cos())
            } else if phase < 0.63 {
                (0.5 * PI * (phase - 0.45) / 0.18).cos()
            } else {
                0.0
            }
        });
        let coefficients: [(f32, f32); MAX_HARMONICS] = std::array::from_fn(|h| {
            let mut cosine = 0.0;
            let mut sine = 0.0;
            for (i, value) in pulse.iter().enumerate() {
                let angle = TAU * (h + 1) as f32 * i as f32 / TABLE_SIZE as f32;
                cosine += value * angle.cos();
                sine += value * angle.sin();
            }
            let scale = 2.0 / TABLE_SIZE as f32;
            (cosine * scale, sine * scale)
        });
        for count in [1, 2, 4, 8, 16, 32, 64] {
            self.tables.push(std::array::from_fn(|i| {
                let phase = TAU * i as f32 / TABLE_SIZE as f32;
                coefficients[..count]
                    .iter()
                    .enumerate()
                    .map(|(h, (cosine, sine))| {
                        let angle = phase * (h + 1) as f32;
                        cosine * angle.cos() + sine * angle.sin()
                    })
                    .sum()
            }));
        }
    }

    pub(super) fn sample(&self, phase: f32, frequency: f32, sample_rate: f32) -> f32 {
        // Leave room for interpolation images; selecting fewer harmonics at high pitches
        // preserves the played fundamental rather than aliasing the glottal closure.
        let available = (sample_rate * 0.45 / frequency.max(1.0)).max(1.0);
        let index = (available.log2().floor() as usize).min(6);
        let Some(table) = self.tables.get(index) else {
            return 0.0;
        };
        let position = phase * TABLE_SIZE as f32;
        let first = position as usize % TABLE_SIZE;
        let fraction = position.fract();
        table[first] + (table[(first + 1) % TABLE_SIZE] - table[first]) * fraction
    }
}
