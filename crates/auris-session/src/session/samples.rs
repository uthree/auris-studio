//! Loading audio files into the audio sampler, and editing its map.
//!
//! Each sample is an ordinary audio source of the project — copied into the folder, found again
//! when it moves, reported when it is missing — with no clip on the timeline. The track's
//! instrument state holds the map, [`auris_sampler::SampleZone`]s naming those sources by id,
//! which is all [`auris_sampler::AudioSampler`] needs. How a note chooses among them is
//! [`auris_sampler::zones`]' account.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_core::{AssetPath, AudioBuffer, PluginState, SourceId, TrackId};
use auris_sampler::{
    AUDIO_SAMPLER_ID, AUDIO_SAMPLER_LOOP_KEY, MAX_SAMPLE_ZONES, SampleZone, store_zones,
    stored_zones,
};

use super::{Session, decode_audio};
use crate::error::SessionError;
use crate::history::Edit;

/// The highest MIDI note a root note may be.
const HIGHEST_NOTE: u8 = 127;

/// The root a sample takes when nothing says otherwise: middle C.
pub const DEFAULT_SAMPLE_ROOT: u8 = 60;

/// One zone's sample as the window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleZoneView {
    /// The zone itself.
    pub zone: SampleZone,
    /// The audio source's display name.
    pub name: String,
    /// Whether its audio is loaded.
    pub loaded: bool,
    /// The keys it actually plays, inside its own velocity layer; `None` when every key goes to
    /// another zone.
    pub plays: Option<auris_sampler::KeyRange>,
}

impl Session {
    /// Adds an audio file to a track's sampler, switching the track to the sampler first.
    ///
    /// [`Self::load_samples`] for one file, keeping whatever the sampler already holds.
    pub fn load_sample(&mut self, track: TrackId, path: &Path) -> Result<SourceId, SessionError> {
        let sources = self.load_samples(track, &[path.to_path_buf()], false)?;
        Ok(sources[0])
    }

    /// Loads audio files into a track's sampler, switching the track to the sampler first.
    ///
    /// Each file becomes one of the project's audio sources, so it is copied into a saved
    /// project's folder exactly as imported audio is, and nothing is placed on the timeline. Each
    /// becomes a zone of the map at the root note its file name spells (`Piano_C3.wav`), or at
    /// middle C; the nearest-root rule then spreads them over the keyboard with nothing more to
    /// set. With `replace` the map is cleared first; otherwise the files join it. A track already
    /// on the sampler keeps its envelope and loop; a track arriving from another instrument starts
    /// from the sampler's defaults, for the reason [`Self::set_track_preset`] gives. One undo step
    /// however many files.
    pub fn load_samples(
        &mut self,
        track: TrackId,
        paths: &[PathBuf],
        replace: bool,
    ) -> Result<Vec<SourceId>, SessionError> {
        let decoded = paths
            .iter()
            .map(|path| Ok((path.clone(), decode_audio(path, self.project.sample_rate)?)))
            .collect::<Result<Vec<_>, SessionError>>()?;
        self.place_samples(track, decoded, replace)
    }

    /// [`Self::load_sample`] for audio that has already been decoded, at the project's rate.
    pub fn place_sample(
        &mut self,
        track: TrackId,
        path: &Path,
        buffer: AudioBuffer,
    ) -> Result<SourceId, SessionError> {
        let sources = self.place_samples(track, vec![(path.to_path_buf(), buffer)], false)?;
        Ok(sources[0])
    }

