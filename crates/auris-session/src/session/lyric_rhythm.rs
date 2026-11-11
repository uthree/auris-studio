//! How a sung Japanese lyric sits in time: a rhythm a composer can read instead of hear.
//!
//! A composer who cannot listen needs the rhythm written out where it can be seen, so every
//! line is drawn on its beat grid, one token per slot, with the mora each note sings. Around
//! that drawing the report measures the few things about Japanese text setting that the
//! literature agrees on and a score can show:
//!
//! * **Japanese is timed in moras, grouped in pairs.** Speech gives each mora roughly one unit
//!   of time (服部 1950), and verse and song pair them into two-mora feet — the タクト of
//!   中道 (1980), which is why 7-5 lines run in four beats with a rest. How many moras a line
//!   sings per second is reported, since a slow line of one mora per note carries little text:
//!   金田一 (1967) gives that low information per note as one reason Japanese sits awkwardly in
//!   Western song forms.
//! * **ん and the い of a diphthong belong to the mora before them.** 窪薗 (1999) shows that
//!   where songs give two moras one note, the two are a syllable (かん, かい, かっ, かー), and
//!   later counts of current songs (平田 2019, 2021) find ん and diphthong い the special moras
//!   most often given notes of their own. Given one, they are held long less often than other
//!   moras. In the 100 PJS songs, a mid-line ん lasted at least twice its head on 11% of its own
//!   notes and diphthong い on 8%, against 20% for every other mora after the one before it.
//!   The tendency is real but loose, so each such note is only a note in the report, never a
//!   warning. The same corpus showed what is *not* a tendency. ん lands on a stronger beat
//!   position than its head (み on the off-beat, ん on the beat) on 39% of its notes, as often
//!   as any mora does (42%). So that is not measured.
//! * **Line endings carry the phrase.** 小泉 (1977), quoted by 城生 (2019), describes Japanese
//!   songs lengthening a line's last mora to mark its close, and 81% of the PJS lines end on a
//!   mora held at least [`HELD_ENDING_RATIO`] times the line's median note. Whether each line
//!   ends held is reported; a section with none is noted, never warned about, since a rest can
//!   close a line as well.
//! * **Pace.** PJS lines sing a median 3.7 moras a second (10th percentile 2.5, 90th 6.0);
//!   a section below [`SLOW_MORAS_PER_SECOND`] is noted.
//! * **Very short notes lose their vowel in synthesis.** The singing backends borrow the next
//!   consonant's time from the end of the note before it, so a mora shorter than
//!   [`SHORT_MORA_SECONDS`] may be left with almost no vowel.

use super::Session;
use super::lyric_accent::{SungNote, sung_lines};
use super::singer::LYRIC_CONTINUATION;
use auris_core::time::Ticks;
use auris_core::{Project, TrackId};
use auris_vocal::kana::phonemes_to_kana;
use auris_vocal::phoneme_moras;
use serde::Serialize;

/// A mora sung for less than this, in seconds, may lose its vowel to the next consonant.
///
/// About a sixteenth note at 167 bpm. Consonants of the singing backends run 50 to 120 ms and
/// are taken from the end of the note before them.
pub const SHORT_MORA_SECONDS: f64 = 0.09;

/// ん or a diphthong い held at least this many times as long as its head, inside a line,
/// gives the syllable's second half the weight of a syllable of its own.
pub const OUTLASTING_RATIO: f64 = 2.0;

/// A line ends held when its last mora lasts at least this many times the line's median note.
pub const HELD_ENDING_RATIO: f64 = 1.5;

/// A section slower than this, in moras per second, carries little text per bar.
pub const SLOW_MORAS_PER_SECOND: f64 = 2.0;

/// Spots listed per section.
const SPOTS_PER_SECTION: usize = 6;

/// Bars drawn per line; a longer line is cut with an ellipsis.
const GRID_BARS: u32 = 8;

/// What a spot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RhythmIssue {
    /// ん or a diphthong い lasts at least [`OUTLASTING_RATIO`] times its head, inside a line.
    OutlastingSyllable,
    /// A mora shorter than [`SHORT_MORA_SECONDS`].
    ShortMora,
}

/// One place to look at, located for edit_notes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RhythmSpot {
    /// What was found.
    pub issue: RhythmIssue,
    /// Song bar of the note, from 1.
    pub bar: u32,
    /// Beat within that bar, from 1.
    pub beat: u32,
    /// The clip holding that note.
    pub clip: u64,
    /// The note's index in its clip.
    pub note: usize,
    /// The moras concerned, e.g. か→ん.
    pub moras: String,
    /// Its length in milliseconds.
    pub ms: u32,
}

