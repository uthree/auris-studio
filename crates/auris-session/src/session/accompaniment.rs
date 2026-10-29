//! Explicit backing generation, preserving foreground and every unselected clip.

use super::{Session, generated::unarranged_project_phrase, lyrics::arrange_backing_around_notes};
use crate::Edit;
use crate::live_agent::recipes::{RecipePatch, summary};
use crate::prelude::{Chart, MusicalKey};
use auris_core::time::Ticks;
use auris_core::{
    ClipId, ClipPreset, ClipRecipe, MidiClip, Note, PluginRegistry, Project, TrackId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A backing writer; lead writing remains an explicit, separate operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// Rhythmic chord accompaniment.
    Chords,
    /// Sustained chord bed.
    Pad,
    /// Broken chords.
    Arp,
    /// Bass line.
    Bass,
    /// Assigned drum kit.
    Drums,
    /// Assigned kick.
    Kick,
    /// Assigned snare.
    Snare,
    /// Assigned hi-hat.
    Hat,
}

impl Part {
    fn preset(self) -> ClipPreset {
        match self {
            Self::Chords => ClipPreset::Chords,
            Self::Pad => ClipPreset::Pad,
            Self::Arp => ClipPreset::Arp,
            Self::Bass => ClipPreset::Bass,
            Self::Drums => ClipPreset::Drums,
            Self::Kick => ClipPreset::Kick,
            Self::Snare => ClipPreset::Snare,
            Self::Hat => ClipPreset::Hat,
        }
    }
}

/// One new backing clip on an existing, explicitly chosen track.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    /// Existing instrument or assigned drum track ID, separate from protected tracks.
    pub track: u64,
    /// Clip label, 1..256 UTF-8 bytes without control characters.
    pub name: String,
    /// Musical role, independent of the track's name.
    pub part: Part,
    /// One-based first bar.
    pub start_bar: u32,
    /// Number of bars, 1..64, also limited to 256 quarter notes.
    pub bars: u32,
    /// Reproducible take within the current composer build.
    pub seed: u64,
    /// Optional recipe controls; seed here must be omitted (use the top-level seed).
    #[serde(default)]
    pub settings: RecipePatch,
}

/// A bounded harmony replacement, applied together with explicit regeneration targets.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarmonyChange {
    /// One-based first bar.
    pub start_bar: u32,
    /// Length in bars, 1..64; selected clips must fit entirely inside this range.
    pub bars: u32,
    /// Optional key, for example C major; the previous key resumes at the end.
    pub key: Option<String>,
    /// Roman-numeral chart, repeated to fill the range. Extended chords are supported.
    pub chords: String,
}

/// Generate backing or regenerate only named clips, with optional harmony replacement.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Add 1..16 backing clips without replacing any existing clip.
    Generate {
        /// Explicit tracks, ranges, roles and recipe controls.
        #[schemars(length(min = 1, max = 16))]
        parts: Vec<Generation>,
    },
    /// Rewrite selected generated backing only, retaining IDs, seeds and performance.
    Regenerate {
        /// 1..16 distinct generated backing clip IDs; hand-edited or frozen clips are refused.
        #[schemars(length(min = 1, max = 16))]
        clips: Vec<u64>,
        /// Optional harmony change in the same Undo step. Other clips are not regenerated.
        harmony: Option<HarmonyChange>,
    },
}

/// An owned snapshot that can generate backing off the document thread.
pub struct Job {
    owner: Arc<PluginRegistry>,
    owner_revision: u64,
    revision: String,
    folder: Option<PathBuf>,
    project: Project,
    protected: Vec<u64>,
    foreground: Vec<u64>,
    operation: Operation,
}

/// A fully generated candidate, accepted only by its unchanged originating session.
pub struct Prepared {
    job: Job,
    clips: Vec<Value>,
    harmony_changed: bool,
}

