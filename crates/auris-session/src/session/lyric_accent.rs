//! How closely a sung melody follows the spoken pitch accent of its Japanese lyric.
//!
//! Tokyo-dialect Japanese gives each mora a high or a low pitch, and a melody that moves the
//! other way where speech rises or falls can make one word sound like another. Real songs
//! follow that tendency without obeying it: the corpus measured by Nishimura and Nakamura
//! (*Evaluation Score Prediction for Japanese Songs Based on Melody Fitness to Lyrics*, APSIPA
//! ASC 2025) moves with the accent more often than chance, but nowhere near always. Their
//! rule-based fitness — of the places where the accent changes level, the share at which the
//! melody steps the same way — is what [`AccentCounts::followed_rate`] is, and on its own it
//! correlated 0.54 with listeners' preferences.
//!
//! What they also found decides how this is reported. A melody that fits worse than usual
//! lost listeners' preference steeply, and one that fits better than usual gained almost
//! nothing. So a section far from its words' accent is a **warning**, and a section that
//! traces speech almost exactly is only a **note**: nothing says it sounds worse, but a
//! melody that never departs from speech can sound like recitation, and departing on purpose
//! is a composer's choice. Neither is ever judged on a single phrase, because one line set
//! against its accent is a decision, and only a section of them is a habit.
//!
//! "Far" is measured by the steps that go the *opposite* way, not by the share that follow:
//! Japanese pop repeats pitches constantly, and a repeated pitch where speech moves neither
//! contradicts the word nor follows it.
//!
//! The thresholds are set from their corpus of 1988 popular songs (JSAI 2025 version of the
//! same study, figure 2). Where the accent rises the melody rose 41%, fell 29% and repeated
//! 30% of the time; where it falls, it fell 42%, rose 32% and repeated 27%. Pooled, a real song
//! follows about 42% of the changes and opposes about 30%, against about 36% each for a melody
//! that ignored its words. Treating a section as draws from that mix, the thresholds below
//! warn about one ordinary section in twenty once it has twenty changes (one in nine at the
//! minimum of twelve), and note at most one in fifty. A section that goes against speech
//! by habit — opposing 42% and following 30%, the pooled figures mirrored — is warned about
//! roughly a third of the time: the warning is for the clear cases, not a test to pass.

use super::Session;
use super::lyric_lines::join_word;
use super::singer::LYRIC_CONTINUATION;
use auris_core::time::{TICKS_PER_QUARTER, Ticks};
use auris_core::{Project, TrackId, loop_passes};
use auris_vocal::kana::phonemes_to_kana;
use auris_vocal::{
    AccentPhrase, JapaneseDictionary, accent_notation, parse_accent_notation, phoneme_moras,
};
use serde::{Deserialize, Serialize};

/// A section in which more than this share of the accent's level changes are sung the
/// opposite way — and more are opposed than followed — is reported as far from its words.
///
/// Real songs oppose about 30%, and a melody that ignores its words about 36%.
pub const MAX_OPPOSED: f64 = 0.45;

/// A section in which at least this share of the accent's level changes are followed is
/// noted as tracing speech. Real songs follow about 42%.
pub const NEAR_FOLLOWED: f64 = 0.7;

/// Level changes a section needs before it is judged either way; fewer is too little to
/// tell a habit from a few decisions.
pub const MIN_CHANGES: usize = 12;

/// Opposed steps listed under each warning, the clearest first.
const SPOTS_PER_SECTION: usize = 5;

/// A rest at least this long between two sung notes ends a line, as it does for the lyric
/// annotation.
const LINE_BREAK: Ticks = Ticks(TICKS_PER_QUARTER);

/// One sung note as the accent measure sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct AccentNote {
    /// MIDI pitch.
    pub pitch: i32,
    /// Moras the note sings: one, several for a held vowel, zero for a melisma that carries
    /// the mora before it on.
    pub moras: usize,
    /// Whether the note is a sokuon closure, sung as silence.
    pub closure: bool,
}