    /// [`Self::load_samples`] for audio that has already been decoded, at the project's rate.
    ///
    /// The split is [`Self::place_audio`]'s: a frontend decodes on a worker and hands the result
    /// to the thread holding the session. Every file is checked before anything changes, so one
    /// empty file refuses the whole batch rather than half of it.
    pub fn place_samples(
        &mut self,
        track: TrackId,
        samples: Vec<(PathBuf, AudioBuffer)>,
        replace: bool,
    ) -> Result<Vec<SourceId>, SessionError> {
        self.require_sampler_target(track)?;
        if samples.is_empty() {
            return Err(SessionError::SampleMap("no files to load".into()));
        }
        let mut checked = Vec::with_capacity(samples.len());
        for (path, buffer) in samples {
            let buffer = match buffer.sample_rate() == self.project.sample_rate {
                true => buffer,
                false => decode_audio(&path, self.project.sample_rate)?,
            };
            if buffer.frame_count() == 0 || buffer.channel_count() == 0 {
                return Err(SessionError::EmptySample(path));
            }
            checked.push((path, buffer));
        }
        let kept = match replace || !self.track_is_on_sampler(track) {
            true => 0,
            false => self.track_sample_zones(track).len(),
        };
        if kept + checked.len() > MAX_SAMPLE_ZONES {
            return Err(SessionError::SampleMap(format!(
                "a sampler holds at most {MAX_SAMPLE_ZONES} samples"
            )));
        }
        self.record(Edit::LoadSample);
        self.put_on_sampler(track);
        let mut zones = match replace {
            true => Vec::new(),
            false => self.track_sample_zones(track),
        };
        let mut sources = Vec::with_capacity(checked.len());
        for (path, buffer) in checked {
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| "Sample".to_string());
            let source = self.project.add_audio_source(
                name.clone(),
                AssetPath::external(&path),
                buffer.frame_count() as u64,
                buffer.sample_rate(),
                buffer.channel_count(),
            );
            self.record_source_size(source, &path);
            if self.path.is_some()
                && let Err(error) = self.collect_source(source, &path)
            {
                log::warn!("could not collect {}: {error}", path.display());
            }
            let root = root_note_in_name(&name).unwrap_or(DEFAULT_SAMPLE_ROOT);
            zones.push(SampleZone::new(source, root));
            self.install_source(source, Arc::new(buffer));
            sources.push(source);
        }
        self.write_zones(track, &zones);
        self.invalidate_graph();
        Ok(sources)
    }

    /// The map a track's audio sampler plays; empty when it is not on the sampler.
    pub fn track_sample_zones(&self, track: TrackId) -> Vec<SampleZone> {
        self.project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .filter(|instrument| instrument.instrument_id == AUDIO_SAMPLER_ID)
            .map(|instrument| stored_zones(&instrument.instrument_state))
            .unwrap_or_default()
    }

    /// The map as a reader wants it: each zone with its file's name, whether that file is loaded,
    /// and the keys it ends up playing.
    pub fn track_sample_map(&self, track: TrackId) -> Vec<SampleZoneView> {
        let zones = self.track_sample_zones(track);
        // The keys are what the instrument will actually do, so they are worked out over the
        // zones it can sound: a missing file's keys go to its neighbours.
        let loaded: Vec<bool> = zones
            .iter()
            .map(|zone| self.bank.get(zone.source).is_some())
            .collect();
        let sounding: Vec<SampleZone> = zones
            .iter()
            .zip(&loaded)
            .filter(|(_, loaded)| **loaded)
            .map(|(zone, _)| *zone)
            .collect();
        let mut position = 0;
        zones
            .iter()
            .zip(&loaded)
            .map(|(zone, &loaded)| {
                let plays = if loaded {
                    position += 1;
                    auris_sampler::keys_played_by(&sounding, position - 1)
                } else {
                    None
                };
                SampleZoneView {
                    zone: *zone,
                    name: self
                        .project
                        .audio_sources
                        .get(&zone.source)
                        .map_or_else(|| format!("#{}", zone.source.0), |s| s.name.clone()),
                    loaded,
                    plays,
                }
            })
            .collect()
    }

    /// The display name of the first sample a track's audio sampler plays.
    pub fn track_sample_name(&self, track: TrackId) -> Option<String> {
        let source = self.track_sample_zones(track).first()?.source;
        Some(self.project.audio_sources.get(&source)?.name.clone())
    }

    /// `true` when the track's sampler holds at least one sample and every one is loaded.
    pub fn track_sample_is_loaded(&self, track: TrackId) -> bool {
        let zones = self.track_sample_zones(track);
        !zones.is_empty()
            && zones
                .iter()
                .all(|zone| self.bank.get(zone.source).is_some())
    }

    /// Replaces a track's sampler map in one undo step.
    ///
    /// Every zone must name an audio source of the project. The sources a zone stops naming stay
    /// in the project, the way a deleted clip's audio does, so undo can bring them back.
    pub fn set_sample_zones(
        &mut self,
        track: TrackId,
        zones: Vec<SampleZone>,
    ) -> Result<(), SessionError> {
        self.require_track(track)?;
        if !self.track_is_on_sampler(track) {
            return Err(SessionError::SampleMap(
                "the track is not on the sampler; load a sample first".into(),
            ));
        }
        if zones.len() > MAX_SAMPLE_ZONES {
            return Err(SessionError::SampleMap(format!(
                "a sampler holds at most {MAX_SAMPLE_ZONES} samples"
            )));
        }
        if let Some(zone) = zones
            .iter()
            .find(|zone| !self.project.audio_sources.contains_key(&zone.source))
        {
            return Err(SessionError::SampleMap(format!(
                "source {} is not in the project",
                zone.source.0
            )));
        }
        let zones: Vec<SampleZone> = zones.into_iter().map(SampleZone::sanitised).collect();
        if zones == self.track_sample_zones(track) {
            return Ok(());
        }
        self.record(Edit::EditSampleMap);
        self.write_zones(track, &zones);
        self.invalidate_graph();
        Ok(())
    }

    /// Changes one zone of a track's sampler map, in one undo step.
    pub fn set_sample_zone(
        &mut self,
        track: TrackId,
        index: usize,
        zone: SampleZone,
    ) -> Result<(), SessionError> {
        let mut zones = self.track_sample_zones(track);
        let slot = zones
            .get_mut(index)
            .ok_or_else(|| SessionError::SampleMap(format!("there is no sample {index}")))?;
        *slot = zone;
        self.set_sample_zones(track, zones)
    }

    /// Takes one recording out of a track's sampler map, in one undo step.
    pub fn remove_sample_zone(&mut self, track: TrackId, index: usize) -> Result<(), SessionError> {
        let mut zones = self.track_sample_zones(track);
        if index >= zones.len() {
            return Err(SessionError::SampleMap(format!(
                "there is no sample {index}"
            )));
        }
        zones.remove(index);
        self.set_sample_zones(track, zones)
    }

    /// `true` when the track plays the audio sampler.
    fn track_is_on_sampler(&self, track: TrackId) -> bool {
        self.project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .is_some_and(|instrument| instrument.instrument_id == AUDIO_SAMPLER_ID)
    }

    /// Refuses a track the sampler cannot be put on.
    pub(crate) fn require_sampler_target(&self, track: TrackId) -> Result<(), SessionError> {
        self.require_track(track)?;
        if !self
            .project
            .track(track)
            .is_some_and(|entry| entry.kind.is_instrument())
        {
            return Err(SessionError::WrongTrackKind {
                id: track.0,
                actual: self
                    .project
                    .track(track)
                    .map_or("a track", |entry| entry.kind.label()),
                expected: "an instrument track",
            });
        }
        Ok(())
    }

    /// Puts the track on the audio sampler, inside an edit already recorded.
    ///
    /// Answers whether the track arrived from another instrument, and so starts from the
    /// sampler's defaults with an empty map.
    pub(super) fn put_on_sampler(&mut self, track: TrackId) -> bool {
        let mut swapped = false;
        if let Some(inner) = self
            .project
            .track_mut(track)
            .and_then(|entry| entry.kind.as_instrument_mut())
            && inner.instrument_id != AUDIO_SAMPLER_ID
        {
            inner.instrument_id = AUDIO_SAMPLER_ID.to_string();
            inner.instrument_state = PluginState::empty();
            inner.file = None;
            swapped = true;
        }
        if swapped {
            self.project.remove_instrument_automation(track);
        }
        swapped
    }

    /// Writes a sampler's map, inside an edit already recorded.
    fn write_zones(&mut self, track: TrackId, zones: &[SampleZone]) {
        if let Some(inner) = self
            .project
            .track_mut(track)
            .and_then(|entry| entry.kind.as_instrument_mut())
        {
            store_zones(&mut inner.instrument_state, zones);
        }
    }

    /// Adopts the samples a model asked for, staged into the project folder by a file job.
    ///
    /// Every file is checked before the document changes, so the batch lands whole or not at
    /// all. Without `append` the map is replaced, which makes a repeated call idempotent rather
    /// than doubling the instrument. `looped` overrides the sampler's loop switch when given.
    pub(crate) fn adopt_agent_samples(
        &mut self,
        staged: Vec<(
            crate::live_agent::file_jobs::StagedSource,
            String,
            AgentSample,
        )>,
        track: TrackId,
        append: bool,
        looped: Option<bool>,
    ) -> Result<String, String> {
        self.require_sampler_target(track)
            .map_err(|e| e.to_string())?;
        let kept = match append && self.track_is_on_sampler(track) {
            true => self.track_sample_zones(track),
            false => Vec::new(),
        };
        if kept.len() + staged.len() > MAX_SAMPLE_ZONES {
            return Err(format!(
                "A sampler holds at most {MAX_SAMPLE_ZONES} samples"
            ));
        }
        let mut zones = kept;
        let mut ready = Vec::with_capacity(staged.len());
        for (staged, name, request) in staged {
            if self.engine.sample_rate() != staged.prepared.render_rate {
                return Err("Audio device changed while the sample was being prepared".into());
            }
            let buffer = &staged.prepared.buffer;
            if buffer.frame_count() == 0 || buffer.channel_count() == 0 {
                return Err(format!("{name} holds no audio to sample"));
            }
            let root = request
                .root_note
                .or_else(|| root_note_in_name(&name))
                .unwrap_or(DEFAULT_SAMPLE_ROOT);
            // The source id is not known yet; the zone is completed once it is.
            let zone = request
                .settings()
                .apply(SampleZone::new(SourceId(0), root))?;
            ready.push((staged, name, zone));
        }
        self.begin_transaction(Edit::LoadSample);
        self.put_on_sampler(track);
        let mut sources = Vec::with_capacity(ready.len());
        for (staged, name, mut zone) in ready {
            let crate::live_agent::file_jobs::StagedSource {
                file,
                prepared,
                bytes,
                provenance,
            } = staged;
            let Some(filename) = file.path().file_name().map(ToOwned::to_owned) else {
                continue;
            };
            if let Err(error) = file.keep() {
                log::warn!("could not keep the staged sample {name}: {error}");
                continue;
            }
            let buffer = &prepared.buffer;
            let source = self.project.add_audio_source(
                name.clone(),
                AssetPath::inside(Path::new(auris_io::AUDIO_DIR).join(filename)),
                buffer.frame_count() as u64,
                buffer.sample_rate(),
                buffer.channel_count(),
            );
            if let Some(metadata) = self.project.audio_sources.get_mut(&source) {
                metadata.byte_size = bytes;
                metadata.provenance = provenance;
            }
            zone.source = source;
            zones.push(zone);
            self.install_prepared_audio(source, prepared);
            sources.push(source);
        }
        self.write_zones(track, &zones);
        if let Some(looped) = looped {
            self.set_sampler_value(track, AUDIO_SAMPLER_LOOP_KEY, f32::from(u8::from(looped)));
        }
        self.end_transaction();
        self.invalidate_graph();
        let mut report = self.agent_sample_map_report(track);
        if let Some(object) = report.as_object_mut() {
            object.insert(
                "loaded_sources".into(),
                sources.iter().map(|source| source.0).collect(),
            );
            object.insert("revision".into(), self.agent_document_revision()?.into());
            object.insert("unsaved".into(), true.into());
        }
        Ok(report.to_string())
    }

    /// Applies a model's edits to a track's sampler map, then its removals, as one undo step.
    ///
    /// Indices are positions in the map as it stood before the call, for edits and removals
    /// alike, so a model never has to work out how one change renumbered the next.
    pub(crate) fn agent_edit_sample_map(
        &mut self,
        track: TrackId,
        edits: Vec<SampleZoneEdit>,
        remove: Vec<usize>,
    ) -> Result<String, String> {
        if !self.track_is_on_sampler(track) {
            return Err("The track is not on the sampler; load_sample first".into());
        }
        let mut zones = self.track_sample_zones(track);
        for edit in edits {
            let count = zones.len();
            let zone = zones
                .get_mut(edit.zone)
                .ok_or_else(|| format!("Zone {} does not exist; the map has {count}", edit.zone))?;
            if edit.root_note.is_some_and(|note| note > HIGHEST_NOTE) {
                return Err("root_note must be a MIDI note 0..127 (60 is C4)".into());
            }
            let mut next = *zone;
            if let Some(root) = edit.root_note {
                next.root = root;
            }
            if edit.auto_keys == Some(true) {
                next.keys = None;
            }
            *zone = edit.settings().apply(next)?;
        }
        let mut remove = remove;
        remove.sort_unstable();
        remove.dedup();
        if let Some(&missing) = remove.iter().find(|&&index| index >= zones.len()) {
            return Err(format!(
                "Zone {missing} does not exist; the map has {}",
                zones.len()
            ));
        }
        for index in remove.into_iter().rev() {
            zones.remove(index);
        }
        self.set_sample_zones(track, zones)
            .map_err(|e| e.to_string())?;
        let mut report = self.agent_sample_map_report(track);
        if let Some(object) = report.as_object_mut() {
            object.insert("revision".into(), self.agent_document_revision()?.into());
            object.insert("unsaved".into(), true.into());
        }
        Ok(report.to_string())
    }

    /// The sampler map as a model reads it.
    pub(crate) fn agent_sample_map_report(&self, track: TrackId) -> serde_json::Value {
        let state = self
            .project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .map(|inner| inner.instrument_state.params.clone())
            .unwrap_or_default();
        let range = |keys: auris_sampler::KeyRange| {
            serde_json::json!({
                "low": keys.low,
                "high": keys.high,
                "names": format!(
                    "{}-{}",
                    auris_core::theory::pitch::midi_name(i32::from(keys.low)),
                    auris_core::theory::pitch::midi_name(i32::from(keys.high))
                ),
            })
        };
        let zones: Vec<serde_json::Value> = self
            .track_sample_map(track)
            .into_iter()
            .enumerate()
            .map(|(index, view)| {
                serde_json::json!({
                    "zone": index,
                    "source": view.zone.source.0,
                    "name": view.name,
                    "loaded": view.loaded,
                    "root_note": view.zone.root,
                    "root_name": auris_core::theory::pitch::midi_name(i32::from(view.zone.root)),
                    "keys": view.zone.keys.map(range),
                    "plays": view.plays.map(range),
                    "low_velocity": rounded(view.zone.velocity.low),
                    "high_velocity": rounded(view.zone.velocity.high),
                    "tune": rounded(view.zone.tune),
                    "gain_db": rounded(view.zone.gain_db),
                })
            })
            .collect();
        serde_json::json!({
            "track": track.0,
            "instrument": AUDIO_SAMPLER_ID,
            "loop": state.get(AUDIO_SAMPLER_LOOP_KEY).copied().unwrap_or(0.0) >= 0.5,
            "zones": zones,
        })
    }

    /// Writes one of the sampler's stored parameters, inside an edit already recorded.
    pub(super) fn set_sampler_value(&mut self, track: TrackId, key: &str, value: f32) {
        if let Some(inner) = self
            .project
            .track_mut(track)
            .and_then(|entry| entry.kind.as_instrument_mut())
        {
            inner.instrument_state.params.insert(key.to_string(), value);
        }
    }
}

