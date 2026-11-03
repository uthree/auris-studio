//! Loading one audio file into the audio sampler.
//!
//! The sample is an ordinary audio source of the project — copied into the folder, found again
//! when it moves, reported when it is missing — with no clip on the timeline. The track's
//! instrument state names it by id, which is all [`auris_sampler::AudioSampler`] needs.

use std::path::Path;
use std::sync::Arc;

use auris_core::{AssetPath, AudioBuffer, PluginState, SourceId, TrackId};
use auris_sampler::{
    AUDIO_SAMPLER_ID, AUDIO_SAMPLER_LOOP_KEY, AUDIO_SAMPLER_ROOT_KEY, store_sample, stored_sample,
};

use super::{Session, decode_audio};
use crate::error::SessionError;
use crate::history::Edit;

/// The highest MIDI note a root note may be.
const HIGHEST_NOTE: u8 = 127;

impl Session {
    /// Loads an audio file into a track's sampler, switching the track to the sampler first.
    ///
    /// The file becomes one of the project's audio sources, so it is copied into a saved
    /// project's folder exactly as imported audio is, and nothing is placed on the timeline. A
    /// track already on the sampler keeps its root note, envelope and loop; a track arriving
    /// from another instrument starts from the sampler's defaults, for the reason
    /// [`Self::set_track_preset`] gives. One undo step either way.
    pub fn load_sample(&mut self, track: TrackId, path: &Path) -> Result<SourceId, SessionError> {
        let buffer = decode_audio(path, self.project.sample_rate)?;
        self.place_sample(track, path, buffer)
    }

    /// [`Self::load_sample`] for audio that has already been decoded, at the project's rate.
    ///
    /// The split is [`Self::place_audio`]'s: a frontend decodes on a worker and hands the result
    /// to the thread holding the session.
    pub fn place_sample(
        &mut self,
        track: TrackId,
        path: &Path,
        buffer: AudioBuffer,
    ) -> Result<SourceId, SessionError> {
        self.require_sampler_target(track)?;
        let buffer = match buffer.sample_rate() == self.project.sample_rate {
            true => buffer,
            false => decode_audio(path, self.project.sample_rate)?,
        };
        if buffer.frame_count() == 0 || buffer.channel_count() == 0 {
            return Err(SessionError::EmptySample(path.to_path_buf()));
        }
        self.record(Edit::LoadSample);
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Sample".to_string());
        let source = self.project.add_audio_source(
            name,
            AssetPath::external(path),
            buffer.frame_count() as u64,
            buffer.sample_rate(),
            buffer.channel_count(),
        );
        self.record_source_size(source, path);
        if self.path.is_some()
            && let Err(error) = self.collect_source(source, path)
        {
            log::warn!("could not collect {}: {error}", path.display());
        }
        let fresh = self.point_sampler_at(track, source);
        if let Some(root) = path
            .file_stem()
            .and_then(|stem| root_note_in_name(&stem.to_string_lossy()))
            .filter(|_| fresh)
        {
            self.set_sampler_value(track, AUDIO_SAMPLER_ROOT_KEY, f32::from(root));
        }
        self.install_source(source, Arc::new(buffer));
        self.invalidate_graph();
        Ok(source)
    }

    /// Which source a track's audio sampler plays, or `None` when it is not on the sampler or
    /// names none.
    pub fn track_sample(&self, track: TrackId) -> Option<SourceId> {
        let instrument = self.project.track(track)?.kind.as_instrument()?;
        (instrument.instrument_id == AUDIO_SAMPLER_ID)
            .then(|| stored_sample(&instrument.instrument_state))
            .flatten()
    }

    /// The display name of the sample a track's audio sampler plays.
    pub fn track_sample_name(&self, track: TrackId) -> Option<String> {
        let source = self.track_sample(track)?;
        Some(self.project.audio_sources.get(&source)?.name.clone())
    }

