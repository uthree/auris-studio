//! Modulation spectra of the rendered song, read in beats: which registers pulse, and how often.
//!
//! [`auris_dsp::modulation`] measures each octave band's loudness envelope in seconds. What a
//! musician — or a model reasoning about a score — wants is the same thing in the score's own
//! time: a kick on every beat is *one per beat* whether the tempo is 92 or 140, and stays one per
//! beat through a ritardando. So the envelopes are resampled onto the beat grid through the tempo
//! map before they are transformed, and the modulation axis comes out in cycles per beat. "Beat"
//! is the meter's felt beat ([`TimeSignature::beat_ticks`]): the quarter in 4/4, the dotted
//! quarter in 6/8.
//!
//! The region is chosen the way a person talks about a song: a named section (every occurrence
//! of it, or one), each section in turn, a bar range, or the whole song. Comparing the sections is
//! the point — a chorus whose hats double the verse's rate, a drop whose low end stops pulsing —
//! so one call can measure them all from a single render.

use crate::{RenderJob, Session, prelude::*};
use auris_dsp::modulation::{
    BandResolution, ENVELOPE_RATE, ModulationSpectrum, band_envelopes, modulation_spectrum,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// Beat-grid samples per beat; 16 cycles per beat is the grid's Nyquist rate.
const SAMPLES_PER_BEAT: f64 = 32.0;
/// Columns of [`RegionModulation::depth`]: 16 per octave from 1/16 to 16 cycles per beat.
pub const COLUMNS: usize = 128;
/// Columns per octave of [`RegionModulation::depth`].
const COLUMNS_PER_OCTAVE: f64 = 16.0;
/// The slowest column's rate, in cycles per beat.
const LOWEST_RATE: f64 = 1.0 / 16.0;
/// The longest single region the renderer is asked for.
const MAX_REGION_SECONDS: f64 = 240.0;
/// The longest stretch one call renders, for `each_section` and repeated sections.
const MAX_TOTAL_SECONDS: f64 = 600.0;
/// The shortest region with any periodicity worth reading.
const MIN_REGION_SECONDS: f64 = 2.0;
/// Most regions one call reports.
const MAX_REGIONS: usize = 16;

/// The rate of the `column`th column of [`RegionModulation::depth`], in cycles per beat.
pub fn column_rate(column: usize) -> f64 {
    LOWEST_RATE * 2.0_f64.powf((column as f64 + 0.5) / COLUMNS_PER_OCTAVE)
}

/// How finely the spectrum is divided into bands.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Bands {
    /// Ten octave bands, 31 Hz to 16 kHz: coarse, for a quick look at the registers.
    Octave,
    /// Thirty third-octave bands, 20 Hz to 16 kHz: kick apart from bass, hats apart from cymbals.
    #[default]
    ThirdOctave,
}

impl From<Bands> for BandResolution {
    fn from(bands: Bands) -> Self {
        match bands {
            Bands::Octave => Self::Octave,
            Bands::ThirdOctave => Self::ThirdOctave,
        }
    }
}

/// Which stretch of the song to measure. At most one of the three ways may be used; none
/// means the whole song.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// A section label from the song's sections, matched exactly after trimming.
    pub section: Option<String>,
    /// Which occurrence of `section`, from 1; omitted measures every occurrence.
    pub instance: Option<usize>,
    /// First bar of an explicit range, from 1.
    pub start_bar: Option<u32>,
    /// Bars in the explicit range.
    pub bars: Option<u32>,
    /// Measure every named section of the song, each as its own region.
    pub each_section: bool,
    /// How finely to divide the spectrum.
    pub bands: Bands,
}

/// A named band's peaks and rhythm grid within one region.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BandModulation {
    /// Band centre in hertz.
    pub centre_hz: f64,
    /// Mean band power in dB (power reference 1); `None` when the band is silent.
    pub level_db: Option<f64>,
    /// The deepest repetitions, deepest first.
    pub peaks: Vec<Peak>,
    /// Depth at the rates a score is built from, keyed as in [`RHYTHM_RATES`] plus the
    /// bar rates; rates below the region's resolution are left out.
    pub rhythm: std::collections::BTreeMap<String, f64>,
}

