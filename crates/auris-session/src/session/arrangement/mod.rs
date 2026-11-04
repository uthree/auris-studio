//! Capability-aware, atomic arrangement proposals over immutable document snapshots.
//!
//! Enable the agent tool explicitly with `AURIS_ARRANGEMENT_HARNESS=1`. Validation
//! prepares a private candidate; applying its process-local ID checks document identity,
//! content revision, asset ownership and expiry before a single ordinary Undo edit.

mod breathing;
mod diagnostics;
mod prepare;
#[cfg(test)]
mod tests;
mod types;

use super::{PreparedAudioSource, Session, sound_search::SoundPreparation};
use crate::live_agent::automation;
use auris_core::automation::AutomationCurve;
use auris_core::time::Ticks;
use auris_core::{ClipId, MidiClip, NoteTransform, ParamTarget, PluginRegistry, Project};
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
pub use types::{
    Action, Alignment, Change, MelodicRole, NoteAddress, Operation, PhraseLanding, RestWindow,
    Transition,
};

const MAX_PLAN_BYTES: usize = 64 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(15 * 60);
static PLANS: OnceLock<Mutex<VecDeque<Arc<Mutex<PreparedPlan>>>>> = OnceLock::new();

/// Whether the explicitly opted-in arrangement tool and workflow are available.
pub fn enabled() -> bool {
    std::env::var("AURIS_ARRANGEMENT_HARNESS").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on"
        )
    })
}

enum Pending {
    Performance {
        clip: ClipId,
        voice: Option<String>,
        stack: Vec<NoteTransform>,
    },
    Automation {
        target: ParamTarget,
        key: Option<String>,
        curve: AutomationCurve,
        value: f32,
        start: Ticks,
        end: Ticks,
        points: Vec<automation::Point>,
    },
    Revoice {
        clips: Vec<ClipId>,
        sound: Box<SoundPreparation>,
        name: String,
        layer: bool,
        stacks: Vec<Vec<NoteTransform>>,
    },
    Other(Action),
}

struct Import {
    file: Option<tempfile::NamedTempFile>,
    source: auris_core::SourceId,
    prepared: PreparedAudioSource,
}

struct PreparedPlan {
    id: String,
    scope: String,
    revision: String,
    owner_revision: u64,
    created: Instant,
    project: Project,
    imports: Vec<Import>,
    sounds: Vec<SoundPreparation>,
    report: Value,
    bytes: usize,
    applied: Option<(String, String)>,
    invalid: bool,
}

/// Detached preparation, containing no live plugin instances or UI objects.
pub struct ArrangementJob {
    owner: Arc<PluginRegistry>,
    owner_revision: u64,
    scope: String,
    revision: String,
    folder: PathBuf,
    render_rate: f64,
    project: Project,
    changes: Vec<(String, Pending)>,
}

/// Worker-prepared candidate awaiting validation by its original document owner.
pub struct ArrangementResult {
    owner: Arc<PluginRegistry>,
    plan: PreparedPlan,
    folder: PathBuf,
    render_rate: f64,
}

fn name(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err("Use a nonempty name/purpose up to 256 bytes without control characters".into())
    } else {
        Ok(())
    }
}

fn gain(value: f32) -> Result<(), String> {
    if value.is_finite() && (-60.0..=0.0).contains(&value) {
        Ok(())
    } else {
        Err("Arrangement gain must be finite and within -60..0 dB".into())
    }
}

fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Arrangement preparation cancelled".into())
    } else {
        Ok(())
    }
}

impl Session {
    fn arrangement_scope(&self) -> String {
        match self.path() {
            Some(path) => std::fs::canonicalize(path)
                .unwrap_or_else(|_| path.to_owned())
                .to_string_lossy()
                .into_owned(),
            None => format!("unsaved:{}", self.sound_scope),
        }
    }

