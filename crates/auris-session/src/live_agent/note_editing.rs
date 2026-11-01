//! Exact tick-based note editing shared by file and live agents.

use crate::{Session, prelude::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// One bounded page of authored notes in a multi-clip read.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteRead {
    /// Stable MIDI clip ID.
    pub clip: u64,
    /// First original storage index; defaults to zero.
    #[serde(default)]
    pub offset: usize,
}

/// A newly authored note at a musical position: song bar and beat, never a bare tick.
///
/// The model thinks in bars and beats; asking it for integer ticks at 960 per quarter made every
/// attack a multiplication it could get wrong, and a wrong product is a note that lands a few
/// hundred ticks from any beat. The editor does that arithmetic instead, and refuses an attack
/// that is not on a musical subdivision rather than writing it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthoredNote {
    /// MIDI pitch, 0..127.
    #[schemars(range(min = 0, max = 127))]
    pub pitch: u8,
    /// 1-based song bar of the attack, the same bar numbers as add_clip and edit_harmony; not
    /// counted from the clip.
    #[schemars(range(min = 1))]
    pub bar: u32,
    /// 1-based beat inside that bar, in the meter's beat (a quarter in 4/4, an eighth in 6/8):
    /// 1 is the downbeat, 2.5 the offbeat of beat 2, 2.25/2.75 sixteenths, 2.333/2.667 triplets.
    #[schemars(range(min = 1))]
    pub beat: f64,
    /// Held duration in the same beats: 1 = one beat, 0.5 half a beat, 0.333 a triplet; may
    /// cross bar lines.
    pub length_beats: f64,
    /// Attack strength, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub velocity: f32,
}

/// Partial changes to an existing note; all other metadata is preserved.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteUpdate {
    /// Zero-based storage index returned by read_notes.
    pub index: usize,
    /// Optional MIDI pitch, 0..127.
    pub pitch: Option<u8>,
    /// Optional new 1-based song bar; supply together with beat.
    pub bar: Option<u32>,
    /// Optional new 1-based beat inside bar; supply together with bar.
    pub beat: Option<f64>,
    /// Optional new duration in the meter's beats.
    pub length_beats: Option<f64>,
    /// Optional attack strength, 0..1.
    pub velocity: Option<f32>,
}

/// How far, in beats, an attack may sit from a subdivision and still be read as meaning it.
///
/// A hundredth of a beat absorbs `2.33` for a triplet and nothing a person would write on
/// purpose; anything further is refused so the caller sees the mistake instead of hearing it.
const GRID_TOLERANCE_BEATS: f64 = 0.01;

/// The subdivisions of a beat an authored attack may name: straight down to an eighth of a
/// beat (32nds in 4/4) and triplets down to a sixth (sixteenth-note triplets).
const BEAT_DIVISIONS: [i64; 2] = [8, 6];

/// Absolute attack of 1-based `bar` and fractional 1-based `beat`, snapped to the beat grid.
fn attack_at(signatures: &SignatureMap, bar: u32, beat: f64) -> Result<Ticks, String> {
    if bar == 0 {
        return Err("bar is 1-based; the first bar is 1".into());
    }
    if !beat.is_finite() {
        return Err("beat must be a finite number".into());
    }
    let start = signatures.bar_start(bar);
    let signature = signatures.signature_at(start);
    let per_beat = signature.ticks_per_beat().raw();
    let beats = f64::from(signature.numerator);
    if beat < 1.0 || beat >= beats + 1.0 {
        return Err(format!(
            "beat {beat} is outside bar {bar}, which is {signature} with beats 1..{}; write a later attack as the next bar",
            signature.numerator
        ));
    }
    let offset = (beat - 1.0) * per_beat as f64;
    let snapped = BEAT_DIVISIONS
        .iter()
        .map(|division| {
            let step = (per_beat / division).max(1);
            (offset / step as f64).round() as i64 * step
        })
        .min_by(|a, b| {
            (*a as f64 - offset)
                .abs()
                .total_cmp(&(*b as f64 - offset).abs())
        })
        .unwrap_or(0);
    if (snapped as f64 - offset).abs() > GRID_TOLERANCE_BEATS * per_beat as f64 {
        let nearest = |division: i64| {
            let step = (per_beat / division).max(1);
            beat_number((offset / step as f64).round() as i64 * step, per_beat)
        };
        return Err(format!(
            "bar {bar} beat {beat} is between subdivisions; the nearest straight beat is {} and the nearest triplet {}. Use swing or humanize performance for feel, not offset attacks",
            nearest(8),
            nearest(6)
        ));
    }
    Ok(start + Ticks(snapped))
}

