//! File-free commands for the agent attached to an open session.

use crate::{Session, prelude::*};

/// An operation on the current document. No operation accepts a filesystem destination.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Read the current arrangement and stable track/clip IDs. Use read_notes for note data.
    Inspect {},
    /// Read a page of notes using zero-based storage indices.
    ReadNotes {
        /// Stable clip ID.
        clip: u64,
        /// First note index; defaults to zero.
        #[serde(default)]
        offset: usize,
    },
    /// Compose into the current document. Replacing existing tracks requires replace=true.
    Compose {
        /// Inline .asong TOML, mutually exclusive with preset.
        spec: Option<String>,
        /// A name returned by list_presets, mutually exclusive with spec.
        preset: Option<String>,
        /// Explicit permission to replace a nonempty arrangement.
        #[serde(default)]
        replace: bool,
    },
    /// Add an empty named track. Use add_clip and add_note to write music by hand.
    AddTrack {
        /// The exact desired name.
        name: String,
        /// Track type.
        kind: TrackKind,
    },
    /// Rename a track using its stable numeric ID from inspect.
    RenameTrack {
        /// Stable track ID.
        track: u64,
        /// New name.
        name: String,
    },
    /// Remove one track and its clips.
    RemoveTrack {
        /// Stable track ID.
        track: u64,
    },
    /// Choose a built-in instrument from list_instruments.
    SetInstrument {
        /// Stable track ID.
        track: u64,
        /// Built-in instrument identifier.
        instrument: String,
    },
    /// Set a fader and pan using native units.
    SetLevel {
        /// Stable track ID.
        track: u64,
        /// Fader in decibels, -60 through 12.
        gain_db: f32,
        /// Pan, -1 through 1.
        pan: f32,
    },
    /// Set mute and solo for one track.
    SetTrackState {
        /// Stable track ID.
        track: u64,
        /// Whether muted.
        mute: bool,
        /// Whether soloed.
        solo: bool,
    },
    /// Add an empty MIDI clip at a 1-based bar position.
    AddClip {
        /// Stable track ID.
        track: u64,
        /// Exact clip name.
        name: String,
        /// First bar, starting at 1.
        start_bar: u32,
        /// Duration in bars, 1 through 1024.
        bars: u32,
    },
    /// Add one note using quarter-note beats relative to the clip start (zero-based).
    AddNote {
        /// Stable clip ID from inspect or add_clip.
        clip: u64,
        /// MIDI pitch, 0 through 127.
        pitch: u8,
        /// Start in quarter-note beats relative to the clip, starting at zero.
        start: f64,
        /// Duration in quarter-note beats.
        beats: f64,
        /// Velocity, 0 through 1.
        velocity: f32,
    },
    /// Remove notes using zero-based storage indices from read_notes.
    RemoveNotes {
        /// Stable clip ID.
        clip: u64,
        /// Zero-based note indices.
        indices: Vec<usize>,
    },
}

/// Types of empty tracks the agent can add.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// A melodic instrument.
    Instrument,
    /// A percussion instrument.
    Drum,
    /// A singing track.
    Singer,
    /// An audio track.
    Audio,
    /// A mixer bus.
    Bus,
}

