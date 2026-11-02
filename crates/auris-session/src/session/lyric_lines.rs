//! The song-wide lyrics: the timeline map of which line is sung when.
//!
//! The words a singer sings ride its notes; the words a *reader* reads ride the timeline, in
//! [`Project::lyrics`](auris_core::Project::lyrics). The commands here edit that map directly,
//! and two of them keep it in step with the notes on request: [`Session::annotate_lyrics_from_vocals`]
//! reads every sung phrase back off a singer track, and the song composer and the agent's
//! `write_lyrics` write the line they have just set as they set it. Nothing else does — a
//! person who retypes a caption has retyped a caption, and no note changes.

use auris_core::time::{TICKS_PER_QUARTER, Ticks};
use auris_core::{ClipId, TrackId};

use crate::history::Edit;

use super::Session;
use super::singer::LYRIC_CONTINUATION;

/// One sung line, as read back off the notes or written by a lyric command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SungLine {
    /// Where the first word is struck, song-absolute.
    pub start: Ticks,
    /// Where the last note of the line lets go.
    pub end: Ticks,
    /// The words as written.
    pub text: String,
}

/// A rest at least this long between two sung notes ends a line.
///
/// A quarter note: a breath inside a phrase is shorter than that, and a gap a beat long is
/// where a person reading along expects the caption to turn over.
const LINE_BREAK: Ticks = Ticks(TICKS_PER_QUARTER);

impl Session {
    /// Writes the line from `at` onwards, or — with `None` — a silence from there.
    pub fn set_lyric_line(&mut self, at: Ticks, text: Option<String>) {
        self.record(Edit::SetLyricLine);
        self.project.lyrics.set_line(at.max_zero(), text);
    }

    /// Writes one line over exactly `start..end`, keeping what was in force after it.
    pub fn write_lyric_line(&mut self, start: Ticks, end: Ticks, text: &str) {
        self.record(Edit::SetLyricLine);
        self.project.lyrics.write_line(start, end, text);
    }

    /// Removes the line in force at `at`, letting the one before it run through.
    pub fn remove_lyric_line(&mut self, at: Ticks) {
        let Some(at) = self.project.lyrics.change_at(at.max_zero()) else {
            return;
        };
        self.record(Edit::SetLyricLine);
        self.project.lyrics.remove_line(at);
    }

    /// Moves the line change in force at `from` so that it starts at `to`.
    pub fn move_lyric_line(&mut self, from: Ticks, to: Ticks) -> bool {
        let Some(from) = self.project.lyrics.change_at(from.max_zero()) else {
            return false;
        };
        if from == to.max_zero() {
            return true;
        }
        self.record(Edit::MoveLyricLine);
        self.project.lyrics.move_line(from, to)
    }

    /// Silences `from..to`, keeping whatever line was in force after it.
    pub fn clear_lyric_lines(&mut self, from: Ticks, to: Ticks) {
        if to <= from {
            return;
        }
        self.record(Edit::SetLyricLine);
        self.project.lyrics.clear_range(from, to);
    }

    /// The lines a singer track sings, read back off its notes.
    ///
    /// `track` names the singer; `None` takes the first singer track with any words on it,
    /// which is the lead in every song the composer writes. A line ends where a rest of a beat
    /// or more falls, and at the end of a clip; its text is the notes' words in order, with
    /// the `+` that continues a word left out — so a phrase written in kanji, which keeps the
    /// whole word on its first note, reads back exactly as it was typed. Muted clips sing
    /// nothing and give nothing. Loops are written out, as the renderer hears them.
    pub fn sung_lyric_lines(&self, track: Option<TrackId>) -> Vec<SungLine> {
        let singers = self.project.tracks.iter().filter(|candidate| {
            candidate.kind.is_singer() && track.is_none_or(|track| candidate.id == track)
        });
        for singer in singers {
            let lines = self.sung_lines_of(singer);
            if !lines.is_empty() {
                return lines;
            }
        }
        Vec::new()
    }