/// The tally of one stretch of melody against its accent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AccentCounts {
    /// Adjacent sung notes whose spoken levels differ: where speech rises or falls.
    pub changes: usize,
    /// Of those, the steps the melody takes the same way.
    pub followed: usize,
    /// Of those, the steps on which the melody repeats its pitch.
    pub same_pitch: usize,
    /// Of those, the steps the melody takes the opposite way.
    pub opposed: usize,
}

impl AccentCounts {
    fn add(&mut self, other: AccentCounts) {
        self.changes += other.changes;
        self.followed += other.followed;
        self.same_pitch += other.same_pitch;
        self.opposed += other.opposed;
    }

    /// The share of level changes the melody follows, `None` with no changes to follow.
    pub fn followed_rate(&self) -> Option<f64> {
        (self.changes > 0).then(|| self.followed as f64 / self.changes as f64)
    }

    /// The share of level changes the melody contradicts.
    pub fn opposed_rate(&self) -> Option<f64> {
        (self.changes > 0).then(|| self.opposed as f64 / self.changes as f64)
    }
}

/// How a section stands against its words' accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccentVerdict {
    /// Within what songs usually do.
    Typical,
    /// More steps against the accent than [`MAX_OPPOSED`], and more against than with it:
    /// a warning.
    Far,
    /// At least [`NEAR_FOLLOWED`] of the changes followed: a note, not a fault.
    Near,
    /// Fewer than [`MIN_CHANGES`] measured changes.
    TooFew,
}

/// The verdict for a tally, by the thresholds above.
pub fn accent_verdict(counts: &AccentCounts) -> AccentVerdict {
    if counts.changes < MIN_CHANGES {
        AccentVerdict::TooFew
    } else if counts.opposed_rate().unwrap_or(0.0) > MAX_OPPOSED && counts.opposed > counts.followed
    {
        AccentVerdict::Far
    } else if counts.followed_rate().unwrap_or(0.0) >= NEAR_FOLLOWED {
        AccentVerdict::Near
    } else {
        AccentVerdict::Typical
    }
}

/// The spoken level each note is sung at, given the levels of the moras of its line.
///
/// A note holding several moras takes the highest of them — the convention of the measure,
/// which compares notes, not moras. A melisma continues the level of the note before it. A
/// closure sings nothing: its level joins the note before and it is `None` itself, so the
/// step measured is the audible one, across it. `None` overall when the notes do not sing
/// exactly the moras given.
pub fn note_levels(notes: &[AccentNote], levels: &[bool]) -> Option<Vec<Option<bool>>> {
    let total: usize = notes.iter().map(|note| note.moras).sum();
    if total != levels.len() {
        return None;
    }
    let mut result: Vec<Option<bool>> = Vec::with_capacity(notes.len());
    let mut at = 0;
    let mut previous: Option<usize> = None;
    for note in notes {
        let own = &levels[at..at + note.moras];
        at += note.moras;
        let high = own.iter().any(|high| *high);
        if note.closure {
            if let Some(index) = previous
                && let Some(level) = result[index].as_mut()
            {
                *level |= high;
            }
            result.push(None);
        } else if note.moras == 0 {
            result.push(previous.and_then(|index| result[index]));
            previous = Some(result.len() - 1);
        } else {
            result.push(Some(high));
            previous = Some(result.len() - 1);
        }
    }
    Some(result)
}

/// Tallies a line's sung steps against its levels; returns the tally and the index of the
/// second note of every opposed step.
pub fn accent_counts(levels: &[Option<bool>], pitches: &[i32]) -> (AccentCounts, Vec<usize>) {
    let mut counts = AccentCounts::default();
    let mut opposed = Vec::new();
    let mut previous: Option<(bool, i32)> = None;
    for (index, (level, pitch)) in levels.iter().zip(pitches).enumerate() {
        let Some(level) = *level else { continue };
        if let Some((before, from)) = previous
            && before != level
        {
            counts.changes += 1;
            let rising = level;
            match (pitch - from).signum() {
                0 => counts.same_pitch += 1,
                step if (step > 0) == rising => counts.followed += 1,
                _ => {
                    counts.opposed += 1;
                    opposed.push(index);
                }
            }
        }
        previous = Some((level, *pitch));
    }
    (counts, opposed)
}

