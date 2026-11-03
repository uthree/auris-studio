//! The notes a video draws, taken from the document once and kept in seconds.

use auris_core::{Color, Project, Ticks, TrackKind};

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

/// A line of the song's words, shown as a caption while it is sung.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreLyric {
    /// When the line starts, in seconds from the start of the song.
    pub start: f64,
    /// When it stops being shown.
    pub end: f64,
    /// The words, as written.
    pub text: String,
}

/// The tempo from a moment of the song onwards.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScoreTempo {
    /// When the tempo takes over, in seconds from the start of the song.
    pub start: f64,
    /// Quarter notes per minute, as the tempo map holds it.
    pub bpm: f64,
}

/// One notated beat of one bar, placed in seconds.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScoreBeat {
    /// When the beat falls, in seconds from the start of the song.
    pub start: f64,
    /// When the next beat falls.
    pub end: f64,
    /// The 1-based bar the beat belongs to.
    pub bar: u32,
    /// Which beat of its bar it is, from 1.
    pub beat: u32,
    /// How many beats its bar has: the upper number of the time signature in force.
    pub beats: u32,
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
    /// The song-wide lyrics, sorted and never overlapping, from the project's lyric annotation.
    pub lyrics: Vec<ScoreLyric>,
    /// The tempo changes, sorted, the first at the start of the song.
    pub tempos: Vec<ScoreTempo>,
    /// Every beat from the start of the song to the end of the bar the music ends in, sorted.
    pub beats: Vec<ScoreBeat>,
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

        let until = project.end_tick().max(project.lyrics.end()) + Ticks(1);
        for span in project.lyrics.spans_in(Ticks::ZERO, until) {
            let start = project.tempo_map.ticks_to_seconds(span.start).0;
            let end = project.tempo_map.ticks_to_seconds(span.end).0;
            if start.is_finite() && end.is_finite() && end > start {
                score.lyrics.push(ScoreLyric {
                    start,
                    end,
                    text: span.text,
                });
            }
        }

        for point in project.tempo_map.points() {
            let start = project.tempo_map.ticks_to_seconds(point.tick).0;
            if start.is_finite() && point.bpm.is_finite() && point.bpm > 0.0 {
                score.tempos.push(ScoreTempo {
                    start,
                    bpm: point.bpm,
                });
            }
        }
        let until = project
            .end_tick()
            .max(project.harmony.chords.end())
            .max(project.lyrics.end());
        score.beats = beats(project, until);
        score
    }

    /// The tempo in force at `seconds`, if the score knows any.
    pub fn tempo_at(&self, seconds: f64) -> Option<f64> {
        let after = self.tempos.partition_point(|tempo| tempo.start <= seconds);
        self.tempos
            .get(after.saturating_sub(1))
            .map(|tempo| tempo.bpm)
    }

    /// The index into [`Self::beats`] of the beat sounding at `seconds`, if any: nothing before
    /// the first and nothing once the last bar is over.
    pub fn beat_at(&self, seconds: f64) -> Option<usize> {
        let after = self.beats.partition_point(|beat| beat.start <= seconds);
        let index = after.checked_sub(1)?;
        (seconds < self.beats[index].end).then_some(index)
    }

    /// The index into [`Self::lyrics`] of the line being sung at `seconds`, if any.
    pub fn lyric_at(&self, seconds: f64) -> Option<usize> {
        let after = self.lyrics.partition_point(|line| line.start <= seconds);
        let index = after.checked_sub(1)?;
        (seconds < self.lyrics[index].end).then_some(index)
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
            lyrics: self.lyrics.clone(),
            tempos: self.tempos.clone(),
            beats: self.beats.clone(),
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

/// The most beats a score lists: hours of music at any sensible tempo, and a bound on what a
/// hand-edited file with a tempo of thousands could ask for.
const MAX_BEATS: usize = 200_000;

/// Every notated beat from the start of the song until the bar holding `until` is over.
///
/// The beat is the one the lower number of the signature names — a quarter in 3/4, an eighth in
/// 6/8 — so a counter shows the upper number's worth of beats, which is what is written.
fn beats(project: &Project, until: Ticks) -> Vec<ScoreBeat> {
    let mut beats = Vec::new();
    if until <= Ticks::ZERO {
        return beats;
    }
    let seconds = |tick: Ticks| project.tempo_map.ticks_to_seconds(tick).0;
    'spans: for span in project.signatures.spans() {
        let per_beat = span.signature.ticks_per_beat().raw().max(1);
        let count = span.signature.numerator.max(1);
        let stop = span.end.unwrap_or(until).min(until);
        let mut bar_start = span.start;
        let mut bar = span.first_bar;
        while bar_start < stop {
            for beat in 0..count {
                let start = bar_start + Ticks(per_beat * i64::from(beat));
                let (from, to) = (seconds(start), seconds(start + Ticks(per_beat)));
                if !(from.is_finite() && to.is_finite()) || to <= from {
                    continue;
                }
                beats.push(ScoreBeat {
                    start: from,
                    end: to,
                    bar,
                    beat: beat + 1,
                    beats: count,
                });
                if beats.len() >= MAX_BEATS {
                    break 'spans;
                }
            }
            bar_start += Ticks(per_beat * i64::from(count));
            bar = bar.saturating_add(1);
        }
    }
    beats
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
    fn beats_follow_the_signature_and_tempo_changes() {
        use auris_core::TimeSignature;
        let q = TICKS_PER_QUARTER;
        // A clip of eight quarters: a bar of 4/4 at 120, then 3/4 at 60 from bar 2.
        let mut project = project_with(vec![note(60, 0, q * 7)], vec![]);
        project.tempo_map = auris_core::TempoMap::constant(120.0);
        project.tempo_map.set_point(Ticks(q * 4), 60.0);
        project
            .signatures
            .set_point(Ticks(q * 4), TimeSignature::new(3, 4));
        let score = NoteScore::from_project(&project);
        let counted: Vec<_> = score
            .beats
            .iter()
            .map(|beat| (beat.bar, beat.beat, beat.beats))
            .collect();
        assert_eq!(
            counted,
            [
                (1, 1, 4),
                (1, 2, 4),
                (1, 3, 4),
                (1, 4, 4),
                (2, 1, 3),
                (2, 2, 3),
                (2, 3, 3),
                (3, 1, 3),
                (3, 2, 3),
                (3, 3, 3),
            ],
            "the bar the clip ends in is counted out to its end"
        );
        assert!(
            (score.beats[1].start - 0.5).abs() < 1e-9,
            "half a second at 120"
        );
        assert!(
            (score.beats[5].start - 3.0).abs() < 1e-9,
            "a whole second at 60"
        );
        assert_eq!(score.beat_at(2.1), Some(4));
        assert_eq!(score.beat_at(5.0), Some(7));
        assert_eq!(score.beat_at(8.5), None, "nothing once the song is over");
        assert_eq!(score.tempo_at(1.0), Some(120.0));
        assert_eq!(score.tempo_at(2.5), Some(60.0));
        assert_eq!(score.without_drums().beats, score.beats);
    }

    #[test]
    fn compound_time_counts_the_written_eighths() {
        use auris_core::TimeSignature;
        let q = TICKS_PER_QUARTER;
        let mut project = project_with(vec![note(60, 0, q * 3)], vec![]);
        project.signatures.set_initial(TimeSignature::new(6, 8));
        let score = NoteScore::from_project(&project);
        assert_eq!(score.beats.len(), 18, "eight quarters end in the third bar");
        assert!(score.beats.iter().all(|beat| beat.beats == 6));
        let eighth = 30.0 / project.tempo_map.bpm_at(Ticks::ZERO);
        assert!((score.beats[1].start - eighth).abs() < 1e-9);
    }

    #[test]
    fn an_empty_song_has_no_beats() {
        let score = NoteScore::from_project(&Project::default());
        assert!(score.beats.is_empty());
        assert!(score.tempo_at(0.0).is_some(), "but it still has a tempo");
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
