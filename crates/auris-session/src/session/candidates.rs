//! Exact, bounded candidate rounds over immutable document snapshots.

use super::{Session, accompaniment};
use crate::{
    Edit, RenderJob,
    live_agent::{file_jobs::BarRange, note_editing::NoteEdit},
};
use auris_core::{ClipId, PluginRegistry, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(15 * 60);
const ROUND_BYTES: usize = 64 * 1024 * 1024;
const CACHE_BYTES: usize = 256 * 1024 * 1024;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

/// Read, audition or adopt an exact candidate without regenerating it.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Prepare a baseline and one to three alternatives; at most three rounds per revision.
    Create {
        /// Current content revision from inspect_project/read_notes.
        expected_revision: String,
        /// Independent alternatives, all starting from the same original document.
        candidates: Vec<Proposal>,
    },
    /// Read summaries and one optional page of exact baseline/candidate notes.
    Read {
        /// Opaque round ID returned by create.
        round_id: String,
        /// Zero is the baseline; 1..3 are alternatives.
        candidate: usize,
        /// Optional MIDI clip ID for a bounded note page and index-aligned difference.
        clip: Option<u64>,
        /// First storage index; each page returns at most 128 notes.
        #[serde(default)]
        offset: usize,
    },
    /// Render baseline and all alternatives with one common protective gain, without editing.
    Preview {
        /// Opaque round ID.
        round_id: String,
        /// Same excerpt for every candidate, at most thirty seconds.
        range: BarRange,
    },
    /// Apply exactly the evaluated project once, as a single Undo edit.
    Apply {
        /// Opaque round ID.
        round_id: String,
        /// 1..3; baseline is retained by cancelling the round.
        candidate: usize,
        /// Original revision bound to this round, including identical retries.
        expected_revision: String,
    },
    /// Discard a pending round; later application and preview publication are refused.
    Cancel {
        /// Opaque round ID.
        round_id: String,
    },
}

/// One alternative with its musical purpose, independent of the other alternatives.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// Short musical rationale, 1..256 UTF-8 bytes.
    pub purpose: String,
    /// Exact note edit or explicitly scoped accompaniment generation.
    pub edit: Action,
}

/// The initial candidate surface is deliberately limited to notes and accompaniment.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Exact phrase edits with enforced motif and range protection.
    Variation {
        /// Pitch or cadence proposal with explicit protected notes.
        request: crate::live_agent::phrase_variation::Request,
    },
    /// The same exact edits accepted by edit_notes, on a private snapshot.
    Notes {
        /// One to sixteen clip patches/replacements; normal note validation applies.
        edits: Vec<NoteEdit>,
    },
    /// Existing backing writers with explicit protected and foreground clip IDs.
    Accompaniment {
        /// Clips whose complete contents must be retained.
        protected_clips: Vec<u64>,
        /// Foreground subset used for adaptation.
        foreground_clips: Vec<u64>,
        /// Bounded generation/regeneration request.
        operation: accompaniment::Operation,
    },
}

#[derive(Default)]
struct Cache {
    rounds: VecDeque<Round>,
    attempts: VecDeque<(String, String, Instant, usize)>,
}

struct Round {
    id: String,
    scope: String,
    revision: String,
    owner_revision: u64,
    instance: String,
    created: Instant,
    projects: Vec<Project>,
    purposes: Vec<String>,
    bytes: usize,
    cancelled: bool,
    applied: Option<(usize, String, Value)>,
    previews: Vec<tempfile::TempDir>,
    preview_count: usize,
}

enum Pending {
    Notes(Vec<NoteEdit>),
    Accompaniment(Box<accompaniment::Job>),
}

/// Detached candidate construction or multi-variant rendering.
pub enum Job {
    /// Private score preparation; no files or live edits.
    Prepare(Box<Preparation>),
    /// Exact score excerpts through the original session's instruments.
    Preview(Box<Preview>),
}

