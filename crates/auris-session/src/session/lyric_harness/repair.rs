//! Validate bounded candidates before a single, revision-checked lyric edit.
use super::*;
use crate::{Edit, prelude::*};

fn musical_slots(notes: &[(usize, &Note)]) -> Vec<serde_json::Value> {
    let start = notes[0].1.start;
    notes
        .iter()
        .map(|(_, note)| {
            serde_json::json!({
                "pitch": note.pitch, "start_tick": (note.start - start).raw(),
                "length_ticks": note.length.raw(), "scoop": note.scoop,
                "fall": note.fall, "vibrato": note.vibrato,
            })
        })
        .collect()
}

fn note_labels(candidate: &LyricCandidate, moras: &[MoraAnalysis]) -> Vec<String> {
    let written: String = moras.iter().map(|m| m.text.as_str()).collect();
    if candidate.reading.is_some()
        || written != candidate.text.trim()
        || split_kana_lyric(candidate.text.trim()).is_none()
    {
        // Match write_lyrics: retain the original surface on the first note, while each
        // continuation carries its own explicit pronunciation rather than guessing a reading.
        std::iter::once(candidate.text.trim().to_string())
            .chain(std::iter::repeat_n(
                "+".to_string(),
                moras.len().saturating_sub(1),
            ))
            .collect()
    } else {
        moras.iter().map(|m| m.text.clone()).collect()
    }
}

/// Exact note slots of one phrase in an existing singer clip.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteSpan {
    /// Stable singer clip ID returned by read_notes.
    pub clip: u64,
    /// 1..256 unique zero-based storage indices; processed in timeline order.
    #[schemars(length(min = 1, max = 256))]
    pub notes: Vec<usize>,
}

/// An explicit shared-melody relationship between two existing phrases.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipComparison {
    /// Original phrase supplying the mora/closure template.
    pub reference: NoteSpan,
    /// Phrase whose stored pronunciation is checked against the template.
    pub target: NoteSpan,
}

/// A proposed replacement for exactly one phrase, retaining its musical note slots.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LyricCandidate {
    /// One nonempty musical phrase; original text is checked for required keywords.
    pub text: String,
    /// Optional explicit kana reading; actually applied as the new per-note pronunciation.
    pub reading: Option<String>,
}

/// A bounded local repair, usable as a read-only preview or an atomic edit.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairRequest {
    /// Exact destination slots; every other note remains unchanged.
    pub target: NoteSpan,
    /// Optional source slots; omit to retain the target's current closure pattern.
    pub reference: Option<NoteSpan>,
    /// Exact document revision returned by read_notes or diagnose_song.
    pub expected_revision: String,
    /// 1..3 complete phrase alternatives, checked in order; the first pass is selected.
    #[schemars(length(min = 1, max = 3))]
    pub candidates: Vec<LyricCandidate>,
    /// Literal, nonempty keywords that must occur in the candidate's original text.
    #[serde(default)]
    pub required_keywords: Vec<String>,
    /// false previews without changing history; true applies only a fully passing candidate.
    #[serde(default)]
    pub apply: bool,
}

