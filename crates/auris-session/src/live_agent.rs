//! File-free commands for the agent attached to an open session.

use crate::{Session, prelude::*};

/// An operation on the current document. No operation accepts a filesystem destination.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Read the current arrangement and stable track/clip IDs. Use read_notes for note data.
    Inspect {},
    /// Inspect actual rendered audio and score without saving: mel image, levels and piano roll. Start within the arrangement and request 1..8 bars fitting in 30 seconds. Visual interpretation is not listening. Use the same range before and after editing.
    InspectAudio {
        /// First bar, starting at 1.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Number of bars, 1..8; the range must fit in 30 seconds.
        #[schemars(range(min = 1, max = 8))]
        bars: u32,
        /// Optional track ID for solo routing; omit for the mix.
        track: Option<u64>,
    },
    /// List available built-in instruments, loaded SoundFont sounds, and installed CLAP/VST3 instruments. Copy a returned id into set_instrument. Results are paged; keep the same query when following next_offset.
    ListInstruments {
        /// Case-insensitive name, library, or vendor search; omit for all sounds.
        query: Option<String>,
        /// First result index, defaults to zero.
        #[serde(default)]
        offset: usize,
        /// Rescan installed plugins; only use on the first page after installing a plugin.
        #[serde(default)]
        refresh: bool,
    },
    /// Read up to 128 notes with zero-based storage indices; follow next_offset for more. Returned note.start and note.length are ticks, not beats: divide by ticks_per_quarter when using add_note or add_notes. Indices may change after edits; read again before removing notes.
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
    /// Add one empty track and return its numeric track ID. Pass kind as one string, for example {"name":"Drums","kind":"drum"}. Instrument and drum tracks start with a default sound; then use set_instrument with an ID from list_instruments. Use add_clip followed by add_notes to write music. Edits affect the open document and remain unsaved.
    AddTrack {
        /// The exact desired name; must contain non-whitespace text.
        #[schemars(length(min = 1))]
        name: String,
        /// One string: instrument, drum, singer, audio or bus. Never an array or instrument ID.
        kind: TrackKind,
    },
    /// Rename a track using its stable numeric ID from inspect_project or add_track.
    RenameTrack {
        /// Stable track ID.
        track: u64,
        /// New name; must contain non-whitespace text.
        #[schemars(length(min = 1))]
        name: String,
    },
    /// Remove one track and its clips.
    RemoveTrack {
        /// Stable track ID.
        track: u64,
    },
    /// Replace the sound of an instrument or drum track using an exact id from list_instruments. Keeps notes, clips, mixer and effects. Replacing the instrument clears its old parameter automation. SoundFonts must already be loaded; plugin IDs expire on rescan.
    SetInstrument {
        /// Stable track ID.
        track: u64,
        /// Exact id returned by list_instruments (built-in, SoundFont, CLAP or VST3).
        instrument: String,
    },
    /// Set a fader and pan using native units.
    SetLevel {
        /// Stable track ID.
        track: u64,
        /// Fader in decibels, -60 through 12. Required along with pan.
        #[schemars(range(min = -60, max = 12))]
        gain_db: f32,
        /// Pan, -1 through 1.
        #[schemars(range(min = -1, max = 1))]
        pan: f32,
    },
    /// Set mute and solo for one track. Both booleans are required; use inspect_project to preserve the other state when changing only one.
    SetTrackState {
        /// Stable track ID.
        track: u64,
        /// Whether muted.
        mute: bool,
        /// Whether soloed.
        solo: bool,
    },
    /// Add an empty MIDI clip to an instrument, drum or singer track and return its numeric clip ID. start_bar is 1-based in the project's meter; bars is a duration. Audio and bus tracks cannot hold MIDI clips.
    AddClip {
        /// Stable track ID.
        track: u64,
        /// Exact clip name; must contain non-whitespace text.
        #[schemars(length(min = 1))]
        name: String,
        /// First bar, starting at 1.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Duration in bars, 1 through 1024.
        #[schemars(range(min = 1, max = 1024))]
        bars: u32,
    },
    /// Add one note using start and beats in quarter-note beats relative to the clip start (zero-based). The note must fit inside the clip and last at least one tick. Unlike add_notes, this tool uses start and beats rather than start_beat and duration_beats. Velocity is 0..1, not MIDI 0..127.
    AddNote {
        /// Stable clip ID from inspect or add_clip.
        clip: u64,
        /// MIDI pitch 0..127 as an integer or string, or a name such as C4 (60).
        #[serde(deserialize_with = "crate::note_pitch::deserialize")]
        #[schemars(with = "crate::note_pitch::Input")]
        pitch: u8,
        /// Start in quarter-note beats relative to the clip, starting at zero.
        #[schemars(range(min = 0))]
        start: f64,
        /// Duration in quarter-note beats.
        #[schemars(extend("exclusiveMinimum" = 0))]
        beats: f64,
        /// Velocity, 0 through 1.
        #[schemars(range(min = 0, max = 1))]
        velocity: f32,
    },
    /// Add 1..256 notes in one undoable edit. Each note uses pitch, start_beat, duration_beats and velocity, for example {"pitch":60,"start_beat":0,"duration_beats":1,"velocity":0.8}. Times are clip-relative quarter-note beats; all notes must fit the clip and last at least one tick. Velocity is 0..1, not MIDI 0..127. Validate the whole batch before writing; larger phrases need multiple calls.
    AddNotes {
        /// Stable clip ID from add_clip or inspect_project.
        clip: u64,
        /// Notes with MIDI pitches and clip-relative quarter-note beats.
        #[schemars(length(min = 1, max = 256))]
        notes: Vec<NoteInput>,
    },
    /// Replace all authored notes in one undoable edit; identical retries are no-ops and [] clears notes. Use pitch, start_beat, duration_beats and velocity as in add_notes, with clip-relative quarter-note beats. All notes must fit the clip. Preserves clip curves, recipe, transforms and length. Edits remain unsaved. Maximum 4096 notes; prefer short clips to avoid large tool arguments.
    ReplaceNotes {
        /// Stable clip ID from inspect_project or add_clip.
        clip: u64,
        /// Complete replacement sequence, or [] to clear the clip.
        #[schemars(length(max = 4096))]
        notes: Vec<NoteInput>,
    },
    /// Set the tempo at the beginning of the project.
    SetTempo {
        /// Tempo in BPM, 20 through 300.
        #[schemars(range(min = 20, max = 300))]
        bpm: f64,
    },
    /// Set and enable a playback loop using a 1-based start bar and duration.
    SetLoop {
        /// First bar, starting at 1.
        #[schemars(range(min = 1))]
        start_bar: u32,
        /// Loop duration in bars, 1 through 1024.
        #[schemars(range(min = 1, max = 1024))]
        bars: u32,
    },
    /// Remove notes using current zero-based storage indices from read_notes. Read again after edits that change indices. Duplicate indices are harmless; an empty array is a no-op.
    RemoveNotes {
        /// Stable clip ID.
        clip: u64,
        /// Zero-based note indices.
        indices: Vec<usize>,
    },
}

