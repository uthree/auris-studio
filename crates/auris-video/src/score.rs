//! The notes a video draws, taken from the document once and kept in seconds.

use auris_core::{Color, Project, Ticks};

/// One sounding note, placed in time.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScoreNote {
    /// When the note starts, in seconds from the start of the song.
    pub start: f64,
    /// When it stops.
    pub end: f64,
    /// MIDI pitch.
    pub pitch: u8,
    /// Strike strength, 0 to 1.
    pub velocity: f32,
    /// Index into [`NoteScore::tracks`].
    pub track: usize,
}

/// A track that contributed notes.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreTrack {
    /// The track's name in the arrangement.
    pub name: String,
    /// The track's colour in the arrangement.
    pub color: Color,
    /// Whether the track is a drum track, whose "pitches" are kit pieces.
    pub drum: bool,
}

/// A chord sounding over a stretch of the song, named for the key it sits in.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreChord {
    /// When the chord starts, in seconds from the start of the song.
    pub start: f64,
    /// When the next chord, or silence, takes over.
    pub end: f64,
    /// The chord symbol as it is shown, such as `Am7` or `Eb/G`.
    pub name: String,
}

/// Every note a video draws, sorted by start, and the chords they make.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteScore {
    /// The tracks, in arrangement order.
    pub tracks: Vec<ScoreTrack>,
    /// The notes, sorted by [`ScoreNote::start`].
    pub notes: Vec<ScoreNote>,
    /// The chord progression, sorted and never overlapping, with gaps where nothing is in force.
    ///
    /// [`Self::from_project`] fills it from the chords written into the project's harmony. A
    /// project with none written leaves it empty, and the session — which owns the recognition
    /// of chords from notes — may fill it with [`Self::set_chords`] instead.
    pub chords: Vec<ScoreChord>,
}

impl NoteScore {
    /// The notes the project *plays*: loops written out, humanisation applied, muted clips and
    /// muted or un-soloed tracks left out — the same reading the renderer and the MIDI export
    /// take, so a bar lights up exactly when its note is heard.
    ///
    /// Singer tracks contribute their melody. Audio tracks and buses have no notes to show.
    pub fn from_project(project: &Project) -> Self {
        let audible = project.solo_resolution();
        let mut score = Self::default();
        for (track, audible) in project.tracks.iter().zip(audible) {
            if track.mixer.mute || !audible {
                continue;
            }
            let Some(played) = project.played_clips(track) else {
                continue;
            };
            let index = score.tracks.len();
            let before = score.notes.len();
            for clip in played.clips.iter().filter(|clip| !clip.muted) {
                let bpm = project.tempo_map.bpm_at(clip.start);
                for note in played.sounding_notes(clip, bpm, project.signatures.clone()) {
                    let start = clip.start + note.start;
                    let start_seconds = project.tempo_map.ticks_to_seconds(start).0;
                    let end_seconds = project.tempo_map.ticks_to_seconds(start + note.length).0;
                    if !(start_seconds.is_finite() && end_seconds.is_finite()) {
                        continue;
                    }
                    score.notes.push(ScoreNote {
                        start: start_seconds,
                        end: end_seconds.max(start_seconds),
                        pitch: note.pitch.min(127),
                        velocity: note.velocity.clamp(0.0, 1.0),
                        track: index,
                    });
                }
            }
            if score.notes.len() > before {
                score.tracks.push(ScoreTrack {
                    name: track.name.clone(),
                    color: track.color,
                    drum: track.kind.is_drum(),
                });
            }
        }
        score.notes.sort_by(|a, b| a.start.total_cmp(&b.start));

        // The written harmony, from the start of the song until the last clip ends — or until
        // the last chord change, when the chart runs on past the music.
        let until = project.end_tick().max(project.harmony.chords.end());
        let events = project
            .harmony
            .events_in(Ticks::ZERO, until, &project.signatures);
        score.set_chords(
            project,
            events.iter().map(|event| {
                (
                    event.start,
                    event.start + event.length,
                    event.chord.name_in(event.key),
                )
            }),
        );
        score
    }

    /// Replaces the chord progression with `spans` — start and end in ticks, and the name to
    /// show — placed in seconds at the project's tempo.
    ///
    /// Spans out of order, empty or overlapping an earlier one are dropped, and neighbours with
    /// the same name are joined: a key change inside one chord, or a recogniser reading the same
    /// chord beat after beat, is still one chord to somebody watching.
    pub fn set_chords(
        &mut self,
        project: &Project,
        spans: impl IntoIterator<Item = (Ticks, Ticks, String)>,
    ) {
        self.chords.clear();
        for (start, end, name) in spans {
            let start = project.tempo_map.ticks_to_seconds(start.max_zero()).0;
            let end = project.tempo_map.ticks_to_seconds(end.max_zero()).0;
            if !(start.is_finite() && end.is_finite()) || end <= start || name.is_empty() {
                continue;
            }
            match self.chords.last_mut() {
                Some(last) if start < last.end - 1e-9 => continue,
                Some(last) if last.name == name && start - last.end < 1e-6 => last.end = end,
                _ => self.chords.push(ScoreChord { start, end, name }),
            }
        }
    }

