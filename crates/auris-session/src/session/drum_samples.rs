//! A drum track played from the project's own audio files: one recording per lane.
//!
//! There is no separate drum sampler. A drum track is put on the audio sampler with key tracking
//! off, and each file becomes a [`SampleZone::hit`] — its lane's key alone, unpitched there,
//! played to its end. What makes it a *kit* rather than a keyboard is the drum map the track
//! already holds: the lanes stay the document's, so choosing files for a built-in kit's Kick,
//! Snare and Hat keeps the rows the drum editor shows and the roles generation writes for.
//!
//! A lane carrying a hi-hat role puts its hits in [`HI_HAT_CHOKE`], so a closed hat cuts an open
//! one without anybody setting it. That reads the role the user or a kit assigned, never the key
//! or the file name: the map is the only place this project says what a sound is for.

use std::path::Path;
use std::sync::Arc;

use auris_core::project::{DrumMap, DrumRole};
use auris_core::{AudioBuffer, SourceId, TrackId};
use auris_sampler::{MAX_SAMPLE_ZONES, SampleZone};

use super::{Session, decode_audio};
use crate::error::SessionError;
use crate::history::Edit;

/// The choke group a hi-hat lane's hits are put in.
pub const HI_HAT_CHOKE: u8 = 1;

/// The recordings on one drum lane, as the drum editor shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct DrumLaneSample {
    /// The key they play on.
    pub note: u8,
    /// Each recording's display name, in the order round-robin takes them.
    pub names: Vec<String>,
    /// Whether every one of them is loaded.
    pub loaded: bool,
}

/// The choke group a new hit on a lane with `roles` takes, given the group a hit already on its
/// key has.
///
/// A second recording on a key joins the first one's group, so layering a hat cannot leave one
/// take that stops the others and one that does not.
fn choke_for(
    roles: impl IntoIterator<Item = DrumRole>,
    existing: Option<Option<u8>>,
) -> Option<u8> {
    if let Some(existing) = existing {
        return existing;
    }
    roles
        .into_iter()
        .any(|role| matches!(role, DrumRole::ClosedHat | DrumRole::OpenHat))
        .then_some(HI_HAT_CHOKE)
}

impl Session {
    /// Plays an audio file on one key of a drum track, putting the track on the sampler first.
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
    /// file joins them as a round-robin alternate. A key with no lane gets one, named after the
    /// file. A track arriving from another instrument keeps its lanes but nothing else of the old
    /// sound: its other keys are silent until they are given files too.
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
        let existing = zones
            .iter()
            .find(|zone| zone.is_only_on(note))
            .map(|zone| zone.choke);
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
        let choke = choke_for(roles, existing.filter(|_| layer));

        self.record(Edit::LoadSample);
        self.put_on_sampler(track);
        let (source, name) = self.add_sample_source(path, &buffer);
        zones.push(SampleZone {
            choke,
            ..SampleZone::hit(source, note)
        });
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

    /// The files each key of a drum track plays, lowest key first; empty off the sampler.
    ///
    /// Only hits — a zone on one key alone — are listed. A zone spread over a range was put there
    /// through the sampler's own map and belongs to no single lane.
    pub fn drum_lane_samples(&self, track: TrackId) -> Vec<DrumLaneSample> {
        if !self
            .project
            .track(track)
            .is_some_and(|entry| entry.kind.is_drum())
        {
            return Vec::new();
        }
        let mut lanes: Vec<DrumLaneSample> = Vec::new();
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
            match lanes.last_mut().filter(|lane| lane.note == keys.low) {
                Some(lane) => {
                    lane.names.push(name);
                    lane.loaded &= loaded;
                }
                None => lanes.push(DrumLaneSample {
                    note: keys.low,
                    names: vec![name],
                    loaded,
                }),
            }
        }
        lanes
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
    use auris_sampler::{AUDIO_SAMPLER_ID, AUDIO_SAMPLER_KEY_TRACKING_KEY};

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

    #[test]
    fn a_file_on_a_kit_lane_puts_the_track_on_the_sampler_and_keeps_its_lanes() {
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("MyKick.wav");
        write_sine(&wav, 60.0, 0.5);
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let lanes = session.drum_lanes(track).unwrap();
        let kit = session.project().track(track).unwrap().kind.clone();
        let kick = kick_note(&session, track);

        let source = session.load_drum_sample(track, kick, &wav, false).unwrap();
        let inner = session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clone();
        assert_eq!(inner.instrument_id, AUDIO_SAMPLER_ID);
        assert_eq!(
            inner.instrument_state.params[AUDIO_SAMPLER_KEY_TRACKING_KEY],
            0.0
        );
        // The kit's rows are still the editor's, and the existing Kick lane was not renamed.
        assert_eq!(session.drum_lanes(track).unwrap(), lanes);
        assert_eq!(
            session.track_sample_zones(track),
            vec![SampleZone::hit(source, kick)]
        );
        assert_eq!(
            session.drum_lane_samples(track),
            vec![DrumLaneSample {
                note: kick,
                names: vec!["MyKick".into()],
                loaded: true,
            }]
        );
        // One undo is the whole change, kit and all.
        session.undo();
        assert_eq!(session.project().track(track).unwrap().kind, kit);
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
    fn a_layer_joins_the_key_and_a_replacement_takes_it_over() {
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
        session.load_drum_sample(track, hat, &b, true).unwrap();
        session.load_drum_sample(track, kick, &c, false).unwrap();
        let zones = session.track_sample_zones(track);
        // Both hat takes choke together; the kick is in no group.
        let on = |note: u8| -> Vec<Option<u8>> {
            zones
                .iter()
                .filter(|zone| zone.is_only_on(note))
                .map(|zone| zone.choke)
                .collect()
        };
        assert_eq!(on(hat), vec![Some(HI_HAT_CHOKE), Some(HI_HAT_CHOKE)]);
        assert_eq!(on(kick), vec![None]);
        let names = session.drum_lane_samples(track);
        let hat_names = &names.iter().find(|lane| lane.note == hat).unwrap().names;
        assert_eq!(hat_names, &vec!["HatA".to_string(), "HatB".to_string()]);

        session.load_drum_sample(track, hat, &c, false).unwrap();
        let names = session.drum_lane_samples(track);
        let hat_names = &names.iter().find(|lane| lane.note == hat).unwrap().names;
        assert_eq!(hat_names, &vec!["HatC".to_string()]);

        assert!(session.clear_drum_sample(track, hat).unwrap());
        assert!(!session.clear_drum_sample(track, hat).unwrap());
        assert!(
            session
                .drum_lane_samples(track)
                .iter()
                .all(|l| l.note != hat)
        );
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
        assert!(session.drum_lane_samples(track).is_empty());
    }

    #[test]
    fn a_hat_lane_chokes_and_a_layer_follows_the_first_take() {
        assert_eq!(choke_for([DrumRole::OpenHat], None), Some(HI_HAT_CHOKE));
        assert_eq!(choke_for([DrumRole::ClosedHat], None), Some(HI_HAT_CHOKE));
        assert_eq!(choke_for([DrumRole::Kick], None), None);
        assert_eq!(choke_for([], None), None);
        // A layer takes the group of what is already on the key, even none on a hat lane.
        assert_eq!(choke_for([DrumRole::OpenHat], Some(None)), None);
        assert_eq!(choke_for([DrumRole::Kick], Some(Some(3))), Some(3));
    }
}
