//! Read-only lyric analysis and deterministic, local repair contracts.
//!
//! Musical phrases use exactly the same reader and rhythm allocator as composition.
//! Character offsets refer to the original text, never to dictionary output. An explicit
//! reading is a diagnostic override and does not change a saved lyric's pronunciation.

use super::lyric_slots::{grouped_reading, mora_slots, note_groups, note_range};
use super::{Session, lyrics};
use auris_compose::SongSpec;
use auris_vocal::{SungMora, split_kana_lyric};
use serde::{Deserialize, Serialize};

mod repair;
#[cfg(test)]
mod tests;
pub use super::lyric_slots::{MoraSlot, NOTE_SEPARATOR};
pub use repair::{ClipComparison, LyricCandidate, NoteSpan, RepairRequest};

const MAX_TEXT_BYTES: usize = 65_536;
const MAX_PHRASES: usize = 512;

/// A request-local kana reading for one nonempty musical phrase.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhraseReading {
    /// One-based phrase number, after splitting at line breaks and sentence punctuation.
    pub phrase: usize,
    /// Kana pronunciation; does not replace the original text or persist to a project.
    pub kana: String,
}

/// A note count a phrase must be sung on, such as the notes of a repeated motif.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhraseFit {
    /// One-based phrase number, after splitting at line breaks and sentence punctuation.
    pub phrase: usize,
    /// Notes available to the phrase, 1..=256.
    #[schemars(range(min = 1, max = 256))]
    pub notes: usize,
}

/// Whether a phrase fits the notes it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FitVerdict {
    /// The phrase fits, holding long vowels where needed.
    Fits,
    /// More moras than notes even with every long vowel held (字余り).
    TooManyMoras,
    /// Fewer moras than notes even with every long vowel on its own note (字足らず).
    TooFewMoras,
}

/// The answer to one [`PhraseFit`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NoteFit {
    /// The requested note count.
    pub notes: usize,
    /// Whether, and which way, it misses.
    pub verdict: FitVerdict,
    /// Moras to remove (positive) or add (negative) before it fits; zero when it fits.
    pub excess: isize,
}

/// How a pronunciation was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingSource {
    /// The installed Japanese dictionary read the complete phrase.
    Dictionary,
    /// The built-in kana table read the complete phrase.
    Kana,
    /// The caller explicitly supplied a kana reading.
    Explicit,
    /// No pronunciation was established.
    Unavailable,
    /// Pronunciation stored on a singer note.
    Stored,
}

/// A phonological mora, independent of the number of musical notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MoraAnalysis {
    /// One-based mora position within this phrase.
    pub position: usize,
    /// Kana spelling supplied by the reader.
    pub text: String,
    /// IPA tokens used for singing.
    pub phonemes: Vec<String>,
    /// Whether this mora is a voiceless sokuon closure.
    pub closure: bool,
    /// Whether this mora is the moraic nasal.
    pub nasal: bool,
    /// Whether the reading explicitly spells a prolonged-sound mark.
    pub long_vowel_mark: bool,
    /// note, hold (a long vowel that may end the previous note) or rest (っ, the rest after
    /// the previous note).
    pub slot: MoraSlot,
}

/// One musical phrase with an honest source location and optional pronunciation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PhraseAnalysis {
    /// One-based phrase number, excluding empty punctuation-delimited segments.
    pub phrase: usize,
    /// One-based original line number.
    pub line: usize,
    /// Zero-based Unicode scalar offset in the original complete lyric, inclusive.
    pub start_char: usize,
    /// Zero-based Unicode scalar offset in the original complete lyric, exclusive.
    pub end_char: usize,
    /// Original text of the trimmed musical phrase.
    pub text: String,
    /// Joined mora spelling, or null when unreadable.
    pub reading: Option<String>,
    /// Provenance of this reading.
    pub reading_source: ReadingSource,
    /// Established moras; empty when unreadable, distinguished by reading and error.
    pub moras: Vec<MoraAnalysis>,
    /// Complete-phrase error; no guessed character alignment is reported.
    pub error: Option<String>,
    /// Fewest notes the phrase can be sung on: every long vowel held, っ as rests.
    pub min_notes: usize,
    /// Most notes: every mora but っ on a note of its own.
    pub max_notes: usize,
    /// The reading cut into notes with '|' for write_lyrics: for the requested fit when it
    /// fits, otherwise for max_notes. Null when unreadable.
    pub note_reading: Option<String>,
    /// The answer to a requested note count.
    pub fit: Option<NoteFit>,
}