/// A request-local correction of the accent the dictionary gave one line.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccentOverride {
    /// Line number from a previous report, counted from 1.
    #[schemars(range(min = 1))]
    pub line: usize,
    /// The line's accent in kana with ＼ after each nucleus, accent phrases separated by
    /// spaces, no mark for 平板: e.g. はしを わた＼る. It must have the line's moras.
    pub accent: String,
}

/// Where a line's accent came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccentSource {
    /// The dictionary read the words as written.
    Written,
    /// The written words did not match the sung moras, so the dictionary read the kana
    /// spelled by the notes' phonemes; less sure, since kana hides which word is meant.
    Reading,
    /// The caller corrected it.
    Override,
}

/// One opposed step, located for an edit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccentSpot {
    /// Song bar of the note the step arrives at, from 1.
    pub bar: u32,
    /// Beat within that bar, from 1.
    pub beat: u32,
    /// The clip holding that note, and its note index for edit_notes.
    pub clip: u64,
    /// The note's index in its clip.
    pub note: usize,
    /// The moras either side of the step, e.g. は→し.
    pub moras: String,
    /// Which way speech goes there: rise or fall.
    pub speech: &'static str,
    /// The melody's step, in semitones.
    pub step: i32,
}

/// One sung line's measure.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LineAccent {
    /// Line number, from 1, for [`AccentOverride::line`].
    pub line: usize,
    /// The section the line starts in.
    pub section: Option<String>,
    /// Song bar where it starts, from 1.
    pub bar: u32,
    /// The words as written on the notes.
    pub text: String,
    /// The accent measured against, in ＼ notation; null when unread.
    pub accent: Option<String>,
    /// Where that accent came from.
    pub source: Option<AccentSource>,
    /// The line's tally.
    #[serde(flatten)]
    pub counts: AccentCounts,
    /// Why the line could not be measured.
    pub error: Option<String>,
    #[serde(skip)]
    spots: Vec<AccentSpot>,
}

/// One section's measure.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SectionAccent {
    /// Section label.
    pub section: String,
    /// Lines measured in it.
    pub lines: usize,
    /// The tally over those lines.
    #[serde(flatten)]
    pub counts: AccentCounts,
    /// Followed share, rounded to hundredths.
    pub followed_rate: Option<f64>,
    /// Opposed share, rounded to hundredths.
    pub opposed_rate: Option<f64>,
    /// How the section stands.
    pub verdict: AccentVerdict,
    /// The clearest opposed steps, for a far section.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spots: Vec<AccentSpot>,
}

/// The whole report.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LyricAccentReport {
    /// Section by section.
    pub sections: Vec<SectionAccent>,
    /// Line by line.
    pub lines: Vec<LineAccent>,
    /// The tally over the whole track.
    pub song: AccentCounts,
    /// Sections far from their accent; act on these or say why they are deliberate.
    pub warnings: Vec<String>,
    /// Observations that need no action.
    pub notes: Vec<String>,
}

pub(super) struct SungNote {
    pub(super) at: Ticks,
    pub(super) length: Ticks,
    pub(super) clip: u64,
    pub(super) index: usize,
    pub(super) pitch: i32,
    pub(super) lyric: String,
    pub(super) phonemes: Vec<String>,
}

