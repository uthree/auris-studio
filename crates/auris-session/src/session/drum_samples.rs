//! The drum sampler: a drum track played from the project's own audio files, one pad per lane.
//!
//! The drum sampler ([`auris_sampler::DRUM_SAMPLER_ID`]) is the audio sampler's player under its
//! own name, with key tracking and the loop taken away. Each file becomes a [`SampleZone::hit`] —
//! its lane's key alone, unpitched there, played to its end — and the recordings on one key are
//! that lane's **pad**. Pads are not stored anywhere of their own: a pad is the zones on one key,
//! and its settings are the settings those zones share, which is why [`Session::set_drum_pad`]
//! writes every zone on the key and a layered or replacing file inherits what the pad already says.
//!
//! What makes it a *kit* rather than a keyboard is the drum map the track already holds. The lanes
//! stay the document's, so putting a built-in kit's track on the drum sampler keeps the Kick,
//! Snare and Hat rows the drum editor shows and the roles generation writes for; they are the
//! pads waiting for files.
//!
//! A lane carrying a hi-hat role puts its first hit in [`HI_HAT_CHOKE`], so a closed hat cuts an
//! open one without anybody setting it. That reads the role the user or a kit assigned, never the
//! key or the file name: the map is the only place this project says what a sound is for.

use std::path::Path;
use std::sync::Arc;

use auris_core::project::{DrumMap, DrumRole};
use auris_core::{AudioBuffer, SourceId, TrackId};
use auris_sampler::{MAX_CHOKE_GROUP, MAX_SAMPLE_ZONES, SampleZone};

use super::{Session, decode_audio};
use crate::error::SessionError;
use crate::history::Edit;

/// The choke group a hi-hat lane's hits are put in.
pub const HI_HAT_CHOKE: u8 = 1;

/// The quietest a pad may be set, in decibels; the sampler's own floor.
pub const PAD_MIN_GAIN_DB: f32 = -60.0;

/// The loudest a pad may be set, in decibels.
pub const PAD_MAX_GAIN_DB: f32 = 12.0;

/// How far a pad may be retuned either way, in semitones.
pub const PAD_TUNE_RANGE: f32 = 24.0;

/// The key a new pad goes on when the kit has nothing there: General MIDI's bass drum.
const FIRST_PAD_KEY: u8 = 36;

/// The key a new pad goes on: the first one from [`FIRST_PAD_KEY`] up that nothing in `used`
/// names, then the first below it; `None` when all 128 are taken.
///
/// Upwards from the bass drum because that is where General MIDI keeps a kit, so pads added one
/// after another land where a keyboard or a General MIDI file expects drums to be.
fn free_pad_key(used: &[u8]) -> Option<u8> {
    (FIRST_PAD_KEY..=127)
        .chain(0..FIRST_PAD_KEY)
        .find(|key| !used.contains(key))
}

/// The recordings on one key of a drum track, and the settings they share.
#[derive(Clone, Debug, PartialEq)]
pub struct DrumPad {
    /// The key they play on.
    pub note: u8,
    /// Each recording's display name, in the order round-robin takes them.
    pub names: Vec<String>,
    /// Whether every one of them is loaded.
    pub loaded: bool,
    /// Level, in decibels.
    pub gain_db: f32,
    /// Tuning, in semitones: a drum sampler pad is retuned rather than played at another pitch.
    pub tune: f32,
    /// Plays to the end of the recording whatever the note's length.
    pub one_shot: bool,
    /// The choke group, 1 to [`MAX_CHOKE_GROUP`], or none.
    pub choke: Option<u8>,
}

/// A change to one pad; a field left `None` keeps what the pad has.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DrumPadEdit {
    /// Level, in decibels, [`PAD_MIN_GAIN_DB`] to [`PAD_MAX_GAIN_DB`].
    pub gain_db: Option<f32>,
    /// Tuning, in semitones, within [`PAD_TUNE_RANGE`] either way.
    pub tune: Option<f32>,
    /// Plays to the end of the recording whatever the note's length.
    pub one_shot: Option<bool>,
    /// `Some(None)` takes the pad out of its choke group.
    pub choke: Option<Option<u8>>,
}