/// One modulation peak.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peak {
    /// Cycles per beat.
    pub cycles_per_beat: f64,
    /// Beats per cycle, the reciprocal.
    pub period_beats: f64,
    /// The nearest rhythm name from [`RHYTHM_RATES`] within 4%, if any.
    pub rhythm: Option<String>,
    /// Envelope swing as a fraction of its mean.
    pub depth: f64,
}

/// The measurement of one region.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegionModulation {
    /// Section label, when the region is a section.
    pub section: Option<String>,
    /// Which occurrence of that label, from 1.
    pub instance: Option<usize>,
    /// First bar, from 1.
    pub start_bar: u32,
    /// First bar after the region (exclusive); a section may end mid-bar.
    pub end_bar: u32,
    /// Absolute start tick.
    pub start_tick: i64,
    /// Absolute end tick.
    pub end_tick: i64,
    /// Duration in seconds.
    pub seconds: f64,
    /// Felt beats in the region.
    pub beats: f64,
    /// Ticks per felt beat at the region's start (960 is a quarter note).
    pub beat_ticks: i64,
    /// Felt beats per bar at the region's start.
    pub beats_per_bar: u32,
    /// Meter at the region's start, such as `4/4`.
    pub meter: String,
    /// Whether the meter changes inside the region; the grid keeps the starting beat.
    pub meter_changes: bool,
    /// Slowest and fastest quarter-note tempo in force inside the region.
    pub bpm_range: (f64, f64),
    /// The slowest rate distinguishable from a steady level: two cycles per analysed segment.
    pub lowest_resolved_cycles_per_beat: f64,
    /// Whether the rendered region is silent.
    pub silent: bool,
    /// One entry per octave band, low to high.
    pub bands: Vec<BandModulation>,
    /// Band-major depth image: `bands × COLUMNS`, columns at [`column_rate`].
    pub depth: Vec<f32>,
}

/// The whole report of one call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModulationReport {
    /// Document revision captured before rendering.
    pub revision: u64,
    /// Soloed track, or `None` for the mix.
    pub track: Option<u64>,
    /// How finely the spectrum was divided.
    pub bands: Bands,
    /// Every region measured, in song order.
    pub regions: Vec<RegionModulation>,
    /// Sections the selection skipped because the call's bounds were reached.
    pub omitted_regions: usize,
}

/// Named rates in cycles per felt beat; `bar` entries depend on the meter and are filled in
/// per region from the meter.
pub const RHYTHM_RATES: [(&str, f64); 6] = [
    ("1_per_beat", 1.0),
    ("2_per_beat", 2.0),
    ("3_per_beat", 3.0),
    ("4_per_beat", 4.0),
    ("6_per_beat", 6.0),
    ("8_per_beat", 8.0),
];

fn rhythm_rates(beats_per_bar: u32) -> Vec<(String, f64)> {
    let bar = 1.0 / f64::from(beats_per_bar.max(1));
    let mut rates = vec![
        ("per_4_bars".to_string(), bar / 4.0),
        ("per_2_bars".to_string(), bar / 2.0),
        ("per_bar".to_string(), bar),
    ];
    if beats_per_bar >= 4 && beats_per_bar.is_multiple_of(2) {
        rates.push(("per_half_bar".into(), bar * 2.0));
    }
    rates.extend(
        RHYTHM_RATES
            .iter()
            .map(|(name, rate)| (name.to_string(), *rate)),
    );
    rates
}

/// A stretch to measure, resolved from a [`Selection`].
#[derive(Clone, Debug)]
pub(crate) struct Region {
    pub(crate) section: Option<String>,
    pub(crate) instance: Option<usize>,
    pub(crate) start: Ticks,
    pub(crate) end: Ticks,
}

/// An owned render and its regions. Run on a worker, never on the UI or audio thread.
pub struct ModulationJob {
    render: RenderJob,
    revision: u64,
    track: Option<TrackId>,
    bands: Bands,
    regions: Vec<Region>,
    omitted: usize,
}