impl Prepared {
    pub(crate) fn into_candidate_project(self) -> Project {
        self.job.project
    }
}

fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Accompaniment cancelled".into())
    } else {
        Ok(())
    }
}

fn ids(values: &[u64], label: &str) -> Result<(), String> {
    if values.is_empty()
        || values.len() > 16
        || values.iter().collect::<HashSet<_>>().len() != values.len()
    {
        return Err(format!("{label} requires 1..16 distinct clip IDs"));
    }
    Ok(())
}

/// Reads score windows and repetitions, leaving performance and source clips untouched.
fn foreground_notes(
    project: &Project,
    clips: &[u64],
    start: Ticks,
    end: Ticks,
) -> Result<Vec<Note>, String> {
    let mut notes = Vec::new();
    let mut stored = 0usize;
    for id in clips {
        let (track, clip) = project
            .midi_clip(ClipId(*id))
            .ok_or("Unknown foreground clip")?;
        if clip.muted
            || project.track(track).is_none_or(|track| track.mixer.mute)
            || clip.start >= end
            || clip.sounding_end() <= start
        {
            continue;
        }
        stored += clip.notes.len();
        if stored > 4096 {
            return Err(
                "Foreground sources are limited to 4096 stored notes; split long clips first"
                    .into(),
            );
        }
        clip.looped_note_instances()
            .map_err(|error| error.to_string())?;
        let playable: Vec<_> = clip
            .playable_notes()
            .filter(|note| note.velocity > 0.0)
            .collect();
        for (offset, span) in auris_core::project::loop_passes(clip.length, clip.loop_end) {
            let at = clip.start + offset;
            if at >= end {
                break;
            }
            if at + span <= start {
                continue;
            }
            for note in &playable {
                let onset = (at + note.start).max(start);
                let release = (at + note.end().min(span)).min(end);
                if release <= onset {
                    continue;
                }
                if notes.len() == 4096 {
                    return Err(
                        "Expanded foreground exceeds 4096 notes; shorten the request".into(),
                    );
                }
                notes.push(Note {
                    start: onset - start,
                    length: release - onset,
                    ..note.clone()
                });
            }
        }
    }
    notes.sort_by_key(|note| (note.start, note.pitch, note.length));
    notes.dedup_by_key(|note| (note.start, note.pitch, note.length));
    Ok(notes)
}

fn write_backing(
    project: &Project,
    foreground: &[u64],
    start: Ticks,
    length: Ticks,
    recipe: &ClipRecipe,
) -> Result<(Vec<Note>, Value), String> {
    let reference = foreground_notes(project, foreground, start, start + length)?;
    let mut notes = unarranged_project_phrase(project, start, length, recipe);
    let before = notes.clone();
    let status =
        arrange_backing_around_notes(project, start, length, recipe, &reference, &mut notes);
    let report = json!({"status":status,"changed":notes != before,
        "foreground_note_count":reference.len(),"notes_before":before.len(),"notes_after":notes.len(),
        "basis":"Selected written notes, clip windows and loops; muted sources omitted. Performance transforms, solo routing, bends and sound are not evaluated."});
    Ok((notes, report))
}

fn range(project: &Project, start_bar: u32, bars: u32) -> Result<(Ticks, Ticks), String> {
    if start_bar == 0 || !(1..=64).contains(&bars) {
        return Err("start_bar must be positive and bars within 1..64".into());
    }
    let after = start_bar.checked_add(bars).ok_or("Bar range overflow")?;
    let start = project.signatures.bar_start(start_bar);
    let end = project.signatures.bar_start(after);
    if end <= start || end - start > Ticks::QUARTER * 256 {
        return Err("Each range must contain at most 256 quarter notes".into());
    }
    Ok((start, end))
}