    /// `true` when the track's sampler names a sample whose audio is loaded.
    pub fn track_sample_is_loaded(&self, track: TrackId) -> bool {
        self.track_sample(track)
            .is_some_and(|source| self.bank.get(source).is_some())
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

    /// Puts the track on the audio sampler, naming `source`, inside an edit already recorded.
    ///
    /// Answers whether the track arrived from another instrument, and so starts from the
    /// sampler's defaults.
    pub(super) fn point_sampler_at(&mut self, track: TrackId, source: SourceId) -> bool {
        let mut swapped = false;
        if let Some(inner) = self
            .project
            .track_mut(track)
            .and_then(|entry| entry.kind.as_instrument_mut())
        {
            if inner.instrument_id != AUDIO_SAMPLER_ID {
                inner.instrument_id = AUDIO_SAMPLER_ID.to_string();
                inner.instrument_state = PluginState::empty();
                inner.file = None;
                swapped = true;
            }
            store_sample(&mut inner.instrument_state, source);
        }
        if swapped {
            self.project.remove_instrument_automation(track);
        }
        swapped
    }

    /// Adopts a sample a model asked for, staged into the project folder by a file job.
    ///
    /// `root` and `looped` override the sampler's settings when given; otherwise a root note
    /// written in the file name is used for a track new to the sampler, as it is from the window.
    pub(crate) fn adopt_agent_sample(
        &mut self,
        staged: crate::live_agent::file_jobs::StagedSource,
        track: TrackId,
        name: String,
        root: Option<u8>,
        looped: Option<bool>,
    ) -> Result<String, String> {
        self.require_sampler_target(track)
            .map_err(|e| e.to_string())?;
        let crate::live_agent::file_jobs::StagedSource {
            file,
            prepared,
            bytes,
            provenance,
        } = staged;
        if self.engine.sample_rate() != prepared.render_rate {
            return Err("Audio device changed while the sample was being prepared".into());
        }
        let buffer = &prepared.buffer;
        if buffer.frame_count() == 0 || buffer.channel_count() == 0 {
            return Err(format!("{name} holds no audio to sample"));
        }
        let filename = file
            .path()
            .file_name()
            .ok_or("Missing staged audio filename")?
            .to_owned();
        let (_file, _path) = file.keep().map_err(|e| e.to_string())?;
        self.begin_transaction(Edit::LoadSample);
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
        let fresh = self.point_sampler_at(track, source);
        let root = root.or_else(|| root_note_in_name(&name).filter(|_| fresh));
        if let Some(root) = root {
            self.set_sampler_value(track, AUDIO_SAMPLER_ROOT_KEY, f32::from(root));
        }
        if let Some(looped) = looped {
            self.set_sampler_value(track, AUDIO_SAMPLER_LOOP_KEY, f32::from(u8::from(looped)));
        }
        self.install_prepared_audio(source, prepared);
        self.end_transaction();
        self.invalidate_graph();
        let state = self
            .project
            .track(track)
            .and_then(|entry| entry.kind.as_instrument())
            .map(|inner| inner.instrument_state.params.clone())
            .unwrap_or_default();
        Ok(serde_json::json!({
            "track": track.0,
            "source": source.0,
            "instrument": AUDIO_SAMPLER_ID,
            "root_note": state.get(AUDIO_SAMPLER_ROOT_KEY).copied().unwrap_or(60.0),
            "loop": state.get(AUDIO_SAMPLER_LOOP_KEY).copied().unwrap_or(0.0) >= 0.5,
            "revision": self.agent_document_revision()?,
            "unsaved": true,
        })
        .to_string())
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

        assert_eq!(session.track_sample(track), Some(source));
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
        assert_eq!(session.track_sample(track), None);
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
                path: wav.display().to_string(),
                root_note: None,
                looped: Some(true),
            })
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        // A3 from the file name, since none was given.
        assert_eq!(reply["root_note"], 57.0);
        assert_eq!(reply["loop"], true);
        let source = session.track_sample(track).unwrap();
        assert!(session.project().audio_sources[&source].path.is_inside());
        assert!(session.track_sample_is_loaded(track));

        let refused = session.agent_command(crate::live_agent::Command::LoadSample {
            track: track.0,
            path: "relative.wav".into(),
            root_note: Some(60),
            looped: None,
        });
        assert!(refused.is_err());
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
