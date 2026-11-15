//! Read-only melodic harmony proposals, shared by live and saved agent transports.

use crate::{Session, prelude::*};
use auris_compose::harmony_suggestions::{self as suggestions, Context};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;

/// Intended ending of the requested phrase.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ending {
    /// Prefer a tonic arrival when compatible with the melody.
    #[default]
    Settled,
    /// Leave the phrase open for continuation.
    Open,
}

/// Bounded source and musical intent for harmony suggestions.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// 1..8 distinct pitched melody clip IDs, selected by musical role, not track name.
    #[schemars(length(min = 1, max = 8))]
    pub melody_clips: Vec<u64>,
    /// One-based first bar of the phrase.
    #[schemars(range(min = 1))]
    pub start_bar: u32,
    /// Phrase length, 1..16 bars and at most 128 quarter notes; split at meter/key changes.
    #[schemars(range(min = 1, max = 16))]
    pub bars: u32,
    /// Optional explicit key. Omitted uses the current project key, without inferring another mode.
    pub key: Option<String>,
    /// neutral, bright, dark, calm, driving, epic, dreamy, tense or funky. Omitted is neutral.
    #[serde(default = "neutral")]
    pub mood: String,
    /// Phrase ending; omitted prefers a settled arrival. Neither choice forces complex chords.
    #[serde(default)]
    pub ending: Ending,
}

fn neutral() -> String {
    "neutral".into()
}

impl Session {
    pub(super) fn agent_suggest_harmony(&self, request: Request) -> Result<String, String> {
        // A chord fitted to a melody is pitch classes fitted to pitch classes.
        crate::session::tuning::require_twelve_tone(self.project())
            .map_err(|error| error.to_string())?;
        if request.melody_clips.is_empty()
            || request.melody_clips.len() > 8
            || request.melody_clips.iter().collect::<HashSet<_>>().len()
                != request.melody_clips.len()
        {
            return Err("melody_clips requires 1..8 distinct pitched clip IDs".into());
        }
        if request.start_bar == 0 || !(1..=16).contains(&request.bars) {
            return Err("Use start_bar >= 1 and bars within 1..16".into());
        }
        let after = request
            .start_bar
            .checked_add(request.bars)
            .ok_or("Bar range overflow")?;
        let project = self.project();
        let start = project.signatures.bar_start(request.start_bar);
        let end = project.signatures.bar_start(after);
        if end <= start || end - start > Ticks::QUARTER * 128 {
            return Err("Phrase exceeds 128 quarter notes".into());
        }
        if project
            .signatures
            .points()
            .iter()
            .any(|p| start < p.tick && p.tick < end)
            || project
                .harmony
                .keys
                .points()
                .iter()
                .any(|p| start < p.tick && p.tick < end)
        {
            return Err("Split the request at internal meter or key changes".into());
        }
        if request.key.as_ref().is_some_and(|key| key.len() > 128) || request.mood.len() > 64 {
            return Err("Key or mood exceeds the input limit".into());
        }
        let key = request
            .key
            .as_deref()
            .map(|text| MusicalKey::parse(text).ok_or("Invalid key"))
            .transpose()?
            .unwrap_or_else(|| project.harmony.key_at(start));
        let mood = Mood::named(&request.mood).ok_or(
            "Unknown mood; use neutral, bright, dark, calm, driving, epic, dreamy, tense or funky",
        )?;
        let mut notes = Vec::new();
        let mut source_count = 0usize;
        for id in &request.melody_clips {
            let (track, clip) = project
                .midi_clip(ClipId(*id))
                .ok_or("Unknown melody clip")?;
            if project
                .track(track)
                .is_none_or(|track| track.kind.is_drum())
            {
                return Err("Select pitched instrument or singer clips, not drums".into());
            }
            source_count += clip.notes.len();
            if source_count > 4096 {
                return Err(
                    "Melody sources are limited to 4096 stored notes; split long clips first"
                        .into(),
                );
            }
            let playable: Vec<_> = clip.playable_notes().collect();
            for (offset, span) in auris_core::project::loop_passes(clip.length, clip.loop_end) {
                let at = clip.start + offset;
                if at >= end {
                    break;
                }
                if at + span <= start {
                    continue;
                }
                for note in &playable {
                    let from = (at + note.start).max(start);
                    let to = (at + note.end().min(span)).min(end);
                    if to <= from {
                        continue;
                    }
                    if notes.len() == 4096 {
                        return Err(
                            "The expanded phrase exceeds 4096 melody notes; shorten the request"
                                .into(),
                        );
                    }
                    let mut note = note.clone();
                    note.start = from - start;
                    note.length = to - from;
                    notes.push(note);
                }
            }
        }
        notes.sort_by_key(|note| (note.start, note.pitch, note.length));
        let context = Context {
            key,
            signature: project.signatures.signature_at(start),
            bars: request.bars as usize,
            mood,
            ending: match request.ending {
                Ending::Settled => suggestions::Ending::Settled,
                Ending::Open => suggestions::Ending::Open,
            },
        };
        let suggestions = suggestions::suggest(&notes, context)?;
        let round = |value: f64| (value * 1000.0).round() / 1000.0;
        let candidates: Vec<_> = suggestions.iter().enumerate().map(|(index, candidate)| {
            let bars: Vec<_> = candidate.evidence.iter().zip(&candidate.chart.bars).map(|(evidence, numerals)| json!({
                "bar":request.start_bar + evidence.bar as u32 - 1,
                "chords":numerals.iter().map(|n| n.chord_in(key).name_in(key)).collect::<Vec<_>>(),
                "melody_fit":round(evidence.melody_fit),"salient_non_chord":round(evidence.salient_non_chord),"reasons":evidence.reasons
            })).collect();
            json!({"id":format!("candidate_{}",index+1),"recommended":index==0,"plain_alternative":candidate.plain,
                "heuristic_score":round(candidate.score),"bars":bars,
                "harmony":{"start_bar":request.start_bar,"bars":request.bars,"key":key.to_text(),"chords":candidate.chart.to_text()}})
        }).collect();
        Ok(json!({"revision":self.agent_document_revision()?,"protected_clips":request.melody_clips,"foreground_clips":request.melody_clips,
            "key":key.to_text(),"mood":request.mood,"ending":request.ending,"note_count":notes.len(),
            "motif_steps":auris_compose::motif_of(&notes,key),"candidates":candidates,"changed":false,
            "basis":"Authored notes with clip windows and loops; performance transforms, bends, timbre and audio are not evaluated. Scores compare heuristic fit, not artistic quality.",
            "usage":"Compare the plain alternative and melodic tensions. To accept a proposal, pass its harmony and this revision to accompaniment regenerate with explicit backing clip IDs, or write harmony before generating new backing. No notes or harmony have been changed. Audition the same bars after applying."}).to_string())
    }
}

#[cfg(test)]
mod tests;