/// How a tool names a stretch of the song: a section (every occurrence, or one), each section,
/// a bar range, or — with nothing given — the whole song. Shared by every measuring tool so
/// `section: "サビ", instance: 2` means the same bars to all of them.
pub(crate) struct RegionQuery<'a> {
    pub(crate) section: Option<&'a str>,
    pub(crate) instance: Option<usize>,
    pub(crate) start_bar: Option<u32>,
    pub(crate) bars: Option<u32>,
    pub(crate) each_section: bool,
    /// The most bars an explicit range may span.
    pub(crate) max_bars: u32,
}

/// Resolve a [`RegionQuery`] to regions in song order, before any length limit is applied.
pub(crate) fn select_regions(
    project: &Project,
    query: &RegionQuery<'_>,
) -> Result<Vec<Region>, String> {
    let ways = usize::from(query.section.is_some())
        + usize::from(query.start_bar.is_some() || query.bars.is_some())
        + usize::from(query.each_section);
    if ways > 1 {
        return Err("Choose one of section, start_bar+bars or each_section".into());
    }
    if query.instance.is_some() && query.section.is_none() {
        return Err("instance needs section".into());
    }
    let song_end = project.end_tick();
    if song_end <= Ticks::ZERO {
        return Err("The song is empty; nothing to measure".into());
    }
    let spans = project.sections.spans_in(Ticks::ZERO, song_end);
    let named = |span: &auris_core::structure::SectionSpan| Region {
        section: Some(span.label.clone()),
        instance: Some(span.instance),
        start: span.start,
        end: span.end,
    };
    Ok(if query.each_section {
        if spans.is_empty() {
            return Err("The song has no named sections; set them with edit_timeline, or give start_bar and bars".into());
        }
        spans.iter().map(named).collect()
    } else if let Some(label) = query.section {
        let label = label.trim();
        let matching: Vec<_> = spans.iter().filter(|span| span.label == label).collect();
        if matching.is_empty() {
            let mut labels: Vec<&str> = spans.iter().map(|span| span.label.as_str()).collect();
            labels.dedup();
            return Err(format!(
                "No section is called {label:?}; the song's sections are {labels:?}"
            ));
        }
        match query.instance {
            Some(instance) => vec![named(
                matching
                    .iter()
                    .find(|span| span.instance == instance)
                    .ok_or_else(|| {
                        format!(
                            "{label:?} occurs {} times; instance is 1-based",
                            matching.len()
                        )
                    })?,
            )],
            None => matching.into_iter().map(named).collect(),
        }
    } else if query.start_bar.is_some() || query.bars.is_some() {
        let (Some(start_bar), Some(bars)) = (query.start_bar, query.bars) else {
            return Err("Give start_bar and bars together".into());
        };
        if start_bar == 0 || !(1..=query.max_bars).contains(&bars) {
            return Err(format!("Use start_bar >= 1 and bars 1..{}", query.max_bars));
        }
        let start = project.signatures.bar_start(start_bar);
        let end = project
            .signatures
            .bar_start(start_bar.checked_add(bars).ok_or("Bar range overflow")?);
        if start >= song_end {
            return Err("The requested range starts after the arrangement".into());
        }
        vec![Region {
            section: None,
            instance: None,
            start,
            end,
        }]
    } else {
        vec![Region {
            section: None,
            instance: None,
            start: Ticks::ZERO,
            end: song_end,
        }]
    })
}