/// Owned score inputs and document identity for detached preparation.
pub struct Preparation {
    owner: Arc<PluginRegistry>,
    round: Round,
    pending: Vec<Pending>,
}

/// Independent render jobs with identical ranges and one common gain.
pub struct Preview {
    owner: Arc<PluginRegistry>,
    owner_revision: u64,
    scope: String,
    round_id: String,
    jobs: Vec<RenderJob>,
    range: BarRange,
}

/// Worker result checked again before retaining candidates or exposing audio paths.
pub enum Prepared {
    /// Completed immutable score alternatives.
    Round(Box<Preparation>),
    /// Audio files still privately owned by the result until publication.
    Preview(Box<PreviewResult>),
}

/// Retained preview files and their measured shared attenuation.
pub struct PreviewResult {
    owner: Arc<PluginRegistry>,
    owner_revision: u64,
    scope: String,
    round_id: String,
    folder: tempfile::TempDir,
    report: Value,
}

fn cache() -> Result<std::sync::MutexGuard<'static, Cache>, String> {
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Candidate cache lock failed")?;
    cache.rounds.retain(|r| r.created.elapsed() < TTL);
    cache
        .attempts
        .retain(|(_, _, created, _)| created.elapsed() < TTL);
    Ok(cache)
}

fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Candidate operation cancelled".into())
    } else {
        Ok(())
    }
}