fn bounded_clip(
    project: &Project,
    start: Ticks,
    end: Ticks,
    total: &mut i64,
) -> Result<(), String> {
    if start < Ticks::ZERO || end <= start || end - start > Ticks::QUARTER * 256 {
        return Err("Each target must have positive duration up to 256 quarter notes".into());
    }
    *total += (end - start).raw();
    if *total > (Ticks::QUARTER * 1024).raw() {
        return Err("One operation is limited to 1024 quarter notes across its targets".into());
    }
    if project
        .signatures
        .points()
        .iter()
        .any(|p| start < p.tick && p.tick < end)
        || project
            .sections
            .points()
            .iter()
            .any(|p| start < p.tick && p.tick < end)
    {
        return Err(
            "Split targets at meter and section boundaries before generating accompaniment".into(),
        );
    }
    Ok(())
}

fn replace_harmony(
    project: &mut Project,
    change: &HarmonyChange,
) -> Result<(Ticks, Ticks), String> {
    let (start, end) = range(project, change.start_bar, change.bars)?;
    if change.chords.len() > 4096 || change.key.as_ref().is_some_and(|key| key.len() > 128) {
        return Err("Chord chart or key exceeds the input limit".into());
    }
    let key = change
        .key
        .as_deref()
        .map(|text| MusicalKey::parse(text).ok_or("Invalid key"))
        .transpose()?;
    let mut chart = Chart::parse(&change.chords)
        .filter(|chart| !chart.bars.is_empty())
        .ok_or("Invalid Roman-numeral chord chart")?
        .spelled_in(key.unwrap_or_else(|| project.harmony.key_at(start)));
    if let Some(key) = key {
        let previous = project.harmony.key_at(end);
        project.harmony.keys.set_point(start, key);
        project.harmony.keys.set_point(end, previous);
    }
    project.harmony.chords.clear_range(start, end);
    let progression = std::mem::take(&mut chart.bars);
    for offset in 0..change.bars {
        let at = project.signatures.bar_start(change.start_bar + offset);
        chart.bars = vec![progression[offset as usize % progression.len()].clone()];
        project
            .harmony
            .stamp(&chart, at, 1, project.signatures.signature_at(at));
    }
    Ok((start, end))
}

impl Session {
    pub(crate) fn begin_accompaniment(
        &self,
        revision: &str,
        protected: &[u64],
        foreground: &[u64],
        operation: Operation,
    ) -> Result<Job, String> {
        if self.transaction.is_some() || self.agent_document_revision()? != revision {
            return Err("Document changed or an edit is in progress; inspect_project again".into());
        }
        ids(protected, "protected_clips")?;
        ids(foreground, "foreground_clips")?;
        if foreground.iter().any(|id| !protected.contains(id)) {
            return Err("foreground_clips must be a subset of protected_clips".into());
        }
        // Protected clips need only exist: they reserve their tracks and remain
        // untouched, but clips outside the explicit foreground need not contain
        // pitched notes (for example an empty pad or a drum clip). Only the
        // foreground is read to adapt generated backing around written notes.
        for id in protected {
            self.project
                .midi_clip(ClipId(*id))
                .ok_or("Unknown protected clip")?;
        }
        for id in foreground {
            let (track, clip) = self
                .project
                .midi_clip(ClipId(*id))
                .ok_or("Unknown foreground clip")?;
            if clip.notes.is_empty()
                || self
                    .project
                    .track(track)
                    .is_none_or(|track| track.kind.is_drum())
            {
                return Err("Foreground clips must contain pitched notes".into());
            }
        }
        // Bound the snapshot before cloning it. The content hash already serializes this document.
        if serde_json::to_vec(&self.project)
            .map_err(|e| e.to_string())?
            .len()
            > 64 * 1024 * 1024
        {
            return Err("Project exceeds the 64 MiB accompaniment snapshot limit".into());
        }
        Ok(Job {
            owner: Arc::clone(self.registry()),
            owner_revision: self.revision(),
            revision: revision.into(),
            folder: self.project_folder().map(ToOwned::to_owned),
            project: self.project.clone(),
            protected: protected.to_vec(),
            foreground: foreground.to_vec(),
            operation,
        })
    }