/// The zone a new file on a pad starts from: what the pad already says, or a fresh hit.
///
/// A second recording on a key — a layer or a replacement — takes the first one's level, tuning,
/// one-shot and choke group, so a pad cannot end up with one take that stops the others and one
/// that does not. A key with nothing on it is a one-shot hit, choked with the hats when its lane
/// is a hi-hat's.
fn pad_template(
    source: SourceId,
    note: u8,
    roles: impl IntoIterator<Item = DrumRole>,
    existing: Option<&SampleZone>,
) -> SampleZone {
    let hit = SampleZone::hit(source, note);
    match existing {
        Some(existing) => SampleZone {
            gain_db: existing.gain_db,
            tune: existing.tune,
            one_shot: existing.one_shot,
            choke: existing.choke,
            velocity: existing.velocity,
            ..hit
        },
        None => SampleZone {
            choke: roles
                .into_iter()
                .any(|role| matches!(role, DrumRole::ClosedHat | DrumRole::OpenHat))
                .then_some(HI_HAT_CHOKE),
            ..hit
        },
    }
}

/// Checks a pad edit's values, or says which is out of range.
fn checked_edit(edit: DrumPadEdit) -> Result<DrumPadEdit, SessionError> {
    let invalid = |what: String| Err(SessionError::SampleMap(what));
    if let Some(gain) = edit.gain_db
        && !(gain.is_finite() && (PAD_MIN_GAIN_DB..=PAD_MAX_GAIN_DB).contains(&gain))
    {
        return invalid(format!(
            "a pad's level is {PAD_MIN_GAIN_DB}..{PAD_MAX_GAIN_DB} dB"
        ));
    }
    if let Some(tune) = edit.tune
        && !(tune.is_finite() && tune.abs() <= PAD_TUNE_RANGE)
    {
        return invalid(format!(
            "a pad's tuning is -{PAD_TUNE_RANGE}..{PAD_TUNE_RANGE} semitones"
        ));
    }
    if let Some(Some(group)) = edit.choke
        && !(1..=MAX_CHOKE_GROUP).contains(&group)
    {
        return invalid(format!("choke groups are 1..{MAX_CHOKE_GROUP}"));
    }
    Ok(edit)
}

impl Session {
    /// Plays an audio file on one key of a drum track, putting the track on the drum sampler
    /// first.
    ///
    /// [`Self::place_drum_sample`] after decoding the file at the project's rate.
    pub fn load_drum_sample(
        &mut self,
        track: TrackId,
        note: u8,
        path: &Path,
        layer: bool,
    ) -> Result<SourceId, SessionError> {
        let buffer = decode_audio(path, self.project.sample_rate)?;
        self.place_drum_sample(track, note, path, buffer, layer)
    }