/// An `f32` as a model should read it: four decimals, without the binary noise of widening.
fn rounded(value: f32) -> f64 {
    (f64::from(value) * 10_000.0).round() / 10_000.0
}

/// One file for `load_sample`, with where it sits in the map.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentSample {
    /// Absolute local audio file path, up to 512 MiB; no network paths.
    pub path: String,
    /// MIDI note 0..127 the recording sounds at; omitted, a note name in the file name such as Piano_C3, else 60 (C4).
    #[schemars(range(min = 0, max = 127))]
    pub root_note: Option<u8>,
    /// Lowest MIDI key it plays. Give low_key/high_key only for a split other than the automatic nearest-root one, e.g. one drum hit per key.
    #[schemars(range(min = 0, max = 127))]
    pub low_key: Option<u8>,
    /// Highest MIDI key it plays.
    #[schemars(range(min = 0, max = 127))]
    pub high_key: Option<u8>,
    /// Softest normalized velocity 0..1 it answers, for velocity layers; default 0.
    pub low_velocity: Option<f32>,
    /// Hardest normalized velocity 0..1 it answers; default 1.
    pub high_velocity: Option<f32>,
    /// Fine tune of this recording, semitones -24..24.
    pub tune: Option<f32>,
    /// Level of this recording, dB -60..12, to balance layers.
    pub gain_db: Option<f32>,
}

