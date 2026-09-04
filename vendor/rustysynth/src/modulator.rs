#![allow(dead_code)]

//! Modulators: the part of a SoundFont that says how a controller reaches a generator.
//!
//! Added by the Auris fork. Upstream reads the `pmod` and `imod` chunks and throws them away, so a
//! font that shapes a sound with modulators is played without that shaping. See the crate README
//! for what is and is not implemented here.

use std::io::Read;

use crate::{binary_reader::BinaryReader, error::SoundFontError};

/// The general controller a modulator can be driven by, when it is not a MIDI continuous
/// controller. These are the values of the SoundFont specification's own palette.
const SOURCE_NONE: u16 = 0;
const SOURCE_VELOCITY: u16 = 2;
const SOURCE_KEY: u16 = 3;

/// `sfModTransOper` values.
const TRANSFORM_LINEAR: u16 = 0;
const TRANSFORM_ABSOLUTE: u16 = 2;

/// One entry of a zone's modulator list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct Modulator {
    /// What drives it, packed as the specification's `sfModSrcOper`.
    pub(crate) source: u16,
    /// Which generator it reaches.
    pub(crate) destination: u16,
    /// How far the destination moves when the source is at full scale, in that generator's units.
    pub(crate) amount: i16,
    /// A second controller scaling `amount`, packed like [`Self::source`].
    pub(crate) amount_source: u16,
    /// What is done to the result before it is added.
    pub(crate) transform: u16,
}

impl Modulator {
    fn new<R: Read>(reader: &mut R) -> Result<Self, SoundFontError> {
        Ok(Self {
            source: BinaryReader::read_u16(reader)?,
            destination: BinaryReader::read_u16(reader)?,
            amount: BinaryReader::read_i16(reader)?,
            amount_source: BinaryReader::read_u16(reader)?,
            transform: BinaryReader::read_u16(reader)?,
        })
    }

    pub(crate) fn read_from_chunk<R: Read>(
        reader: &mut R,
        size: usize,
    ) -> Result<Vec<Modulator>, SoundFontError> {
        if size % 10 != 0 {
            return Err(SoundFontError::InvalidModulatorList);
        }

        // A list of nothing but the terminator is a legal empty list.
        let count = size / 10;
        let mut modulators: Vec<Modulator> = Vec::new();
        for i in 0..count {
            let modulator = Modulator::new(reader)?;
            // The last one is the terminator and carries no meaning.
            if i + 1 < count {
                modulators.push(modulator);
            }
        }

        Ok(modulators)
    }

    /// Whether two modulators address the same thing, which is what makes one replace the other.
    ///
    /// The specification's rule for a local zone overriding a global one: same source, same
    /// destination, same amount source and same transform is the *same* modulator, whatever its
    /// amount, and a zone that declares one twice means the second.
    pub(crate) fn addresses_the_same(&self, other: &Modulator) -> bool {
        self.source == other.source
            && self.destination == other.destination
            && self.amount_source == other.amount_source
            && self.transform == other.transform
    }

    /// How far this modulator moves its destination for a note of `key` struck at `velocity`, in
    /// the destination generator's own units.
    ///
    /// `None` for a modulator this fork does not model — anything driven by a continuous
    /// controller, the pitch wheel or aftertouch, or carrying a transform that is not a plain
    /// one. Those are the sources whose value changes while a note sounds, and a voice here reads
    /// its modulators once at note-on; answering with a frozen value would be worse than not
    /// answering at all, because it would look like support.
    pub(crate) fn contribution(&self, key: i32, velocity: i32) -> Option<f32> {
        let source = Self::source_value(self.source, key, velocity)?;
        let scale = Self::source_value(self.amount_source, key, velocity)?;
        let value = source * scale * self.amount as f32;
        match self.transform {
            TRANSFORM_LINEAR => Some(value),
            TRANSFORM_ABSOLUTE => Some(value.abs()),
            _ => None,
        }
    }