    /// Plays decoded audio on one key of a drum track, in one undo step.
    ///
    /// The file becomes one of the project's audio sources, copied into a saved project's folder
    /// as imported audio is. Without `layer` it replaces whatever the key played; with it the
    /// file joins them as a round-robin alternate. Either way it keeps the pad's settings. A key
    /// with no lane gets one, named after the file. A track on another instrument is put on the
    /// drum sampler first and keeps its lanes but nothing else of the old sound: its other keys
    /// are silent until they are given files too.
    pub fn place_drum_sample(
        &mut self,
        track: TrackId,
        note: u8,
        path: &Path,
        buffer: AudioBuffer,
        layer: bool,
    ) -> Result<SourceId, SessionError> {
        if note > 127 {
            return Err(SessionError::InvalidDrumAssignment(format!(
                "MIDI note {note} is outside 0..127"
            )));
        }
        let map = self.checked_drum_map(track)?;
        let buffer = self.checked_sample(path, buffer)?;
        let mut zones = self.track_sample_zones(track);
        let existing = zones.iter().find(|zone| zone.is_only_on(note)).copied();
        if !layer {
            zones.retain(|zone| !zone.is_only_on(note));
        }
        if zones.len() >= MAX_SAMPLE_ZONES {
            return Err(SessionError::SampleMap(format!(
                "a sampler holds at most {MAX_SAMPLE_ZONES} samples"
            )));
        }
        let roles = map
            .lanes
            .iter()
            .filter(|lane| lane.note == note)
            .flat_map(|lane| lane.roles.iter().copied())
            .collect::<Vec<_>>();

        self.record(Edit::LoadSample);
        self.put_on_sampler(track);
        let (source, name) = self.add_sample_source(path, &buffer);
        zones.push(pad_template(source, note, roles, existing.as_ref()));
        self.write_zones(track, &zones);
        let mut map = DrumMap::load(&self.track_state(track)).unwrap_or_default();
        if !map.lanes.iter().any(|lane| lane.note == note) {
            map.add_lane(note, name);
            if let Some(inner) = self
                .project
                .track_mut(track)
                .and_then(|entry| entry.kind.as_instrument_mut())
            {
                map.store(&mut inner.instrument_state);
            }
        }
        self.install_source(source, Arc::new(buffer));
        self.invalidate_graph();
        Ok(source)
    }

    /// Plays decoded audio on a new pad of a drum track — a key no lane or file uses yet — and
    /// answers that key.
    ///
    /// The pad's lane is named after the file. Otherwise [`Self::place_drum_sample`].
    pub fn add_drum_pad(
        &mut self,
        track: TrackId,
        path: &Path,
        buffer: AudioBuffer,
    ) -> Result<u8, SessionError> {
        self.checked_drum_map(track)?;
        let note = self.free_drum_pad_key(track).ok_or_else(|| {
            SessionError::InvalidDrumAssignment("every MIDI key already has a pad".into())
        })?;
        self.place_drum_sample(track, note, path, buffer, false)?;
        Ok(note)
    }

    /// The key [`Self::add_drum_pad`] would use next, when there is one.
    pub fn free_drum_pad_key(&self, track: TrackId) -> Option<u8> {
        let mut used: Vec<u8> = self
            .drum_lanes(track)
            .unwrap_or_default()
            .iter()
            .map(|lane| lane.note)
            .collect();
        used.extend(self.drum_pads(track).iter().map(|pad| pad.note));
        free_pad_key(&used)
    }

    /// Takes a pad off a drum track — its recordings and the lane on its key — in one undo step.
    ///
    /// Notes already written on the key stay in the clips, as they do when a lane is removed.
    /// Answers `false`, changing nothing, when the key has neither.
    pub fn remove_drum_pad(&mut self, track: TrackId, note: u8) -> Result<bool, SessionError> {
        let mut map = self.checked_drum_map(track)?;
        let before = self.track_sample_zones(track);
        let mut zones = before.clone();
        zones.retain(|zone| !zone.is_only_on(note));
        let lanes = map.lanes.len();
        map.lanes.retain(|lane| lane.note != note);
        if zones.len() == before.len() && map.lanes.len() == lanes {
            return Ok(false);
        }
        map.sync_voices_from_lanes();
        self.record(Edit::SetDrumAssignment);
        if zones.len() != before.len() {
            self.write_zones(track, &zones);
        }
        if let Some(inner) = self
            .project
            .track_mut(track)
            .and_then(|entry| entry.kind.as_instrument_mut())
        {
            map.store(&mut inner.instrument_state);
        }
        self.invalidate_graph();
        Ok(true)
    }

    /// Takes every recording off one key of a drum track, in one undo step; the lane stays.
    ///
    /// Answers `false`, changing nothing, when the key played no file.
    pub fn clear_drum_sample(&mut self, track: TrackId, note: u8) -> Result<bool, SessionError> {
        self.checked_drum_map(track)?;
        let mut zones = self.track_sample_zones(track);
        let before = zones.len();
        zones.retain(|zone| !zone.is_only_on(note));
        if zones.len() == before {
            return Ok(false);
        }
        self.set_sample_zones(track, zones)?;
        Ok(true)
    }