/// One change to an existing zone, for `edit_sample_map`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleZoneEdit {
    /// Zero-based zone index from the current map (load_sample or inspect_instrument report it).
    pub zone: usize,
    /// MIDI note 0..127 the recording sounds at.
    #[schemars(range(min = 0, max = 127))]
    pub root_note: Option<u8>,
    /// Lowest MIDI key it plays; fixes its range instead of the automatic nearest-root split.
    #[schemars(range(min = 0, max = 127))]
    pub low_key: Option<u8>,
    /// Highest MIDI key it plays.
    #[schemars(range(min = 0, max = 127))]
    pub high_key: Option<u8>,
    /// true returns the zone to the automatic nearest-root split, dropping low_key/high_key.
    pub auto_keys: Option<bool>,
    /// Softest normalized velocity 0..1 it answers.
    pub low_velocity: Option<f32>,
    /// Hardest normalized velocity 0..1 it answers.
    pub high_velocity: Option<f32>,
    /// Fine tune of this recording, semitones -24..24.
    pub tune: Option<f32>,
    /// Level of this recording, dB -60..12.
    pub gain_db: Option<f32>,
}

/// The settings `load_sample` and `edit_sample_map` share, applied to a zone after checking.
struct ZoneSettings {
    low_key: Option<u8>,
    high_key: Option<u8>,
    low_velocity: Option<f32>,
    high_velocity: Option<f32>,
    tune: Option<f32>,
    gain_db: Option<f32>,
}