/// One sung line.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LineRhythm {
    /// Line number, from 1, in the order lyric_accent uses.
    pub line: usize,
    /// The section the line starts in.
    pub section: Option<String>,
    /// Song bar where the grid starts, from 1.
    pub bar: u32,
    /// The words as written on the notes.
    pub text: String,
    /// The line on its beat grid; see the report's legend.
    pub grid: String,
    /// What one grid slot is.
    pub grid_unit: String,
    /// Moras sung, っ included.
    pub moras: usize,
    /// Moras per second from the first onset to the end of the last note.
    pub moras_per_second: f64,
    /// The shortest time any one mora gets, in milliseconds.
    pub shortest_mora_ms: u32,
    /// Whether the last mora is held (with any melisma after it).
    pub ending_held: bool,
    #[serde(skip)]
    spots: Vec<RhythmSpot>,
}

/// One section.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SectionRhythm {
    /// Section label.
    pub section: String,
    /// Lines starting in it.
    pub lines: usize,
    /// Moras sung in it.
    pub moras: usize,
    /// Its lines' combined moras per second.
    pub moras_per_second: f64,
    /// The shortest mora in it, in milliseconds.
    pub shortest_mora_ms: u32,
    /// Lines ending on a held mora.
    pub held_endings: usize,
    /// ん or diphthong い notes much longer than their head inside a line.
    pub outlasting_syllables: usize,
    /// Moras shorter than [`SHORT_MORA_SECONDS`].
    pub short_moras: usize,
    /// Where they are.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spots: Vec<RhythmSpot>,
}

/// The whole report.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LyricRhythmReport {
    /// How to read each line's grid.
    pub legend: &'static str,
    /// Section by section.
    pub sections: Vec<SectionRhythm>,
    /// Line by line.
    pub lines: Vec<LineRhythm>,
    /// Habits to act on, or to keep with a reason.
    pub warnings: Vec<String>,
    /// Observations that need no action.
    pub notes: Vec<String>,
}

const LEGEND: &str = "Each line is drawn bar by bar from the bar it starts in: ‖ divides bars, | divides beats, and each token is one grid slot (grid_unit). A kana token is a note starting there with the moras it sings, - the note still sounding, _ silence, ~ a melisma carrying the vowel on, っ a closure. Two moras a beat in eighths is the usual pop pace: each two-mora foot (analyze_lyrics feet) then fills one beat.";

/// Second halves of a heavy syllable that can take a note of their own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dependent {
    Nasal,
    Diphthong,
}

fn vowel(phonemes: &[String]) -> Option<&str> {
    phonemes
        .last()
        .map(String::as_str)
        .filter(|p| ["a", "i", "ɯ", "e", "o"].contains(p))
}

/// Whether a one-mora note sings ん, or い after another vowel inside a written word.
fn dependent(note: &SungNote, previous: Option<&SungNote>) -> Option<Dependent> {
    let previous = previous?;
    if note.phonemes == ["ɴ"] {
        return Some(Dependent::Nasal);
    }
    // い on a note of its own begins a word when the note carries that word's text (今).
    let inside_word = note.lyric == LYRIC_CONTINUATION || note.lyric == "い" || note.lyric == "イ";
    let after = vowel(&previous.phonemes)?;
    (note.phonemes == ["i"] && inside_word && (after == "a" || after == "o"))
        .then_some(Dependent::Diphthong)
}

fn mora_count(note: &SungNote) -> usize {
    if note.phonemes.is_empty() {
        0
    } else {
        phoneme_moras(&note.phonemes).len()
    }
}

fn token(note: &SungNote) -> String {
    if note.phonemes.is_empty() {
        return "~".into();
    }
    if note.phonemes == ["ʔ"] {
        return "っ".into();
    }
    phonemes_to_kana(&note.phonemes).unwrap_or_else(|| note.lyric.clone())
}

fn unit_name(slot: i64, beat: i64) -> String {
    match (slot, beat) {
        (480, 960) => "eighth".into(),
        (240, 960) => "sixteenth".into(),
        (320, 960) => "eighth triplet".into(),
        (160, 960) => "sixteenth triplet".into(),
        (120, 960) => "thirty-second".into(),
        (960, 960) => "quarter".into(),
        (480, 1440) => "eighth".into(),
        (240, 1440) => "sixteenth".into(),
        (slot, _) => format!("{slot} ticks"),
    }
}