fn resolve(project: &Project, selection: &Selection) -> Result<(Vec<Region>, usize), String> {
    let mut regions = select_regions(
        project,
        &RegionQuery {
            section: selection.section.as_deref(),
            instance: selection.instance,
            start_bar: selection.start_bar,
            bars: selection.bars,
            each_section: selection.each_section,
            max_bars: 256,
        },
    )?;
    let seconds = |region: &Region| {
        project.tempo_map.ticks_to_seconds(region.end).0
            - project.tempo_map.ticks_to_seconds(region.start).0
    };
    regions.retain(|region| seconds(region) >= MIN_REGION_SECONDS);
    if regions.is_empty() {
        return Err(format!(
            "Every selected region is shorter than {MIN_REGION_SECONDS} seconds"
        ));
    }
    if let Some(long) = regions
        .iter()
        .find(|region| seconds(region) > MAX_REGION_SECONDS)
    {
        return Err(format!(
            "A region lasts {:.0} seconds; measure at most {MAX_REGION_SECONDS:.0} seconds at a time (use each_section or a bar range)",
            seconds(long)
        ));
    }
    let mut kept = Vec::new();
    let first = project.tempo_map.ticks_to_seconds(regions[0].start).0;
    for region in &regions {
        let reach = project.tempo_map.ticks_to_seconds(region.end).0 - first;
        if kept.len() == MAX_REGIONS || reach > MAX_TOTAL_SECONDS {
            break;
        }
        kept.push(region.clone());
    }
    let omitted = regions.len() - kept.len();
    Ok((kept, omitted))
}

impl Session {
    /// Prepare a modulation measurement of the selected regions without altering the document.
    pub fn modulation_job(
        &mut self,
        selection: &Selection,
        track: Option<u64>,
    ) -> Result<ModulationJob, String> {
        let project = self.project();
        if track.is_some_and(|id| project.track(TrackId(id)).is_none()) {
            return Err("Unknown track ID".into());
        }
        let (regions, omitted) = resolve(project, selection)?;
        Ok(ModulationJob {
            revision: self.revision(),
            render: self.render_job_at(Some(48_000.0)),
            track: track.map(TrackId),
            bands: selection.bands,
            regions,
            omitted,
        })
    }
}

/// The beat's name, for the presenter: "quarter", "dotted quarter", ….
pub fn beat_label(ticks: i64) -> &'static str {
    match ticks {
        480 => "eighth",
        720 => "dotted eighth",
        960 => "quarter",
        1440 => "dotted quarter",
        1920 => "half",
        _ => "other",
    }
}

impl ModulationJob {
    /// Render once across every region, then measure each on its own beat grid.
    pub fn run(mut self, cancel: &AtomicBool) -> Result<ModulationReport, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Modulation analysis cancelled".into());
        }
        self.render.validate_complete().map_err(|e| e.to_string())?;
        let project = self.render.project().clone();
        let tempo = &project.tempo_map;
        let first = self.regions.first().ok_or("Nothing to measure")?.start;
        let last = self.regions.iter().map(|r| r.end).max().unwrap_or(first);
        let origin = tempo.ticks_to_seconds(first).0;
        let rate = 48_000.0;
        let options = OfflineOptions {
            start_frames: (origin * rate).round() as u64,
            end_frames: Some((tempo.ticks_to_seconds(last).0 * rate).round() as u64),
            sample_rate: Some(rate),
            include_tail: false,
            ..OfflineOptions::default()
        };
        let audio = self
            .render
            .render_target(
                self.track,
                &options,
                &mut RenderProgress::default().cancelled_by(cancel),
            )
            .map_err(|e| e.to_string())?;
        let mut regions = Vec::with_capacity(self.regions.len());
        for region in &self.regions {
            if cancel.load(Ordering::Relaxed) {
                return Err("Modulation analysis cancelled".into());
            }
            let from = ((tempo.ticks_to_seconds(region.start).0 - origin) * rate).round() as usize;
            let to = (((tempo.ticks_to_seconds(region.end).0 - origin) * rate).round() as usize)
                .min(audio.frame_count());
            let slice = AudioBuffer::from_planar(
                audio
                    .iter_channels()
                    .map(|channel| channel[from.min(to)..to].to_vec())
                    .collect(),
                rate,
            )
            .map_err(|e| e.to_string())?;
            regions.push(measure(&project, region, &slice, self.bands, cancel)?);
        }
        Ok(ModulationReport {
            revision: self.revision,
            track: self.track.map(|id| id.0),
            bands: self.bands,
            regions,
            omitted_regions: self.omitted,
        })
    }
}