impl AgentSample {
    fn settings(&self) -> ZoneSettings {
        ZoneSettings {
            low_key: self.low_key,
            high_key: self.high_key,
            low_velocity: self.low_velocity,
            high_velocity: self.high_velocity,
            tune: self.tune,
            gain_db: self.gain_db,
        }
    }
}

impl SampleZoneEdit {
    fn settings(&self) -> ZoneSettings {
        ZoneSettings {
            low_key: self.low_key,
            high_key: self.high_key,
            low_velocity: self.low_velocity,
            high_velocity: self.high_velocity,
            tune: self.tune,
            gain_db: self.gain_db,
        }
    }
}

impl ZoneSettings {
    /// The zone with these settings on it, or why they are out of range.
    ///
    /// One end of a range given alone keeps the zone's other end, or the keyboard's or the
    /// velocity scale's when it had none.
    fn apply(&self, mut zone: SampleZone) -> Result<SampleZone, String> {
        if self
            .low_key
            .into_iter()
            .chain(self.high_key)
            .any(|key| key > HIGHEST_NOTE)
        {
            return Err("low_key and high_key must be MIDI notes 0..127".into());
        }
        let velocity_ok =
            |v: Option<f32>| v.is_none_or(|v| v.is_finite() && (0.0..=1.0).contains(&v));
        if !velocity_ok(self.low_velocity) || !velocity_ok(self.high_velocity) {
            return Err("Velocities are normalized 0..1, never MIDI 0..127".into());
        }
        if self
            .tune
            .is_some_and(|tune| !(tune.is_finite() && (-24.0..=24.0).contains(&tune)))
        {
            return Err("tune must be -24..24 semitones".into());
        }
        if self
            .gain_db
            .is_some_and(|gain| !(gain.is_finite() && (-60.0..=12.0).contains(&gain)))
        {
            return Err("gain_db must be -60..12".into());
        }
        if self.low_key.is_some() || self.high_key.is_some() {
            let current = zone.keys.unwrap_or(auris_sampler::KeyRange {
                low: 0,
                high: HIGHEST_NOTE,
            });
            let low = self.low_key.unwrap_or(current.low);
            let high = self.high_key.unwrap_or(current.high);
            if low > high {
                return Err("low_key must not be above high_key".into());
            }
            zone.keys = Some(auris_sampler::KeyRange::new(low, high));
        }
        if self.low_velocity.is_some() || self.high_velocity.is_some() {
            let low = self.low_velocity.unwrap_or(zone.velocity.low);
            let high = self.high_velocity.unwrap_or(zone.velocity.high);
            if low > high {
                return Err("low_velocity must not be above high_velocity".into());
            }
            zone.velocity = auris_sampler::VelocityRange::new(low, high);
        }
        if let Some(tune) = self.tune {
            zone.tune = tune;
        }
        if let Some(gain) = self.gain_db {
            zone.gain_db = gain;
        }
        Ok(zone)
    }
}

