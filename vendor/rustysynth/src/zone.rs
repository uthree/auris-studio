use crate::error::SoundFontError;
use crate::generator::Generator;
use crate::modulator::Modulator;
use crate::zone_info::ZoneInfo;

#[non_exhaustive]
pub(crate) struct Zone {
    pub(crate) generators: Vec<Generator>,
    /// Added by the Auris fork: upstream discards these at parse time.
    pub(crate) modulators: Vec<Modulator>,
}

impl Zone {
    pub(crate) fn empty() -> Self {
        Self {
            generators: Vec::new(),
            modulators: Vec::new(),
        }
    }

    fn new(
        info: &ZoneInfo,
        generators: &[Generator],
        modulators: &[Modulator],
    ) -> Result<Self, SoundFontError> {
        let generator_start = usize::try_from(info.generator_index)
            .map_err(|_| SoundFontError::InvalidGeneratorList)?;
        let generator_count = usize::try_from(info.generator_count)
            .map_err(|_| SoundFontError::InvalidGeneratorList)?;
        let generator_end = generator_start
            .checked_add(generator_count)
            .ok_or(SoundFontError::InvalidGeneratorList)?;
        let segment = generators
            .get(generator_start..generator_end)
            .ok_or(SoundFontError::InvalidGeneratorList)?
            .to_vec();

        // A bag may name more modulators than the chunk holds, which is a broken file rather than
        // a reason to refuse one: what is there is taken and the rest is left alone.
        let mut modulator_segment: Vec<Modulator> = Vec::new();
        for i in 0..info.modulator_count {
            if let Some(modulator) = modulators.get((info.modulator_index + i) as usize) {
                modulator_segment.push(*modulator);
            }
        }

        Ok(Self {
            generators: segment,
            modulators: modulator_segment,
        })
    }

    pub(crate) fn create(
        infos: &[ZoneInfo],
        generators: &[Generator],
        modulators: &[Modulator],
    ) -> Result<Vec<Zone>, SoundFontError> {
        if infos.len() <= 1 {
            return Err(SoundFontError::ZoneNotFound);
        }

        // The last one is the terminator.
        let count = infos.len() - 1;

        let mut zones: Vec<Zone> = Vec::new();
        for info in infos.iter().take(count) {
            zones.push(Zone::new(info, generators, modulators)?);
        }

        Ok(zones)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_cannot_name_generators_outside_the_table() {
        let infos = [
            ZoneInfo {
                generator_index: 1,
                modulator_index: 0,
                generator_count: 1,
                modulator_count: 0,
            },
            ZoneInfo {
                generator_index: 2,
                modulator_index: 0,
                generator_count: 0,
                modulator_count: 0,
            },
        ];

        assert!(matches!(
            Zone::create(&infos, &[], &[]),
            Err(SoundFontError::InvalidGeneratorList)
        ));
    }

    #[test]
    fn a_backwards_generator_span_is_an_error() {
        let infos = [
            ZoneInfo {
                generator_index: 1,
                modulator_index: 0,
                generator_count: -1,
                modulator_count: 0,
            },
            ZoneInfo {
                generator_index: 0,
                modulator_index: 0,
                generator_count: 0,
                modulator_count: 0,
            },
        ];

        assert!(matches!(
            Zone::create(&infos, &[], &[]),
            Err(SoundFontError::InvalidGeneratorList)
        ));
    }
}
