//! Exact phrase variations with explicit protected notes and editable fields.

use super::note_editing::{NoteEdit, NoteUpdate, candidate_project};
use auris_core::{ClipId, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

/// One scoped operation; unspecified notes and note metadata remain intact.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Keep rhythm and opening motif exactly, varying only specified later pitches.
    Pitches {
        /// All notes starting before this clip-relative tick are protected.
        preserve_prefix_until_tick: i64,
        /// Explicit pitch changes, 1..128 unique indices.
        changes: Vec<Pitch>,
    },
    /// Change only the pitch and held duration of explicit cadence arrivals.
    Cadence {
        /// Explicit landing changes, 1..128 unique indices.
        landings: Vec<Landing>,
    },
}

/// One existing note's proposed pitch.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pitch {
    /// Original storage index from read_notes.
    pub index: usize,
    /// MIDI pitch, 0..127.
    pub pitch: u8,
}

/// One arrival's proposed pitch and duration; attack and velocity are fixed.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Landing {
    /// Original storage index from read_notes.
    pub index: usize,
    /// MIDI pitch, 0..127.
    pub pitch: u8,
    /// Positive duration that must fit inside the requested range.
    pub length_ticks: i64,
}

/// A bounded, read-only proposal against one instrument or drum clip.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Stable clip ID from inspect_project.
    pub clip: u64,
    /// Inclusive clip-relative start tick.
    pub start_tick: i64,
    /// Exclusive clip-relative end tick; notes must fit completely inside the range.
    pub end_tick: i64,
    /// Original storage indices whose complete note data must remain identical.
    pub protected_indices: Vec<usize>,
    /// Allowed edits with exact model-authored values, not a random rewrite.
    pub operation: Operation,
}

/// Validate and construct the equivalent edit_notes patch without changing the source.
pub(crate) fn proposal(project: &Project, request: Request) -> Result<NoteEdit, String> {
    let (_, clip) = project
        .midi_clip(ClipId(request.clip))
        .ok_or("Unknown MIDI clip")?;
    if request.start_tick < 0
        || request.end_tick <= request.start_tick
        || request.end_tick > clip.length.raw()
        || request.protected_indices.len() > 4096
    {
        return Err(
            "Use a positive range inside the clip and at most 4096 protected indices".into(),
        );
    }
    let protected: HashSet<_> = request.protected_indices.iter().copied().collect();
    if protected.len() != request.protected_indices.len()
        || protected.iter().any(|&i| i >= clip.notes.len())
    {
        return Err("Protected indices must be unique existing notes".into());
    }
    // (index, pitch, new length in ticks): validated in ticks against the requested range, then
    // handed out as the edit_notes patch the caller can apply, which counts in beats.
    let (prefix, updates): (i64, Vec<(usize, u8, Option<i64>)>) = match request.operation {
        Operation::Pitches {
            preserve_prefix_until_tick,
            changes,
        } => {
            if !(request.start_tick..=request.end_tick).contains(&preserve_prefix_until_tick) {
                return Err("The protected opening must end inside the requested range".into());
            }
            (
                preserve_prefix_until_tick,
                changes
                    .into_iter()
                    .map(|p| (p.index, p.pitch, None))
                    .collect(),
            )
        }
        Operation::Cadence { landings } => (
            request.start_tick,
            landings
                .into_iter()
                .map(|p| (p.index, p.pitch, Some(p.length_ticks)))
                .collect(),
        ),
    };
    if updates.is_empty() || updates.len() > 128 {
        return Err("Supply 1..128 changes".into());
    }
    let mut seen = HashSet::new();
    for &(index, _, length_ticks) in &updates {
        let note = clip.notes.get(index).ok_or("Unknown note index")?;
        if !seen.insert(index)
            || protected.contains(&index)
            || note.start.raw() < prefix.max(request.start_tick)
            || note
                .start
                .raw()
                .checked_add(note.length.raw())
                .is_none_or(|end| end > request.end_tick)
            || note
                .start
                .raw()
                .checked_add(length_ticks.unwrap_or(note.length.raw()))
                .is_none_or(|end| end > request.end_tick)
        {
            return Err("Change touches a protected, duplicate or out-of-range note".into());
        }
    }
    let update = updates
        .into_iter()
        .map(|(index, pitch, length_ticks)| {
            let per_beat = project
                .signatures
                .signature_at(clip.start + clip.notes[index].start)
                .ticks_per_beat()
                .raw() as f64;
            NoteUpdate {
                index,
                pitch: Some(pitch),
                bar: None,
                beat: None,
                length_beats: length_ticks.map(|ticks| ticks as f64 / per_beat),
                velocity: None,
            }
        })
        .collect();
    let edit = NoteEdit::Patch {
        clip: request.clip,
        remove: vec![],
        update,
        add: vec![],
    };
    // Use exactly the normal note validator, including render expansion and clip bounds.
    candidate_project(project, vec![edit.clone()])?;
    Ok(edit)
}