    pub(crate) fn continue_accompaniment(
        &mut self,
        result: Prepared,
        cancelled: &AtomicBool,
    ) -> Result<String, String> {
        check(cancelled)?;
        let Prepared {
            job,
            clips,
            harmony_changed,
        } = result;
        if self.transaction.is_some()
            || self.revision() != job.owner_revision
            || self.agent_document_revision()? != job.revision
            || !Arc::ptr_eq(self.registry(), &job.owner)
            || self.project_folder() != job.folder.as_deref()
        {
            return Err(
                "Document changed during accompaniment generation; inspect_project and retry"
                    .into(),
            );
        }
        let changed = self.project != job.project;
        check(cancelled)?;
        if changed {
            self.begin_transaction(Edit::ExternalChanges);
            self.record(Edit::ExternalChanges);
            self.project = job.project;
            self.invalidate_graph();
            self.end_transaction();
        }
        Ok(
            json!({"changed": changed, "revision": self.agent_document_revision()?,
            "protected_clips": job.protected, "foreground_clips": job.foreground, "clips": clips, "harmony_changed": harmony_changed,
            "unselected_clips_preserved": true})
            .to_string(),
        )
    }
}

impl Job {
    pub(crate) fn run(mut self, cancelled: &AtomicBool) -> Result<Prepared, String> {
        check(cancelled)?;
        let protected_tracks: HashSet<_> = self
            .protected
            .iter()
            .filter_map(|id| self.project.track_of_clip(ClipId(*id)))
            .collect();
        let mut candidates = Vec::new();
        let mut adaptations = Vec::new();
        let mut total = 0;
        let mut harmony_changed = false;
        match &self.operation {
            Operation::Generate { parts } => {
                if parts.is_empty() || parts.len() > 16 {
                    return Err("Generate requires 1..16 parts".into());
                }
                for part in parts {
                    check(cancelled)?;
                    let track = self
                        .project
                        .track(TrackId(part.track))
                        .ok_or("Unknown accompaniment track")?;
                    let preset = part.part.preset();
                    let instrument = track
                        .kind
                        .as_instrument()
                        .ok_or("Backing requires an instrument or drum track")?;
                    if protected_tracks.contains(&track.id)
                        || track.kind.is_drum() != preset.is_drums()
                    {
                        return Err("Choose a matching accompaniment track separate from protected melody tracks".into());
                    }
                    if part.name.trim().is_empty()
                        || part.name.len() > 256
                        || part.name.chars().any(char::is_control)
                    {
                        return Err(
                            "Clip name must contain 1..256 UTF-8 bytes without control characters"
                                .into(),
                        );
                    }
                    let (start, end) = range(&self.project, part.start_bar, part.bars)?;
                    bounded_clip(&self.project, start, end, &mut total)?;
                    if instrument
                        .clips
                        .iter()
                        .any(|clip| start < clip.sounding_end() && clip.start < end)
                        || candidates.iter().any(|(id, clip): &(TrackId, MidiClip)| {
                            *id == track.id && start < clip.end() && clip.start < end
                        })
                    {
                        return Err("Accompaniment range overlaps an existing or requested clip; choose a free range or regenerate its clip".into());
                    }
                    let mut recipe = ClipRecipe::new(preset, part.seed);
                    if part.settings.seed.is_some() {
                        return Err("Use the generation part's seed, not settings.seed".into());
                    }
                    part.settings.apply(&mut recipe)?;
                    if let Some(map) =
                        auris_core::project::DrumMap::load(&instrument.instrument_state)
                    {
                        auris_compose::apply_drum_map(&mut recipe, &map);
                    } else if preset.is_drums() {
                        return Err(
                            "Assign the track's drum sounds before generating accompaniment".into(),
                        );
                    }
                    let mut clip = MidiClip::new(ClipId(0), &part.name, start, end - start);
                    let (notes, adaptation) = write_backing(
                        &self.project,
                        &self.foreground,
                        start,
                        end - start,
                        &recipe,
                    )?;
                    clip.notes = notes;
                    adaptations.push(adaptation);
                    clip.transforms = auris_compose::clip_performance(preset, recipe.seed);
                    recipe.text_digest = auris_core::notes_digest(&clip.notes);
                    clip.recipe = Some(recipe);
                    candidates.push((track.id, clip));
                }
            }
            Operation::Regenerate { clips, harmony } => {
                ids(clips, "Regenerate")?;
                let changed_range = if let Some(change) = harmony {
                    let before = self.project.harmony.clone();
                    let range = replace_harmony(&mut self.project, change)?;
                    harmony_changed = before != self.project.harmony;
                    Some(range)
                } else {
                    None
                };
                for id in clips {
                    check(cancelled)?;
                    let (track, original) = self
                        .project
                        .midi_clip(ClipId(*id))
                        .ok_or("Unknown accompaniment clip")?;
                    let mut clip = original.clone();
                    let mut recipe = clip
                        .recipe
                        .clone()
                        .ok_or("Regeneration requires a generated clip with its recipe")?;
                    if self.protected.contains(id)
                        || recipe.preset == ClipPreset::Lead
                        || self
                            .project
                            .track(track)
                            .is_none_or(|track| track.kind.as_instrument().is_none())
                    {
                        return Err("Regeneration only accepts unprotected backing clips".into());
                    }
                    if recipe.text_digest != 0
                        && recipe.text_digest != auris_core::notes_digest(&clip.notes)
                    {
                        return Err("Backing clip contains hand edits; preserve or explicitly edit its recipe separately".into());
                    }
                    bounded_clip(&self.project, clip.start, clip.sounding_end(), &mut total)?;
                    if changed_range
                        .is_some_and(|(start, end)| clip.start < start || clip.sounding_end() > end)
                    {
                        return Err("Regeneration targets must fit inside the harmony change; split spanning clips first".into());
                    }
                    let (notes, adaptation) = write_backing(
                        &self.project,
                        &self.foreground,
                        clip.start,
                        clip.length,
                        &recipe,
                    )?;
                    clip.notes = notes;
                    adaptations.push(adaptation);
                    recipe.text_digest = auris_core::notes_digest(&clip.notes);
                    clip.recipe = Some(recipe);
                    candidates.push((track, clip));
                }
            }
        }
        let mut count = 0usize;
        let mut reports = Vec::new();
        for ((track, mut clip), adaptation) in candidates.into_iter().zip(adaptations) {
            check(cancelled)?;
            count += clip.notes.len();
            if clip.notes.is_empty() || count > 65_536 {
                return Err("Backing generation must produce notes and is limited to 65536 notes; check harmony, drum assignments and recipe controls".into());
            }
            clip.looped_note_instances().map_err(|e| e.to_string())?;
            if matches!(self.operation, Operation::Generate { .. }) {
                clip.id = self
                    .project
                    .add_midi_clip(track, clip.name.clone(), clip.start, clip.length)
                    .ok_or("Unknown track")?;
            }
            reports.push(json!({"clip": clip.id.0, "track": track.0, "name": clip.name,
                "start_tick": clip.start.raw(), "length_ticks": clip.length.raw(), "note_count": clip.notes.len(),
                "recipe": summary(clip.recipe.as_ref().expect("generated recipe")), "adaptation":adaptation}));
            let id = clip.id;
            *self
                .project
                .midi_clip_mut(id)
                .ok_or("Missing generated clip")? = clip;
        }
        check(cancelled)?;
        Ok(Prepared {
            job: self,
            clips: reports,
            harmony_changed,
        })
    }
}

#[cfg(test)]
mod tests;