/// Resample one band envelope (frames at [`ENVELOPE_RATE`] from the region's start) onto the
/// region's beat grid.
fn beat_grid(project: &Project, region: &Region, step: f64, envelope: &[f32]) -> Vec<f32> {
    let tempo = &project.tempo_map;
    let origin = tempo.ticks_to_seconds(region.start).0;
    let span = (region.end - region.start).raw() as f64;
    let count = (span / step).floor() as usize;
    (0..count)
        .map(|index| {
            let tick = region.start.raw() as f64 + index as f64 * step;
            let seconds = tempo.ticks_to_seconds(Ticks(tick.round() as i64)).0 - origin;
            // Frame k averages [k, k+1) / rate, so it stands for the moment (k + 0.5) / rate.
            let position = (seconds * ENVELOPE_RATE - 0.5).max(0.0);
            let below = (position.floor() as usize).min(envelope.len().saturating_sub(1));
            let above = (below + 1).min(envelope.len().saturating_sub(1));
            let fraction = (position - below as f64).clamp(0.0, 1.0) as f32;
            envelope
                .get(below)
                .map_or(0.0, |&a| a + (envelope[above] - a) * fraction)
        })
        .collect()
}

fn measure(
    project: &Project,
    region: &Region,
    audio: &AudioBuffer,
    bands: Bands,
    cancel: &AtomicBool,
) -> Result<RegionModulation, String> {
    let signature = project.signatures.signature_at(region.start);
    let beat_ticks = signature.beat_ticks().raw().max(1);
    let beats_per_bar = signature.felt_beats();
    let step = beat_ticks as f64 / SAMPLES_PER_BEAT;
    let envelopes = band_envelopes(audio, bands.into(), cancel)
        .ok_or("Modulation analysis cancelled or unusable audio")?;
    let rates = rhythm_rates(beats_per_bar);
    let label = |rate: f64| {
        rates
            .iter()
            .find(|(_, named)| (rate / named).max(named / rate) <= 1.04)
            .map(|(name, _)| name.clone())
    };
    let round = |value: f64, places: i32| {
        let scale = 10.0_f64.powi(places);
        (value * scale).round() / scale
    };
    let centres = envelopes.centres.clone();
    let mut bands = Vec::with_capacity(centres.len());
    let mut depth = Vec::with_capacity(centres.len() * COLUMNS);
    let mut lowest = 0.0_f64;
    for (band, centre) in centres.iter().enumerate() {
        let grid = beat_grid(project, region, step, &envelopes.amplitude[band]);
        let silent = envelopes.level_db[band].is_none();
        let spectrum = if silent {
            None
        } else {
            Some(modulation_spectrum(&grid, SAMPLES_PER_BEAT))
        };
        let spectrum: Option<&ModulationSpectrum> = spectrum.as_ref();
        if let Some(spectrum) = spectrum {
            lowest = lowest.max(2.0 / spectrum.segment_units);
        }
        let column_tolerance = 2.0_f64.powf(0.5 / COLUMNS_PER_OCTAVE) - 1.0;
        depth.extend((0..COLUMNS).map(|column| {
            spectrum.map_or(0.0, |s| s.depth_near(column_rate(column), column_tolerance))
        }));
        let floor = spectrum.map_or(f64::INFINITY, |s| 2.0 / s.segment_units);
        let peaks = spectrum
            .map(|s| s.peaks(floor.max(LOWEST_RATE), 16.0, 3, 0.05))
            .unwrap_or_default()
            .into_iter()
            .map(|(rate, depth)| Peak {
                cycles_per_beat: round(rate, 3),
                period_beats: round(1.0 / rate, 2),
                rhythm: label(rate),
                depth: round(f64::from(depth), 2),
            })
            .collect();
        let rhythm = rates
            .iter()
            .filter(|(_, rate)| *rate >= floor)
            .map(|(name, rate)| {
                let value = spectrum.map_or(0.0, |s| s.depth_near(*rate, 0.03));
                (name.clone(), round(f64::from(value), 2))
            })
            .collect();
        bands.push(BandModulation {
            // Three significant figures: 19.7, 63, 1260.
            centre_hz: {
                let scale = 10.0_f64.powi(2 - centre.log10().floor() as i32);
                (centre * scale).round() / scale
            },
            level_db: envelopes.level_db[band].map(|db| round(f64::from(db), 1)),
            peaks,
            rhythm,
        });
    }
    let tempo = &project.tempo_map;
    let inside = tempo
        .points()
        .iter()
        .filter(|point| point.tick > region.start && point.tick < region.end)
        .map(|point| point.bpm);
    let at_start = tempo.bpm_at(region.start);
    let bpm_range = inside.fold((at_start, at_start), |(low, high), bpm| {
        (low.min(bpm), high.max(bpm))
    });
    let meter_changes = project
        .signatures
        .points()
        .iter()
        .any(|point| point.tick > region.start && point.tick < region.end);
    let (bar, beat, offset) = project.signatures.bar_beat_at(region.end);
    let end_bar = if beat == 1 && offset == 0 {
        bar
    } else {
        bar + 1
    };
    Ok(RegionModulation {
        section: region.section.clone(),
        instance: region.instance,
        start_bar: project.signatures.bar_beat_at(region.start).0,
        end_bar,
        start_tick: region.start.raw(),
        end_tick: region.end.raw(),
        seconds: round(
            tempo.ticks_to_seconds(region.end).0 - tempo.ticks_to_seconds(region.start).0,
            3,
        ),
        beats: round(
            (region.end - region.start).raw() as f64 / beat_ticks as f64,
            2,
        ),
        beat_ticks,
        beats_per_bar,
        meter: signature.to_string(),
        meter_changes,
        bpm_range,
        lowest_resolved_cycles_per_beat: round(lowest, 3),
        silent: envelopes.level_db.iter().all(Option::is_none),
        bands,
        depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drum_song(bpm: f64) -> (Session, TrackId) {
        pulse_song(bpm, Ticks::QUARTER)
    }

    /// Eight bars of short notes, one every `step`.
    fn pulse_song(bpm: f64, step: Ticks) -> (Session, TrackId) {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        session.set_tempo_at(Ticks::ZERO, bpm);
        let track = session.add_default_instrument_track("Pulse").unwrap();
        session
            .set_track_instrument(track, "auris.synth.fm2")
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Beat", Ticks::ZERO, Ticks::QUARTER * 32)
            .unwrap();
        let notes = (0..(Ticks::QUARTER * 32).raw() / step.raw())
            .map(|index| Note::new(48, step * index, Ticks(120)))
            .collect();
        session.replace_notes(clip, notes).unwrap();
        (session, track)
    }

    fn strongest(region: &RegionModulation) -> &BandModulation {
        region
            .bands
            .iter()
            .filter(|band| band.level_db.is_some())
            .max_by(|a, b| a.level_db.unwrap().total_cmp(&b.level_db.unwrap()))
            .unwrap()
    }

    fn rhythm_of(bpm: f64, step: Ticks) -> BandModulation {
        let (mut session, _) = pulse_song(bpm, step);
        let selection = Selection {
            start_bar: Some(1),
            bars: Some(8),
            ..Selection::default()
        };
        let report = session
            .modulation_job(&selection, None)
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        let region = &report.regions[0];
        assert_eq!(region.beats, 32.0);
        assert_eq!(region.beat_ticks, 960);
        assert_eq!(region.bands.len(), 30);
        assert_eq!(region.depth.len(), 30 * COLUMNS);
        strongest(region).clone()
    }

    fn depth(band: &BandModulation, name: &str) -> f64 {
        band.rhythm[name]
    }

    #[test]
    fn a_note_on_every_beat_reads_one_per_beat_at_any_tempo() {
        for bpm in [90.0, 150.0] {
            let band = rhythm_of(bpm, Ticks::QUARTER);
            let peak = &band.peaks[0];
            assert!(
                (peak.cycles_per_beat - 1.0).abs() < 0.03,
                "{bpm}: {:?}",
                band.peaks
            );
            assert_eq!(peak.rhythm.as_deref(), Some("1_per_beat"));
            assert!(depth(&band, "1_per_beat") > 0.5, "{bpm}: {:?}", band.rhythm);
            // Nothing repeats slower than the beat; a train of short hits has its harmonics.
            for slower in ["per_bar", "per_half_bar"] {
                assert!(depth(&band, slower) < 0.1, "{bpm}: {:?}", band.rhythm);
            }
            assert!(depth(&band, "2_per_beat") > 0.3, "{bpm}: {:?}", band.rhythm);
        }
    }

    #[test]
    fn eighth_notes_read_two_per_beat_and_nothing_at_the_beat() {
        let band = rhythm_of(120.0, Ticks(480));
        assert!(
            (band.peaks[0].cycles_per_beat - 2.0).abs() < 0.06,
            "{:?}",
            band.peaks
        );
        assert!(
            depth(&band, "1_per_beat") < 0.2 * depth(&band, "2_per_beat"),
            "{:?}",
            band.rhythm
        );
        assert!(
            depth(&band, "3_per_beat") < 0.2 * depth(&band, "2_per_beat"),
            "{:?}",
            band.rhythm
        );
    }

    #[test]
    fn sections_are_selected_by_name_and_occurrence() {
        let (mut session, _) = drum_song(120.0);
        for (beat, label) in [(0, "Aメロ"), (8, "サビ"), (16, "Aメロ"), (24, "サビ")] {
            session.set_section(Ticks::QUARTER * beat, Some(label.into()));
        }
        let chorus = Selection {
            section: Some(" サビ ".into()),
            ..Selection::default()
        };
        let (regions, omitted) = resolve(session.project(), &chorus).unwrap();
        assert_eq!(omitted, 0);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[1].instance, Some(2));
        assert_eq!(regions[1].start, Ticks::QUARTER * 24);
        let second = Selection {
            instance: Some(2),
            ..chorus.clone()
        };
        let (regions, _) = resolve(session.project(), &second).unwrap();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start, Ticks::QUARTER * 24);
        let each = Selection {
            each_section: true,
            ..Selection::default()
        };
        let report = session
            .modulation_job(&each, None)
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        assert_eq!(report.regions.len(), 4);
        assert_eq!(report.regions[2].section.as_deref(), Some("Aメロ"));
        assert_eq!(
            (report.regions[2].start_bar, report.regions[2].end_bar),
            (5, 7)
        );
        let error = resolve(
            session.project(),
            &Selection {
                section: Some("Bメロ".into()),
                ..Selection::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("サビ"), "{error}");
        let mixed = Selection {
            each_section: true,
            start_bar: Some(1),
            bars: Some(2),
            ..Selection::default()
        };
        assert!(resolve(session.project(), &mixed).is_err());
    }

    #[test]
    fn the_whole_song_is_the_default_and_the_document_is_untouched() {
        let (mut session, track) = drum_song(120.0);
        let before = session.project().clone();
        let job = session
            .modulation_job(&Selection::default(), Some(track.0))
            .unwrap();
        session.set_track_mute(track, true).unwrap();
        let report = job.run(&AtomicBool::new(false)).unwrap();
        assert_eq!(report.track, Some(track.0));
        assert_eq!(report.regions[0].start_tick, 0);
        assert!(!report.regions[0].silent);
        assert_eq!(&before.tracks[0].kind, &session.project().tracks[0].kind);
        assert!(
            session
                .modulation_job(&Selection::default(), Some(u64::MAX))
                .is_err()
        );
        let cancelled = AtomicBool::new(true);
        assert!(
            session
                .modulation_job(&Selection::default(), None)
                .unwrap()
                .run(&cancelled)
                .is_err()
        );
    }

    #[test]
    fn columns_span_eight_octaves_of_beat_rates() {
        assert!((column_rate(0) / LOWEST_RATE - 2.0_f64.powf(1.0 / 32.0)).abs() < 1e-9);
        assert!((column_rate(COLUMNS - 1) - 16.0 / 2.0_f64.powf(1.0 / 32.0)).abs() < 1e-9);
        let bars = rhythm_rates(3);
        assert!(bars.iter().all(|(name, _)| name != "per_half_bar"));
        assert_eq!(bars[2], ("per_bar".to_string(), 1.0 / 3.0));
    }
}
