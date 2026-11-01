//! The notes a video draws, taken from the document once and kept in seconds.

use auris_core::{Color, Project, TrackKind};

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

/// Every note a video draws, sorted by start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteScore {
    /// The tracks, in arrangement order.
    pub tracks: Vec<ScoreTrack>,
    /// The notes, sorted by [`ScoreNote::start`].
    pub notes: Vec<ScoreNote>,
}

impl NoteScore {
    /// The notes the project *plays*: loops written out, humanisation applied, muted clips and
    /// muted or un-soloed tracks left out — the same reading the renderer and the MIDI export
    /// take, so a bar lights up exactly when its note is heard.
    ///
    /// Singer tracks contribute their melody. Audio tracks and buses have no notes to show.
    pub fn from_project(project: &Project) -> Self {
        let soloing = project.tracks.iter().any(|track| track.mixer.solo);
        let mut score = Self::default();
        for track in &project.tracks {
            if track.mixer.mute || (soloing && !track.mixer.solo) {
                continue;
            }
            let (clips, drum) = match &track.kind {
                TrackKind::Instrument(instrument) => (&instrument.clips, false),
                TrackKind::Drum(instrument) => (&instrument.clips, true),
                TrackKind::Singer(singer) => (&singer.clips, false),
                TrackKind::Audio(_) | TrackKind::Bus => continue,
            };
            let index = score.tracks.len();
            let before = score.notes.len();
            for clip in clips.iter().filter(|clip| !clip.muted) {
                let bpm = project.tempo_map.bpm_at(clip.start);
                for note in clip.sounding_notes_with_meter(bpm, project.signatures.clone()) {
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
                    drum,
                });
            }
        }
        score.notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        score
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
    use auris_core::{ClipId, MidiClip, Note, TICKS_PER_QUARTER, Ticks};

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
}