/// A singer track's notes cut into lines at rests of a beat or more and at clip edges.
pub(super) fn sung_lines(project: &Project, track: TrackId) -> Result<Vec<Vec<SungNote>>, String> {
    let source = project.track(track).ok_or("Unknown track ID")?;
    if !source.kind.is_singer() {
        return Err("Choose a singer track; only sung lyrics are measured".into());
    }
    let clips = source.kind.note_clips().ok_or("The track holds no notes")?;
    let mut lines = Vec::new();
    for clip in clips.iter().filter(|clip| !clip.muted) {
        let mut notes = Vec::new();
        for (offset, span) in loop_passes(clip.length, clip.loop_end) {
            for (index, note) in clip.playable_notes_with_indices() {
                if note.start >= span {
                    continue;
                }
                notes.push((
                    clip.start + offset + note.start,
                    note.length.min(span - note.start),
                    index,
                    note,
                ));
            }
        }
        notes.sort_by_key(|(at, ..)| *at);
        let mut line: Vec<SungNote> = Vec::new();
        let mut end = Ticks::ZERO;
        for (at, length, index, note) in notes {
            if !line.is_empty() && at - end >= LINE_BREAK {
                lines.push(std::mem::take(&mut line));
            }
            if line.last().is_some_and(|last| last.at == at) {
                continue;
            }
            end = at + length;
            line.push(SungNote {
                at,
                length,
                clip: clip.id.0,
                index,
                pitch: i32::from(note.pitch),
                lyric: note.lyric.trim().to_owned(),
                phonemes: note.phonemes,
            });
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines.sort_by_key(|line| line[0].at);
    Ok(lines)
}

/// The phrases' moras' texts and levels, flattened, or `None` when an accent is unknown.
fn flat_levels(phrases: &[AccentPhrase]) -> Option<(Vec<String>, Vec<bool>)> {
    let mut texts = Vec::new();
    let mut levels = Vec::new();
    for phrase in phrases {
        levels.extend(phrase.levels()?);
        texts.extend(phrase.moras.iter().map(|mora| mora.text.clone()));
    }
    Some((texts, levels))
}

/// The tally of `text`, read by the dictionary, sung on `notes`; `None` when its accent is
/// unknown or its moras are not the ones the notes sing.
pub(crate) fn text_counts(
    dictionary: &JapaneseDictionary,
    text: &str,
    notes: &[AccentNote],
) -> Option<AccentCounts> {
    let (_, levels) = flat_levels(&dictionary.accent_phrases(text).ok()?)?;
    let per_note = note_levels(notes, &levels)?;
    let pitches: Vec<i32> = notes.iter().map(|note| note.pitch).collect();
    Some(accent_counts(&per_note, &pitches).0)
}

fn rounded(value: Option<f64>) -> Option<f64> {
    value.map(|value| (value * 100.0).round() / 100.0)
}

impl Session {
    /// Measures how a singer track's melody moves against its lyric's spoken accent, section
    /// by section, warning where a section is far from it.
    ///
    /// Needs the Japanese dictionary, which is the only source of the accent. `overrides`
    /// correct individual lines for this one measurement and are not saved.
    pub fn lyric_accent(
        &self,
        track: TrackId,
        overrides: &[AccentOverride],
    ) -> Result<LyricAccentReport, String> {
        let dictionary = self.japanese.as_ref().ok_or(
            "The accent comes from the Japanese dictionary, and none is loaded; install it from Settings",
        )?;
        let lines = sung_lines(&self.project, track)?;
        if lines.is_empty() {
            return Err("The track sings nothing".into());
        }
        for (index, correction) in overrides.iter().enumerate() {
            if !(1..=lines.len()).contains(&correction.line) {
                return Err(format!(
                    "accents[{index}] names line {}, but the track sings {} lines",
                    correction.line,
                    lines.len()
                ));
            }
        }
        let end = lines
            .iter()
            .flatten()
            .map(|note| note.at)
            .max()
            .unwrap_or_default()
            + Ticks(1);
        let spans = self.project.sections.spans_in(Ticks::ZERO, end);
        let section_at = |at: Ticks| {
            spans
                .iter()
                .find(|span| at >= span.start && at < span.end)
                .map(|span| span.label.clone())
        };
        let measured: Vec<LineAccent> = lines
            .iter()
            .enumerate()
            .map(|(index, notes)| {
                let correction = overrides
                    .iter()
                    .rev()
                    .find(|correction| correction.line == index + 1);
                let mut line = measure_line(
                    dictionary,
                    notes,
                    correction.map(|c| c.accent.as_str()),
                    &self.project,
                );
                line.line = index + 1;
                line.section = section_at(notes[0].at);
                line
            })
            .collect();
        Ok(report(measured))
    }
}

fn measure_line(
    dictionary: &JapaneseDictionary,
    notes: &[SungNote],
    correction: Option<&str>,
    project: &Project,
) -> LineAccent {
    let mut text = String::new();
    let mut kana = String::new();
    let mut accent_notes = Vec::with_capacity(notes.len());
    for note in notes {
        let word = if note.lyric == LYRIC_CONTINUATION {
            ""
        } else {
            note.lyric.as_str()
        };
        join_word(&mut text, word);
        let spelled = phonemes_to_kana(&note.phonemes);
        if let Some(spelled) = &spelled {
            kana.push_str(spelled);
        }
        accent_notes.push(AccentNote {
            pitch: note.pitch,
            moras: if note.phonemes.is_empty() {
                0
            } else {
                phoneme_moras(&note.phonemes).len()
            },
            closure: note.phonemes == ["ʔ"],
        });
    }
    let (bar, _, _) = project.signatures.bar_beat_at(notes[0].at);
    let mut line = LineAccent {
        line: 0,
        section: None,
        bar,
        text: text.clone(),
        accent: None,
        source: None,
        counts: AccentCounts::default(),
        error: None,
        spots: Vec::new(),
    };
    let fits = |phrases: &[AccentPhrase]| {
        let (texts, levels) = flat_levels(phrases)?;
        let per_note = note_levels(&accent_notes, &levels)?;
        Some((texts, per_note))
    };
    let read = |text: &str| dictionary.accent_phrases(text).ok();
    let attempt = match correction {
        Some(written) => match parse_accent_notation(written) {
            Some(phrases) => match fits(&phrases) {
                Some(fitted) => Ok((phrases, AccentSource::Override, fitted)),
                None => Err(format!(
                    "The corrected accent {written} does not have the {} moras the notes sing",
                    accent_notes.iter().map(|n| n.moras).sum::<usize>()
                )),
            },
            None => Err(format!(
                "Could not read the corrected accent {written}; write kana with ＼ after the nucleus"
            )),
        },
        None => [
            (text.as_str(), AccentSource::Written),
            (&kana, AccentSource::Reading),
        ]
        .into_iter()
        .filter(|(text, _)| !text.is_empty())
        .find_map(|(text, source)| {
            let phrases = read(text)?;
            let fitted = fits(&phrases)?;
            Some((phrases, source, fitted))
        })
        .ok_or_else(|| {
            "The dictionary's reading of this line does not match the moras its notes sing"
                .to_string()
        }),
    };
    let (phrases, source, (texts, levels)) = match attempt {
        Ok(found) => found,
        Err(error) => {
            line.error = Some(error);
            return line;
        }
    };
    line.accent = accent_notation(&phrases);
    line.source = Some(source);
    let pitches: Vec<i32> = notes.iter().map(|note| note.pitch).collect();
    let (counts, opposed) = accent_counts(&levels, &pitches);
    line.counts = counts;
    // The moras each note starts on, to name the two sides of a step.
    let mut first_mora = Vec::with_capacity(notes.len());
    let mut at = 0;
    for note in &accent_notes {
        first_mora.push(at);
        at += note.moras;
    }
    for index in opposed {
        let before = (0..index)
            .rev()
            .find(|i| levels[*i].is_some())
            .unwrap_or(index);
        let mora = |i: usize| -> String {
            let start = first_mora[i];
            match accent_notes[i].moras {
                0 => "ー".into(),
                n => texts[start..start + n].concat(),
            }
        };
        let (bar, beat, _) = project.signatures.bar_beat_at(notes[index].at);
        line.spots.push(AccentSpot {
            bar,
            beat,
            clip: notes[index].clip,
            note: notes[index].index,
            moras: format!("{}→{}", mora(before), mora(index)),
            speech: if levels[index] == Some(true) {
                "rise"
            } else {
                "fall"
            },
            step: pitches[index] - pitches[before],
        });
    }
    line
}

fn report(lines: Vec<LineAccent>) -> LyricAccentReport {
    let mut sections: Vec<SectionAccent> = Vec::new();
    let mut song = AccentCounts::default();
    for line in &lines {
        song.add(line.counts);
        let label = line
            .section
            .clone()
            .unwrap_or_else(|| "(no section)".into());
        let section = match sections.iter_mut().find(|s| s.section == label) {
            Some(section) => section,
            None => {
                sections.push(SectionAccent {
                    section: label,
                    lines: 0,
                    counts: AccentCounts::default(),
                    followed_rate: None,
                    opposed_rate: None,
                    verdict: AccentVerdict::TooFew,
                    spots: Vec::new(),
                });
                sections.last_mut().unwrap()
            }
        };
        if line.error.is_none() {
            section.lines += 1;
            section.counts.add(line.counts);
            section.spots.extend(line.spots.iter().cloned());
        }
    }
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    for section in &mut sections {
        section.followed_rate = rounded(section.counts.followed_rate());
        section.opposed_rate = rounded(section.counts.opposed_rate());
        section.verdict = accent_verdict(&section.counts);
        let percent = |share: Option<f64>| (share.unwrap_or(0.0) * 100.0).round();
        match section.verdict {
            AccentVerdict::Far => {
                // The widest leaps against speech are the ones most likely to be heard.
                section.spots.sort_by_key(|spot| -spot.step.abs());
                section.spots.truncate(SPOTS_PER_SECTION);
                warnings.push(format!(
                    "{}: {} of {} places where speech rises or falls are sung the other way ({}%); words may be misheard. Reshape the listed steps, or choose words whose accent fits the melody; if the contour is deliberate, say so",
                    section.section,
                    section.counts.opposed,
                    section.counts.changes,
                    percent(section.counts.opposed_rate())
                ));
            }
            AccentVerdict::Near => {
                section.spots.clear();
                notes.push(format!(
                    "{}: the melody follows {}% of the accent's rises and falls, close to speech; fine if intended, though a line that never departs from speech can sound recited",
                    section.section,
                    percent(section.counts.followed_rate())
                ));
            }
            _ => section.spots.clear(),
        }
    }
    let unread = lines.iter().filter(|line| line.error.is_some()).count();
    if unread > 0 {
        notes.push(format!(
            "{unread} line(s) could not be measured; see each line's error, or pass accents for them"
        ));
    }
    LyricAccentReport {
        sections,
        lines,
        song,
        warnings,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;
    use auris_core::Note;

    /// A session with the real dictionary and one singer clip singing `text` on `pitches`,
    /// a quarter note each; `None` without `AURIS_JAPANESE_DICTIONARY`.
    fn singing(text: &str, pitches: &[u8]) -> Option<(Session, TrackId)> {
        let folder = std::env::var_os("AURIS_JAPANESE_DICTIONARY")?;
        let mut session = session();
        session
            .set_japanese_dictionary(Some(std::path::Path::new(&folder)))
            .unwrap();
        let track = session.add_singer_track("Lead");
        let clip = session
            .add_midi_clip(
                track,
                "Verse",
                Ticks::ZERO,
                Ticks::from_beats(pitches.len() as f64),
            )
            .unwrap();
        for (at, pitch) in pitches.iter().enumerate() {
            session
                .add_note(
                    clip,
                    Note::new(*pitch, Ticks::from_beats(at as f64), Ticks::QUARTER),
                )
                .unwrap();
        }
        let indices: Vec<usize> = (0..pitches.len()).collect();
        session
            .write_lyrics_with_reading(clip, &indices, text, None)
            .unwrap();
        Some((session, track))
    }

    #[test]
    fn a_mirrored_melody_swaps_what_it_follows_and_opposes() {
        let tune = [60, 64, 62, 60, 65, 64, 62, 59];
        let mirror: Vec<u8> = tune.iter().map(|p| 124 - p).collect();
        let Some((session, track)) = singing("橋を渡りたい", &tune) else {
            return;
        };
        let (mirrored, mirror_track) = singing("橋を渡りたい", &mirror).unwrap();
        let report = session.lyric_accent(track, &[]).unwrap();
        let inverted = mirrored.lyric_accent(mirror_track, &[]).unwrap();
        let line = &report.lines[0];
        assert_eq!(line.error, None);
        assert_eq!(line.source, Some(AccentSource::Written));
        assert_eq!(line.text, "橋を渡りたい");
        let accent = line.accent.as_deref().unwrap();
        assert!(accent.starts_with("はし＼"), "橋 is 尾高: {accent}");
        assert!(report.song.changes > 0);
        assert_eq!(report.song.followed, inverted.song.opposed);
        assert_eq!(report.song.opposed, inverted.song.followed);
        assert_eq!(report.song.same_pitch, inverted.song.same_pitch);
        // Eight notes are far too few to judge a section by.
        assert_eq!(report.sections[0].verdict, AccentVerdict::TooFew);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn a_corrected_accent_is_measured_instead_and_checked_against_the_notes() {
        let Some((session, track)) = singing("はしをわたる", &[60, 62, 64, 62, 60, 59])
        else {
            return;
        };
        let corrected = [AccentOverride {
            line: 1,
            accent: "は＼しを わたる".into(),
        }];
        let report = session.lyric_accent(track, &corrected).unwrap();
        let line = &report.lines[0];
        assert_eq!(line.source, Some(AccentSource::Override));
        assert_eq!(line.accent.as_deref(), Some("は＼しを わたる"));
        // HLL LHH sung 60 62 64 | 62 60 59: the fall after は rises and the rise onto た
        // falls, both opposed; を→わ stays low and asks nothing of the melody.
        assert_eq!(
            line.counts,
            AccentCounts {
                changes: 2,
                followed: 0,
                same_pitch: 0,
                opposed: 2
            }
        );
        let short = [AccentOverride {
            line: 1,
            accent: "は＼し".into(),
        }];
        let report = session.lyric_accent(track, &short).unwrap();
        assert!(
            report.lines[0]
                .error
                .as_deref()
                .unwrap()
                .contains("6 moras")
        );
        let beyond = [AccentOverride {
            line: 2,
            accent: "はし".into(),
        }];
        assert!(session.lyric_accent(track, &beyond).is_err());
    }

    #[test]
    fn a_section_sung_against_its_words_is_warned_about_with_its_widest_steps() {
        // Every line set to the mirror image of the spoken contour. The dictionary reads the
        // words as written, spaces and all: they are word boundaries to it.
        let text = "さくらが さいた ひかりの なかで きみと あるいた かわの みちを あしたも ゆめを みていた";
        let kana: String = text.split_whitespace().collect();
        let moras = auris_vocal::split_kana_moras(&kana).unwrap().len();
        let Some((plain, plain_track)) = singing(text, &vec![60; moras]) else {
            return;
        };
        let folder = plain.japanese_dictionary().unwrap().to_path_buf();
        let dictionary = JapaneseDictionary::load(&folder).unwrap();
        let (_, levels) = flat_levels(&dictionary.accent_phrases(text).unwrap()).unwrap();
        let against: Vec<u8> = levels
            .iter()
            .map(|high| if *high { 57 } else { 67 })
            .collect();
        let (session, track) = singing(text, &against).unwrap();
        let report = session.lyric_accent(track, &[]).unwrap();
        assert_eq!(report.song.followed, 0);
        assert_eq!(report.song.opposed, report.song.changes);
        assert!(report.song.changes >= MIN_CHANGES, "{:?}", report.song);
        assert_eq!(report.sections[0].verdict, AccentVerdict::Far);
        assert_eq!(report.warnings.len(), 1);
        let spots = &report.sections[0].spots;
        assert_eq!(spots.len(), SPOTS_PER_SECTION);
        assert!(
            spots
                .iter()
                .all(|spot| spot.step.abs() == 10 && spot.moras.contains('→'))
        );
        // The same words sung on one pitch neither follow nor oppose: typical, not far.
        let flat = plain.lyric_accent(plain_track, &[]).unwrap();
        assert_eq!(flat.song.same_pitch, flat.song.changes);
        assert!(flat.warnings.is_empty());
    }

    #[test]
    fn without_a_dictionary_nothing_is_guessed() {
        let mut session = session();
        session.set_japanese_dictionary(None).ok();
        let track = session.add_singer_track("Lead");
        if session.japanese_dictionary().is_none() {
            assert!(session.lyric_accent(track, &[]).is_err());
        }
    }

    fn sung(moras: &[usize]) -> Vec<AccentNote> {
        moras
            .iter()
            .map(|&moras| AccentNote {
                pitch: 0,
                moras,
                closure: false,
            })
            .collect()
    }

    fn levels(pattern: &str) -> Vec<bool> {
        pattern.chars().map(|c| c == 'H').collect()
    }

    #[test]
    fn hashi_sung_three_ways_against_its_three_accents() {
        // 箸 (HL), 橋/端 (LH): a falling step follows the first and opposes the others.
        let notes = sung(&[1, 1]);
        let falling = [64, 62];
        let chopsticks = note_levels(&notes, &levels("HL")).unwrap();
        let bridge = note_levels(&notes, &levels("LH")).unwrap();
        assert_eq!(
            accent_counts(&chopsticks, &falling).0,
            AccentCounts {
                changes: 1,
                followed: 1,
                same_pitch: 0,
                opposed: 0
            }
        );
        let (counts, opposed) = accent_counts(&bridge, &falling);
        assert_eq!((counts.opposed, opposed), (1, vec![1]));
        assert_eq!(accent_counts(&bridge, &[62, 62]).0.same_pitch, 1);
    }

    #[test]
    fn steps_inside_a_level_are_not_counted() {
        // さくらが: LHHH — one rise, then nothing the accent asks of the melody.
        let notes = sung(&[1, 1, 1, 1]);
        let per = note_levels(&notes, &levels("LHHH")).unwrap();
        let (counts, _) = accent_counts(&per, &[60, 64, 59, 72]);
        assert_eq!(counts.changes, 1);
        assert_eq!(counts.followed, 1);
    }

    #[test]
    fn held_notes_take_their_highest_mora_and_melismas_continue() {
        // とーきょー (LHHH) on two notes: と+ー low-high → high; きょ+ー high.
        let per = note_levels(&sung(&[2, 2]), &levels("LHHH")).unwrap();
        assert_eq!(per, [Some(true), Some(true)]);
        // A melisma carries the level of the note before it on.
        let per = note_levels(&sung(&[1, 0, 1]), &levels("HL")).unwrap();
        assert_eq!(per, [Some(true), Some(true), Some(false)]);
        // The melisma's pitch is the one the next step leaves from.
        let (counts, _) = accent_counts(&per, &[60, 67, 64]);
        assert_eq!(counts.followed, 1, "67 → 64 falls with the accent");
    }

    #[test]
    fn a_closure_is_silent_and_the_step_is_measured_across_it() {
        // きって (HLL as 切手): き, っ, て — the っ note sings nothing.
        let mut notes = sung(&[1, 1, 1]);
        notes[1].closure = true;
        let per = note_levels(&notes, &levels("HLL")).unwrap();
        assert_eq!(per, [Some(true), None, Some(false)]);
        let (counts, opposed) = accent_counts(&per, &[60, 90, 62]);
        assert_eq!((counts.changes, counts.opposed), (1, 1));
        assert_eq!(opposed, [2], "the step arrives at て, across the closure");
    }

    #[test]
    fn notes_that_sing_other_moras_are_not_guessed_at() {
        assert_eq!(note_levels(&sung(&[1, 1]), &levels("LHH")), None);
        assert_eq!(note_levels(&sung(&[1, 1, 1]), &levels("LH")), None);
    }

    #[test]
    fn verdicts_are_asymmetric_and_need_enough_evidence() {
        let counts = |changes, followed, same_pitch, opposed| AccentCounts {
            changes,
            followed,
            same_pitch,
            opposed,
        };
        assert_eq!(accent_verdict(&counts(11, 0, 0, 11)), AccentVerdict::TooFew);
        assert_eq!(accent_verdict(&counts(20, 6, 4, 10)), AccentVerdict::Far);
        // What real songs do on average is nowhere near either end.
        assert_eq!(
            accent_verdict(&counts(100, 42, 28, 30)),
            AccentVerdict::Typical
        );
        // Neither is a melody that ignores its words.
        assert_eq!(
            accent_verdict(&counts(100, 36, 28, 36)),
            AccentVerdict::Typical
        );
        // Opposing often is not far while the melody follows as often.
        assert_eq!(
            accent_verdict(&counts(20, 10, 0, 10)),
            AccentVerdict::Typical
        );
        assert_eq!(accent_verdict(&counts(20, 14, 4, 2)), AccentVerdict::Near);
        // Repeated pitches neither follow nor oppose: a monotone line is not far.
        assert_eq!(
            accent_verdict(&counts(20, 4, 15, 1)),
            AccentVerdict::Typical
        );
    }
}