/// Analysis that does not require a voice model, audio device or project edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LyricsAnalysis {
    /// Whether every nonempty phrase has a pronunciation.
    pub complete: bool,
    /// Whether a Japanese dictionary was available during this analysis.
    pub dictionary_available: bool,
    /// Musical phrases in source order.
    pub phrases: Vec<PhraseAnalysis>,
}

/// Deterministic diagnostic severity; incomplete input is distinct from a violation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A known violation of the requested musical contract.
    Error,
    /// A required check could not be completed.
    Incomplete,
}

/// A stable, addressable complaint suitable for a local lyric repair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LyricIssue {
    /// Stable snake_case diagnostic identifier.
    pub code: String,
    /// Error or incomplete, never an inferred artistic quality score.
    pub severity: Severity,
    /// Section name or live clip label.
    pub section: String,
    /// One-based phrase, when the complaint can be located within a section.
    pub phrase: Option<usize>,
    /// Comparison source, when this checks a shared melody.
    pub reference_section: Option<String>,
    /// Expected value, with units specified by the code and message.
    pub expected: Option<serde_json::Value>,
    /// Observed value, with units specified by the code and message.
    pub actual: Option<serde_json::Value>,
    /// English explanation for a model or frontend to present.
    pub message: String,
    /// Default smallest repair domain; changing it requires an explicit new contract.
    pub repair_scope: String,
}

/// One section's lyric analysis, retained even if another section is unreadable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SectionAnalysis {
    /// Song section name or clip label.
    pub section: String,
    /// Source-order phrase details.
    pub analysis: LyricsAnalysis,
}

/// A complete diagnostic snapshot. An empty complaint list alone is not proof of a pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LyricDiagnosis {
    /// pass, fail, or incomplete; known violations take precedence over incompleteness.
    pub status: String,
    /// Whether every required pronunciation and comparison was available.
    pub complete: bool,
    /// Source-ordered section analyses.
    pub sections: Vec<SectionAnalysis>,
    /// All independent complaints in deterministic order.
    pub issues: Vec<LyricIssue>,
}

impl LyricDiagnosis {
    pub(super) fn new() -> Self {
        Self {
            status: "pass".into(),
            complete: true,
            sections: Vec::new(),
            issues: Vec::new(),
        }
    }

    pub(super) fn finish(&mut self) {
        self.complete = self.sections.iter().all(|s| s.analysis.complete)
            && !self
                .issues
                .iter()
                .any(|i| i.severity == Severity::Incomplete);
        self.status = if self.issues.iter().any(|i| i.severity == Severity::Error) {
            "fail"
        } else if !self.complete {
            "incomplete"
        } else {
            "pass"
        }
        .into();
    }
}

pub(super) fn issue(
    code: &str,
    section: &str,
    phrase: Option<usize>,
    message: String,
) -> LyricIssue {
    LyricIssue {
        code: code.into(),
        severity: Severity::Error,
        section: section.into(),
        phrase,
        reference_section: None,
        expected: None,
        actual: None,
        message,
        repair_scope: "lyrics".into(),
    }
}

fn mora_analysis(mora: &SungMora, position: usize) -> MoraAnalysis {
    MoraAnalysis {
        position,
        text: mora.text.clone(),
        phonemes: mora.phonemes.clone(),
        closure: mora.phonemes == ["ʔ"],
        nasal: mora.phonemes == ["ɴ"],
        long_vowel_mark: mora.text == "ー",
        slot: MoraSlot::Note,
    }
}