    fn sung_lines_of(&self, track: &auris_core::Track) -> Vec<SungLine> {
        let auris_core::TrackKind::Singer(singer) = &track.kind else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        for clip in singer.clips.iter().filter(|clip| !clip.muted) {
            let bpm = self.project.tempo_map.bpm_at(clip.start);
            let mut notes: Vec<_> = clip
                .sounding_notes_with_meter(bpm, self.project.signatures.clone())
                .collect();
            notes.sort_by_key(|note| note.start);
            let mut current: Option<SungLine> = None;
            for note in notes {
                let start = clip.start + note.start;
                let end = start + note.length;
                let word = note.lyric.trim();
                let word = if word == LYRIC_CONTINUATION { "" } else { word };
                let breaks = current
                    .as_ref()
                    .is_some_and(|line| start - line.end >= LINE_BREAK);
                if breaks && let Some(line) = current.take() {
                    push_line(&mut lines, line);
                }
                match &mut current {
                    Some(line) => {
                        join_word(&mut line.text, word);
                        line.end = line.end.max(end);
                    }
                    None => {
                        current = Some(SungLine {
                            start,
                            end,
                            text: word.to_owned(),
                        });
                    }
                }
            }
            if let Some(line) = current {
                push_line(&mut lines, line);
            }
        }
        lines.sort_by_key(|line| line.start);
        lines
    }

    /// Rewrites the song-wide lyrics from what a singer track sings, as one undo step, and
    /// says how many lines were written.
    ///
    /// The stretch from the first sung line to the last is replaced; the annotation outside it
    /// — an instrumental intro's title card, a second singer's verse — is left alone. A track
    /// with no words on it writes nothing and records nothing.
    pub fn annotate_lyrics_from_vocals(&mut self, track: Option<TrackId>) -> usize {
        let lines = self.sung_lyric_lines(track);
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
            return 0;
        };
        let (from, to) = (first.start, last.end);
        self.record(Edit::SetLyricLine);
        self.project.lyrics.clear_range(from, to);
        for line in &lines {
            self.project
                .lyrics
                .write_line(line.start, line.end, &line.text);
        }
        lines.len()
    }

    /// The lines one call of [`Self::write_lyrics_with_reading`] has just laid on `clip`,
    /// given the notes it filled in time order.
    ///
    /// A text of several lines is split back into them when each line's own reading accounts
    /// for exactly its share of the notes — which it does for kana, for kanji with a reading
    /// written line for line, and for kanji the dictionary reads. Otherwise the phrase is one
    /// line from its first note to its last.
    pub(crate) fn written_lyric_lines(
        &self,
        clip: ClipId,
        filled: &[usize],
        text: &str,
        reading: Option<&str>,
    ) -> Vec<SungLine> {
        let Some((_, target)) = self.project.midi_clip(clip) else {
            return Vec::new();
        };
        let notes: Vec<_> = filled
            .iter()
            .filter_map(|index| target.notes.get(*index))
            .collect();
        if notes.is_empty() {
            return Vec::new();
        }
        let span = |run: &[&auris_core::Note], text: &str| SungLine {
            start: target.start + run[0].start,
            end: target.start
                + run
                    .iter()
                    .map(|note| note.start + note.length)
                    .max()
                    .unwrap_or(run[0].start),
            text: text.to_owned(),
        };

        let written: Vec<&str> = lines_of(text);
        let readings: Option<Vec<&str>> = reading.map(lines_of);
        if written.len() > 1 {
            let counts: Option<Vec<usize>> = written
                .iter()
                .enumerate()
                .map(|(at, line)| {
                    let reading = match &readings {
                        Some(readings) if readings.len() == written.len() => Some(readings[at]),
                        Some(_) => return None,
                        None => None,
                    };
                    self.lyric_portions(line, reading)
                        .ok()
                        .map(|portions| portions.len())
                })
                .collect();
            if let Some(counts) = counts
                && counts.iter().all(|count| *count > 0)
                && counts.iter().sum::<usize>() == notes.len()
            {
                let mut lines = Vec::new();
                let mut at = 0;
                for (line, count) in written.iter().zip(counts) {
                    lines.push(span(&notes[at..at + count], line));
                    at += count;
                }
                return lines;
            }
        }
        vec![span(&notes, &written.join(" "))]
    }

    /// Writes `lines` into the song-wide lyrics, without an undo step of its own: the caller
    /// is the lyric command that set them, and this is part of that edit.
    pub(crate) fn annotate_written_lines(&mut self, lines: &[SungLine]) {
        for line in lines {
            self.project
                .lyrics
                .write_line(line.start, line.end, &line.text);
        }
    }
}