    /// The index into [`Self::chords`] of the chord in force at `seconds`, if any.
    pub fn chord_at(&self, seconds: f64) -> Option<usize> {
        let after = self.chords.partition_point(|chord| chord.start <= seconds);
        let index = after.checked_sub(1)?;
        (seconds < self.chords[index].end).then_some(index)
    }

    /// The same score without its drum tracks.
    pub fn without_drums(&self) -> Self {
        let mut kept = Vec::new();
        let mut remap = vec![None; self.tracks.len()];
        for (index, track) in self.tracks.iter().enumerate() {
            if !track.drum {
                remap[index] = Some(kept.len());
                kept.push(track.clone());
            }
        }
        Self {
            tracks: kept,
            notes: self
                .notes
                .iter()
                .filter_map(|note| remap[note.track].map(|track| ScoreNote { track, ..*note }))
                .collect(),
            chords: self.chords.clone(),
        }
    }

    /// Lowest and highest pitch, when there are notes.
    pub fn pitch_range(&self) -> Option<(u8, u8)> {
        let low = self.notes.iter().map(|note| note.pitch).min()?;
        let high = self.notes.iter().map(|note| note.pitch).max()?;
        Some((low, high))
    }

    /// When the last note ends, in seconds.
    pub fn end(&self) -> f64 {
        self.notes.iter().map(|note| note.end).fold(0.0, f64::max)
    }