impl crate::Session {
    pub(super) fn agent_phrase_variation(
        &self,
        expected_revision: &str,
        request: Request,
    ) -> Result<String, String> {
        if self.agent_document_revision()? != expected_revision {
            return Err("Document changed; read notes again".into());
        }
        let clip = ClipId(request.clip);
        let edit = proposal(self.project(), request)?;
        let candidate = candidate_project(self.project(), vec![edit.clone()])?;
        let before = &self.project().midi_clip(clip).unwrap().1.notes;
        let after = &candidate.midi_clip(clip).unwrap().1.notes;
        let changes: Vec<Value> = before
            .iter()
            .zip(after)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, (a, b))| json!({"index":index,"before":a,"after":b}))
            .collect();
        Ok(json!({"revision":expected_revision,"document_unchanged":true,"changes":changes,
            "edit":edit,"note_units":"request range is clip-relative ticks; the returned edit counts length_beats in the meter's beat, as edit_notes does",
            "next":"Use this request as a candidates variation for comparison, or apply the returned edit through edit_notes. No musical preference has been established."}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Session, SessionOptions,
        prelude::{Note, Ticks},
    };
    fn fixture() -> (Session, u64) {
        let mut s = Session::new(SessionOptions::headless()).unwrap();
        let t = s.add_default_instrument_track("Lead").unwrap();
        let c = s
            .add_midi_clip(t, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        s.replace_notes(
            c,
            (0..4)
                .map(|i| Note::new(60 + i as u8, Ticks::QUARTER * i, Ticks::QUARTER))
                .collect(),
        )
        .unwrap();
        s.forget_history();
        (s, c.0)
    }
    fn request(clip: u64, index: usize) -> Request {
        Request {
            clip,
            start_tick: 0,
            end_tick: Ticks::QUARTER.raw() * 4,
            protected_indices: vec![1],
            operation: Operation::Pitches {
                preserve_prefix_until_tick: Ticks::QUARTER.raw(),
                changes: vec![Pitch { index, pitch: 67 }],
            },
        }
    }
    #[test]
    fn pitch_variation_matches_exact_edit_notes_and_preserves_every_other_field() {
        let (mut s, c) = fixture();
        let before = s.project().clone();
        let edit = proposal(&before, request(c, 2)).unwrap();
        let candidate = candidate_project(&before, vec![edit.clone()]).unwrap();
        s.agent_edit_notes(&s.agent_document_revision().unwrap(), vec![edit])
            .unwrap();
        assert_eq!(s.project(), &candidate);
        let mut expected = before.clone();
        expected.midi_clip_mut(ClipId(c)).unwrap().notes[2].pitch = 67;
        assert_eq!(candidate, expected);
        s.undo();
        assert_eq!(s.project(), &before);
    }
    #[test]
    fn protected_prefix_duplicates_and_outside_landings_are_rejected_atomically() {
        let (s, c) = fixture();
        let before = s.project().clone();
        for i in [0, 1, 4] {
            assert!(proposal(&before, request(c, i)).is_err());
        }
        let mut req = request(c, 2);
        req.end_tick = Ticks::QUARTER.raw() * 2;
        assert!(proposal(&before, req).is_err());
        let mut req = request(c, 3);
        req.operation = Operation::Cadence {
            landings: vec![Landing {
                index: 3,
                pitch: 60,
                length_ticks: Ticks::QUARTER.raw() + 1,
            }],
        };
        assert!(proposal(&before, req).is_err());
        let mut req = request(c, 2);
        req.operation = Operation::Cadence {
            landings: vec![Landing {
                index: 2,
                pitch: 60,
                length_ticks: Ticks::QUARTER.raw() * 2,
            }],
        };
        let candidate = candidate_project(&before, vec![proposal(&before, req).unwrap()]).unwrap();
        let mut expected = before.clone();
        let note = &mut expected.midi_clip_mut(ClipId(c)).unwrap().notes[2];
        note.pitch = 60;
        note.length = Ticks::QUARTER * 2;
        assert_eq!(candidate, expected);
        assert_eq!(s.project(), &before);
        assert!(!s.can_undo());
    }
}