    /// What a controller reads, from 0 to 1 unipolar or -1 to 1 bipolar.
    fn source_value(spec: u16, key: i32, velocity: i32) -> Option<f32> {
        let is_continuous_controller = spec & 0x80 != 0;
        if is_continuous_controller {
            return None;
        }
        // The SoundFont "no controller" source is the constant 1.0; its shape flags do not
        // turn it into a decreasing or bipolar controller because there is no controller value
        // to transform.
        if spec & 0x7F == SOURCE_NONE {
            return Some(1.0);
        }
        let decreasing = spec & 0x100 != 0;
        let bipolar = spec & 0x200 != 0;
        let curve = spec >> 10;
        let raw = match spec & 0x7F {
            SOURCE_VELOCITY => velocity,
            SOURCE_KEY => key,
            _ => return None,
        };

        let x = f32::from((raw.clamp(0, 127)) as u8) / 127.0;
        let x = if decreasing { 1.0 - x } else { x };
        let unipolar = match curve {
            0 => x,
            1 => concave(x),
            2 => convex(x),
            3 => f32::from(x >= 0.5),
            _ => return None,
        };
        Some(match bipolar {
            true => 2.0 * unipolar - 1.0,
            false => unipolar,
        })
    }
}

/// The specification's concave curve: slow to leave zero, steep at the top.
fn concave(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    (-(20.0 / 96.0) * ((1.0 - x) * (1.0 - x)).log10()).clamp(0.0, 1.0)
}

/// The concave curve read backwards: quick to rise, then flat.
fn convex(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    (1.0 + (20.0 / 96.0) * (x * x).log10()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source specification, packed the way a font stores one.
    fn source(index: u16, decreasing: bool, bipolar: bool, curve: u16) -> u16 {
        index | (u16::from(decreasing) << 8) | (u16::from(bipolar) << 9) | (curve << 10)
    }

    #[test]
    fn a_linear_velocity_source_reads_the_velocity() {
        let m = Modulator {
            source: source(SOURCE_VELOCITY, false, false, 0),
            destination: 8,
            amount: 1200,
            amount_source: 0,
            transform: TRANSFORM_LINEAR,
        };
        assert_eq!(m.contribution(60, 127), Some(1200.0));
        assert_eq!(m.contribution(60, 0), Some(0.0));
        let half = m.contribution(60, 64).expect("velocity is modelled");
        assert!((half - 604.7).abs() < 0.1, "{half}");
    }

    #[test]
    fn a_bipolar_source_reads_from_the_middle() {
        let m = Modulator {
            source: source(SOURCE_KEY, false, true, 0),
            destination: 8,
            amount: 1200,
            amount_source: 0,
            transform: TRANSFORM_LINEAR,
        };
        assert_eq!(m.contribution(127, 100), Some(1200.0));
        assert_eq!(m.contribution(0, 100), Some(-1200.0));
    }

    #[test]
    fn no_controller_is_a_constant() {
        let m = Modulator {
            source: source(SOURCE_NONE, false, false, 0),
            destination: 48,
            amount: -100,
            amount_source: 0,
            transform: TRANSFORM_LINEAR,
        };
        assert_eq!(m.contribution(0, 1), Some(-100.0));
        assert_eq!(m.contribution(127, 127), Some(-100.0));
    }

    #[test]
    fn a_source_this_fork_does_not_model_answers_nothing() {
        // A continuous controller, which changes while the note is held.
        let m = Modulator {
            source: 0x80 | 1,
            destination: 8,
            amount: 1000,
            amount_source: 0,
            transform: TRANSFORM_LINEAR,
        };
        assert_eq!(m.contribution(60, 100), None);
    }

    #[test]
    fn the_curves_run_from_nothing_to_everything() {
        for curve in [concave, convex] {
            assert_eq!(curve(0.0), 0.0);
            assert_eq!(curve(1.0), 1.0);
            assert!(curve(0.5) > 0.0 && curve(0.5) < 1.0);
        }
        // Concave is the slow one at the bottom and convex the quick one, which is the whole
        // difference between them.
        assert!(concave(0.5) < 0.5);
        assert!(convex(0.5) > 0.5);
    }
}