    /// The longest note, which bounds how far back a frame has to look for one still sounding.
    pub fn longest(&self) -> f64 {
        self.notes
            .iter()
            .map(|note| note.end - note.start)
            .fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::{
        AuxSend, ClipId, MidiClip, Note, NoteTransform, Output, SendId, TICKS_PER_QUARTER, Ticks,
        TrackId,
    };

    fn note(pitch: u8, start: i64, length: i64) -> Note {
        Note {
            velocity: 0.8,
            ..Note::new(pitch, Ticks(start), Ticks(length))
        }
    }

    fn project_with(notes: Vec<Note>, drums: Vec<Note>) -> Project {
        let mut project = Project::default();
        for (name, notes, drum) in [("Keys", notes, false), ("Kit", drums, true)] {
            let id = if drum {
                project.add_drum_track(name, "test")
            } else {
                project.add_instrument_track(name, "test")
            };
            let mut clip = MidiClip::new(
                ClipId(900 + id.0),
                name,
                Ticks::ZERO,
                Ticks(TICKS_PER_QUARTER * 8),
            );
            clip.notes = notes;
            let track = project
                .tracks
                .iter_mut()
                .find(|track| track.id == id)
                .unwrap();
            track.kind.as_instrument_mut().unwrap().clips.push(clip);
        }
        project
    }

    fn add_part(project: &mut Project, name: &str, pitch: u8) -> TrackId {
        let track = project.add_instrument_track(name, "test");
        let clip = project
            .add_midi_clip(track, name, Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        project.midi_clip_mut(clip).unwrap().notes = vec![note(pitch, 0, TICKS_PER_QUARTER)];
        track
    }

    #[test]
    fn a_layer_shows_its_performed_notes_independently_of_the_sources_mute_and_solo() {
        let mut project = Project::default();
        let source = add_part(&mut project, "Source", 60);
        let clip = project.track(source).unwrap().kind.note_clips().unwrap()[0].id;
        project
            .midi_clip_mut(clip)
            .unwrap()
            .transforms
            .push(NoteTransform::Transpose { semitones: 12 });
        let layer = project.add_layer_track(source, "Octave", "test").unwrap();
        let shape = &mut project
            .track_mut(layer)
            .unwrap()
            .kind
            .layer_mut()
            .unwrap()
            .shape;
        shape.lowest = 72;
        shape.transpose = 12;
        shape.offset = Ticks::QUARTER;
        shape.length = 0.5;
        shape.velocity = 0.5;
        project.track_mut(source).unwrap().mixer.mute = true;
        project.track_mut(layer).unwrap().mixer.solo = true;

        let score = NoteScore::from_project(&project);
        assert_eq!(score.tracks.len(), 1);
        assert_eq!(score.tracks[0].name, "Octave");
        assert_eq!(score.notes.len(), 1);
        assert_eq!(
            score.notes[0].pitch, 84,
            "source performance precedes layer shape"
        );
        assert_eq!(score.notes[0].track, 0);
        assert!((score.notes[0].velocity - 0.4).abs() < 1e-6);
        let beat = 60.0 / project.tempo_map.bpm_at(Ticks::ZERO);
        assert!((score.notes[0].start - beat).abs() < 1e-9);
        assert!((score.notes[0].end - beat * 1.5).abs() < 1e-9);

        project.track_mut(layer).unwrap().mixer.mute = true;
        assert!(NoteScore::from_project(&project).notes.is_empty());
        project.track_mut(layer).unwrap().mixer.mute = false;
        project.track_mut(layer).unwrap().mixer.solo = false;
        project.track_mut(source).unwrap().mixer.solo = true;
        assert!(
            NoteScore::from_project(&project).notes.is_empty(),
            "following notes does not make a layer an audio feeder of its soloed source"
        );
        project.track_mut(source).unwrap().mixer.solo = false;
        project.midi_clip_mut(clip).unwrap().muted = true;
        assert!(
            NoteScore::from_project(&project).notes.is_empty(),
            "muted clips stay muted on layers"
        );
    }

    #[test]
    fn solo_buses_include_their_feeders_but_a_soloed_feeder_does_not_include_its_siblings() {
        let mut project = Project::default();
        let lead = add_part(&mut project, "Lead", 60);
        let backing = add_part(&mut project, "Backing", 48);
        add_part(&mut project, "Unrelated", 90);
        let bus = project.add_bus_track("Music");
        for track in [lead, backing] {
            project.track_mut(track).unwrap().outputs = vec![Output::Bus(bus)];
        }
        project.track_mut(bus).unwrap().mixer.solo = true;
        let score = NoteScore::from_project(&project);
        assert_eq!(
            score.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            [60, 48]
        );
        assert_eq!(
            score.notes.iter().map(|n| n.track).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(
            score
                .tracks
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["Lead", "Backing"]
        );

        project.track_mut(backing).unwrap().mixer.mute = true;
        assert_eq!(NoteScore::from_project(&project).notes.len(), 1);
        project.track_mut(backing).unwrap().mixer.mute = false;
        project.track_mut(bus).unwrap().mixer.solo = false;
        project.track_mut(lead).unwrap().mixer.solo = true;
        let score = NoteScore::from_project(&project);
        assert_eq!(score.notes.len(), 1);
        assert_eq!(score.notes[0].pitch, 60);
    }

    #[test]
    fn send_routes_share_audio_solo_resolution_and_keep_muted_senders_out() {
        let mut project = Project::default();
        let sent = add_part(&mut project, "Sent", 60);
        let sibling = add_part(&mut project, "Sibling", 65);
        let bus = project.add_bus_track("Aux");
        for (index, track) in [sent, sibling].into_iter().enumerate() {
            let mut send = AuxSend::new(SendId(index as u64 + 100), bus);
            send.pre_fader = true;
            project.track_mut(track).unwrap().sends.push(send);
        }
        project.track_mut(bus).unwrap().mixer.solo = true;
        assert_eq!(NoteScore::from_project(&project).notes.len(), 2);
        project.track_mut(sent).unwrap().mixer.mute = true;
        let score = NoteScore::from_project(&project);
        assert_eq!(score.notes.len(), 1);
        assert_eq!(score.notes[0].pitch, 65);
        assert_eq!(score.notes[0].track, 0);
        project.track_mut(sent).unwrap().mixer.mute = false;
        project.track_mut(bus).unwrap().mixer.solo = false;
        project.track_mut(sent).unwrap().mixer.solo = true;
        let score = NoteScore::from_project(&project);
        assert_eq!(score.notes.len(), 1);
        assert_eq!(score.notes[0].pitch, 60);
    }

    #[test]
    fn dropping_drums_remaps_the_layer_track_index() {
        let q = TICKS_PER_QUARTER;
        let mut project = project_with(vec![note(60, 0, q)], vec![note(36, 0, q)]);
        let source = project.tracks[0].id;
        let kit = project.tracks[1].id;
        project.add_layer_track(source, "Layer", "test").unwrap();
        project.move_track(kit, 0);
        project.track_mut(source).unwrap().mixer.mute = true;
        let score = NoteScore::from_project(&project);
        assert_eq!(
            score.notes.iter().map(|n| n.track).collect::<Vec<_>>(),
            [0, 1]
        );
        let pitched = score.without_drums();
        assert_eq!(pitched.tracks.len(), 1);
        assert_eq!(pitched.tracks[0].name, "Layer");
        assert_eq!(pitched.notes.len(), 1);
        assert_eq!(pitched.notes[0].pitch, 60);
        assert_eq!(pitched.notes[0].track, 0);
    }

    #[test]
    fn notes_are_placed_in_seconds_at_the_project_tempo() {
        let q = TICKS_PER_QUARTER;
        let project = project_with(vec![note(64, q, q), note(60, 0, q * 2)], vec![]);
        let score = NoteScore::from_project(&project);
        let bpm = project.tempo_map.bpm_at(Ticks::ZERO);
        let beat = 60.0 / bpm;
        assert_eq!(score.tracks.len(), 1, "a track with no notes is not listed");
        assert_eq!(score.notes.len(), 2);
        assert_eq!(score.notes[0].pitch, 60, "sorted by start");
        assert!((score.notes[0].end - 2.0 * beat).abs() < 1e-9);
        assert!((score.notes[1].start - beat).abs() < 1e-9);
        assert_eq!(score.pitch_range(), Some((60, 64)));
    }

    #[test]
    fn muted_tracks_are_left_out_and_drums_can_be_dropped() {
        let q = TICKS_PER_QUARTER;
        let mut project = project_with(vec![note(60, 0, q)], vec![note(36, 0, q)]);
        let score = NoteScore::from_project(&project);
        assert_eq!(score.tracks.len(), 2);
        let pitched = score.without_drums();
        assert_eq!(pitched.tracks.len(), 1);
        assert_eq!(pitched.notes.len(), 1);
        assert_eq!(pitched.notes[0].pitch, 60);

        project.tracks[0].mixer.mute = true;
        let score = NoteScore::from_project(&project);
        assert_eq!(score.notes.len(), 1);
        assert_eq!(score.notes[0].pitch, 36);
        assert_eq!(score.notes[0].track, 0, "indices follow the kept tracks");
    }

    #[test]
    fn written_chords_are_placed_in_seconds_and_spelled_for_the_key() {
        use auris_core::theory::key::Key;
        use auris_core::theory::numeral::Numeral;
        let q = TICKS_PER_QUARTER;
        let mut project = project_with(vec![note(60, 0, q * 8)], vec![]);
        assert!(
            NoteScore::from_project(&project).chords.is_empty(),
            "no chords written, none shown"
        );
        project.harmony.keys.set_initial(Key::parse("Eb").unwrap());
        let numeral = |text: &str| Numeral::parse(text).unwrap();
        project
            .harmony
            .chords
            .set_point(Ticks::ZERO, Some(numeral("I")));
        project
            .harmony
            .chords
            .set_point(Ticks(q * 2), Some(numeral("IV")));
        project.harmony.chords.set_point(Ticks(q * 4), None);
        project
            .harmony
            .chords
            .set_point(Ticks(q * 6), Some(numeral("V7")));
        let score = NoteScore::from_project(&project);
        let beat = 60.0 / project.tempo_map.bpm_at(Ticks::ZERO);
        let names: Vec<_> = score
            .chords
            .iter()
            .map(|chord| chord.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["Eb", "Ab", "Bb7"],
            "flats, as E flat major spells them"
        );
        assert!((score.chords[1].start - 2.0 * beat).abs() < 1e-9);
        assert!(
            (score.chords[1].end - 4.0 * beat).abs() < 1e-9,
            "cleared at beat 4"
        );
        assert!(
            (score.chords[2].end - 8.0 * beat).abs() < 1e-9,
            "the last chord lasts until the music ends"
        );
        assert_eq!(score.chord_at(beat), Some(0));
        assert_eq!(score.chord_at(5.0 * beat), None, "the gap shows no chord");
        assert_eq!(score.chord_at(7.0 * beat), Some(2));
        assert_eq!(score.without_drums().chords, score.chords);
    }

    #[test]
    fn neighbouring_spans_of_one_chord_are_joined() {
        let project = Project::default();
        let q = TICKS_PER_QUARTER;
        let mut score = NoteScore::default();
        score.set_chords(
            &project,
            [
                (Ticks(0), Ticks(q), "C".to_string()),
                (Ticks(q), Ticks(q * 2), "C".to_string()),
                (Ticks(q), Ticks(q * 3), "Dm".to_string()),
                (Ticks(q * 2), Ticks(q * 3), "G".to_string()),
                (Ticks(q * 4), Ticks(q * 4), "F".to_string()),
            ],
        );
        let names: Vec<_> = score
            .chords
            .iter()
            .map(|chord| chord.name.as_str())
            .collect();
        assert_eq!(names, ["C", "G"], "overlapping and empty spans are dropped");
        let beat = 60.0 / project.tempo_map.bpm_at(Ticks::ZERO);
        assert!((score.chords[0].end - 2.0 * beat).abs() < 1e-9);
    }
}