    pub(crate) fn agent_arrangement(&mut self, operation: Operation) -> Result<String, String> {
        match operation {
            Operation::InspectBreathing {
                track,
                role,
                start_tick,
                end_tick,
                min_rest_ticks,
                phrase_ends,
            } => {
                let mut value = breathing::inspect(
                    &self.project,
                    track,
                    role,
                    start_tick,
                    end_tick,
                    min_rest_ticks,
                    &phrase_ends,
                )?;
                value["ending_guidance"]["envelope_controls"] =
                    breathing::envelope_controls(self.registry(), &self.project, track);
                let route = breathing::envelope_route(&self.project, track);
                if !route.is_null() {
                    value["ending_guidance"]["envelope_route"] = route;
                }
                value["revision"] = self.agent_document_revision()?.into();
                Ok(value.to_string())
            }
            Operation::Inspect {
                start_bar,
                bars,
                offset,
            } => self.inspect_arrangement(start_bar, bars, offset),
            Operation::Apply {
                plan_id,
                expected_revision,
            } => self.apply_arrangement(&plan_id, &expected_revision),
            Operation::Validate { .. } => {
                Err("Arrangement validation requires the detached worker protocol".into())
            }
        }
    }

    pub(crate) fn begin_arrangement(
        &mut self,
        revision: String,
        changes: Vec<Change>,
        plugin_paths: &[PathBuf],
    ) -> Result<ArrangementJob, String> {
        if self.transaction.is_some() {
            return Err("Finish the active gesture before preparing an arrangement".into());
        }
        if self.agent_document_revision()? != revision {
            return Err("Stale arrangement revision; inspect the current document first".into());
        }
        if changes.is_empty() || changes.len() > 16 {
            return Err("Supply 1..16 arrangement changes".into());
        }
        if serde_json::to_vec(&changes)
            .map_err(|e| e.to_string())?
            .len()
            > 128 * 1024
        {
            return Err("Arrangement changes exceed the 128 KiB argument limit".into());
        }
        let mut targets = HashSet::new();
        let mut pending = Vec::new();
        for change in changes {
            name(&change.purpose)?;
            let mut claim = |id: u64| -> Result<ClipId, String> {
                let clip = ClipId(id);
                if self.project.track_of_clip(clip).is_none() {
                    return Err(format!("Unknown clip ID {id}"));
                }
                if !targets.insert(clip) {
                    return Err(format!(
                        "Clip {id} is targeted twice; combine its edits in one revoice/settings change or prepare a later plan"
                    ));
                }
                Ok(clip)
            };
            let prepared = match change.edit {
                Action::Performance {
                    clip,
                    drum_voice,
                    settings,
                } => {
                    let clip = claim(clip)?;
                    let stack =
                        self.prepare_agent_performance(clip, drum_voice.as_deref(), *settings)?;
                    let mut candidate = self
                        .project
                        .midi_clip(clip)
                        .ok_or("Expected MIDI clip")?
                        .1
                        .clone();
                    candidate.transforms = stack.clone();
                    candidate
                        .looped_note_instances()
                        .map_err(|e| e.to_string())?;
                    Pending::Performance {
                        clip,
                        voice: drum_voice,
                        stack,
                    }
                }
                Action::Automation {
                    target,
                    start_tick,
                    end_tick,
                    points,
                } => {
                    if start_tick < 0
                        || end_tick <= start_tick
                        || end_tick > self.project.end_tick().raw()
                        || points.is_empty()
                        || points.len() > 128
                    {
                        return Err("Automation requires a passage inside the existing song and 1..128 interior points".into());
                    }
                    let target = self.agent_control_target(&target)?;
                    let descriptor = self
                        .automatable(target)
                        .ok_or("Target cannot be automated")?;
                    let mut ticks = HashSet::new();
                    for p in &points {
                        if p.at_tick <= start_tick
                            || p.at_tick >= end_tick
                            || !ticks.insert(p.at_tick)
                            || !p.value.is_finite()
                            || (descriptor.clamp(p.value) - p.value).abs() > 1e-5
                        {
                            return Err("Automation points must be unique, strictly inside the passage and match native ranges/choices".into());
                        }
                    }
                    let curve = self
                        .project
                        .automation
                        .lane(target)
                        .map(|l| l.curve)
                        .unwrap_or(if descriptor.steps.is_some() {
                            AutomationCurve::Hold
                        } else {
                            AutomationCurve::Linear
                        });
                    let key = match target {
                        ParamTarget::Instrument { .. } | ParamTarget::Effect { .. } => {
                            Some(descriptor.key.to_string())
                        }
                        _ => None,
                    };
                    Pending::Automation {
                        target,
                        key,
                        curve,
                        value: self.param_value(target, &descriptor),
                        start: Ticks(start_tick),
                        end: Ticks(end_tick),
                        points,
                    }
                }
                Action::Revoice {
                    clips,
                    sound_id,
                    name: track_name,
                    layer,
                    settings,
                } => {
                    name(&track_name)?;
                    if clips.is_empty() || clips.len() > 16 {
                        return Err("Revoice requires 1..16 complete clips from one track".into());
                    }
                    let mut selected = Vec::new();
                    let mut stacks = Vec::new();
                    let mut source = None;
                    for id in clips {
                        let clip = claim(id)?;
                        let (track, midi) = self
                            .project
                            .midi_clip(clip)
                            .ok_or("Revoice requires MIDI clips")?;
                        if source.is_some_and(|s| s != track) {
                            return Err("Revoice clips must belong to the same track".into());
                        }
                        source = Some(track);
                        let track = self.project.track(track).ok_or("Unknown source track")?;
                        if track.kind.as_instrument().is_none() {
                            return Err("Use revoice for instrument/drum clips; singer voices use the vocal pipeline".into());
                        }
                        let stack = if let Some(patch) = &settings {
                            let patch = serde_json::from_value(
                                serde_json::to_value(patch).map_err(|e| e.to_string())?,
                            )
                            .map_err(|e| e.to_string())?;
                            self.prepare_agent_performance(clip, None, patch)?
                        } else {
                            midi.transforms.clone()
                        };
                        selected.push(clip);
                        stacks.push(stack);
                    }
                    let sound = self.prepare_arrangement_sound(&sound_id, plugin_paths)?;
                    Pending::Revoice {
                        clips: selected,
                        sound: Box::new(sound),
                        name: track_name,
                        layer,
                        stacks,
                    }
                }
                other => {
                    match &other {
                        Action::Breath { clips, .. } => {
                            if clips.is_empty() || clips.len() > 16 {
                                return Err("Breath requires 1..16 foreground clips".into());
                            }
                            for clip in clips {
                                claim(*clip)?;
                            }
                        }
                        Action::ClipMute { clip, .. } => {
                            claim(*clip)?;
                        }
                        Action::Transition {
                            name: label,
                            gain_db,
                            at_tick,
                            duration_ticks,
                            ..
                        } => {
                            name(label)?;
                            gain(*gain_db)?;
                            if *at_tick < 0
                                || !(1..=16 * 960).contains(duration_ticks)
                                || at_tick.checked_add(*duration_ticks).is_none()
                            {
                                return Err("Transition requires a nonnegative position and duration 1..15360 ticks".into());
                            }
                        }
                        Action::AudioSfx {
                            name: label,
                            gain_db,
                            at_tick,
                            fade_in_ms,
                            fade_out_ms,
                            path,
                            ..
                        } => {
                            name(label)?;
                            gain(*gain_db)?;
                            if *at_tick < 0
                                || !fade_in_ms.is_finite()
                                || !fade_out_ms.is_finite()
                                || *fade_in_ms < 0.0
                                || *fade_out_ms < 0.0
                                || *fade_in_ms + *fade_out_ms > 30_000.0
                                || !std::path::Path::new(path).is_absolute()
                                || path.starts_with("\\\\")
                                || path.starts_with("//")
                            {
                                return Err("SFX requires a local absolute file, nonnegative at_tick and finite nonnegative fades up to thirty seconds".into());
                            }
                        }
                        _ => unreachable!(),
                    }
                    Pending::Other(other)
                }
            };
            pending.push((change.purpose, prepared));
        }
        Ok(ArrangementJob {
            owner: Arc::clone(self.registry()),
            owner_revision: self.revision(),
            scope: self.arrangement_scope(),
            revision,
            folder: self
                .project_folder()
                .ok_or("Missing project working folder")?
                .to_owned(),
            render_rate: self.sample_rate(),
            project: self.project.clone(),
            changes: pending,
        })
    }