fn delimiter(c: char) -> bool {
    matches!(c, '\n' | '\r' | '、' | '。' | '！' | '？' | '!' | '?')
}

/// Source locations use Unicode scalars; dictionary segmentation never fabricates spans.
fn source_phrases(text: &str) -> Vec<(usize, usize, usize, &str)> {
    let mut result = Vec::new();
    let mut char_offset = 0;
    let mut line = 1;
    for segment in text.split_inclusive(delimiter) {
        let content = segment.trim_end_matches(delimiter);
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            let leading = content.chars().take_while(|c| c.is_whitespace()).count();
            let start = char_offset + leading;
            result.push((line, start, start + trimmed.chars().count(), trimmed));
        }
        char_offset += segment.chars().count();
        line += segment.chars().filter(|c| *c == '\n').count();
    }
    result
}

impl Session {
    /// Analyze source-order musical phrases; explicit readings are request-local kana only.
    pub fn analyze_lyrics(
        &self,
        text: &str,
        readings: &[PhraseReading],
    ) -> Result<LyricsAnalysis, String> {
        self.analyze_lyrics_fitted(text, readings, &[])
    }

    /// [`Self::analyze_lyrics`], also checking phrases against the note counts they must fit.
    pub fn analyze_lyrics_fitted(
        &self,
        text: &str,
        readings: &[PhraseReading],
        fits: &[PhraseFit],
    ) -> Result<LyricsAnalysis, String> {
        if text.len() > MAX_TEXT_BYTES || readings.len() > MAX_PHRASES || fits.len() > MAX_PHRASES {
            return Err("Lyrics are limited to 65536 bytes and 512 phrase readings or fits".into());
        }
        let sources = source_phrases(text);
        if sources.len() > MAX_PHRASES {
            return Err("Lyrics are limited to 512 musical phrases".into());
        }
        let mut overrides = std::collections::BTreeMap::new();
        for reading in readings {
            if reading.phrase == 0
                || reading.phrase > sources.len()
                || overrides
                    .insert(reading.phrase, reading.kana.as_str())
                    .is_some()
                || reading.kana.len() > MAX_TEXT_BYTES
                || reading.kana.chars().any(delimiter)
                || split_kana_lyric(&reading.kana).is_none_or(|m| m.is_empty())
            {
                return Err("Readings need unique existing one-based phrases and nonempty kana without phrase breaks".into());
            }
        }
        let mut targets = std::collections::BTreeMap::new();
        for fit in fits {
            if fit.phrase == 0
                || fit.phrase > sources.len()
                || !(1..=256).contains(&fit.notes)
                || targets.insert(fit.phrase, fit.notes).is_some()
            {
                return Err("Fits need unique existing one-based phrases and 1..=256 notes".into());
            }
        }
        let mut phrases = Vec::with_capacity(sources.len());
        for (index, (line, start_char, end_char, text)) in sources.into_iter().enumerate() {
            let phrase = index + 1;
            let explicit = overrides.get(&phrase);
            let mut word_starts = Vec::new();
            let read = match explicit {
                Some(kana) => auris_vocal::spoken_kana_lyric(kana, self.japanese.as_ref())
                    .filter(|moras| !moras.is_empty())
                    .map(|moras| {
                        moras
                            .into_iter()
                            .map(|(text, phonemes)| SungMora { text, phonemes })
                            .collect::<Vec<_>>()
                    })
                    .ok_or_else(|| "Invalid explicit reading".to_string()),
                None => lyrics::read_lyrics(text, self.japanese.as_ref())
                    .map(|read| {
                        let mut moras = Vec::new();
                        for phrase in read {
                            let offset = moras.len();
                            word_starts.extend(phrase.group_starts.iter().map(|at| at + offset));
                            moras.extend(phrase.moras);
                        }
                        moras
                    })
                    .map_err(|e| e.to_string()),
            };
            let (reading_source, moras, error) = match read {
                Ok(moras) if !moras.is_empty() => (
                    if explicit.is_some() {
                        ReadingSource::Explicit
                    } else if self.japanese.is_some() {
                        ReadingSource::Dictionary
                    } else {
                        ReadingSource::Kana
                    },
                    moras,
                    None,
                ),
                Ok(_) => (
                    ReadingSource::Unavailable,
                    Vec::new(),
                    Some("Phrase has no readable moras".into()),
                ),
                Err(error) => (ReadingSource::Unavailable, Vec::new(), Some(error)),
            };
            let borrowed: Vec<(&str, &[String])> = moras
                .iter()
                .map(|m| (m.text.as_str(), m.phonemes.as_slice()))
                .collect();
            let slots = mora_slots(&borrowed, &word_starts);
            let (min_notes, max_notes) = note_range(&slots);
            let fit = targets
                .get(&phrase)
                .filter(|_| error.is_none())
                .map(|&notes| NoteFit {
                    notes,
                    verdict: if notes < min_notes {
                        FitVerdict::TooManyMoras
                    } else if notes > max_notes {
                        FitVerdict::TooFewMoras
                    } else {
                        FitVerdict::Fits
                    },
                    excess: if notes < min_notes {
                        (min_notes - notes) as isize
                    } else if notes > max_notes {
                        -((notes - max_notes) as isize)
                    } else {
                        0
                    },
                });
            let layout = fit
                .as_ref()
                .filter(|fit| fit.verdict == FitVerdict::Fits)
                .map_or(max_notes, |fit| fit.notes);
            let texts: Vec<&str> = borrowed.iter().map(|(t, _)| *t).collect();
            let note_reading = note_groups(&slots, layout)
                .map(|sizes| grouped_reading(&texts, &sizes, &word_starts));
            phrases.push(PhraseAnalysis {
                phrase,
                line,
                start_char,
                end_char,
                text: text.into(),
                reading_source,
                reading: error
                    .is_none()
                    .then(|| moras.iter().map(|m| m.text.as_str()).collect()),
                moras: moras
                    .iter()
                    .enumerate()
                    .map(|(i, m)| MoraAnalysis {
                        slot: slots[i],
                        ..mora_analysis(m, i + 1)
                    })
                    .collect(),
                error,
                min_notes,
                max_notes,
                note_reading,
                fit,
            });
        }
        Ok(LyricsAnalysis {
            complete: phrases.iter().all(|p| p.error.is_none()),
            dictionary_available: self.japanese.is_some(),
            phrases,
        })
    }