impl Session {
    fn lyric_span_notes<'a>(&'a self, span: &NoteSpan) -> Result<Vec<(usize, &'a Note)>, String> {
        if span.notes.is_empty() || span.notes.len() > 256 {
            return Err("A lyric span needs 1..256 note indices".into());
        }
        let track = self
            .track_of_clip(ClipId(span.clip))
            .ok_or("Unknown clip ID")?;
        if self
            .project()
            .track(track)
            .and_then(|t| t.kind.as_singer())
            .is_none()
        {
            return Err("Lyric spans require singer clips".into());
        }
        let clip = self
            .midi_clip(ClipId(span.clip))
            .ok_or("Unknown singer clip")?;
        let mut seen = std::collections::BTreeSet::new();
        let mut notes = Vec::with_capacity(span.notes.len());
        for index in &span.notes {
            if !seen.insert(*index) {
                return Err("Lyric span contains duplicate note indices".into());
            }
            notes.push((
                *index,
                clip.notes
                    .get(*index)
                    .ok_or("Unknown note index; read_notes again")?,
            ));
        }
        notes.sort_by_key(|(index, note)| (note.start, *index));
        // Phrase slots must be a contiguous, monophonic run, not a selection that skips
        // protected words or interleaves two voices at the same timeline position.
        let first = notes[0].1.start;
        let last = notes[notes.len() - 1].1.start;
        let end = notes[notes.len() - 1].1.end();
        if clip.notes.iter().enumerate().any(|(i, n)| {
            ((n.start >= first && n.start <= last) || (n.start < end && n.end() > first))
                && !seen.contains(&i)
        }) || notes
            .windows(2)
            .any(|n| n[0].1.start + n[0].1.length > n[1].1.start)
            || notes.iter().any(|(_, n)| n.length.raw() <= 0)
        {
            return Err("Select a contiguous, nonoverlapping phrase in timeline order".into());
        }
        Ok(notes)
    }

    fn analyze_lyric_span(&self, span: &NoteSpan) -> Result<LyricsAnalysis, String> {
        let notes = self.lyric_span_notes(span)?;
        let text: String = notes
            .iter()
            .filter(|(_, n)| n.lyric != "+")
            .map(|(_, n)| n.lyric.as_str())
            .collect();
        let mut moras = Vec::new();
        let mut error = None;
        for (_, note) in &notes {
            if note.lyric.trim().is_empty()
                || auris_vocal::phoneme_moras(&note.phonemes).len() != 1
                || note
                    .phonemes
                    .iter()
                    .filter(|p| auris_vocal::is_syllabic(p))
                    .count()
                    != 1
                || auris_vocal::kana::phonemes_to_kana(&note.phonemes).is_none()
            {
                error = Some("Every selected slot needs a lyric and one established Japanese mora; empty or multi-mora phonemes require an explicit alignment".into());
                break;
            }
            let spelling = if note.lyric == "ー"
                || (note.lyric != "+"
                    && split_kana_lyric(&note.lyric)
                        .is_some_and(|m| m.len() == 1 && m[0].1 == note.phonemes))
            {
                note.lyric.clone()
            } else {
                // write_lyrics stores an original kanji phrase on its first note and +
                // on later notes. Their explicit per-note phonemes establish the slots.
                auris_vocal::kana::phonemes_to_kana(&note.phonemes).unwrap_or_default()
            };
            moras.push(mora_analysis(
                &SungMora {
                    text: spelling,
                    phonemes: note.phonemes.clone(),
                },
                moras.len() + 1,
            ));
        }
        let complete = error.is_none();
        Ok(LyricsAnalysis {
            complete,
            dictionary_available: self.japanese.is_some(),
            phrases: vec![PhraseAnalysis {
                phrase: 1,
                line: 1,
                start_char: 0,
                end_char: text.chars().count(),
                reading: complete.then(|| moras.iter().map(|m| m.text.as_str()).collect()),
                text,
                reading_source: ReadingSource::Stored,
                // Stored notes already sing one mora each; their count is the note count.
                min_notes: moras.len(),
                max_notes: moras.len(),
                moras,
                error,
                note_reading: None,
                fit: None,
            }],
        })
    }

    /// Check explicit phrase relationships using stored pronunciation, without changing notes.
    /// The returned revision binds any later local repair to this exact document.
    pub fn diagnose_clip_lyrics(
        &self,
        comparisons: &[ClipComparison],
    ) -> Result<LyricDiagnosis, String> {
        if comparisons.is_empty() || comparisons.len() > 16 {
            return Err("Supply 1..16 explicit lyric comparisons".into());
        }
        let mut report = LyricDiagnosis::new();
        for (index, pair) in comparisons.iter().enumerate() {
            let original = self.analyze_lyric_span(&pair.reference)?;
            let repeated = self.analyze_lyric_span(&pair.target)?;
            let source = format!(
                "comparison{}:clip{}:reference",
                index + 1,
                pair.reference.clip
            );
            let name = format!("comparison{}:clip{}:target", index + 1, pair.target.clip);
            for (label, analysis) in [(&source, &original), (&name, &repeated)] {
                if let Some(error) = &analysis.phrases[0].error {
                    let mut item = issue(
                        "unestablished_note_alignment",
                        label,
                        Some(1),
                        error.clone(),
                    );
                    item.severity = Severity::Incomplete;
                    report.issues.push(item);
                }
                report.sections.push(SectionAnalysis {
                    section: label.clone(),
                    analysis: analysis.clone(),
                });
            }
            compare_phrases(&name, &source, &original, &repeated, &mut report.issues);
            let expected = musical_slots(&self.lyric_span_notes(&pair.reference)?);
            let actual = musical_slots(&self.lyric_span_notes(&pair.target)?);
            if expected != actual {
                let mut item = issue(
                    "musical_slot_mismatch",
                    &name,
                    Some(1),
                    "Shared phrases must retain pitch, relative onsets, durations and ornaments"
                        .into(),
                );
                item.reference_section = Some(source);
                item.expected = Some(serde_json::json!(expected));
                item.actual = Some(serde_json::json!(actual));
                item.repair_scope = "musical_contract".into();
                report.issues.push(item);
            }
        }
        report.finish();
        Ok(report)
    }

    /// Preview up to three phrase repairs or apply the first passing candidate as one Undo.
    /// Keeps all notes' pitch, timing, velocity and ornaments. Only destination lyrics,
    /// phonemes and obsolete duration pins may change. Never applies a failing candidate.
    pub fn repair_lyrics(&mut self, request: &RepairRequest) -> Result<serde_json::Value, String> {
        if request.expected_revision != self.agent_document_revision()? {
            return Err(
                "Document changed; read_notes or diagnose_song again before repairing lyrics"
                    .into(),
            );
        }
        if request.candidates.is_empty()
            || request.candidates.len() > 3
            || request.required_keywords.len() > 32
            || request
                .required_keywords
                .iter()
                .any(|k| k.trim().is_empty() || k.len() > 256)
        {
            return Err(
                "Supply 1..3 candidates and at most 32 nonempty keywords, each up to 256 bytes"
                    .into(),
            );
        }
        let destination = self.lyric_span_notes(&request.target)?;
        let indices: Vec<_> = destination.iter().map(|(i, _)| *i).collect();
        let template = request.reference.as_ref().unwrap_or(&request.target);
        if request.reference.as_ref().is_some_and(|s| {
            s.clip == request.target.clip
                && s.notes.iter().any(|i| request.target.notes.contains(i))
        }) {
            return Err("Reference and target phrases must not overlap".into());
        }
        let original = self.analyze_lyric_span(template)?;
        if request.reference.is_some()
            && musical_slots(&destination) != musical_slots(&self.lyric_span_notes(template)?)
        {
            return Err("Reference and target musical slots differ; a lyric-only repair cannot change the musical contract".into());
        }
        if !original.complete {
            return Err("The template has no established one-mora-per-note alignment; supply a readable reference phrase".into());
        }
        if original.phrases[0].moras.len() != indices.len() {
            return Err("Reference and target must have the same number of note slots".into());
        }
        let mut reports = Vec::new();
        let mut selected = None;
        for (index, candidate) in request.candidates.iter().enumerate() {
            let readings: Vec<_> = candidate
                .reading
                .as_ref()
                .map(|kana| PhraseReading {
                    phrase: 1,
                    kana: kana.clone(),
                })
                .into_iter()
                .collect();
            let mut report = LyricDiagnosis::new();
            let analysis = match self.analyze_lyrics(&candidate.text, &readings) {
                Ok(analysis) => analysis,
                Err(error) => {
                    report
                        .issues
                        .push(issue("invalid_candidate_input", "candidate", None, error));
                    report.finish();
                    reports.push(report);
                    continue;
                }
            };
            for phrase in &analysis.phrases {
                if let Some(error) = &phrase.error {
                    let mut item = issue(
                        "unreadable_phrase",
                        "candidate",
                        Some(phrase.phrase),
                        error.clone(),
                    );
                    item.severity = Severity::Incomplete;
                    report.issues.push(item);
                }
            }
            compare_phrases(
                "candidate",
                "template",
                &original,
                &analysis,
                &mut report.issues,
            );
            if analysis.phrases.len() != 1 {
                report.issues.push(issue(
                    "single_phrase_required",
                    "candidate",
                    None,
                    "Replace exactly one nonempty musical phrase".into(),
                ));
            }
            for keyword in &request.required_keywords {
                if !candidate.text.contains(keyword) {
                    let mut item = issue(
                        "required_keyword_missing",
                        "candidate",
                        Some(1),
                        format!("Required keyword `{keyword}` is missing"),
                    );
                    item.expected = Some(keyword.clone().into());
                    report.issues.push(item);
                }
            }
            report.sections.push(SectionAnalysis {
                section: "candidate".into(),
                analysis,
            });
            report.finish();
            if selected.is_none() && report.status == "pass" {
                selected = Some(index);
            }
            reports.push(report);
        }
        let mut applied = false;
        let mut changes = Vec::new();
        let labels = selected
            .map(|index| {
                note_labels(
                    &request.candidates[index],
                    &reports[index].sections[0].analysis.phrases[0].moras,
                )
            })
            .unwrap_or_default();
        if let Some(index) = selected {
            for (((index, note), mora), label) in destination
                .iter()
                .zip(&reports[index].sections[0].analysis.phrases[0].moras)
                .zip(&labels)
            {
                if note.lyric != *label || note.phonemes != mora.phonemes {
                    changes.push(serde_json::json!({
                        "index":index,"before_lyric":note.lyric,"after_lyric":label,
                        "before_phonemes":note.phonemes,"after_phonemes":mora.phonemes,
                        "clears_timing_pins":!note.phoneme_seconds.is_empty()
                    }));
                }
            }
        }
        if request.apply
            && let Some(index) = selected
            && !changes.is_empty()
        {
            let moras = &reports[index].sections[0].analysis.phrases[0].moras;
            // Everything fallible has completed. Recording once changes only the proven
            // destination fields and preserves every other byte of the authored score.
            self.record(Edit::WriteLyrics);
            if let Some(clip) = self.project.midi_clip_mut(ClipId(request.target.clip)) {
                for ((note_index, mora), label) in indices.iter().zip(moras).zip(&labels) {
                    let note = &mut clip.notes[*note_index];
                    if note.lyric != *label || note.phonemes != mora.phonemes {
                        note.lyric.clone_from(label);
                        note.phonemes.clone_from(&mora.phonemes);
                        note.phoneme_seconds.clear();
                    }
                }
            }
            self.sync_singer_clip_names();
            applied = true;
        }
        Ok(serde_json::json!({
            "applied": applied, "selected_candidate": selected.map(|i|i+1),
            "revision": self.agent_document_revision()?, "candidate_reports": reports,
            "target": request.target, "reference": request.reference,
            "original_text": selected.map(|i| &request.candidates[i].text),
            "changes": changes,
            "attempt_limit": 3,
            "next_action": if selected.is_none() { "If fewer than three alternatives have been checked in this round, propose another within the same contract. Otherwise stop and report remaining diagnostics and the needed lyric or musical contract change." }
                else if applied { "Read notes and diagnose all related phrases again; inspect_singer and sing if the take is stale." }
                else { "Review the selected candidate, then apply with the same revision if still current." }
        }))
    }
}