/// One explicitly authored note, expressed in clip-relative quarter-note beats.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteInput {
    /// MIDI pitch 0..127 as an integer or string, or a name such as C4 (60).
    #[serde(deserialize_with = "crate::note_pitch::deserialize")]
    #[schemars(with = "crate::note_pitch::Input")]
    pub pitch: u8,
    /// Start in quarter-note beats, starting at zero.
    #[serde(rename = "start_beat")]
    #[schemars(range(min = 0))]
    pub start: f64,
    /// Positive duration in quarter-note beats.
    #[serde(rename = "duration_beats")]
    #[schemars(extend("exclusiveMinimum" = 0))]
    pub beats: f64,
    /// Velocity, 0 through 1.
    #[schemars(range(min = 0, max = 1))]
    pub velocity: f32,
}

impl NoteInput {
    fn validate(self, clip_length: Ticks) -> Result<Note, String> {
        let field = if self.pitch > 127 {
            Some("pitch must be 0..127")
        } else if !self.start.is_finite() || self.start < 0.0 {
            Some("start_beat must be finite and >= 0")
        } else if !self.beats.is_finite() || self.beats <= 0.0 {
            Some("duration_beats must be finite and > 0")
        } else if !(0.0..=1.0).contains(&self.velocity) {
            Some("velocity must be 0..1")
        } else if self.start + self.beats > clip_length.as_beats() {
            Some("start_beat + duration_beats must fit inside the clip")
        } else {
            None
        };
        if let Some(field) = field {
            return Err(format!(
                "{field}. Example: {{\"pitch\":60,\"start_beat\":0,\"duration_beats\":1,\"velocity\":0.75}}"
            ));
        }
        let start = Ticks::from_beats(self.start);
        let length = Ticks::from_beats(self.beats);
        if length.raw() < 1
            || start
                .raw()
                .checked_add(length.raw())
                .is_none_or(|end| end > clip_length.raw())
        {
            return Err(
                "Note must fit inside the clip with a duration of at least one tick".into(),
            );
        }
        Ok(Note {
            velocity: self.velocity,
            ..Note::new(self.pitch, start, length)
        })
    }
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
        self.agent_command_with_plugin_paths(command, &[])
    }

    /// Run a live command using the frontend's additional plugin search folders.
    pub fn agent_command_with_plugin_paths(
        &mut self,
        command: Command,
        plugin_paths: &[std::path::PathBuf],
    ) -> Result<String, String> {
        let error = |error: crate::SessionError| error.to_string();
        match command {
            Command::ListInstruments {
                query,
                offset,
                refresh,
            } => self.agent_instrument_list(query.as_deref(), offset, refresh, plugin_paths),
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
                        "kind":track.kind.label(), "mixer":track.mixer, "clips":clips,
                        "instrument":track.kind.as_instrument().map(|inner| &inner.instrument_id),
                        "soundfont_preset":self.track_preset(track.id)})
                    })
                    .collect();
                Ok(serde_json::json!({"title":project.name, "tracks":tracks,
                    "duration_seconds":project.duration_seconds(), "harmony":project.harmony,
                    "sections":project.sections, "tempo_map":project.tempo_map, "signatures":project.signatures, "loop_region":project.loop_region, "loop_enabled":project.loop_enabled, "ticks_per_quarter":Ticks::QUARTER.raw()})
                .to_string())
            }
            Command::InspectAudio {
                start_bar,
                bars,
                track,
            } => {
                let report = self
                    .audio_inspection_job(start_bar, bars, track)?
                    .run(&std::sync::atomic::AtomicBool::new(false))?;
                serde_json::to_string(&report).map_err(|e| e.to_string())
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
                self.agent_set_instrument(TrackId(track), &instrument)?;
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
                let (_, target) = self
                    .project()
                    .midi_clip(ClipId(clip))
                    .ok_or("Unknown MIDI clip ID")?;
                let note = NoteInput {
                    pitch,
                    start,
                    beats,
                    velocity,
                }
                .validate(target.length)
                .map_err(|e| {
                    e.replace("start_beat", "start")
                        .replace("duration_beats", "beats")
                })?;
                let index = self.add_note(ClipId(clip), note).map_err(error)?;
                Ok(format!("Added note index {index}"))
            }
            Command::AddNotes { clip, notes } => {
                if notes.is_empty() || notes.len() > 256 {
                    return Err("Pass 1..256 notes per call".into());
                }
                let (_, target) = self
                    .project()
                    .midi_clip(ClipId(clip))
                    .ok_or("Unknown MIDI clip ID")?;
                let first = target.notes.len();
                let notes = notes
                    .into_iter()
                    .enumerate()
                    .map(|(index, note)| {
                        note.validate(target.length)
                            .map_err(|error| format!("notes[{index}]: {error}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let count = notes.len();
                self.begin_transaction(crate::Edit::ExternalChanges);
                let result = notes
                    .into_iter()
                    .try_for_each(|note| self.add_note(ClipId(clip), note).map(|_| ()));
                self.end_transaction();
                result.map_err(error)?;
                Ok(format!("Added {count} notes, starting at index {first}"))
            }
            Command::ReplaceNotes { clip, notes } => {
                if notes.len() > 4096 {
                    return Err("Pass at most 4096 notes; use shorter clips".into());
                }
                let (_, target) = self
                    .project()
                    .midi_clip(ClipId(clip))
                    .ok_or("Unknown MIDI clip ID")?;
                let notes = notes
                    .into_iter()
                    .enumerate()
                    .map(|(index, note)| {
                        note.validate(target.length)
                            .map_err(|e| format!("notes[{index}]: {e}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let count = notes.len();
                self.replace_notes(ClipId(clip), notes).map_err(error)?;
                Ok(format!(
                    "Clip now holds {count} authored notes; edits remain unsaved"
                ))
            }
            Command::SetTempo { bpm } => {
                if !(20.0..=300.0).contains(&bpm) {
                    return Err("bpm must be 20..300".into());
                }
                self.set_tempo_at(Ticks::ZERO, bpm);
                Ok(format!("Set tempo to {bpm} BPM"))
            }
            Command::SetLoop { start_bar, bars } => {
                if start_bar == 0 || !(1..=1024).contains(&bars) {
                    return Err("Use start_bar >= 1 and bars 1..1024".into());
                }
                let end_bar = start_bar.checked_add(bars).ok_or("Bar range overflow")?;
                let start = self.project().signatures.bar_start(start_bar);
                let end = self.project().signatures.bar_start(end_bar);
                self.set_loop_region(start, end);
                self.set_loop_enabled(true);
                Ok("Set playback loop".into())
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
    fn replacement_preserves_clip_state_is_atomic_and_undoes_once() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 8)
            .unwrap();
        session
            .add_note(clip, Note::new(48, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.set_curve_point(
            clip,
            auris_core::project::ClipCurve::Bend,
            Ticks::ZERO,
            0.25,
        );
        session
            .set_clip_transforms(
                clip,
                vec![auris_core::NoteTransform::Transpose { semitones: 12 }],
            )
            .unwrap();
        let before = session.project().clone();
        let command = |notes| {
            serde_json::from_value::<Command>(
                serde_json::json!({"action":"replace_notes","clip":clip.0,"notes":notes}),
            )
            .unwrap()
        };
        let notes = serde_json::json!([
            {"pitch":"C4","start_beat":0,"duration_beats":1,"velocity":0.7},
            {"pitch":"64","start_beat":1,"duration_beats":1,"velocity":0.8}
        ]);
        session.agent_command(command(notes.clone())).unwrap();
        let after = session.project().clone();
        let target = after.midi_clip(clip).unwrap().1;
        let mut expected = before.midi_clip(clip).unwrap().1.clone();
        expected.notes = target.notes.clone();
        assert_eq!(target, &expected);
        assert_eq!(
            target.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            vec![60, 64]
        );
        session.agent_command(command(notes)).unwrap();
        let error = session
            .agent_command(command(serde_json::json!([
                {"pitch":60,"start_beat":0,"duration_beats":1,"velocity":0.7},
                {"pitch":61,"start_beat":7,"duration_beats":2,"velocity":0.7}
            ])))
            .unwrap_err();
        assert!(
            error.contains("notes[1]") && error.contains("duration_beats"),
            "{error}"
        );
        assert_eq!(session.project(), &after);
        session.undo();
        assert_eq!(session.project(), &before);
        session.redo();
        assert_eq!(session.project(), &after);
        session
            .agent_command(command(serde_json::json!([])))
            .unwrap();
        assert!(
            session
                .project()
                .midi_clip(clip)
                .unwrap()
                .1
                .notes
                .is_empty()
        );
        session.undo();
        assert_eq!(session.project(), &after);
        assert!(session.path().is_none());
        let operation = crate::agent_policy::Operation::parse(
            "edit_project",
            &serde_json::json!({"command":{"action":"replace_notes","clip":clip.0,"notes":[]}}),
        )
        .unwrap();
        assert!(operation.mutating && operation.confirm);
    }

    #[test]
    fn direct_replacement_rejects_invalid_notes_without_recording_history() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let note = Note::new(60, Ticks::ZERO, Ticks::QUARTER);
        session.replace_notes(clip, vec![note.clone()]).unwrap();
        let before = session.project().clone();
        for bad in [
            Note {
                pitch: 128,
                ..note.clone()
            },
            Note {
                velocity: f32::NAN,
                ..note.clone()
            },
            Note {
                start: Ticks(-1),
                ..note.clone()
            },
            Note {
                length: Ticks::ZERO,
                ..note.clone()
            },
            Note {
                start: Ticks(i64::MAX),
                ..note.clone()
            },
        ] {
            assert!(
                session
                    .replace_notes(clip, vec![note.clone(), bad])
                    .unwrap_err()
                    .to_string()
                    .contains("notes[1]")
            );
            assert_eq!(session.project(), &before);
        }
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

    #[test]
    fn note_batches_validate_before_writing_and_undo_together() {
        let mut session = session();
        let track = session.add_default_instrument_track("Strings").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        let before = session.project().clone();
        let note = |start| NoteInput {
            pitch: 62,
            start,
            beats: 0.5,
            velocity: 0.8,
        };
        for notes in [
            vec![],
            vec![note(0.0), note(8.0)],
            (0..257).map(|_| note(0.0)).collect(),
        ] {
            assert!(
                session
                    .agent_command(Command::AddNotes {
                        clip: clip.0,
                        notes
                    })
                    .is_err()
            );
            assert_eq!(session.project(), &before);
        }
        session
            .agent_command(Command::AddNotes {
                clip: clip.0,
                notes: vec![note(0.0), note(0.5), note(1.0)],
            })
            .unwrap();
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes.len(), 3);
        session.undo();
        assert_eq!(session.project(), &before);
        session.redo();
        assert_eq!(session.project().midi_clip(clip).unwrap().1.notes.len(), 3);
        assert!(session.path().is_none());
    }

    #[test]
    fn musical_controls_validate_before_editing() {
        let mut session = session();
        let before = session.project().clone();
        for command in [
            Command::SetTempo { bpm: f64::NAN },
            Command::SetLoop {
                start_bar: 0,
                bars: 8,
            },
            Command::SetLoop {
                start_bar: u32::MAX,
                bars: 8,
            },
        ] {
            assert!(session.agent_command(command).is_err());
            assert_eq!(session.project(), &before);
        }
        session
            .agent_command(Command::SetTempo { bpm: 144.0 })
            .unwrap();
        session.undo();
        assert_eq!(session.project(), &before);
        session
            .agent_command(Command::SetLoop {
                start_bar: 3,
                bars: 4,
            })
            .unwrap();
        assert!(session.project().loop_enabled);
        assert_eq!(
            session.project().loop_region,
            Some((Ticks::from_beats(8.0), Ticks::from_beats(24.0)))
        );
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