/// The line drawn on the coarsest grid that holds every onset.
fn grid(project: &Project, notes: &[SungNote]) -> (String, String) {
    let first = notes[0].at;
    let end = notes
        .iter()
        .map(|note| note.at + note.length)
        .max()
        .unwrap_or(first);
    let signature = project.signatures.signature_at(first);
    let beat = signature.beat_ticks().raw().max(1);
    let divisions: &[i64] = if signature.is_compound() {
        &[3, 6, 12]
    } else {
        &[2, 4, 3, 6, 8]
    };
    let offsets: Vec<i64> = notes
        .iter()
        .map(|note| (note.at - project.signatures.bar_floor(note.at)).raw())
        .collect();
    let slot = divisions
        .iter()
        .map(|d| beat / d)
        .find(|slot| *slot > 0 && offsets.iter().all(|o| o % slot == 0))
        .unwrap_or(beat / divisions[divisions.len() - 1]);
    let unit = unit_name(slot, beat);
    let mut bars = Vec::new();
    let mut bar = project.signatures.bar_floor(first);
    let mut drawn = 0;
    while bar < end {
        if drawn == GRID_BARS {
            bars.push("…".to_string());
            break;
        }
        let per_bar = project.signatures.signature_at(bar).ticks_per_bar().raw();
        let beat = project
            .signatures
            .signature_at(bar)
            .beat_ticks()
            .raw()
            .max(1);
        let mut beats = Vec::new();
        let mut beat_start = 0;
        while beat_start < per_bar {
            let mut slots = Vec::new();
            let mut at = beat_start;
            while at < (beat_start + beat).min(per_bar) {
                let from = bar + Ticks(at);
                let to = from + Ticks(slot);
                let starting: Vec<String> = notes
                    .iter()
                    .filter(|note| note.at >= from && note.at < to)
                    .map(token)
                    .collect();
                slots.push(if !starting.is_empty() {
                    starting.concat()
                } else if notes
                    .iter()
                    .any(|note| note.at < from && note.at + note.length > from)
                {
                    "-".into()
                } else {
                    "_".into()
                });
                at += slot;
            }
            beats.push(slots.join(" "));
            beat_start += beat;
        }
        bars.push(beats.join(" | "));
        bar += Ticks(per_bar.max(1));
        drawn += 1;
    }
    (bars.join(" ‖ "), unit)
}

fn measure_line(project: &Project, notes: &[SungNote]) -> LineRhythm {
    let seconds = |tick: Ticks| project.tempo_map.ticks_to_seconds(tick).0;
    let mut text = String::new();
    let mut spots = Vec::new();
    let mut moras = 0;
    let mut shortest = f64::INFINITY;
    let mut lengths = Vec::new();
    for (index, note) in notes.iter().enumerate() {
        let word = if note.lyric == LYRIC_CONTINUATION {
            ""
        } else {
            note.lyric.as_str()
        };
        super::lyric_lines::join_word(&mut text, word);
        let count = mora_count(note);
        moras += count;
        let duration = seconds(note.at + note.length) - seconds(note.at);
        let closure = note.phonemes == ["ʔ"];
        let (bar, beat, _) = project.signatures.bar_beat_at(note.at);
        let spot = |issue, moras: String| RhythmSpot {
            issue,
            bar,
            beat,
            clip: note.clip,
            note: note.index,
            moras,
            ms: (duration * 1000.0).round() as u32,
        };
        if count > 0 && !closure {
            lengths.push(note.length.raw());
            let per_mora = duration / count as f64;
            shortest = shortest.min(per_mora);
            if per_mora < SHORT_MORA_SECONDS {
                spots.push(spot(RhythmIssue::ShortMora, token(note)));
            }
        }
        let previous = index.checked_sub(1).map(|i| &notes[i]);
        if let (Some(_), Some(head)) = (dependent(note, previous), previous) {
            let pair = format!("{}→{}", token(head), token(note));
            let last = index + 1 == notes.len();
            if !last && note.length.raw() as f64 >= OUTLASTING_RATIO * head.length.raw() as f64 {
                spots.push(spot(RhythmIssue::OutlastingSyllable, pair));
            }
        }
    }
    let first = notes[0].at;
    let end = notes
        .iter()
        .map(|note| note.at + note.length)
        .max()
        .unwrap_or(first);
    let span = (seconds(end) - seconds(first)).max(f64::EPSILON);
    lengths.sort_unstable();
    let median = lengths.get(lengths.len() / 2).copied().unwrap_or(0) as f64;
    let ending_held = notes
        .iter()
        .rposition(|note| mora_count(note) > 0 && note.phonemes != ["ʔ"])
        .is_some_and(|last| {
            let held = (end - notes[last].at).raw() as f64;
            lengths.len() <= 2 || held >= HELD_ENDING_RATIO * median
        });
    let (grid, grid_unit) = grid(project, notes);
    LineRhythm {
        line: 0,
        section: None,
        bar: project.signatures.bar_beat_at(first).0,
        text,
        grid,
        grid_unit,
        moras,
        moras_per_second: (moras as f64 / span * 10.0).round() / 10.0,
        shortest_mora_ms: if shortest.is_finite() {
            (shortest * 1000.0).round() as u32
        } else {
            0
        },
        ending_held,
        spots,
    }
}