impl Session {
    /// Execute a file-free command against this session, preserving ordinary undo semantics.
    pub fn agent_command(&mut self, command: Command) -> Result<String, String> {
        let error = |error: crate::SessionError| error.to_string();
        match command {
            Command::Inspect {} => {
                let project = self.project();
                let tracks: Vec<_> = project
                    .tracks
                    .iter()
                    .map(|track| {
                        let clips: Vec<_> = track
                            .kind
                            .note_clips()
                            .into_iter()
                            .flatten()
                            .map(|clip| {
                                serde_json::json!({"id":clip.id.0, "name":clip.name,
                            "start_tick":clip.start.raw(), "length_ticks":clip.length.raw(),
                            "note_count":clip.notes.len()})
                            })
                            .collect();
                        serde_json::json!({"id":track.id.0, "name":track.name,
                        "kind":track.kind.label(), "mixer":track.mixer, "clips":clips})
                    })
                    .collect();
                Ok(serde_json::json!({"title":project.name, "tracks":tracks,
                    "duration_seconds":project.duration_seconds(), "harmony":project.harmony,
                    "sections":project.sections, "ticks_per_quarter":Ticks::QUARTER.raw()})
                .to_string())
            }
            Command::ReadNotes { clip, offset } => {
                let (_, clip) = self
                    .project()
                    .midi_clip(ClipId(clip))
                    .ok_or("Unknown MIDI clip ID")?;
                if offset > clip.notes.len() {
                    return Err("Offset exceeds the note count".into());
                }
                let notes: Vec<_> = clip
                    .notes
                    .iter()
                    .enumerate()
                    .skip(offset)
                    .take(128)
                    .map(|(index, note)| serde_json::json!({"index":index, "note":note}))
                    .collect();
                let next = offset + notes.len();
                Ok(serde_json::json!({"notes":notes, "total":clip.notes.len(),
                    "next_offset":(next < clip.notes.len()).then_some(next),
                    "ticks_per_quarter":Ticks::QUARTER.raw()})
                .to_string())
            }
            Command::Compose {
                spec,
                preset: name,
                replace,
            } => {
                if !replace && !self.project().tracks.is_empty() {
                    return Err("The arrangement is not empty. Edit its tracks, or pass replace=true only when the user requests a replacement.".into());
                }
                let source = match (spec, name) {
                    (Some(spec), None) => spec,
                    (None, Some(name)) => preset(&name)
                        .ok_or("Unknown preset; use list_presets")?
                        .source
                        .to_string(),
                    _ => return Err("Pass exactly one of spec or preset".into()),
                };
                let spec = SongSpec::parse(&source).map_err(|e| format!("{e:?}"))?;
                let piece = compose(&spec);
                let report = self.compose_without_balance(&piece).map_err(error)?;
                Ok(format!(
                    "Composed {} tracks and {} notes in the current document. Changes are unsaved.",
                    report.tracks, report.notes
                ))
            }
            Command::AddTrack { name, kind } => {
                if name.trim().is_empty() {
                    return Err("A track needs a name".into());
                }
                let id = match kind {
                    TrackKind::Instrument => {
                        self.add_default_instrument_track(name).map_err(error)?
                    }
                    TrackKind::Drum => self.add_default_drum_track(name).map_err(error)?,
                    TrackKind::Singer => self.add_singer_track(name),
                    TrackKind::Audio => self.add_audio_track(name),
                    TrackKind::Bus => self.add_bus_track(name),
                };
                Ok(format!("Added track ID {}", id.0))
            }
            Command::RenameTrack { track, name } => {
                if name.trim().is_empty() {
                    return Err("A track needs a name".into());
                }
                self.rename_track(TrackId(track), name).map_err(error)?;
                Ok("Renamed track".into())
            }
            Command::RemoveTrack { track } => {
                self.remove_track(TrackId(track)).map_err(error)?;
                Ok("Removed track".into())
            }
            Command::SetInstrument { track, instrument } => {
                self.set_track_instrument(TrackId(track), &instrument)
                    .map_err(error)?;
                Ok("Changed instrument".into())
            }
            Command::SetLevel {
                track,
                gain_db,
                pan,
            } => {
                if !(-60.0..=12.0).contains(&gain_db) || !(-1.0..=1.0).contains(&pan) {
                    return Err("gain_db must be -60..12 and pan -1..1".into());
                }
                if self.project().track(TrackId(track)).is_none() {
                    return Err("Unknown track ID".into());
                }
                self.begin_transaction(crate::Edit::ExternalChanges);
                self.set_param(crate::ParamTarget::TrackGain(TrackId(track)), gain_db);
                self.set_param(crate::ParamTarget::TrackPan(TrackId(track)), pan);
                self.end_transaction();
                Ok("Changed level and pan".into())
            }
            Command::SetTrackState { track, mute, solo } => {
                self.begin_transaction(crate::Edit::ExternalChanges);
                let result = self
                    .set_track_mute(TrackId(track), mute)
                    .and_then(|()| self.set_track_solo(TrackId(track), solo));
                self.end_transaction();
                result.map_err(error)?;
                Ok("Changed mute and solo".into())
            }
            Command::AddClip {
                track,
                name,
                start_bar,
                bars,
            } => {
                if name.trim().is_empty() || start_bar == 0 || !(1..=1024).contains(&bars) {
                    return Err("Use a name, start_bar >= 1 and bars 1..1024".into());
                }
                let end = start_bar.checked_add(bars).ok_or("Bar range overflow")?;
                let start = self.project().signatures.bar_start(start_bar);
                let length = self.project().signatures.bar_start(end) - start;
                let id = self
                    .add_midi_clip(TrackId(track), name, start, length)
                    .map_err(error)?;
                Ok(format!("Added clip ID {}", id.0))
            }
            Command::AddNote {
                clip,
                pitch,
                start,
                beats,
                velocity,
            } => {
                if pitch > 127
                    || !start.is_finite()
                    || start < 0.0
                    || !beats.is_finite()
                    || beats <= 0.0
                    || !(0.0..=1.0).contains(&velocity)
                {
                    return Err("Invalid note pitch, timing or velocity".into());
                }
                let (_, target) = self
                    .project()
                    .midi_clip(ClipId(clip))
                    .ok_or("Unknown MIDI clip ID")?;
                let start = Ticks::from_beats(start);
                let length = Ticks::from_beats(beats);
                if length.raw() < 1
                    || start
                        .raw()
                        .checked_add(length.raw())
                        .is_none_or(|end| end > target.length.raw())
                {
                    return Err("Note must fit inside the clip".into());
                }
                let index = self
                    .add_note(
                        ClipId(clip),
                        Note {
                            velocity,
                            ..Note::new(pitch, start, length)
                        },
                    )
                    .map_err(error)?;
                Ok(format!("Added note index {index}"))
            }
            Command::RemoveNotes { clip, indices } => {
                let (_, target) = self
                    .project()
                    .midi_clip(ClipId(clip))
                    .ok_or("Unknown MIDI clip ID")?;
                if indices.iter().any(|&i| i >= target.notes.len()) {
                    return Err("Unknown note index".into());
                }
                self.remove_notes(ClipId(clip), &indices).map_err(error)?;
                Ok("Removed notes".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap()
    }

    #[test]
    fn composition_changes_the_open_document_without_creating_a_file() {
        let mut session = session();
        let before = session.project().clone();
        let name = PRESETS.first().unwrap().name;
        session
            .agent_command(Command::Compose {
                spec: None,
                preset: Some(name.into()),
                replace: false,
            })
            .unwrap();
        assert!(!session.project().tracks.is_empty());
        assert!(session.project().tracks.iter().any(|track| {
            track
                .kind
                .note_clips()
                .is_some_and(|clips| clips.iter().any(|clip| !clip.notes.is_empty()))
        }));
        assert!(session.path().is_none());
        assert!(session.is_dirty());
        session.undo();
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn composing_does_not_silently_replace_existing_tracks() {
        let mut session = session();
        session.add_audio_track("Existing");
        let before = session.project().clone();
        assert!(
            session
                .agent_command(Command::Compose {
                    spec: None,
                    preset: Some("anything".into()),
                    replace: false,
                })
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn file_commands_and_destinations_are_rejected() {
        for value in [
            serde_json::json!({"action":"create_project", "output":"song.auris"}),
            serde_json::json!({"action":"render", "output":"song.wav"}),
            serde_json::json!({"action":"compose", "preset":"game-loop", "output":"song.auris"}),
            serde_json::json!({"action":"inspect", "project":"other.auris"}),
        ] {
            assert!(serde_json::from_value::<Command>(value).is_err());
        }
    }

    #[test]
    fn saved_document_edits_leave_the_file_unchanged() {
        let mut session = session();
        let folder = tempfile::tempdir().unwrap();
        let path = session
            .save_as(&folder.path().join("Song.auris"))
            .unwrap()
            .document;
        let before = std::fs::read(&path).unwrap();
        session
            .agent_command(Command::AddTrack {
                name: "Unsaved".into(),
                kind: TrackKind::Audio,
            })
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(session.path(), Some(path.as_path()));
        assert!(session.is_dirty());
    }

    #[test]
    fn notes_use_clip_relative_beats_and_reject_out_of_range_edits() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .agent_command(Command::AddClip {
                track: track.0,
                name: "Phrase".into(),
                start_bar: 3,
                bars: 2,
            })
            .unwrap();
        let clip = session
            .project()
            .track(track)
            .unwrap()
            .kind
            .note_clips()
            .unwrap()[0]
            .id;
        session
            .agent_command(Command::AddNote {
                clip: clip.0,
                pitch: 60,
                start: 1.0,
                beats: 0.5,
                velocity: 0.6,
            })
            .unwrap();
        let note = &session.project().midi_clip(clip).unwrap().1.notes[0];
        assert_eq!(note.start, Ticks::QUARTER);
        assert_eq!(note.length, Ticks::from_beats(0.5));
        assert_eq!(note.velocity, 0.6);
        let read: serde_json::Value = serde_json::from_str(
            &session
                .agent_command(Command::ReadNotes {
                    clip: clip.0,
                    offset: 0,
                })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(read["notes"][0]["index"], 0);
        let before = session.project().clone();
        assert!(
            session
                .agent_command(Command::AddNote {
                    clip: clip.0,
                    pitch: 60,
                    start: 7.0,
                    beats: 2.0,
                    velocity: 0.6,
                })
                .is_err()
        );
        assert_eq!(session.project(), &before);
        session.undo();
        assert!(
            session
                .project()
                .midi_clip(clip)
                .unwrap()
                .1
                .notes
                .is_empty()
        );
    }
}