    /// Changes one pad of a drum track — every recording on its key — in one undo step.
    ///
    /// Answers `false`, changing nothing, when the key plays no file or the pad already says so.
    pub fn set_drum_pad(
        &mut self,
        track: TrackId,
        note: u8,
        edit: DrumPadEdit,
    ) -> Result<bool, SessionError> {
        self.checked_drum_map(track)?;
        let edit = checked_edit(edit)?;
        let before = self.track_sample_zones(track);
        let mut zones = before.clone();
        for zone in zones.iter_mut().filter(|zone| zone.is_only_on(note)) {
            if let Some(gain) = edit.gain_db {
                zone.gain_db = gain;
            }
            if let Some(tune) = edit.tune {
                zone.tune = tune;
            }
            if let Some(one_shot) = edit.one_shot {
                zone.one_shot = one_shot;
            }
            if let Some(choke) = edit.choke {
                zone.choke = choke;
            }
        }
        if zones == before {
            return Ok(false);
        }
        self.set_sample_zones(track, zones)?;
        Ok(true)
    }

    /// The pads of a drum track, lowest key first; empty off the samplers.
    ///
    /// Only hits — a zone on one key alone — make a pad. A zone spread over a range was put there
    /// through the audio sampler's own map and belongs to no single lane. A pad's settings are
    /// its first recording's, which [`Self::set_drum_pad`] and a new file keep the same for all.
    pub fn drum_pads(&self, track: TrackId) -> Vec<DrumPad> {
        if !self
            .project
            .track(track)
            .is_some_and(|entry| entry.kind.is_drum())
        {
            return Vec::new();
        }
        let mut pads: Vec<DrumPad> = Vec::new();
        let mut zones = self.track_sample_zones(track);
        zones.sort_by_key(|zone| zone.root);
        for zone in zones {
            let Some(keys) = zone.keys.filter(|keys| keys.low == keys.high) else {
                continue;
            };
            let name = self
                .project
                .audio_sources
                .get(&zone.source)
                .map_or_else(|| format!("#{}", zone.source.0), |s| s.name.clone());
            let loaded = self.bank.get(zone.source).is_some();
            match pads.last_mut().filter(|pad| pad.note == keys.low) {
                Some(pad) => {
                    pad.names.push(name);
                    pad.loaded &= loaded;
                }
                None => pads.push(DrumPad {
                    note: keys.low,
                    names: vec![name],
                    loaded,
                    gain_db: zone.gain_db,
                    tune: zone.tune,
                    one_shot: zone.one_shot,
                    choke: zone.choke,
                }),
            }
        }
        pads
    }

    /// `true` when a drum track plays the drum sampler, so its lanes take files.
    pub fn is_on_drum_sampler(&self, track: TrackId) -> bool {
        self.project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .is_some_and(|inner| inner.instrument_id == auris_sampler::DRUM_SAMPLER_ID)
    }

    /// The instrument state a track stores, or an empty one.
    fn track_state(&self, track: TrackId) -> auris_core::PluginState {
        self.project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .map(|inner| inner.instrument_state.clone())
            .unwrap_or_else(auris_core::PluginState::empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, SessionOptions};
    use auris_core::Ticks;
    use auris_sampler::{AUDIO_SAMPLER_ID, DRUM_SAMPLER_ID};

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

    fn kick_note(session: &Session, track: TrackId) -> u8 {
        session.drum_assignments(track).unwrap().voices[&DrumRole::Kick]
    }

    fn instrument_id(session: &Session, track: TrackId) -> String {
        session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_id
            .clone()
    }

    #[test]
    fn a_file_on_a_kit_lane_puts_the_track_on_the_drum_sampler_and_keeps_its_lanes() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("MyKick.wav");
        write_sine(&wav, 60.0, 0.5);
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let lanes = session.drum_lanes(track).unwrap();
        let kit = session.project().track(track).unwrap().kind.clone();
        let kick = kick_note(&session, track);
        assert!(!session.is_on_drum_sampler(track));

        let source = session.load_drum_sample(track, kick, &wav, false).unwrap();
        assert_eq!(instrument_id(&session, track), DRUM_SAMPLER_ID);
        assert!(session.is_on_drum_sampler(track));
        // The kit's rows are still the editor's, and the existing Kick lane was not renamed.
        assert_eq!(session.drum_lanes(track).unwrap(), lanes);
        assert_eq!(
            session.track_sample_zones(track),
            vec![SampleZone::hit(source, kick)]
        );
        assert_eq!(
            session.drum_pads(track),
            vec![DrumPad {
                note: kick,
                names: vec!["MyKick".into()],
                loaded: true,
                gain_db: 0.0,
                tune: 0.0,
                one_shot: true,
                choke: None,
            }]
        );
        // One undo is the whole change, kit and all.
        session.undo();
        assert_eq!(session.project().track(track).unwrap().kind, kit);
    }