impl Session {
    /// Draws a singer track's lines on their beat grid and measures how the Japanese lyric sits
    /// in time, section by section: the pace in moras per second, held line endings, ん or
    /// diphthong い held long inside a line, and moras too short to keep their vowel.
    pub fn lyric_rhythm(&self, track: TrackId) -> Result<LyricRhythmReport, String> {
        let lines = sung_lines(&self.project, track)?;
        if lines.is_empty() {
            return Err("The track sings nothing".into());
        }
        let end = lines
            .iter()
            .flatten()
            .map(|note| note.at)
            .max()
            .unwrap_or_default()
            + Ticks(1);
        let spans = self.project.sections.spans_in(Ticks::ZERO, end);
        let measured: Vec<LineRhythm> = lines
            .iter()
            .enumerate()
            .map(|(index, notes)| {
                let mut line = measure_line(&self.project, notes);
                line.line = index + 1;
                line.section = spans
                    .iter()
                    .find(|span| notes[0].at >= span.start && notes[0].at < span.end)
                    .map(|span| span.label.clone());
                line
            })
            .collect();
        Ok(report(measured))
    }
}

fn report(lines: Vec<LineRhythm>) -> LyricRhythmReport {
    let mut sections: Vec<(SectionRhythm, f64)> = Vec::new();
    for line in &lines {
        let label = line
            .section
            .clone()
            .unwrap_or_else(|| "(no section)".into());
        let index = match sections.iter().position(|(s, _)| s.section == label) {
            Some(index) => index,
            None => {
                sections.push((
                    SectionRhythm {
                        section: label,
                        lines: 0,
                        moras: 0,
                        moras_per_second: 0.0,
                        shortest_mora_ms: u32::MAX,
                        held_endings: 0,
                        outlasting_syllables: 0,
                        short_moras: 0,
                        spots: Vec::new(),
                    },
                    0.0,
                ));
                sections.len() - 1
            }
        };
        let (section, seconds) = &mut sections[index];
        section.lines += 1;
        section.moras += line.moras;
        if line.moras_per_second > 0.0 {
            *seconds += line.moras as f64 / line.moras_per_second;
        }
        if line.shortest_mora_ms > 0 {
            section.shortest_mora_ms = section.shortest_mora_ms.min(line.shortest_mora_ms);
        }
        section.held_endings += usize::from(line.ending_held);
        for spot in &line.spots {
            match spot.issue {
                RhythmIssue::OutlastingSyllable => section.outlasting_syllables += 1,
                RhythmIssue::ShortMora => section.short_moras += 1,
            }
            section.spots.push(spot.clone());
        }
    }
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    let sections: Vec<SectionRhythm> = sections
        .into_iter()
        .map(|(mut section, seconds)| {
            if section.shortest_mora_ms == u32::MAX {
                section.shortest_mora_ms = 0;
            }
            section.moras_per_second = if seconds > 0.0 {
                (section.moras as f64 / seconds * 10.0).round() / 10.0
            } else {
                0.0
            };
            let name = &section.section;
            if section.outlasting_syllables > 0 {
                notes.push(format!(
                    "{name}: {} ん or diphthong い note(s) last at least twice as long as their head inside a line, which gives the second half of the syllable the weight of one of its own. Songs do this about half as often as with other moras; if it is not deliberate, give the length to the head or hold it at the line end",
                    section.outlasting_syllables
                ));
            }
            if section.short_moras > 0 {
                warnings.push(format!(
                    "{name}: {} mora(s) shorter than {} ms (the shortest {} ms) may lose their vowel to the next consonant in synthesis. Lengthen them, sing fewer moras in that span, or group a ん or long vowel with its head",
                    section.short_moras,
                    (SHORT_MORA_SECONDS * 1000.0) as u32,
                    section.shortest_mora_ms
                ));
            }
            if section.lines >= 3 && section.held_endings == 0 {
                notes.push(format!(
                    "{name}: no line ends on a held mora. Japanese lines usually close by lengthening the last mora; a rest after a short note can close one too, but not every line"
                ));
            }
            if section.lines >= 2 && section.moras_per_second > 0.0 && section.moras_per_second < SLOW_MORAS_PER_SECOND {
                notes.push(format!(
                    "{name}: {} moras a second carries little text per bar, and Japanese already spends more notes than English on the same words. Fine for a slow, sustained section; elsewhere two moras a beat (eighths at a pop tempo) is the usual pace",
                    section.moras_per_second
                ));
            }
            section.spots.truncate(SPOTS_PER_SECTION);
            section
        })
        .collect();
    LyricRhythmReport {
        legend: LEGEND,
        sections,
        lines,
        warnings,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    use auris_core::Note;

    /// A session with one singer clip singing kana `text` on notes given as (start, length)
    /// in eighths, with `reading` grouping notes when given.
    fn singing(text: &str, reading: Option<&str>, eighths: &[(i64, i64)]) -> (Session, TrackId) {
        let mut session = session();
        let track = session.add_singer_track("Lead");
        let end = eighths.iter().map(|(at, len)| at + len).max().unwrap();
        let clip = session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks(end * 480))
            .unwrap();
        for (at, length) in eighths {
            session
                .add_note(clip, Note::new(64, Ticks(at * 480), Ticks(length * 480)))
                .unwrap();
        }
        let indices: Vec<usize> = (0..eighths.len()).collect();
        session
            .write_lyrics_with_reading(clip, &indices, text, reading)
            .unwrap();
        (session, track)
    }

    #[test]
    fn eighths_are_drawn_and_a_held_ending_is_recognised() {
        let (session, track) = singing(
            "さくらがさいた",
            None,
            &[(0, 1), (1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 4)],
        );
        let report = session.lyric_rhythm(track).unwrap();
        let line = &report.lines[0];
        assert_eq!(line.grid_unit, "eighth");
        assert_eq!(
            line.grid,
            "さ く | ら が | さ い | た - ‖ - - | _ _ | _ _ | _ _"
        );
        assert_eq!(line.moras, 7);
        assert!(line.ending_held);
        // Seven moras over five seconds' worth of eighths at 120 bpm: 2.5 s.
        assert_eq!(line.moras_per_second, 2.8);
        assert_eq!(line.shortest_mora_ms, 250);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn a_nasal_held_far_longer_than_its_head_inside_a_line_is_reported() {
        // か (an eighth) ん (a half note) then ぱい: かん sounds like か・ん.
        let (session, track) = singing("かんぱい", None, &[(0, 1), (1, 4), (5, 1), (6, 2)]);
        let report = session.lyric_rhythm(track).unwrap();
        assert_eq!(report.sections[0].outlasting_syllables, 1);
        // Human songs do it too, if less often than with other moras: a note, never a warning.
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.notes.iter().any(|n| n.contains("twice as long")));
        // Held at the end of a line, the same ん is a held ending, not a fault.
        let (session, track) = singing("ぱいかん", None, &[(0, 1), (1, 1), (2, 1), (3, 4)]);
        let report = session.lyric_rhythm(track).unwrap();
        assert_eq!(report.sections[0].outlasting_syllables, 0);
    }

    #[test]
    fn a_word_starting_with_i_is_not_a_diphthong() {
        // あい as one word: い continues あ. In かお いま the い is the first sound of 今.
        let note = |lyric: &str, phonemes: &[&str]| SungNote {
            at: Ticks::ZERO,
            length: Ticks(480),
            clip: 0,
            index: 0,
            pitch: 60,
            lyric: lyric.into(),
            phonemes: phonemes.iter().map(|p| p.to_string()).collect(),
        };
        let a = note("あ", &["a"]);
        assert!(dependent(&note("い", &["i"]), Some(&a)) == Some(Dependent::Diphthong));
        assert!(dependent(&note("今", &["i"]), Some(&a)).is_none());
        assert!(dependent(&note("い", &["i"]), Some(&note("き", &["k", "i"]))).is_none());
        assert!(dependent(&note("ん", &["ɴ"]), None).is_none());
    }

    #[test]
    fn sixteenths_at_a_fast_tempo_are_too_short_to_keep_their_vowels() {
        let (mut session, track) = singing(
            "たたたたたたたた",
            None,
            &[
                (0, 1),
                (1, 1),
                (2, 1),
                (3, 1),
                (4, 1),
                (5, 1),
                (6, 1),
                (7, 1),
            ],
        );
        // Eighths at 360 bpm are sixteenths at 180: 83 ms.
        session.set_bpm(360.0);
        let report = session.lyric_rhythm(track).unwrap();
        assert_eq!(report.sections[0].short_moras, 8);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("lose their vowel"))
        );
        assert_eq!(report.lines[0].shortest_mora_ms, 83);
    }
}