impl Session {
    fn candidate_scope(&self) -> String {
        self.path()
            .map(|p| {
                std::fs::canonicalize(p)
                    .unwrap_or_else(|_| p.to_owned())
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| format!("unsaved:{}", self.document_instance()))
    }

    fn check_candidate_round(&self, round: &Round) -> Result<(), String> {
        if round.cancelled
            || round.created.elapsed() >= TTL
            || round.scope != self.candidate_scope()
            || self.agent_document_revision()? != round.revision
            || (self.document_instance() == round.instance
                && self.revision() != round.owner_revision)
            || self.transaction.is_some()
            || round.applied.is_some()
        {
            return Err("Candidate round is stale, cancelled, applied or belongs to another document; create a fresh round".into());
        }
        Ok(())
    }

    pub(crate) fn begin_candidates(&mut self, operation: Operation) -> Result<Job, String> {
        match operation {
            Operation::Create {
                expected_revision,
                candidates,
            } => {
                if candidates.is_empty()
                    || candidates.len() > 3
                    || self.transaction.is_some()
                    || self.agent_document_revision()? != expected_revision
                {
                    return Err(
                        "Supply 1..3 candidates with the current revision and no open gesture"
                            .into(),
                    );
                }
                let bytes = serde_json::to_vec(&self.project)
                    .map_err(|e| e.to_string())?
                    .len();
                if bytes.saturating_mul(candidates.len() + 1) > ROUND_BYTES {
                    return Err("Candidate round exceeds 64 MiB".into());
                }
                if serde_json::to_vec(&candidates)
                    .map_err(|e| e.to_string())?
                    .len()
                    > 256 * 1024
                {
                    return Err("Candidate arguments exceed 256 KiB".into());
                }
                let mut pending = vec![];
                let mut purposes = vec!["Original document".into()];
                for candidate in candidates {
                    if candidate.purpose.trim().is_empty() || candidate.purpose.len() > 256 {
                        return Err("Each candidate needs a purpose up to 256 bytes".into());
                    }
                    purposes.push(candidate.purpose);
                    pending.push(match candidate.edit {
                        Action::Variation { request } => {
                            Pending::Notes(vec![crate::live_agent::phrase_variation::proposal(
                                &self.project,
                                request,
                            )?])
                        }
                        Action::Notes { edits } => Pending::Notes(edits),
                        Action::Accompaniment {
                            protected_clips,
                            foreground_clips,
                            operation,
                        } => Pending::Accompaniment(Box::new(self.begin_accompaniment(
                            &expected_revision,
                            &protected_clips,
                            &foreground_clips,
                            operation,
                        )?)),
                    });
                }
                let scope = self.candidate_scope();
                let mut cache = cache()?;
                if let Some((_, _, _, count)) = cache
                    .attempts
                    .iter_mut()
                    .find(|(s, r, _, _)| *s == scope && *r == expected_revision)
                {
                    if *count >= 3 {
                        return Err("Three candidate rounds exhausted for this revision; stop and report unresolved choices".into());
                    }
                    *count += 1;
                } else {
                    if cache.attempts.len() >= 64 {
                        return Err("Candidate attempt cache is full; wait for expiry".into());
                    }
                    cache.attempts.push_back((
                        scope.clone(),
                        expected_revision.clone(),
                        Instant::now(),
                        1,
                    ));
                }
                Ok(Job::Prepare(Box::new(Preparation {
                    owner: Arc::clone(self.registry()),
                    round: Round {
                        id: crate::transient_id::transient_id("candidates"),
                        scope,
                        revision: expected_revision,
                        owner_revision: self.revision(),
                        instance: self.document_instance().into(),
                        created: Instant::now(),
                        projects: vec![self.project.clone()],
                        purposes,
                        bytes,
                        cancelled: false,
                        applied: None,
                        previews: vec![],
                        preview_count: 0,
                    },
                    pending,
                })))
            }
            Operation::Preview { round_id, range } => {
                let mut cache = cache()?;
                let round = cache
                    .rounds
                    .iter_mut()
                    .find(|r| r.id == round_id)
                    .ok_or("Unknown or expired candidate round")?;
                self.check_candidate_round(round)?;
                if round.preview_count >= 3 {
                    return Err("Three preview ranges exhausted for this round".into());
                }
                for project in &round.projects {
                    crate::audio_inspection::bar_range(project, range.start_bar, range.bars)?;
                }
                round.preview_count += 1;
                let projects = round.projects.clone();
                drop(cache);
                Ok(Job::Preview(Box::new(Preview {
                    owner: Arc::clone(self.registry()),
                    owner_revision: self.revision(),
                    scope: self.candidate_scope(),
                    round_id,
                    jobs: projects
                        .into_iter()
                        .map(|p| self.job_for_at(p, Some(24_000.0)))
                        .collect(),
                    range,
                })))
            }
            _ => Err("Expected candidate create or preview".into()),
        }
    }

    pub(crate) fn agent_candidates(&mut self, operation: Operation) -> Result<String, String> {
        let mut cache = cache()?;
        let id = match &operation {
            Operation::Read { round_id, .. }
            | Operation::Apply { round_id, .. }
            | Operation::Cancel { round_id } => round_id,
            _ => return Err("Candidate creation/preview requires the worker protocol".into()),
        };
        let round = cache
            .rounds
            .iter_mut()
            .find(|r| r.id == *id)
            .ok_or("Unknown or expired candidate round")?;
        if round.scope != self.candidate_scope() {
            return Err("Candidate round belongs to another document".into());
        }
        if let Operation::Cancel { .. } = &operation {
            round.cancelled = true;
            round.projects.clear();
            round.previews.clear();
            round.bytes = 0;
            return Ok(
                json!({"round_id":id,"cancelled":true,"document_unchanged":true}).to_string(),
            );
        }
        if let Operation::Apply {
            candidate,
            expected_revision,
            ..
        } = &operation
            && let Some((chosen, after, response)) = &round.applied
        {
            if !round.cancelled
                && candidate == chosen
                && *expected_revision == round.revision
                && *after == self.agent_document_revision()?
            {
                let mut replay = response.clone();
                replay["replayed"] = true.into();
                return Ok(replay.to_string());
            }
            return Err("Round already adopted; use Undo/Redo or prepare a new round".into());
        }
        self.check_candidate_round(round)?;
        match operation {
            Operation::Read {
                candidate,
                clip,
                offset,
                ..
            } => {
                let project = round
                    .projects
                    .get(candidate)
                    .ok_or("Unknown candidate index")?;
                let mut report = round.summary();
                if let Some(clip) = clip {
                    let (_, selected) = project
                        .midi_clip(ClipId(clip))
                        .ok_or("Unknown candidate MIDI clip")?;
                    let baseline = round.projects[0].midi_clip(ClipId(clip)).map(|(_, c)| c);
                    let total = selected
                        .notes
                        .len()
                        .max(baseline.map_or(0, |c| c.notes.len()));
                    if offset > total {
                        return Err("Offset exceeds candidate/baseline note count".into());
                    }
                    report["notes"] = json!((offset..total).take(128).map(|index|json!({"index":index,"after":selected.notes.get(index),"before":baseline.and_then(|c|c.notes.get(index))})).collect::<Vec<_>>());
                    report["note_units"] = "start/length are exact clip-relative ticks; before/after are aligned by storage index, not inferred musical identity".into();
                    report["total"] = total.into();
                    report["next_offset"] = json!((offset + 128 < total).then_some(offset + 128));
                    report["clip"] = clip.into();
                    report["candidate"] = candidate.into();
                    report["recipe_changed"] =
                        json!(baseline.map(|c| &c.recipe) != Some(&selected.recipe));
                } else {
                    let before = &round.projects[0].harmony;
                    let after = &project.harmony;
                    let total = before
                        .keys
                        .points()
                        .len()
                        .max(after.keys.points().len())
                        .max(before.chords.points().len())
                        .max(after.chords.points().len());
                    if offset > total {
                        return Err("Offset exceeds harmony point count".into());
                    }
                    report["harmony"] = json!({
                        "keys":(offset..total).take(128).map(|index|json!({"index":index,"before":before.keys.points().get(index),"after":after.keys.points().get(index)})).collect::<Vec<_>>(),
                        "chords":(offset..total).take(128).map(|index|json!({"index":index,"before":before.chords.points().get(index),"after":after.chords.points().get(index)})).collect::<Vec<_>>()});
                    report["next_offset"] = json!((offset + 128 < total).then_some(offset + 128));
                    report["candidate"] = candidate.into();
                }
                Ok(report.to_string())
            }
            Operation::Apply {
                candidate,
                expected_revision,
                ..
            } => {
                if expected_revision != round.revision || candidate == 0 {
                    return Err("Use the original revision and an alternative index; cancel to retain the baseline".into());
                }
                let project = round
                    .projects
                    .get(candidate)
                    .ok_or("Unknown candidate index")?;
                let changed = self.project != *project;
                if changed {
                    self.begin_transaction(Edit::ExternalChanges);
                    self.record(Edit::ExternalChanges);
                    self.project = project.clone();
                    self.invalidate_graph();
                    self.end_transaction();
                }
                let after = self.agent_document_revision()?;
                let response = json!({"round_id":round.id,"candidate":candidate,"applied":true,"changed":changed,"revision":after,"replayed":false,"exact_candidate":true});
                round.applied = Some((candidate, after, response.clone()));
                Ok(response.to_string())
            }
            _ => Err("Expected candidate read, apply or cancel".into()),
        }
    }

    pub(crate) fn continue_candidates(
        &mut self,
        result: Prepared,
        cancelled: &AtomicBool,
    ) -> Result<String, String> {
        check(cancelled)?;
        match result {
            Prepared::Round(preparation) => {
                if !Arc::ptr_eq(self.registry(), &preparation.owner)
                    || self.revision() != preparation.round.owner_revision
                {
                    return Err("Candidate owner or revision changed".into());
                }
                let round = preparation.round;
                self.check_candidate_round(&round)?;
                let response = round.summary().to_string();
                let mut cache = cache()?;
                while cache.rounds.len() >= 8
                    || cache.rounds.iter().map(|r| r.bytes).sum::<usize>() + round.bytes
                        > CACHE_BYTES
                {
                    cache.rounds.pop_front();
                }
                cache.rounds.push_back(round);
                Ok(response)
            }
            Prepared::Preview(result) => {
                if result.scope != self.candidate_scope()
                    || result.owner_revision != self.revision()
                    || !Arc::ptr_eq(self.registry(), &result.owner)
                {
                    return Err("Document changed during candidate rendering".into());
                }
                let mut cache = cache()?;
                let round = cache
                    .rounds
                    .iter_mut()
                    .find(|r| r.id == result.round_id)
                    .ok_or("Candidate round expired during preview")?;
                self.check_candidate_round(round)?;
                let report = result.report.to_string();
                round.previews.push(result.folder);
                Ok(report)
            }
        }
    }
}

impl Round {
    fn summary(&self) -> Value {
        let baseline = &self.projects[0];
        json!({"round_id":self.id,"revision":self.revision,"expires_after_seconds":TTL.as_secs(),"document_unchanged":true,
            "candidates":self.projects.iter().enumerate().map(|(index,p)| json!({"candidate":index,"purpose":self.purposes[index],"harmony_changed":p.harmony != baseline.harmony,
                "changed_clips":p.tracks.iter().flat_map(|t|t.kind.note_clips().into_iter().flatten()).filter(|c| baseline.midi_clip(c.id).is_none_or(|(_,b)|b != *c))
                    .map(|c|json!({"clip":c.id.0,"notes":c.notes.len(),"before_notes":baseline.midi_clip(c.id).map(|(_,c)|c.notes.len()),"start_tick":c.start.raw(),"length_ticks":c.length.raw()})).collect::<Vec<_>>() })).collect::<Vec<_>>(),
            "verification":"Read exact notes and preview the same range. Structural validity is not musical preference. Apply retains the evaluated notes; it does not regenerate."})
    }
}

impl Job {
    /// Complete detached construction/rendering with cooperative cancellation.
    pub fn run(self, cancelled: &AtomicBool) -> Result<Prepared, String> {
        check(cancelled)?;
        match self {
            Self::Prepare(mut preparation) => {
                for pending in std::mem::take(&mut preparation.pending) {
                    check(cancelled)?;
                    let project = match pending {
                        Pending::Notes(edits) => {
                            crate::live_agent::note_editing::candidate_project(
                                &preparation.round.projects[0],
                                edits,
                            )?
                        }
                        Pending::Accompaniment(job) => job.run(cancelled)?.into_candidate_project(),
                    };
                    preparation.round.bytes += serde_json::to_vec(&project)
                        .map_err(|e| e.to_string())?
                        .len();
                    if preparation.round.bytes > ROUND_BYTES {
                        return Err("Generated candidate round exceeds 64 MiB".into());
                    }
                    preparation.round.projects.push(project);
                }
                check(cancelled)?;
                Ok(Prepared::Round(preparation))
            }
            Self::Preview(mut preview) => {
                use crate::prelude::{
                    OfflineOptions, RenderProgress, WavBitDepth, WavExportSettings,
                };
                let folder = tempfile::Builder::new()
                    .prefix("auris-candidates-")
                    .tempdir()
                    .map_err(|e| e.to_string())?;
                let mut audio = vec![];
                let mut peak = 0.0f32;
                for job in &mut preview.jobs {
                    check(cancelled)?;
                    job.validate_complete().map_err(|e| e.to_string())?;
                    let (start, end) = crate::audio_inspection::bar_range(
                        job.project(),
                        preview.range.start_bar,
                        preview.range.bars,
                    )?;
                    let options = OfflineOptions {
                        sample_rate: Some(24_000.0),
                        include_tail: false,
                        ..Default::default()
                    }
                    .with_range(
                        job.project()
                            .tempo_map
                            .ticks_to_samples(start, 24_000.0)
                            .raw(),
                        job.project()
                            .tempo_map
                            .ticks_to_samples(end, 24_000.0)
                            .raw(),
                    );
                    let rendered = job
                        .render(
                            &options,
                            &mut RenderProgress::default().cancelled_by(cancelled),
                        )
                        .map_err(|e| e.to_string())?;
                    if rendered.iter_channels().flatten().any(|s| !s.is_finite()) {
                        return Err("Nonfinite candidate audio".into());
                    }
                    peak = peak.max(
                        auris_gpu::analysis::analyze_loudness_cpu(&rendered).true_peak_estimate,
                    );
                    audio.push(rendered);
                }
                let ceiling = auris_core::param::db_to_gain(-1.0) - 4.0 / 32768.0;
                let gain = if peak > ceiling { ceiling / peak } else { 1.0 };
                let mut files = vec![];
                for (index, mut buffer) in audio.into_iter().enumerate() {
                    check(cancelled)?;
                    buffer.apply_gain(gain);
                    let path = folder.path().join(format!("candidate-{index}.wav"));
                    let settings = WavExportSettings {
                        sample_rate: 24_000,
                        bit_depth: WavBitDepth::Int16,
                        dither: false,
                    };
                    let mut writer = auris_io::AudioExportWriter::create(
                        &path,
                        buffer.channel_count(),
                        buffer.frame_count(),
                        &settings.into(),
                    )
                    .map_err(|e| e.to_string())?;
                    writer.write(&buffer).map_err(|e| e.to_string())?;
                    let staged = writer.finish_staged().map_err(|e| e.to_string())?;
                    let measured =
                        crate::inspect_export(staged.staged_path()).map_err(|e| e.to_string())?;
                    if measured.full_scale_samples != 0 || measured.true_peak_db > -1.0 {
                        return Err("Candidate preview exceeded protective ceiling".into());
                    }
                    staged.publish_noclobber().map_err(|e| e.to_string())?;
                    files.push(json!({"candidate":index,"path":path,"measurements":measured}));
                }
                check(cancelled)?;
                let report = json!({"round_id":preview.round_id,"files":files,"common_gain":gain,"range":preview.range,"document_unchanged":true,"lifetime":"Until round cancellation, expiry/cache eviction or process exit","quality":"Unscored; common gain preserves relative loudness, no independent normalization"});
                Ok(Prepared::Preview(Box::new(PreviewResult {
                    owner: preview.owner,
                    owner_revision: preview.owner_revision,
                    scope: preview.scope,
                    round_id: preview.round_id,
                    folder,
                    report,
                })))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SessionOptions,
        prelude::{Note, Ticks},
    };
    static SERIAL: Mutex<()> = Mutex::new(());
    fn fixture() -> (Session, ClipId) {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let track = session.add_default_instrument_track("Melody").unwrap();
        let clip = session
            .add_midi_clip(track, "Opening", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .replace_notes(
                clip,
                vec![
                    Note::new(60, Ticks::ZERO, Ticks::QUARTER),
                    Note::new(64, Ticks::QUARTER, Ticks::QUARTER),
                ],
            )
            .unwrap();
        session.forget_history();
        (session, clip)
    }
    fn request(session: &Session, clip: ClipId) -> Operation {
        serde_json::from_value(json!({"kind":"create","expected_revision":session.agent_document_revision().unwrap(),"candidates":[
            {"purpose":"Rising answer","edit":{"kind":"notes","edits":[{"kind":"patch","clip":clip.0,"update":[{"index":1,"pitch":67}]}]}},
            {"purpose":"Falling answer","edit":{"kind":"notes","edits":[{"kind":"patch","clip":clip.0,"update":[{"index":1,"pitch":57}]}]}}
        ]})).unwrap()
    }
    fn create(session: &mut Session, clip: ClipId) -> Value {
        let result = session
            .begin_candidates(request(session, clip))
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        serde_json::from_str(
            &session
                .continue_candidates(result, &AtomicBool::new(false))
                .unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn accompaniment_candidates_keep_protected_notes_and_apply_the_prepared_recipe() {
        let _serial = SERIAL.lock().unwrap();
        let (mut session, clip) = fixture();
        let bass = session.add_default_instrument_track("Bass").unwrap();
        session.stamp_progression(
            &crate::prelude::Chart::parse("| I |").unwrap(),
            Ticks::ZERO,
            1,
        );
        session.forget_history();
        let before = session.project().clone();
        let operation=serde_json::from_value(json!({"kind":"create","expected_revision":session.agent_document_revision().unwrap(),"candidates":[{"purpose":"Support the melody with a bass","edit":{"kind":"accompaniment","protected_clips":[clip.0],"foreground_clips":[clip.0],"operation":{"kind":"generate","parts":[{"track":bass.0,"name":"Bass","part":"bass","start_bar":1,"bars":1,"seed":91}]}}}]})).unwrap();
        let prepared = session
            .begin_candidates(operation)
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        let round: Value = serde_json::from_str(
            &session
                .continue_candidates(prepared, &AtomicBool::new(false))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(session.project(), &before);
        let exact = cache()
            .unwrap()
            .rounds
            .iter()
            .find(|r| r.id == round["round_id"])
            .unwrap()
            .projects[1]
            .clone();
        assert_eq!(exact.midi_clip(clip), before.midi_clip(clip));
        session
            .agent_candidates(Operation::Apply {
                round_id: round["round_id"].as_str().unwrap().into(),
                candidate: 1,
                expected_revision: round["revision"].as_str().unwrap().into(),
            })
            .unwrap();
        assert_eq!(session.project(), &exact);
        session.undo();
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn preview_publication_refuses_cancellation_and_revision_changes() {
        let _serial = SERIAL.lock().unwrap();
        let (mut session, clip) = fixture();
        let round = create(&mut session, clip);
        let id = round["round_id"].as_str().unwrap().to_string();
        let result = session
            .begin_candidates(Operation::Preview {
                round_id: id.clone(),
                range: BarRange {
                    start_bar: 1,
                    bars: 1,
                },
            })
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        session
            .agent_candidates(Operation::Cancel { round_id: id })
            .unwrap();
        assert!(
            session
                .continue_candidates(result, &AtomicBool::new(false))
                .is_err()
        );
        let round = create(&mut session, clip);
        let id = round["round_id"].as_str().unwrap().to_string();
        let result = session
            .begin_candidates(Operation::Preview {
                round_id: id,
                range: BarRange {
                    start_bar: 1,
                    bars: 1,
                },
            })
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        session
            .add_note(clip, Note::new(72, Ticks::QUARTER * 2, Ticks::QUARTER))
            .unwrap();
        session.undo();
        assert!(
            session
                .continue_candidates(result, &AtomicBool::new(false))
                .is_err()
        );
    }

    #[test]
    fn exact_candidate_adoption_is_read_only_until_one_undoable_commit() {
        let _serial = SERIAL.lock().unwrap();
        let (mut session, clip) = fixture();
        let before = session.project().clone();
        let report = create(&mut session, clip);
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
        let id = report["round_id"].as_str().unwrap().to_string();
        let expected = cache()
            .unwrap()
            .rounds
            .iter()
            .find(|r| r.id == id)
            .unwrap()
            .projects[1]
            .clone();
        let read: Value = serde_json::from_str(
            &session
                .agent_candidates(Operation::Read {
                    round_id: id.clone(),
                    candidate: 1,
                    clip: Some(clip.0),
                    offset: 0,
                })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(read["notes"][1]["before"]["pitch"], 64);
        assert_eq!(read["notes"][1]["after"]["pitch"], 67);
        let apply = || Operation::Apply {
            round_id: id.clone(),
            candidate: 1,
            expected_revision: report["revision"].as_str().unwrap().into(),
        };
        session.agent_candidates(apply()).unwrap();
        assert_eq!(session.project(), &expected);
        let replay: Value =
            serde_json::from_str(&session.agent_candidates(apply()).unwrap()).unwrap();
        assert_eq!(replay["replayed"], true);
        assert!(session.undo().is_some());
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
        assert!(session.agent_candidates(apply()).is_err());
        session.redo();
        assert_eq!(session.project(), &expected);
    }
    #[test]
    fn cancelled_stale_foreign_and_expired_rounds_never_apply() {
        let _serial = SERIAL.lock().unwrap();
        for mode in ["cancel", "stale", "foreign", "expired"] {
            let (mut session, clip) = fixture();
            let report = create(&mut session, clip);
            let id = report["round_id"].as_str().unwrap().to_string();
            match mode {
                "cancel" => {
                    session
                        .agent_candidates(Operation::Cancel {
                            round_id: id.clone(),
                        })
                        .unwrap();
                }
                "stale" => {
                    session
                        .add_note(clip, Note::new(72, Ticks::QUARTER * 2, Ticks::QUARTER))
                        .unwrap();
                }
                "foreign" => {
                    session = fixture().0;
                }
                "expired" => {
                    cache()
                        .unwrap()
                        .rounds
                        .iter_mut()
                        .find(|r| r.id == id)
                        .unwrap()
                        .created = Instant::now() - TTL - Duration::from_secs(1);
                }
                _ => unreachable!(),
            }
            let before = session.project().clone();
            assert!(
                session
                    .agent_candidates(Operation::Apply {
                        round_id: id,
                        candidate: 1,
                        expected_revision: report["revision"].as_str().unwrap().into()
                    })
                    .is_err()
            );
            assert_eq!(session.project(), &before);
        }
    }
    #[test]
    fn preparation_checks_owner_revision_cancellation_and_round_budget() {
        let _serial = SERIAL.lock().unwrap();
        let (mut session, clip) = fixture();
        let result = session
            .begin_candidates(request(&session, clip))
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        assert!(
            session
                .continue_candidates(result, &AtomicBool::new(true))
                .is_err()
        );
        let result = session
            .begin_candidates(request(&session, clip))
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        session
            .add_note(clip, Note::new(72, Ticks::QUARTER * 2, Ticks::QUARTER))
            .unwrap();
        assert!(
            session
                .continue_candidates(result, &AtomicBool::new(false))
                .is_err()
        );
        for _ in 0..3 {
            create(&mut session, clip);
        }
        assert!(session.begin_candidates(request(&session, clip)).is_err());
    }
    #[test]
    fn preview_uses_exact_variants_and_common_gain_without_document_edits() {
        let _serial = SERIAL.lock().unwrap();
        let (mut session, clip) = fixture();
        let before = session.project().clone();
        let report = create(&mut session, clip);
        let id = report["round_id"].as_str().unwrap().to_string();
        let rendered = session
            .begin_candidates(Operation::Preview {
                round_id: id.clone(),
                range: BarRange {
                    start_bar: 1,
                    bars: 1,
                },
            })
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        let response: Value = serde_json::from_str(
            &session
                .continue_candidates(rendered, &AtomicBool::new(false))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(response["files"].as_array().unwrap().len(), 3);
        assert!(
            response["common_gain"]
                .as_f64()
                .is_some_and(|gain| gain > 0.0 && gain <= 1.0)
        );
        let paths: Vec<_> = response["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| std::path::PathBuf::from(f["path"].as_str().unwrap()))
            .collect();
        assert_ne!(
            std::fs::read(&paths[0]).unwrap(),
            std::fs::read(&paths[1]).unwrap()
        );
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
        session
            .agent_candidates(Operation::Cancel { round_id: id })
            .unwrap();
        assert!(paths.iter().all(|p| !p.exists()));
    }
}