fn lines_of(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn push_line(lines: &mut Vec<SungLine>, line: SungLine) {
    if !line.text.trim().is_empty() {
        lines.push(line);
    }
}

/// Appends one note's word to a line: run together, as Japanese is written, except between two
/// words of Latin script, which a reader needs a space between.
fn join_word(line: &mut String, word: &str) {
    if word.is_empty() {
        return;
    }
    let latin = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    if latin(line.chars().last()) && latin(word.chars().next()) {
        line.push(' ');
    }
    line.push_str(word);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    use auris_core::Note;
    use auris_core::time::TimeSignature;

    fn bar(n: i64) -> Ticks {
        TimeSignature::default().ticks_per_bar() * n
    }

    #[test]
    fn lines_are_read_back_off_the_notes_split_at_a_rest() {
        let mut session = session();
        let vocal = session
            .project
            .add_singer_track("Vocal", auris_synth::Vocal::ID);
        let clip = session
            .project
            .add_midi_clip(vocal, "verse", bar(1), bar(2))
            .unwrap();
        let notes = ["さ", "く", "ら", "は", "る"]
            .iter()
            .enumerate()
            .map(|(at, word)| {
                // さくら on beats 1–3, a bar's rest, はる after it.
                let start = if at < 3 { at as i64 } else { at as i64 + 3 };
                let mut note = Note::new(67, Ticks(start * 960), Ticks(960));
                note.lyric = (*word).into();
                note
            })
            .collect();
        session.project.midi_clip_mut(clip).unwrap().notes = notes;

        let lines = session.sung_lyric_lines(None);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "さくら");
        assert_eq!(
            (lines[0].start, lines[0].end),
            (bar(1), bar(1) + Ticks(2880))
        );
        assert_eq!(lines[1].text, "はる");

        assert_eq!(session.annotate_lyrics_from_vocals(None), 2);
        assert_eq!(
            session.project().lyrics.text_at(bar(1) + Ticks(10)),
            Some("さくら")
        );
        assert_eq!(session.project().lyrics.text_at(bar(1) + Ticks(3000)), None);
        assert_eq!(
            session.project().lyrics.text_at(bar(2) + Ticks(2000)),
            Some("はる")
        );
        assert!(session.undo().is_some());
        assert!(session.project().lyrics.is_empty(), "one undo step");
    }

    #[test]
    fn a_word_held_on_its_first_note_reads_back_whole_and_latin_words_are_spaced() {
        let mut line = String::new();
        for word in ["Hello", "world", "、", "君"] {
            join_word(&mut line, word);
        }
        assert_eq!(line, "Hello world、君");

        let mut session = session();
        let vocal = session
            .project
            .add_singer_track("Vocal", auris_synth::Vocal::ID);
        let clip = session
            .project
            .add_midi_clip(vocal, "verse", Ticks::ZERO, bar(1))
            .unwrap();
        session.project.midi_clip_mut(clip).unwrap().notes = (0..3)
            .map(|at| {
                let mut note = Note::new(67, Ticks(at * 480), Ticks(480));
                note.lyric = if at == 0 { "桜".into() } else { "+".into() };
                note
            })
            .collect();
        assert_eq!(session.sung_lyric_lines(None)[0].text, "桜");
    }

    #[test]
    fn nothing_sung_writes_nothing_and_records_nothing() {
        let mut session = session();
        session
            .project
            .add_singer_track("Vocal", auris_synth::Vocal::ID);
        assert_eq!(session.annotate_lyrics_from_vocals(None), 0);
        assert!(session.undo().is_none());
    }

    #[test]
    fn line_edits_are_undoable_and_move_from_inside_the_line() {
        let mut session = session();
        session.write_lyric_line(bar(1), bar(2), "一行目");
        assert!(session.move_lyric_line(bar(1) + Ticks(100), bar(0)));
        assert_eq!(
            session.project().lyrics.text_at(Ticks::ZERO),
            Some("一行目")
        );
        session.remove_lyric_line(Ticks(10));
        assert!(session.project().lyrics.text_at(Ticks::ZERO).is_none());
        assert!(session.undo().is_some());
        assert!(session.undo().is_some());
        assert_eq!(session.project().lyrics.text_at(bar(1)), Some("一行目"));
    }
}