    /// Diagnose all vocal sections without changing a document or loading instruments.
    /// Uses the composer's reader and exact fixed-bar allocator, including shared closures.
    pub fn diagnose_song_lyrics(&self, spec: &SongSpec) -> Result<LyricDiagnosis, String> {
        let mut report = LyricDiagnosis::new();
        let mut seen = std::collections::BTreeSet::new();
        for name in &spec.form {
            if !seen.insert(name) {
                continue;
            }
            let Some(section) = spec.sections.get(name) else {
                continue;
            };
            // An empty later lyric intentionally leaves that section instrumental.
            if section.lyrics.trim().is_empty() {
                continue;
            }
            let analysis = self.analyze_lyrics(&section.lyrics, &[])?;
            for phrase in &analysis.phrases {
                if let Some(error) = &phrase.error {
                    let mut item = issue(
                        "unreadable_phrase",
                        name,
                        Some(phrase.phrase),
                        error.clone(),
                    );
                    item.severity = Severity::Incomplete;
                    report.issues.push(item);
                }
            }
            if analysis.complete {
                let phrases = lyrics::read_lyrics(&section.lyrics, self.japanese.as_ref())
                    .map_err(|e| e.to_string())?;
                let bars = lyrics::vocal_bars(spec, section.melody_from.as_deref().unwrap_or(name));
                if lyrics::fitted_lyric_rhythm(
                    &phrases,
                    spec.meter,
                    bars,
                    spec.writing_style,
                    spec.seed,
                )
                .is_none()
                {
                    let mut item = issue(
                        "lyric_capacity_exceeded",
                        name,
                        None,
                        format!(
                            "{name}: too many syllables for the fixed section length; shorten the lyrics or reduce phrase breaks"
                        ),
                    );
                    item.expected = Some(serde_json::json!({"bars": bars}));
                    item.actual = Some(
                        serde_json::json!({"moras_per_phrase": analysis.phrases.iter().map(|p| p.moras.len()).collect::<Vec<_>>()}),
                    );
                    report.issues.push(item);
                }
            }
            report.sections.push(SectionAnalysis {
                section: name.clone(),
                analysis,
            });
        }
        seen.clear();
        for name in &spec.form {
            if !seen.insert(name) {
                continue;
            }
            let Some(section) = spec.sections.get(name) else {
                continue;
            };
            let Some(source) = &section.melody_from else {
                continue;
            };
            if section.lyrics.trim().is_empty() {
                continue;
            }
            if !spec.sections.get(source).is_some_and(|s| {
                s.melody_from.is_none() && source != name && spec.form.contains(source)
            }) {
                if !report
                    .issues
                    .iter()
                    .any(|i| i.section == *name && i.code == "invalid_melody_source")
                {
                    report.issues.push(issue(
                        "invalid_melody_source",
                        name,
                        None,
                        format!("{name}: original section `{source}` must be in the form"),
                    ));
                }
                continue;
            }
            let original = report
                .sections
                .iter()
                .find(|s| s.section == *source)
                .map(|s| &s.analysis);
            let repeated = report
                .sections
                .iter()
                .find(|s| s.section == *name)
                .map(|s| &s.analysis);
            if let Some(repeated) = repeated {
                let empty = LyricsAnalysis {
                    complete: true,
                    dictionary_available: self.japanese.is_some(),
                    phrases: Vec::new(),
                };
                compare_phrases(
                    name,
                    source,
                    original.unwrap_or(&empty),
                    repeated,
                    &mut report.issues,
                );
            }
        }
        report.finish();
        Ok(report)
    }
}