/// Ticks held by `length_beats` of the meter in force at the absolute `attack`.
fn duration_at(
    signatures: &SignatureMap,
    attack: Ticks,
    length_beats: f64,
) -> Result<Ticks, String> {
    let per_beat = signatures.signature_at(attack).ticks_per_beat().raw();
    if !length_beats.is_finite() || length_beats <= 0.0 {
        return Err("length_beats must be a positive number of beats".into());
    }
    let ticks = (length_beats * per_beat as f64).round() as i64;
    if ticks < 1 {
        return Err(format!(
            "length_beats {length_beats} is shorter than the editor resolves"
        ));
    }
    Ok(Ticks(ticks))
}

/// A 1-based beat number for an offset into a bar, to four decimal places.
fn beat_number(offset: i64, per_beat: i64) -> f64 {
    rounded(1.0 + offset as f64 / per_beat.max(1) as f64)
}

fn rounded(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// A stored note's position as read_notes reports it: song bar, beat, length in beats, and
/// whether the attack is one edit_notes would accept unchanged.
fn musical_position(
    signatures: &SignatureMap,
    attack: Ticks,
    length: Ticks,
) -> (u32, f64, f64, bool) {
    let bar_start = signatures.bar_floor(attack);
    let per_beat = signatures
        .signature_at(bar_start)
        .ticks_per_beat()
        .raw()
        .max(1);
    let offset = attack.raw() - bar_start.raw();
    let on_grid = BEAT_DIVISIONS
        .iter()
        .any(|division| offset % (per_beat / division).max(1) == 0);
    let length_per_beat = signatures
        .signature_at(attack)
        .ticks_per_beat()
        .raw()
        .max(1);
    (
        signatures.bar_of(attack),
        beat_number(offset, per_beat),
        rounded(length.raw() as f64 / length_per_beat as f64),
        on_grid,
    )
}

/// Resolves authored positions against one clip, so a message can name its bars.
struct ClipClock<'a> {
    signatures: &'a SignatureMap,
    id: ClipId,
    start: Ticks,
    length: Ticks,
}

impl ClipClock<'_> {
    fn relative(&self, bar: u32, beat: f64) -> Result<Ticks, String> {
        let attack = attack_at(self.signatures, bar, beat)?;
        let relative = attack - self.start;
        if relative.raw() < 0 || relative >= self.length {
            let first = self.signatures.bar_of(self.start);
            let last = self.signatures.bar_of(self.start + self.length - Ticks(1));
            return Err(format!(
                "bar {bar} beat {beat} is outside clip {}, which covers bars {first}..{last}; positions are song bars, not counted from the clip",
                self.id.0
            ));
        }
        Ok(relative)
    }

    fn length(&self, relative: Ticks, length_beats: f64) -> Result<Ticks, String> {
        duration_at(self.signatures, self.start + relative, length_beats)
    }

    fn note(&self, authored: AuthoredNote) -> Result<Note, String> {
        let start = self.relative(authored.bar, authored.beat)?;
        let length = self.length(start, authored.length_beats)?;
        Ok(Note {
            velocity: authored.velocity,
            ..Note::new(authored.pitch, start, length)
        })
    }
}

