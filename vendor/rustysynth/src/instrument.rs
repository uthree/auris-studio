#![allow(dead_code)]

use crate::error::SoundFontError;
use crate::instrument_info::InstrumentInfo;
use crate::instrument_region::InstrumentRegion;
use crate::sample_header::SampleHeader;
use crate::zone::Zone;

/// Represents an instrument in the SoundFont.
#[derive(Debug)]
#[non_exhaustive]
pub struct Instrument {
    pub(crate) name: String,
    pub(crate) regions: Vec<InstrumentRegion>,
}

impl Instrument {
    fn new(
        info: &InstrumentInfo,
        instrument_id: usize,
        zones: &[Zone],
        samples: &[SampleHeader],
    ) -> Result<Self, SoundFontError> {
        let name = info.name.clone();

        let Ok(span_start) = usize::try_from(info.zone_start_index) else {
            return Err(SoundFontError::InvalidInstrument(instrument_id));
        };
        let Some(span_end) = usize::try_from(info.zone_end_index)
            .ok()
            .and_then(|end| end.checked_add(1))
        else {
            return Err(SoundFontError::InvalidInstrument(instrument_id));
        };
        let Some(zone_span) = zones.get(span_start..span_end) else {
            return Err(SoundFontError::InvalidInstrument(instrument_id));
        };
        if zone_span.is_empty() {
            return Err(SoundFontError::InvalidInstrument(instrument_id));
        }
        let regions = InstrumentRegion::create(instrument_id, zone_span, samples)?;

        Ok(Self { name, regions })
    }

    pub(crate) fn create(
        infos: &[InstrumentInfo],
        zones: &[Zone],
        samples: &[SampleHeader],
    ) -> Result<Vec<Instrument>, SoundFontError> {
        if infos.len() <= 1 {
            return Err(SoundFontError::InstrumentNotFound);
        }

        // The last one is the terminator.
        let count = infos.len() - 1;

        let mut instruments: Vec<Instrument> = Vec::new();
        for (instrument_id, info) in infos.iter().take(count).enumerate() {
            instruments.push(Instrument::new(info, instrument_id, zones, samples)?);
        }

        Ok(instruments)
    }

    /// Gets the name of the instrument.
    pub fn get_name(&self) -> &str {
        &self.name
    }

    /// Gets the regions of the instrument.
    pub fn get_regions(&self) -> &[InstrumentRegion] {
        &self.regions[..]
    }
}