pub(super) fn compare_phrases(
    name: &str,
    source: &str,
    original: &LyricsAnalysis,
    repeated: &LyricsAnalysis,
    issues: &mut Vec<LyricIssue>,
) {
    if original.phrases.is_empty() || original.phrases.len() != repeated.phrases.len() {
        let mut item = issue(
            "phrase_count_mismatch",
            name,
            None,
            format!("{name} → {source}: notes per phrase must match; phrase counts differ"),
        );
        item.reference_section = Some(source.into());
        item.expected = Some(original.phrases.len().into());
        item.actual = Some(repeated.phrases.len().into());
        issues.push(item);
    }
    for (expected, actual) in original.phrases.iter().zip(&repeated.phrases) {
        if expected.error.is_some() || actual.error.is_some() {
            continue;
        }
        let mut item = if expected.moras.len() != actual.moras.len() {
            let mut item = issue(
                "mora_count_mismatch",
                name,
                Some(actual.phrase),
                format!(
                    "{name} → {source}: notes per phrase must match; expected {}, got {}",
                    expected.moras.len(),
                    actual.moras.len()
                ),
            );
            item.expected = Some(expected.moras.len().into());
            item.actual = Some(actual.moras.len().into());
            item
        } else {
            let closures = |p: &PhraseAnalysis| {
                p.moras
                    .iter()
                    .filter(|m| m.closure)
                    .map(|m| m.position)
                    .collect::<Vec<_>>()
            };
            let a = closures(expected);
            let b = closures(actual);
            if a == b {
                continue;
            }
            let mut item = issue(
                "closure_position_mismatch",
                name,
                Some(actual.phrase),
                format!("{name} → {source}: closure positions per phrase must match"),
            );
            item.expected = Some(serde_json::json!(a));
            item.actual = Some(serde_json::json!(b));
            item
        };
        item.reference_section = Some(source.into());
        // Repeated occurrences in form refer to the same stored section, not separate errors.
        if !issues.contains(&item) {
            issues.push(item);
        }
    }
}
