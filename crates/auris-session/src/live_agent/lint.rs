//! `lint_project` and `apply_lint_fixes`: the music linter, as a model reads it.

use auris_core::time::Ticks;

use crate::Session;
use crate::lint::{Diagnostic, LintReport};

/// A validated lint request: which bars, and which page of the findings.
#[derive(Clone, Debug)]
pub struct LintRequest {
    start_bar: Option<u32>,
    bars: Option<u32>,
    offset: usize,
    limit: usize,
}

impl LintRequest {
    pub(crate) fn new(
        start_bar: Option<u32>,
        bars: Option<u32>,
        offset: usize,
        limit: usize,
    ) -> Result<Self, String> {
        if start_bar == Some(0) || bars == Some(0) || !(1..=64).contains(&limit) {
            return Err("Use start_bar >= 1, bars >= 1 and limit 1..64".into());
        }
        if bars.is_some() && start_bar.is_none() {
            return Err("bars needs start_bar".into());
        }
        Ok(Self {
            start_bar,
            bars,
            offset,
            limit,
        })
    }
}

impl Session {
    /// The score lint plus the last audio lint, when that still describes this revision.
    pub(crate) fn current_lint(&self) -> LintReport {
        let mut report = self.lint_score();
        if let Some(audio) = &self.lint_audio
            && audio.revision == self.revision()
        {
            report.merge_audio(audio.clone(), self.project());
        }
        report
    }

    pub(crate) fn agent_lint_report(&self, request: &LintRequest) -> Result<String, String> {
        let report = self.current_lint();
        let project = self.project();
        let signatures = &project.signatures;
        let range = request.start_bar.map(|start| {
            let from = signatures.bar_start(start);
            let to = request
                .bars
                .map_or(Ticks(i64::MAX), |bars| signatures.bar_start(start + bars));
            (from, to)
        });
        let selected: Vec<&Diagnostic> = report
            .diagnostics
            .iter()
            .filter(|d| range.is_none_or(|(from, to)| d.overlaps(from, to)))
            .collect();
        let mut by_rule = serde_json::Map::new();
        for diagnostic in &selected {
            let count = by_rule
                .entry(diagnostic.rule.name())
                .or_insert(serde_json::json!(0));
            *count = serde_json::json!(count.as_u64().unwrap_or(0) + 1);
        }
        let page: Vec<_> = selected
            .iter()
            .skip(request.offset)
            .take(request.limit)
            .map(|d| {
                let track = d.track.and_then(|id| project.track(id));
                let (bar, beat, _) = signatures.bar_beat_at(d.start);
                let end_bar = signatures.bar_of(d.end - Ticks(1));
                serde_json::json!({
                    "id": d.id(),
                    "rule": d.rule.name(),
                    "severity": d.severity,
                    "track": d.track.map(|t| t.0),
                    "track_name": track.map(|t| t.name.as_str()),
                    "start_tick": d.start.raw(),
                    "end_tick": d.end.raw(),
                    "bar": bar,
                    "beat": beat,
                    "end_bar": end_bar,
                    "notes": d.notes.iter().map(|n| serde_json::json!({"clip": n.clip.0, "index": n.index})).collect::<Vec<_>>(),
                    "message": d.describe(),
                    "fix": d.describe_fix(),
                })
            })
            .collect();
        let audio = report.audio.as_ref();
        Ok(serde_json::json!({
            "revision": self.agent_document_revision()?,
            "total": selected.len(),
            "suppressed": report.suppressed,
            // What the user chose not to hear about, and why when they said: not to "fix".
            "ignore_rules": self.project().lint_suppressions,
            "offset": request.offset,
            "returned": page.len(),
            "by_rule": by_rule,
            "audio_checked": audio.is_some(),
            "audio": audio,
            "diagnostics": page,
            "note": if audio.is_none() {
                "Score rules only. Pass audio=true to also check clipping, loudness and silent tracks."
            } else {
                "Score and audio rules. Mechanical checks only: a clean report is not evidence of musical quality."
            },
        })
        .to_string())
    }

    pub(crate) fn agent_apply_lint_fixes(&mut self, ids: &[String]) -> Result<String, String> {
        if ids.is_empty() || ids.len() > 64 {
            return Err("Pass 1..64 ids from lint_project".into());
        }
        let report = self.current_lint();
        let mut applied = Vec::new();
        let mut missing = Vec::new();
        let mut unfixable = Vec::new();
        let mut chosen = Vec::new();
        for id in ids {
            match report.find(id) {
                Some(d) if d.fix.is_some() => {
                    if !chosen.iter().any(|c: &&Diagnostic| c.id() == *id) {
                        chosen.push(d);
                        applied.push(id.clone());
                    }
                }
                Some(_) => unfixable.push(id.clone()),
                None => missing.push(id.clone()),
            }
        }
        let count = self
            .apply_lint_fixes(&chosen, report.revision)
            .map_err(|e| e.to_string())?;
        let after = self.lint_score();
        Ok(serde_json::json!({
            "applied": applied,
            "fixes": count,
            "not_found": missing,
            "no_obvious_fix": unfixable,
            "revision": self.agent_document_revision()?,
            "score_diagnostics_now": after.diagnostics.len(),
            "note": "not_found ids were already fixed, moved by an edit, or came from an audio pass at an older revision; lint again. no_obvious_fix needs a musical decision.",
        })
        .to_string())
    }
}