    pub(crate) fn continue_arrangement(
        &mut self,
        result: ArrangementResult,
        cancelled: &AtomicBool,
    ) -> Result<String, String> {
        check(cancelled)?;
        if self.revision() != result.plan.owner_revision
            || self.agent_document_revision()? != result.plan.revision
            || self.arrangement_scope() != result.plan.scope
            || !Arc::ptr_eq(self.registry(), &result.owner)
            || self.project_folder() != Some(result.folder.as_path())
            || self.sample_rate() != result.render_rate
        {
            return Err("Document, folder or audio device changed during arrangement preparation; validate again".into());
        }
        let response = result.plan.report.to_string();
        let mut cache = PLANS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Arrangement cache lock failed")?;
        cache.retain(|p| p.lock().is_ok_and(|p| p.created.elapsed() < TTL));
        let mut bytes = result.plan.bytes;
        for entry in cache.iter() {
            bytes = bytes.saturating_add(
                entry
                    .lock()
                    .map_err(|_| "Arrangement plan lock failed")?
                    .bytes,
            );
        }
        while cache.len() >= 8 || bytes > MAX_CACHE_BYTES {
            let Some(old) = cache.pop_front() else {
                break;
            };
            bytes = bytes.saturating_sub(
                old.lock()
                    .map_err(|_| "Arrangement plan lock failed")?
                    .bytes,
            );
        }
        cache.push_back(Arc::new(Mutex::new(result.plan)));
        Ok(response)
    }