    #[test]
    fn choosing_the_drum_sampler_keeps_the_lanes_as_empty_pads() {
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let lanes = session.drum_lanes(track).unwrap();
        session
            .set_track_instrument(track, DRUM_SAMPLER_ID)
            .unwrap();
        assert_eq!(session.drum_lanes(track).unwrap(), lanes);
        assert!(session.drum_pads(track).is_empty());
        // A map remembered per source would name another project's files.
        assert!(session.drum_map_source(track).is_none());
        assert!(session.suggested_drum_map(track).is_none());
    }

    #[test]
    fn a_drum_track_already_on_the_audio_sampler_stays_there() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("Hit.wav");
        write_sine(&wav, 440.0, 0.1);
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        session
            .set_track_instrument(track, AUDIO_SAMPLER_ID)
            .unwrap();
        session.load_drum_sample(track, 36, &wav, false).unwrap();
        assert_eq!(instrument_id(&session, track), AUDIO_SAMPLER_ID);
        assert_eq!(session.drum_pads(track).len(), 1);
    }

    #[test]
    fn a_new_key_gets_a_lane_named_after_its_file() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("Clap.wav");
        write_sine(&wav, 1_000.0, 0.1);
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let free = (0..=127u8)
            .find(|note| {
                !session
                    .drum_lanes(track)
                    .unwrap()
                    .iter()
                    .any(|lane| lane.note == *note)
            })
            .unwrap();
        session.load_drum_sample(track, free, &wav, false).unwrap();
        let lane = session
            .drum_lanes(track)
            .unwrap()
            .into_iter()
            .find(|lane| lane.note == free)
            .unwrap();
        assert_eq!(lane.name, "Clap");
        assert!(lane.roles.is_empty());
    }

    #[test]
    fn a_layer_joins_the_pad_and_a_replacement_takes_it_over_keeping_its_settings() {
        let directory = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            directory.path().join("HatA.wav"),
            directory.path().join("HatB.wav"),
            directory.path().join("HatC.wav"),
        );
        for path in [&a, &b, &c] {
            write_sine(path, 8_000.0, 0.1);
        }
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let map = session.drum_assignments(track).unwrap();
        let hat = map.voices[&DrumRole::ClosedHat];
        let kick = map.voices[&DrumRole::Kick];
        session.load_drum_sample(track, hat, &a, false).unwrap();
        session
            .set_drum_pad(
                track,
                hat,
                DrumPadEdit {
                    gain_db: Some(-6.0),
                    ..Default::default()
                },
            )
            .unwrap();
        session.load_drum_sample(track, hat, &b, true).unwrap();
        session.load_drum_sample(track, kick, &c, false).unwrap();
        let zones = session.track_sample_zones(track);
        // Both hat takes choke together at the pad's level; the kick is in no group.
        let on = |note: u8| -> Vec<(Option<u8>, f32)> {
            zones
                .iter()
                .filter(|zone| zone.is_only_on(note))
                .map(|zone| (zone.choke, zone.gain_db))
                .collect()
        };
        assert_eq!(
            on(hat),
            vec![(Some(HI_HAT_CHOKE), -6.0), (Some(HI_HAT_CHOKE), -6.0)]
        );
        assert_eq!(on(kick), vec![(None, 0.0)]);
        let pads = session.drum_pads(track);
        let hat_pad = pads.iter().find(|pad| pad.note == hat).unwrap();
        assert_eq!(hat_pad.names, vec!["HatA".to_string(), "HatB".to_string()]);

        session.load_drum_sample(track, hat, &c, false).unwrap();
        let pads = session.drum_pads(track);
        let hat_pad = pads.iter().find(|pad| pad.note == hat).unwrap();
        assert_eq!(hat_pad.names, vec!["HatC".to_string()]);
        assert_eq!((hat_pad.gain_db, hat_pad.choke), (-6.0, Some(HI_HAT_CHOKE)));

        assert!(session.clear_drum_sample(track, hat).unwrap());
        assert!(!session.clear_drum_sample(track, hat).unwrap());
        assert!(session.drum_pads(track).iter().all(|p| p.note != hat));
        // The lane outlives its file.
        assert!(
            session
                .drum_lanes(track)
                .unwrap()
                .iter()
                .any(|l| l.note == hat)
        );
    }

    #[test]
    fn a_pad_edit_reaches_every_take_on_its_key_as_one_undo() {
        let directory = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            directory.path().join("A.wav"),
            directory.path().join("B.wav"),
            directory.path().join("C.wav"),
        );
        for path in [&a, &b, &c] {
            write_sine(path, 200.0, 0.1);
        }
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        session.load_drum_sample(track, 38, &a, false).unwrap();
        session.load_drum_sample(track, 38, &b, true).unwrap();
        session.load_drum_sample(track, 40, &c, false).unwrap();
        let edit = DrumPadEdit {
            gain_db: Some(-3.0),
            tune: Some(2.0),
            one_shot: Some(false),
            choke: Some(Some(4)),
        };
        assert!(session.set_drum_pad(track, 38, edit).unwrap());
        assert!(!session.set_drum_pad(track, 38, edit).unwrap());
        for zone in session.track_sample_zones(track) {
            match zone.root {
                38 => {
                    assert_eq!((zone.gain_db, zone.tune), (-3.0, 2.0));
                    assert_eq!((zone.one_shot, zone.choke), (false, Some(4)));
                }
                _ => assert_eq!(zone, SampleZone::hit(zone.source, 40)),
            }
        }
        session.undo();
        assert!(
            session
                .drum_pads(track)
                .iter()
                .all(|pad| pad.gain_db == 0.0)
        );
        // Out of range is refused whole, and a key with no file changes nothing.
        for bad in [
            DrumPadEdit {
                gain_db: Some(20.0),
                ..Default::default()
            },
            DrumPadEdit {
                tune: Some(f32::NAN),
                ..Default::default()
            },
            DrumPadEdit {
                choke: Some(Some(0)),
                ..Default::default()
            },
        ] {
            assert!(session.set_drum_pad(track, 38, bad).is_err());
        }
        assert!(!session.set_drum_pad(track, 50, edit).unwrap());
    }

    #[test]
    fn a_short_drum_note_rings_for_the_whole_recording() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("Crash.wav");
        write_sine(&wav, 440.0, 1.0);
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let kick = kick_note(&session, track);
        session.load_drum_sample(track, kick, &wav, false).unwrap();
        let clip = session
            .add_midi_clip(track, "Beat", Ticks::ZERO, Ticks(3_840))
            .unwrap();
        // A sixteenth: an eighth of a second at 120 bpm.
        session
            .add_note(
                clip,
                auris_core::Note::new(kick, Ticks::ZERO, Ticks(Ticks::QUARTER.0 / 4)),
            )
            .unwrap();
        let options = auris_engine::OfflineOptions {
            end_frames: Some(48_000),
            include_tail: false,
            ..Default::default()
        };
        let rendered = session
            .render_job()
            .render(&options, &mut Default::default())
            .unwrap();
        // Well past the note and any release, the hit is still sounding.
        let late = &rendered.channel(0)[30_000..40_000];
        let peak = late.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!(peak > 0.05, "peak {peak}");
    }

    #[test]
    fn only_a_drum_track_takes_a_drum_sample() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("Hit.wav");
        write_sine(&wav, 440.0, 0.1);
        let mut session = session();
        let track = session.add_default_instrument_track("Keys").unwrap();
        assert!(matches!(
            session.load_drum_sample(track, 36, &wav, false),
            Err(SessionError::WrongTrackKind { .. })
        ));
        assert!(session.drum_pads(track).is_empty());
    }

    #[test]
    fn a_new_pad_takes_the_first_free_key_from_the_bass_drum_up() {
        assert_eq!(free_pad_key(&[]), Some(36));
        assert_eq!(free_pad_key(&[36, 37, 39]), Some(38));
        let upper: Vec<u8> = (36..=127).collect();
        assert_eq!(free_pad_key(&upper), Some(0));
        let all: Vec<u8> = (0..=127).collect();
        assert_eq!(free_pad_key(&all), None);
    }

    #[test]
    fn pads_are_added_and_removed_from_the_sampler_alone() {
        let directory = tempfile::tempdir().unwrap();
        let (kick, snare) = (
            directory.path().join("Kick.wav"),
            directory.path().join("Snare.wav"),
        );
        write_sine(&kick, 60.0, 0.1);
        write_sine(&snare, 200.0, 0.1);
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        session
            .set_track_instrument(track, DRUM_SAMPLER_ID)
            .unwrap();
        for lane in session.drum_lanes(track).unwrap() {
            session.remove_drum_lane(track, lane.id).unwrap();
        }
        let buffer = |path: &Path| decode_audio(path, 48_000.0).unwrap();
        let first = session.add_drum_pad(track, &kick, buffer(&kick)).unwrap();
        let second = session.add_drum_pad(track, &snare, buffer(&snare)).unwrap();
        assert_eq!((first, second), (36, 37));
        let names: Vec<String> = session
            .drum_lanes(track)
            .unwrap()
            .into_iter()
            .map(|lane| lane.name)
            .collect();
        assert_eq!(names, vec!["Kick".to_string(), "Snare".to_string()]);

        // Removing a pad takes its lane with it, in one undo step.
        assert!(session.remove_drum_pad(track, first).unwrap());
        assert!(!session.remove_drum_pad(track, first).unwrap());
        assert_eq!(session.drum_pads(track).len(), 1);
        assert!(
            session
                .drum_lanes(track)
                .unwrap()
                .iter()
                .all(|l| l.note != first)
        );
        assert_eq!(session.free_drum_pad_key(track), Some(36));
        session.undo();
        assert_eq!(session.drum_pads(track).len(), 2);
        assert_eq!(session.drum_lanes(track).unwrap().len(), 2);
    }

    #[test]
    fn a_new_pad_chokes_on_a_hat_lane_and_a_second_take_follows_the_first() {
        let hit = SampleZone::hit(SourceId(1), 42);
        assert_eq!(
            pad_template(SourceId(1), 42, [DrumRole::OpenHat], None).choke,
            Some(HI_HAT_CHOKE)
        );
        assert_eq!(
            pad_template(SourceId(1), 42, [DrumRole::ClosedHat], None).choke,
            Some(HI_HAT_CHOKE)
        );
        assert_eq!(
            pad_template(SourceId(1), 36, [DrumRole::Kick], None).choke,
            None
        );
        assert_eq!(pad_template(SourceId(1), 42, [], None), hit);
        // What the pad already says wins, even no group on a hat lane.
        let existing = SampleZone {
            gain_db: -4.0,
            ..hit
        };
        let next = pad_template(SourceId(2), 42, [DrumRole::OpenHat], Some(&existing));
        assert_eq!(
            (next.source, next.choke, next.gain_db),
            (SourceId(2), None, -4.0)
        );
    }
}