/// One clip's patch or complete replacement within an atomic edit.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NoteEdit {
    /// Update and remove original indices, then append new notes.
    Patch {
        /// Stable MIDI clip ID.
        clip: u64,
        /// Original storage indices to remove; duplicate indices are refused.
        #[serde(default)]
        remove: Vec<usize>,
        /// Partial updates against original storage indices; removed notes cannot be updated.
        #[serde(default)]
        update: Vec<NoteUpdate>,
        /// New notes appended after removals and updates.
        #[serde(default)]
        add: Vec<AuthoredNote>,
    },
    /// Replace all authored notes; an empty array clears the clip.
    Replace {
        /// Stable MIDI clip ID.
        clip: u64,
        /// Complete replacement, in exact ticks.
        notes: Vec<AuthoredNote>,
    },
}

impl NoteEdit {
    /// Whether this edit may remove or replace existing note data.
    pub fn requires_confirmation(&self) -> bool {
        match self {
            Self::Replace { .. } => true,
            Self::Patch { remove, .. } => !remove.is_empty(),
        }
    }
}

/// How read_notes and edit_notes count time, said once in every read.
const TIME_UNITS: &str = "bar is the 1-based song bar (clip bars are [first,last]); beat is 1-based in the meter's beat, fractional within the bar; length_beats uses the same beat. on_grid:false marks a stored attack between subdivisions.";

impl Session {
    pub(super) fn agent_read_notes(&self, requests: &[NoteRead]) -> Result<String, String> {
        if requests.is_empty() || requests.len() > 8 {
            return Err("Supply 1..8 distinct clip pages".into());
        }
        let signatures = &self.project().signatures;
        let mut seen = HashSet::new();
        let mut pages = Vec::with_capacity(requests.len());
        for request in requests {
            if !seen.insert(request.clip) {
                return Err("Duplicate clip in read_notes".into());
            }
            let (_, clip) = self
                .project()
                .midi_clip(ClipId(request.clip))
                .ok_or_else(|| {
                    let prefix = if self.project().track(TrackId(request.clip)).is_some() {
                        format!("{} is a track ID, not a MIDI clip ID", request.clip)
                    } else {
                        format!("Unknown MIDI clip ID {}", request.clip)
                    };
                    let available: Vec<_> = self.project().tracks.iter()
                        .flat_map(|track| track.kind.note_clips().into_iter().flatten()
                            .map(move |clip| serde_json::json!({"track":track.id.0,"clip":clip.id.0})))
                        .take(8).collect();
                    format!("{prefix}; MIDI clip addresses (first 8): {}. Copy tracks[].clips[].clip from inspect_project, not tracks[].track.", serde_json::json!(available))
                })?;
            if request.offset > clip.notes.len() {
                return Err("Offset exceeds the note count".into());
            }
            // Pages retain storage order because `index` is the edit address.  Give every
            // note its rank in a separate, stable timeline view so callers can sort lyric
            // phrases without ever confusing that rank with an editable storage index.
            let mut timeline_order: Vec<_> = (0..clip.notes.len()).collect();
            timeline_order.sort_by_key(|index| (clip.notes[*index].start, *index));
            let mut timeline_indices = vec![0; clip.notes.len()];
            for (timeline_index, storage_index) in timeline_order.into_iter().enumerate() {
                timeline_indices[storage_index] = timeline_index;
            }
            let notes: Vec<_> = clip
                .notes
                .iter()
                .enumerate()
                .skip(request.offset)
                .take(128)
                .map(|(index, note)| {
                    let mut value =
                        serde_json::to_value(note).map_err(|error| error.to_string())?;
                    let fields = value.as_object_mut().ok_or("Expected a note object")?;
                    fields.remove("start");
                    fields.remove("length");
                    let (bar, beat, length_beats, on_grid) =
                        musical_position(signatures, clip.start + note.start, note.length);
                    fields.insert("bar".into(), bar.into());
                    fields.insert("beat".into(), beat.into());
                    fields.insert("length_beats".into(), length_beats.into());
                    if !on_grid {
                        fields.insert("on_grid".into(), false.into());
                    }
                    fields.insert("index".into(), index.into());
                    fields.insert("timeline_index".into(), timeline_indices[index].into());
                    Ok::<_, String>(value)
                })
                .collect::<Result<_, _>>()?;
            let next = request.offset + notes.len();
            let first_bar = signatures.bar_of(clip.start);
            let last_bar = signatures.bar_of(clip.start + clip.length - Ticks(1));
            pages.push(serde_json::json!({"clip":request.clip,"bars":[first_bar,last_bar],"offset":request.offset,"notes":notes,"total":clip.notes.len(),
                "next_offset":(next < clip.notes.len()).then_some(next),"truncated":next < clip.notes.len()}));
        }
        Ok(serde_json::json!({"revision":self.agent_document_revision()?,"time":TIME_UNITS,"clips":pages}).to_string())
    }