    fn apply_arrangement(&mut self, id: &str, revision: &str) -> Result<String, String> {
        if self.transaction.is_some() {
            return Err("Finish the active gesture before applying an arrangement".into());
        }
        let entry = {
            let cache = PLANS
                .get_or_init(Default::default)
                .lock()
                .map_err(|_| "Arrangement cache lock failed")?;
            cache
                .iter()
                .find(|p| p.lock().is_ok_and(|p| p.id == id))
                .cloned()
                .ok_or("Unknown or evicted arrangement plan; validate again")?
        };
        let mut plan = entry.lock().map_err(|_| "Arrangement plan lock failed")?;
        if plan.created.elapsed() >= TTL
            || plan.invalid
            || plan.scope != self.arrangement_scope()
            || plan.revision != revision
        {
            return Err(
                "Expired plan, wrong document or mismatched original revision; validate again"
                    .into(),
            );
        }
        let current = self.agent_document_revision()?;
        if let Some((after, response)) = &plan.applied {
            if *after != current {
                return Err("Document changed after this plan was applied; use history redo or validate a fresh plan".into());
            }
            let mut response: Value = serde_json::from_str(response).map_err(|e| e.to_string())?;
            response["replayed"] = true.into();
            return Ok(response.to_string());
        }
        if current != revision || (self.path().is_none() && self.revision() != plan.owner_revision)
        {
            return Err("Stale arrangement plan; inspect and validate the current document".into());
        }
        for sound in &plan.sounds {
            sound.still_current()?;
        }
        if plan
            .imports
            .iter()
            .any(|i| i.prepared.render_rate != self.sample_rate())
        {
            return Err("Audio device changed; validate the SFX again".into());
        }
        let mut published = Vec::new();
        for import in &mut plan.imports {
            let result = import
                .file
                .take()
                .ok_or("SFX stage is no longer available")?
                .keep();
            match result {
                Ok((file, path)) => {
                    drop(file);
                    published.push(path);
                }
                Err(error) => {
                    for path in published {
                        let _ = std::fs::remove_file(path);
                    }
                    plan.invalid = true;
                    return Err(format!(
                        "Could not retain prepared SFX; document unchanged: {error}"
                    ));
                }
            }
        }
        let changed = self.project != plan.project;
        if changed {
            self.begin_transaction(crate::Edit::ExternalChanges);
            self.record(crate::Edit::ExternalChanges);
            self.project = plan.project.clone();
            for import in &plan.imports {
                self.install_prepared_audio(import.source, import.prepared.clone());
            }
            self.invalidate_graph();
            self.end_transaction();
        }
        let after = self.agent_document_revision()?;
        let response=json!({"plan_id":id,"applied":true,"changed":changed,"replayed":false,"revision":after,
            "changes":plan.report["changes"],"issues":plan.report["issues"],
            "verification":"Read back arrangement/notes/automation independently, then compare the same rendered bars. Settings and numeric deltas do not establish musical improvement."}).to_string();
        plan.applied = Some((after, response.clone()));
        Ok(response)
    }
}