/// Reads a root note written as a MIDI number or a name such as `C4`, `F#3` or `Bb2`.
///
/// Middle C is C4 = 60, which is what the rest of the application calls it.
pub fn parse_root_note(text: &str) -> Option<u8> {
    let text = text.trim();
    if let Ok(number) = text.parse::<u8>() {
        return (number <= HIGHEST_NOTE).then_some(number);
    }
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let base: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (accidental, octave) = match rest.chars().next() {
        Some('#') | Some('♯') => (1, &rest[rest.chars().next()?.len_utf8()..]),
        Some('b') | Some('♭') => (-1, &rest[rest.chars().next()?.len_utf8()..]),
        _ => (0, rest.as_str()),
    };
    let octave: i32 = octave.parse().ok()?;
    let number = (octave + 1) * 12 + base + accidental;
    u8::try_from(number)
        .ok()
        .filter(|number| *number <= HIGHEST_NOTE)
}

/// A note name written into a file name, such as the `C3` of `Piano_C3_soft.wav`.
///
/// Only a word that is a note name and nothing else counts, and a bare number never does: a
/// `01` in a file name is a take number far more often than it is a MIDI note.
fn root_note_in_name(name: &str) -> Option<u8> {
    name.split(|c: char| !(c.is_ascii_alphanumeric() || c == '#' || c == '-'))
        .flat_map(|word| word.split('-').chain(std::iter::once(word)))
        .filter(|word| word.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
        .find_map(parse_root_note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, SessionOptions};
    use auris_core::Ticks;

    fn session() -> Session {
        Session::new(SessionOptions::headless()).unwrap()
    }

    fn write_sine(path: &Path, hz: f64, seconds: f64) {
        let rate = 48_000.0;
        let frames = (seconds * rate) as usize;
        let data: Vec<f32> = (0..frames)
            .map(|n| (std::f64::consts::TAU * hz * n as f64 / rate).sin() as f32 * 0.5)
            .collect();
        let buffer = AudioBuffer::from_planar(vec![data], rate).unwrap();
        auris_io::write_wav(path, &buffer, &Default::default()).unwrap();
    }

    fn instrument_clips(session: &Session, track: TrackId) -> &[auris_core::MidiClip] {
        &session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clips
    }

    #[test]
    fn root_notes_read_as_numbers_and_names() {
        assert_eq!(parse_root_note("60"), Some(60));
        assert_eq!(parse_root_note("C4"), Some(60));
        assert_eq!(parse_root_note("a4"), Some(69));
        assert_eq!(parse_root_note("F#3"), Some(54));
        assert_eq!(parse_root_note("Bb2"), Some(46));
        assert_eq!(parse_root_note("C-1"), Some(0));
        assert_eq!(parse_root_note("G9"), Some(127));
        assert_eq!(parse_root_note("G#9"), None);
        assert_eq!(parse_root_note("H2"), None);
        assert_eq!(parse_root_note("200"), None);
    }

    #[test]
    fn a_note_name_in_a_file_name_is_its_root() {
        assert_eq!(root_note_in_name("Piano_C3_soft"), Some(48));
        assert_eq!(root_note_in_name("vox chop A#4"), Some(70));
        assert_eq!(root_note_in_name("kick_01"), None);
        assert_eq!(root_note_in_name("Bass"), None);
    }

    #[test]
    fn loading_a_sample_puts_the_track_on_the_sampler_with_no_clip() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("pluck.wav");
        write_sine(&wav, 440.0, 0.5);
        let mut session = session();
        let track = session.add_default_instrument_track("Pluck").unwrap();

        let source = session.load_sample(track, &wav).unwrap();

        assert_eq!(session.track_sample_zones(track)[0].source, source);
        assert_eq!(session.track_sample_name(track).as_deref(), Some("pluck"));
        assert!(session.track_sample_is_loaded(track));
        // Nothing went on the timeline: no clip and no audio track.
        assert_eq!(session.project().tracks.len(), 1);
        assert!(instrument_clips(&session, track).is_empty());
        let instrument = session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap();
        assert_eq!(instrument.instrument_id, AUDIO_SAMPLER_ID);

        session.undo();
        assert!(session.track_sample_zones(track).is_empty());
        assert_ne!(
            session
                .project()
                .track(track)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap()
                .instrument_id,
            AUDIO_SAMPLER_ID
        );
    }

    #[test]
    fn a_loaded_sample_is_heard_through_the_render() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("tone.wav");
        write_sine(&wav, 440.0, 1.0);
        let mut session = session();
        let track = session.add_default_instrument_track("Tone").unwrap();
        session.load_sample(track, &wav).unwrap();
        let clip = session
            .add_midi_clip(track, "Tone", Ticks::ZERO, Ticks(3_840))
            .unwrap();
        session
            .add_note(clip, auris_core::Note::new(72, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let options = auris_engine::OfflineOptions {
            end_frames: Some(24_000),
            include_tail: false,
            ..Default::default()
        };
        let rendered = session
            .render_job()
            .render(&options, &mut Default::default())
            .unwrap();
        assert!(rendered.peak() > 0.1, "peak {}", rendered.peak());
    }

    #[test]
    fn a_saved_project_keeps_its_sample_inside_the_folder() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("hit.wav");
        write_sine(&wav, 220.0, 0.2);
        let mut session = session();
        let project = directory.path().join("Song.auris");
        session.save_as(&project).unwrap();
        let track = session.add_default_instrument_track("Hit").unwrap();
        let source = session.load_sample(track, &wav).unwrap();
        let stored = &session.project().audio_sources[&source].path;
        assert!(stored.is_inside(), "{stored}");
    }

    fn agent_sample(path: &Path) -> AgentSample {
        AgentSample {
            path: path.display().to_string(),
            root_note: None,
            low_key: None,
            high_key: None,
            low_velocity: None,
            high_velocity: None,
            tune: None,
            gain_db: None,
        }
    }

    #[test]
    fn the_agent_loads_a_sample_into_the_project_folder() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("Choir_A3.wav");
        write_sine(&wav, 220.0, 0.5);
        let mut session = session();
        session
            .save_as(&directory.path().join("Song.auris"))
            .unwrap();
        let track = session.add_default_instrument_track("Choir").unwrap();
        let reply = session
            .agent_command(crate::live_agent::Command::LoadSample {
                track: track.0,
                samples: vec![agent_sample(&wav)],
                append: None,
                looped: Some(true),
            })
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        // A3 from the file name, since none was given.
        assert_eq!(reply["zones"][0]["root_note"], 57);
        assert_eq!(reply["loop"], true);
        let source = session.track_sample_zones(track)[0].source;
        assert!(session.project().audio_sources[&source].path.is_inside());
        assert!(session.track_sample_is_loaded(track));

        let refused = session.agent_command(crate::live_agent::Command::LoadSample {
            track: track.0,
            samples: vec![agent_sample(Path::new("relative.wav"))],
            append: None,
            looped: None,
        });
        assert!(refused.is_err());
    }

    #[test]
    fn the_agent_builds_a_layered_multisample_and_edits_it() {
        let directory = tempfile::tempdir().unwrap();
        let low = directory.path().join("Keys_C3.wav");
        let soft = directory.path().join("Keys_C4_soft.wav");
        let hard = directory.path().join("Keys_C4_hard.wav");
        write_sine(&low, 130.81, 0.3);
        write_sine(&soft, 261.63, 0.3);
        write_sine(&hard, 261.63, 0.3);
        let mut session = session();
        session
            .save_as(&directory.path().join("Song.auris"))
            .unwrap();
        let track = session.add_default_instrument_track("Keys").unwrap();
        let reply = session
            .agent_command(crate::live_agent::Command::LoadSample {
                track: track.0,
                samples: vec![
                    agent_sample(&low),
                    AgentSample {
                        high_velocity: Some(0.6),
                        ..agent_sample(&soft)
                    },
                    AgentSample {
                        low_velocity: Some(0.6),
                        gain_db: Some(-2.0),
                        ..agent_sample(&hard)
                    },
                ],
                append: None,
                looped: None,
            })
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["zones"].as_array().unwrap().len(), 3);
        assert_eq!(reply["zones"][1]["high_velocity"], 0.6);
        assert_eq!(reply["zones"][2]["gain_db"], -2.0);
        let zones = session.track_sample_zones(track);
        assert_eq!(
            zones.iter().map(|z| z.root).collect::<Vec<_>>(),
            [48, 60, 60]
        );
        // One undo takes the whole batch.
        session.undo();
        assert!(session.track_sample_zones(track).is_empty());
        session.redo();
        assert_eq!(session.track_sample_zones(track).len(), 3);

        // Move the low zone, give it fixed keys, and drop the hard layer, in one call.
        let edited = session
            .agent_command(crate::live_agent::Command::EditSampleMap {
                track: track.0,
                edits: vec![SampleZoneEdit {
                    zone: 0,
                    root_note: Some(47),
                    low_key: Some(0),
                    high_key: Some(52),
                    auto_keys: None,
                    low_velocity: None,
                    high_velocity: None,
                    tune: None,
                    gain_db: None,
                }],
                remove: vec![2],
            })
            .unwrap();
        let edited: serde_json::Value = serde_json::from_str(&edited).unwrap();
        assert_eq!(edited["zones"].as_array().unwrap().len(), 2);
        assert_eq!(edited["zones"][0]["root_note"], 47);
        assert_eq!(edited["zones"][0]["keys"]["high"], 52);
        // The C4 zone takes every key the fixed range leaves.
        assert_eq!(edited["zones"][1]["plays"]["low"], 53);
        session.undo();
        assert_eq!(session.track_sample_zones(track).len(), 3);

        let refused = session.agent_command(crate::live_agent::Command::EditSampleMap {
            track: track.0,
            edits: vec![],
            remove: vec![7],
        });
        assert!(refused.is_err());
    }

    #[test]
    fn loading_several_files_from_the_window_appends_zones_by_their_names() {
        let directory = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = ["Bass_E1.wav", "Bass_A1.wav", "Bass_D2.wav"]
            .iter()
            .map(|name| directory.path().join(name))
            .collect();
        for path in &paths {
            write_sine(path, 55.0, 0.2);
        }
        let mut session = session();
        let track = session.add_default_instrument_track("Bass").unwrap();
        session.load_samples(track, &paths[..2], false).unwrap();
        session.load_samples(track, &paths[2..], false).unwrap();
        let map = session.track_sample_map(track);
        assert_eq!(
            map.iter().map(|view| view.zone.root).collect::<Vec<_>>(),
            [28, 33, 38]
        );
        assert!(map.iter().all(|view| view.loaded));
        assert_eq!(map[1].plays, Some(auris_sampler::KeyRange::new(31, 35)));
        // Replacing starts the map over.
        session.load_samples(track, &paths[..1], true).unwrap();
        assert_eq!(session.track_sample_zones(track).len(), 1);
        // A zone can be retuned and removed, each one undo.
        let mut zone = session.track_sample_zones(track)[0];
        zone.root = 40;
        session.set_sample_zone(track, 0, zone).unwrap();
        assert_eq!(session.track_sample_zones(track)[0].root, 40);
        session.remove_sample_zone(track, 0).unwrap();
        assert!(session.track_sample_zones(track).is_empty());
        assert!(!session.track_sample_is_loaded(track));
        session.undo();
        assert_eq!(session.track_sample_zones(track)[0].root, 40);
        assert!(session.remove_sample_zone(track, 5).is_err());
    }

    #[test]
    fn an_audio_track_cannot_take_a_sample() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("x.wav");
        write_sine(&wav, 220.0, 0.1);
        let mut session = session();
        let track = session.add_audio_track("Audio");
        assert!(session.load_sample(track, &wav).is_err());
    }
}