    /// Content-based revision, also stable across saved-project calls and process restarts.
    pub fn agent_document_revision(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(self.project()).map_err(|e| e.to_string())?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    /// Apply exact note changes atomically, preserving clip recipes, curves and performance.
    pub fn agent_edit_notes(
        &mut self,
        expected_revision: &str,
        edits: Vec<NoteEdit>,
    ) -> Result<String, String> {
        if edits.is_empty() || edits.len() > 16 {
            return Err("edits must contain 1..16 clips".into());
        }
        if self.agent_document_revision()? != expected_revision {
            return Err("Document changed; call read_notes again before editing".into());
        }
        let replacements = note_replacements(self.project(), edits)?;
        let changed = self.replace_notes_batch(replacements)?;
        Ok(
            serde_json::json!({"changed_clips":changed,"revision":self.agent_document_revision()?})
                .to_string(),
        )
    }
}

fn note_replacements(
    project: &Project,
    edits: Vec<NoteEdit>,
) -> Result<Vec<(ClipId, Vec<Note>)>, String> {
    if edits.is_empty() || edits.len() > 16 {
        return Err("edits must contain 1..16 clips".into());
    }
    let mut replacements = Vec::with_capacity(edits.len());
    let mut seen = HashSet::new();
    let mut total = 0usize;
    for edit in edits {
        let clip = match &edit {
            NoteEdit::Patch { clip, .. } | NoteEdit::Replace { clip, .. } => ClipId(*clip),
        };
        if !seen.insert(clip) {
            return Err("Duplicate clip in edits".into());
        }
        let (_, target) = project.midi_clip(clip).ok_or("Unknown MIDI clip ID")?;
        let clock = ClipClock {
            signatures: &project.signatures,
            id: clip,
            start: target.start,
            length: target.length,
        };
        let authored = |notes: Vec<AuthoredNote>| -> Result<Vec<Note>, String> {
            notes
                .into_iter()
                .enumerate()
                .map(|(i, note)| clock.note(note).map_err(|e| format!("notes[{i}]: {e}")))
                .collect()
        };
        let notes = match edit {
            NoteEdit::Replace { notes, .. } => authored(notes)?,
            NoteEdit::Patch {
                remove,
                update,
                add,
                ..
            } => {
                if remove.len() > 65_536 || update.len() > 65_536 || add.len() > 65_536 {
                    return Err("At most 65536 note changes per clip".into());
                }
                let removals: HashSet<_> = remove.iter().copied().collect();
                if removals.len() != remove.len() || remove.iter().any(|&i| i >= target.notes.len())
                {
                    return Err("Remove indices must be unique existing note indices".into());
                }
                let mut notes = target.notes.clone();
                let mut updated = HashSet::new();
                for change in update {
                    if removals.contains(&change.index) || !updated.insert(change.index) {
                        return Err("Cannot update a removed note or the same note twice".into());
                    }
                    let note = notes.get_mut(change.index).ok_or("Unknown note index")?;
                    let index = change.index;
                    let located = |e: String| format!("update index {index}: {e}");
                    if let Some(value) = change.pitch {
                        note.pitch = value;
                    }
                    match (change.bar, change.beat) {
                        (Some(bar), Some(beat)) => {
                            note.start = clock.relative(bar, beat).map_err(located)?;
                        }
                        (None, None) => {}
                        _ => return Err(located("supply bar and beat together".into())),
                    }
                    if let Some(value) = change.length_beats {
                        note.length = clock.length(note.start, value).map_err(located)?;
                    }
                    if let Some(value) = change.velocity {
                        note.velocity = value;
                    }
                }
                let mut index = 0;
                notes.retain(|_| {
                    let keep = !removals.contains(&index);
                    index += 1;
                    keep
                });
                notes.extend(authored(add)?);
                notes
            }
        };
        total += notes.len();
        if total > 65_536 {
            return Err("At most 65536 resulting notes per batch".into());
        }
        replacements.push((clip, notes));
    }
    Ok(replacements)
}

pub(crate) fn candidate_project(
    project: &Project,
    edits: Vec<NoteEdit>,
) -> Result<Project, String> {
    let replacements = note_replacements(project, edits)?;
    let mut candidate = project.clone();
    for (clip, notes) in replacements {
        let (track, target) = project.midi_clip(clip).ok_or("Unknown MIDI clip")?;
        if project
            .track(track)
            .is_some_and(|t| matches!(t.kind, auris_core::project::TrackKind::Singer(_)))
        {
            return Err(
                "Note candidates require instrument or drum clips; singer notes require synthesis"
                    .into(),
            );
        }
        *candidate.midi_clip_mut(clip).ok_or("Unknown MIDI clip")? =
            crate::session::notes::replacement_clip(target, notes).map_err(|e| e.to_string())?;
    }
    Ok(candidate)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_and_wrong_track_id_distinguish_the_clip_address() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        let before = session.project().clone();
        let inspected: serde_json::Value = serde_json::from_str(
            &session
                .agent_command(super::super::Command::Inspect {})
                .unwrap(),
        )
        .unwrap();
        assert_eq!(inspected["tracks"][0]["track"], track.0);
        assert_eq!(inspected["tracks"][0]["clips"][0]["clip"], clip.0);
        assert!(inspected["tracks"][0].get("id").is_none());
        assert!(inspected["tracks"][0]["clips"][0].get("id").is_none());
        let error = session
            .agent_read_notes(&[NoteRead {
                clip: track.0,
                offset: 0,
            }])
            .unwrap_err();
        assert!(
            error.contains(&format!("{} is a track ID", track.0)),
            "{error}"
        );
        assert!(error.contains(&format!("\"clip\":{}", clip.0)), "{error}");
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn multi_clip_reads_share_a_revision_and_page_original_indices_without_editing() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Read").unwrap();
        let a = session
            .add_midi_clip(track, "Long", Ticks::ZERO, Ticks::QUARTER * 200)
            .unwrap();
        let b = session
            .add_midi_clip(track, "Empty", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session
            .replace_notes(
                a,
                (0..129)
                    .map(|i| Note::new(60, Ticks::QUARTER * i, Ticks(13)))
                    .collect(),
            )
            .unwrap();
        let before = session.project().clone();
        let result: serde_json::Value = serde_json::from_str(
            &session
                .agent_read_notes(&[
                    NoteRead {
                        clip: a.0,
                        offset: 0,
                    },
                    NoteRead {
                        clip: b.0,
                        offset: 0,
                    },
                ])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            result["revision"],
            session.agent_document_revision().unwrap()
        );
        assert_eq!(result["clips"][0]["notes"].as_array().unwrap().len(), 128);
        assert_eq!(result["clips"][0]["next_offset"], 128);
        assert_eq!(result["clips"][0]["truncated"], true);
        assert_eq!(result["clips"][1]["total"], 0);
        let last: serde_json::Value = serde_json::from_str(
            &session
                .agent_read_notes(&[NoteRead {
                    clip: a.0,
                    offset: 128,
                }])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(last["revision"], result["revision"]);
        assert_eq!(last["clips"][0]["notes"][0]["index"], 128);
        assert_eq!(last["clips"][0]["notes"][0]["bar"], 33);
        assert_eq!(last["clips"][0]["notes"][0]["beat"], 1.0);
        assert!(last["clips"][0]["next_offset"].is_null());
        assert!(
            session
                .agent_read_notes(&[
                    NoteRead {
                        clip: a.0,
                        offset: 0
                    },
                    NoteRead {
                        clip: a.0,
                        offset: 0
                    }
                ])
                .is_err()
        );
        assert!(
            session
                .agent_read_notes(&[NoteRead {
                    clip: a.0,
                    offset: 130
                }])
                .is_err()
        );
        assert!(
            session
                .agent_read_notes(&[NoteRead {
                    clip: u64::MAX,
                    offset: 0
                }])
                .is_err()
        );
        assert!(session.agent_read_notes(&[]).is_err());
        assert!(
            session
                .agent_read_notes(&vec![
                    NoteRead {
                        clip: a.0,
                        offset: 0
                    };
                    9
                ])
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn read_notes_exposes_stable_timeline_rank_without_changing_storage_addresses() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Vocal").unwrap();
        let clip = session
            .add_midi_clip(track, "Unsorted", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .replace_notes(
                clip,
                vec![
                    Note::new(62, Ticks(960), Ticks(120)),
                    Note::new(60, Ticks::ZERO, Ticks(120)),
                    Note::new(64, Ticks::ZERO, Ticks(120)),
                ],
            )
            .unwrap();

        let result: serde_json::Value = serde_json::from_str(
            &session
                .agent_read_notes(&[NoteRead {
                    clip: clip.0,
                    offset: 0,
                }])
                .unwrap(),
        )
        .unwrap();
        let notes = result["clips"][0]["notes"].as_array().unwrap();
        assert_eq!(notes[0]["index"], 0);
        assert_eq!(notes[0]["timeline_index"], 2);
        assert_eq!(notes[1]["index"], 1);
        assert_eq!(notes[1]["timeline_index"], 0);
        assert_eq!(notes[2]["index"], 2);
        assert_eq!(notes[2]["timeline_index"], 1);
        assert_eq!(
            (notes[1]["bar"].clone(), notes[1]["beat"].clone()),
            (1.into(), 1.0.into())
        );
        assert_eq!(
            (notes[2]["bar"].clone(), notes[2]["beat"].clone()),
            (1.into(), 1.0.into())
        );
    }

    #[test]
    fn timeline_rank_is_global_across_storage_pages() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Paged", Ticks::ZERO, Ticks::QUARTER * 200)
            .unwrap();
        session
            .replace_notes(
                clip,
                (0..129)
                    .rev()
                    .map(|index| Note::new(60, Ticks(index * 960), Ticks(12)))
                    .collect(),
            )
            .unwrap();

        let last: serde_json::Value = serde_json::from_str(
            &session
                .agent_read_notes(&[NoteRead {
                    clip: clip.0,
                    offset: 128,
                }])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(last["clips"][0]["notes"][0]["index"], 128);
        assert_eq!(last["clips"][0]["notes"][0]["timeline_index"], 0);
    }

    #[test]
    fn write_lyrics_uses_timeline_order_but_keeps_storage_indices_editable() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_singer_track("Vocal");
        let clip = session
            .add_midi_clip(track, "Unsorted", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .replace_notes(
                clip,
                vec![
                    Note::new(62, Ticks(960), Ticks(120)),
                    Note::new(60, Ticks::ZERO, Ticks(120)),
                    Note::new(64, Ticks(1920), Ticks(120)),
                ],
            )
            .unwrap();

        session.write_lyrics(clip, &[0, 1, 2], "あいう").unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!(notes[0].lyric, "い");
        assert_eq!(notes[1].lyric, "あ");
        assert_eq!(notes[2].lyric, "う");

        let result: serde_json::Value = serde_json::from_str(
            &session
                .agent_read_notes(&[NoteRead {
                    clip: clip.0,
                    offset: 0,
                }])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["clips"][0]["notes"][0]["index"], 0);
        assert_eq!(result["clips"][0]["notes"][0]["timeline_index"], 1);
        assert_eq!(result["clips"][0]["notes"][1]["index"], 1);
        assert_eq!(result["clips"][0]["notes"][1]["timeline_index"], 0);
        let earliest = result["clips"][0]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|note| note["timeline_index"] == 0)
            .unwrap();
        let edit: NoteEdit = serde_json::from_value(serde_json::json!({
            "kind":"patch", "clip":clip.0,
            "update":[{"index":earliest["index"],"pitch":67}]
        }))
        .unwrap();
        session
            .agent_edit_notes(result["revision"].as_str().unwrap(), vec![edit])
            .unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!((notes[1].pitch, notes[1].lyric.as_str()), (67, "あ"));
        assert_eq!((notes[0].pitch, notes[0].lyric.as_str()), (62, "い"));
    }

    #[test]
    fn tick_edits_preserve_metadata_rollback_stale_reads_and_undo_as_one() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Notes", Ticks::ZERO, Ticks(3840))
            .unwrap();
        let mut note = Note::new(60, Ticks(13), Ticks(97));
        note.lyric = "keep".into();
        note.phonemes = vec!["a".into()];
        session.replace_notes(clip, vec![note]).unwrap();
        let before = session.project().clone();
        let revision = session.agent_document_revision().unwrap();
        let edit = NoteEdit::Patch {
            clip: clip.0,
            remove: vec![],
            add: vec![],
            update: vec![NoteUpdate {
                index: 0,
                pitch: Some(62),
                bar: None,
                beat: None,
                length_beats: None,
                velocity: None,
            }],
        };
        session
            .agent_edit_notes(&revision, vec![edit.clone()])
            .unwrap();
        let result = &session.project().midi_clip(clip).unwrap().1.notes[0];
        assert_eq!(
            (
                result.start.raw(),
                result.length.raw(),
                result.lyric.as_str(),
                result.phonemes.as_slice()
            ),
            (13, 97, "keep", ["a".to_owned()].as_slice())
        );
        let after = session.project().clone();
        assert!(session.agent_edit_notes(&revision, vec![edit]).is_err());
        assert_eq!(session.project(), &after);
        session.undo();
        assert_eq!(session.project(), &before);
        let valid = NoteEdit::Replace {
            clip: clip.0,
            notes: vec![],
        };
        let invalid = NoteEdit::Replace {
            clip: u64::MAX,
            notes: vec![],
        };
        assert!(
            session
                .agent_edit_notes(&revision, vec![valid, invalid])
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }

    fn clip_at_bar_three(session: &mut Session) -> ClipId {
        let track = session.add_default_instrument_track("Lead").unwrap();
        let start = session.project().signatures.bar_start(3);
        session
            .add_midi_clip(track, "Verse", start, Ticks::QUARTER * 8)
            .unwrap()
    }

    fn edit(session: &mut Session, json: serde_json::Value) -> Result<String, String> {
        let edit: NoteEdit = serde_json::from_value(json).unwrap();
        let revision = session.agent_document_revision().unwrap();
        session.agent_edit_notes(&revision, vec![edit])
    }

    #[test]
    fn song_bars_and_fractional_beats_become_exact_clip_relative_ticks() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let clip = clip_at_bar_three(&mut session);
        edit(
            &mut session,
            serde_json::json!({"kind":"replace","clip":clip.0,"notes":[
                {"pitch":60,"bar":3,"beat":1,"length_beats":1.5,"velocity":0.8},
                {"pitch":62,"bar":3,"beat":2.5,"length_beats":0.5,"velocity":0.8},
                {"pitch":64,"bar":3,"beat":3.333,"length_beats":0.333,"velocity":0.8},
                {"pitch":65,"bar":3,"beat":3.67,"length_beats":0.3333,"velocity":0.8},
                {"pitch":67,"bar":4,"beat":4.75,"length_beats":0.25,"velocity":0.8}
            ]}),
        )
        .unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        let ticks: Vec<_> = notes
            .iter()
            .map(|n| (n.start.raw(), n.length.raw()))
            .collect();
        assert_eq!(
            ticks,
            [
                (0, 1440),
                (1440, 480),
                (2240, 320),
                (2560, 320),
                (3840 + 2880 + 720, 240)
            ]
        );
    }

    #[test]
    fn compound_meter_counts_the_written_eighth() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        session.set_signature_at(Ticks::ZERO, TimeSignature::new(6, 8));
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Jig", Ticks::ZERO, Ticks::QUARTER * 6)
            .unwrap();
        edit(
            &mut session,
            serde_json::json!({"kind":"patch","clip":clip.0,"add":[
                {"pitch":60,"bar":1,"beat":4,"length_beats":3,"velocity":0.8},
                {"pitch":62,"bar":2,"beat":1.5,"length_beats":0.5,"velocity":0.8}
            ]}),
        )
        .unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!((notes[0].start.raw(), notes[0].length.raw()), (1440, 1440));
        assert_eq!(
            (notes[1].start.raw(), notes[1].length.raw()),
            (2880 + 240, 240)
        );
    }

    #[test]
    fn attacks_between_subdivisions_beyond_the_bar_or_outside_the_clip_are_refused() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let clip = clip_at_bar_three(&mut session);
        let before = session.project().clone();
        let add = |bar: u32, beat: f64| {
            serde_json::json!({"kind":"patch","clip":clip.0,"add":[
                {"pitch":60,"bar":bar,"beat":beat,"length_beats":0.5,"velocity":0.8}]})
        };
        let between = edit(&mut session, add(3, 2.3)).unwrap_err();
        assert!(
            between.contains("2.25") && between.contains("2.3333"),
            "{between}"
        );
        let beyond = edit(&mut session, add(3, 5.0)).unwrap_err();
        assert!(beyond.contains("next bar"), "{beyond}");
        let outside = edit(&mut session, add(1, 1.0)).unwrap_err();
        assert!(outside.contains("bars 3..4"), "{outside}");
        assert!(edit(&mut session, add(0, 1.0)).is_err());
        let half = serde_json::json!({"kind":"patch","clip":clip.0,"update":[{"index":0,"bar":3}]});
        assert!(edit(&mut session, half).is_err());
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn read_back_positions_round_trip_through_a_replacement() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let clip = clip_at_bar_three(&mut session);
        session
            .replace_notes(
                clip,
                vec![
                    Note::new(60, Ticks(320), Ticks(320)),
                    Note::new(62, Ticks(3840 + 1440), Ticks(97)),
                    Note::new(64, Ticks(13), Ticks(240)),
                ],
            )
            .unwrap();
        let read: serde_json::Value = serde_json::from_str(
            &session
                .agent_read_notes(&[NoteRead {
                    clip: clip.0,
                    offset: 0,
                }])
                .unwrap(),
        )
        .unwrap();
        let page = &read["clips"][0];
        assert_eq!(page["bars"], serde_json::json!([3, 4]));
        let notes = page["notes"].as_array().unwrap();
        assert_eq!(
            (notes[0]["bar"].clone(), notes[0]["beat"].clone()),
            (3.into(), 1.3333.into())
        );
        assert_eq!(
            (notes[1]["bar"].clone(), notes[1]["beat"].clone()),
            (4.into(), 2.5.into())
        );
        assert!(notes[0].get("on_grid").is_none());
        assert_eq!(notes[2]["on_grid"], false);
        let on_grid: Vec<_> = notes[..2]
            .iter()
            .map(|n| {
                serde_json::json!({"pitch":n["pitch"],"bar":n["bar"],"beat":n["beat"],
                "length_beats":n["length_beats"],"velocity":n["velocity"]})
            })
            .collect();
        edit(
            &mut session,
            serde_json::json!({"kind":"replace","clip":clip.0,"notes":on_grid}),
        )
        .unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!(
            notes
                .iter()
                .map(|n| (n.start.raw(), n.length.raw()))
                .collect::<Vec<_>>(),
            [(320, 320), (3840 + 1440, 97)]
        );
    }
}
