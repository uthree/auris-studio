//! Detached, rendered mix and performance matching, with explicit undoable adoption.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use auris_core::rng::Rng;
use auris_core::{
    AudioBuffer, AudioSourceBank, Expression, MidiClip, NoteTransform, ParamTarget, PluginRegistry,
    Project, TrackId,
};
use auris_engine::{EngineError, OfflineOptions, RenderProgress};

use super::{PlaybackState, Session};
use crate::audio_evaluation::{AudioEvaluation, AudioEvaluator};
use crate::{Edit, RenderJob, SessionError};

const SAMPLE_RATE: f64 = 44_100.0;

mod arrangement;
mod excerpt;
mod instruments;
mod seeds;

/// The fixed project excerpt and bounded search controls.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceMatchSettings {
    /// Beginning of the project excerpt, in seconds on the project timeline.
    pub project_start_seconds: f64,
    /// Excerpt duration, from one to thirty seconds.
    pub duration_seconds: f64,
    /// Maximum render attempts, including the unchanged baseline, from two to 512.
    pub attempts: usize,
    /// Seed ordering the coordinate proposals within this build.
    pub seed: u64,
    /// Adjust track gain and pan within 3 dB and 0.3 of their original values.
    pub mix: bool,
    /// Adjust expression and gate on instrument and drum clips without rewriting notes.
    pub performance: bool,
    /// Regenerate clips carrying a recipe with fresh deterministic take seeds.
    pub generation_seeds: bool,
    /// Explore built-in sounds and presets from already loaded SoundFonts.
    pub instruments: bool,
    /// Explore note-preserving articulation, groove and pitch-gesture settings.
    pub arrangement: bool,
}

impl Default for ReferenceMatchSettings {
    fn default() -> Self {
        Self {
            project_start_seconds: 0.0,
            duration_seconds: 12.0,
            attempts: 32,
            seed: 0,
            mix: true,
            performance: true,
            generation_seeds: true,
            instruments: true,
            arrangement: true,
        }
    }
}

impl ReferenceMatchSettings {
    fn validate(&self, project: &Project) -> Result<(), SessionError> {
        if !self.project_start_seconds.is_finite()
            || self.project_start_seconds < 0.0
            || self.project_start_seconds >= project.duration_seconds()
            || !self.duration_seconds.is_finite()
            || !(1.0..=30.0).contains(&self.duration_seconds)
            || !(2..=512).contains(&self.attempts)
            || (!self.mix
                && !self.performance
                && !self.generation_seeds
                && !self.instruments
                && !self.arrangement)
        {
            return Err(failure(
                "choose a project excerpt, a duration of 1–30 seconds, 2–512 attempts, and at least one adjustment type",
            ));
        }
        Ok(())
    }
}

/// Number of budgeted attempts completed before the next render begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReferenceMatchProgress {
    /// Completed attempts, including the baseline and rejected candidates.
    pub completed: usize,
    /// Requested evaluation budget, including the baseline.
    pub total: usize,
}

/// One render captured on the session thread and ready to move to a worker.
pub struct ReferenceMatchJob {
    state: Box<MatchState>,
    render: Box<RenderJob>,
}

/// An opaque worker result, accepted only by its unchanged originating session.
pub struct ReferenceMatchResult {
    state: Box<MatchState>,
}

/// The next detached render, or a completed report awaiting explicit adoption.
pub enum ReferenceMatchStep {
    /// Run this job on a worker, then continue on the session thread.
    Pending(ReferenceMatchJob),
    /// The original document is still unchanged; preview or explicitly apply this report.
    Complete(ReferenceMatchReport),
}

/// The unchanged baseline and the strictly best rendered candidate.
///
/// Both buffers contain the exact evaluated full-mix excerpt at 44.1 kHz. They should be
/// previewed directly at the final output, without passing through the project's mixer again.
/// The private candidate remains exact even if a caller edits these presentation fields.
#[derive(Clone)]
pub struct ReferenceMatchReport {
    /// Evaluation of the project before any adjustment.
    pub baseline: AudioEvaluation,
    /// Best evaluation; higher fitness wins and ties retain the earlier candidate.
    pub best: AudioEvaluation,
    /// Exact audio evaluated for the baseline.
    pub baseline_audio: Arc<AudioBuffer>,
    /// Exact audio evaluated for the retained candidate.
    pub best_audio: Arc<AudioBuffer>,
    /// Completed attempts, including the baseline and rejected candidates.
    pub attempts: usize,
    /// Post-baseline candidates rejected because rendering or audio evaluation failed.
    pub failed_attempts: usize,
    /// Whether cancellation ended the pass after a usable baseline was measured.
    pub cancelled: bool,
    /// Human-readable changes from the baseline to the retained candidate.
    pub changes: Vec<String>,
    adoption: Box<Adoption>,
}

#[derive(Clone)]
struct Adoption {
    provenance: Provenance,
    project: Project,
}

#[derive(Clone)]
struct Provenance {
    owner: Arc<PluginRegistry>,
    revision: u64,
    original: Project,
    folder: Option<PathBuf>,
}

struct Measurement {
    evaluation: AudioEvaluation,
    audio: Arc<AudioBuffer>,
}

struct MatchState {
    provenance: Provenance,
    registry: Arc<PluginRegistry>,
    bank: AudioSourceBank,
    settings: ReferenceMatchSettings,
    evaluator: Arc<dyn AudioEvaluator>,
    options: OfflineOptions,
    families: Vec<Vec<SearchDial>>,
    candidate: Project,
    best_project: Project,
    baseline: Option<Measurement>,
    best: Option<Measurement>,
    completed: usize,
    failed_attempts: usize,
    cancelled: bool,
}

impl ReferenceMatchJob {
    /// The position of this render within the complete evaluation budget.
    pub fn progress(&self) -> ReferenceMatchProgress {
        ReferenceMatchProgress {
            completed: self.state.completed,
            total: self.state.settings.attempts,
        }
    }

    /// Render and evaluate on the calling worker thread, without changing the session.
    ///
    /// Cancellation before a complete baseline returns a cancellation error. Later cancellation
    /// returns the measured partial best through the ordinary continuation protocol.
    /// A failed baseline is fatal; a later render or evaluation failure consumes one attempt
    /// without displacing the retained best candidate.
    pub fn run(
        mut self,
        cancelled: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<ReferenceMatchResult, SessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return self.cancel();
        }
        let offset = self.state.completed as f32 / self.state.settings.attempts as f32;
        let scale = 1.0 / self.state.settings.attempts as f32;
        let mut report_render = |fraction| progress(offset + fraction * scale * 0.95);
        let audio = self.render.render_complete(
            &self.state.options,
            &mut RenderProgress::reporting(&mut report_render).cancelled_by(cancelled),
        );
        let audio = match audio {
            Ok(audio) => audio,
            Err(error) if error.is_cancellation() => return self.cancel(),
            Err(error) => return self.reject(error, cancelled, progress),
        };
        if cancelled.load(Ordering::Relaxed) {
            return self.cancel();
        }
        if audio
            .channels()
            .iter()
            .flatten()
            .any(|sample| !sample.is_finite())
        {
            return self.reject(
                failure("the renderer produced non-finite samples"),
                cancelled,
                progress,
            );
        }
        let evaluation = self.state.evaluator.evaluate(&audio);
        if cancelled.load(Ordering::Relaxed) {
            return self.cancel();
        }
        let evaluation = match evaluation {
            Ok(evaluation) => evaluation,
            Err(error) => return self.reject(failure(error), cancelled, progress),
        };
        if let Err(error) = evaluation.validate() {
            return self.reject(failure(error), cancelled, progress);
        }
        let audio = Arc::new(audio);
        if self.state.baseline.is_none() {
            self.state.baseline = Some(Measurement {
                evaluation: evaluation.clone(),
                audio: Arc::clone(&audio),
            });
        }
        if self
            .state
            .best
            .as_ref()
            .is_none_or(|best| evaluation.fitness > best.evaluation.fitness)
        {
            self.state.best = Some(Measurement { evaluation, audio });
            self.state.best_project = self.state.candidate.clone();
        }
        self.state.completed += 1;
        self.state.cancelled = cancelled.load(Ordering::Relaxed);
        progress(self.state.completed as f32 / self.state.settings.attempts as f32);
        Ok(ReferenceMatchResult { state: self.state })
    }

    fn reject(
        mut self,
        error: SessionError,
        cancelled: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<ReferenceMatchResult, SessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return self.cancel();
        }
        if self.state.baseline.is_none() {
            return Err(error);
        }
        self.state.completed += 1;
        self.state.failed_attempts += 1;
        self.state.cancelled = cancelled.load(Ordering::Relaxed);
        progress(self.state.completed as f32 / self.state.settings.attempts as f32);
        Ok(ReferenceMatchResult { state: self.state })
    }

    fn cancel(mut self) -> Result<ReferenceMatchResult, SessionError> {
        if self.state.baseline.is_none() {
            return Err(EngineError::RenderCancelled.into());
        }
        self.state.cancelled = true;
        Ok(ReferenceMatchResult { state: self.state })
    }
}

impl Session {
    /// Capture an unchanged baseline and begin rendered matching on a fixed project excerpt.
    ///
    /// Proposals alternate enabled families: mix, performance, generated takes, instruments,
    /// and non-destructive arrangement. Generated takes explicitly rewrite recipe-backed notes;
    /// the other families preserve the stored score. Every candidate stays detached until adoption.
    /// Automated faders are left alone. Source banks and SoundFonts are frozen for the pass;
    /// missing instruments, effects, audio, or current rendered vocals are refused explicitly.
    pub fn begin_reference_match(
        &mut self,
        settings: ReferenceMatchSettings,
        evaluator: Arc<dyn AudioEvaluator>,
    ) -> Result<ReferenceMatchJob, SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        if self.is_recording() {
            return Err(SessionError::RecordingInProgress);
        }
        self.poll();
        self.collect_reference_native_state()?;
        settings.validate(&self.project)?;
        for track in self.playback_readiness() {
            if track.state != PlaybackState::Ready {
                return Err(failure(format!(
                    "{} is not ready for a complete render ({:?})",
                    track.name, track.state
                )));
            }
        }
        let families = search_families(self, &settings);
        if families.is_empty() {
            return Err(failure(
                "the project has no eligible controls in the selected search families",
            ));
        }
        let fonts = auris_sampler::SoundFontBank::shared();
        for reference in self.soundfonts() {
            if let Some(font) = self.fonts.get(reference.id) {
                fonts.insert(reference.id, font);
            }
        }
        let original = self.project.clone();
        let options = OfflineOptions {
            start_frames: (settings.project_start_seconds * SAMPLE_RATE).round() as u64,
            end_frames: Some(
                ((settings.project_start_seconds + settings.duration_seconds) * SAMPLE_RATE).round()
                    as u64,
            ),
            sample_rate: Some(SAMPLE_RATE),
            include_tail: false,
            looping: false,
            block_frames: self.engine.max_block(),
        };
        let state = Box::new(MatchState {
            provenance: Provenance {
                owner: Arc::clone(&self.registry),
                revision: self.revision,
                original: original.clone(),
                folder: self.project_folder().map(PathBuf::from),
            },
            registry: crate::default_registry(fonts),
            bank: self.bank.clone(),
            settings,
            evaluator,
            options,
            families,
            candidate: original.clone(),
            best_project: original,
            baseline: None,
            best: None,
            completed: 0,
            failed_attempts: 0,
            cancelled: false,
        });
        self.reference_job_for(state)
    }

    fn reference_job_for(
        &mut self,
        mut state: Box<MatchState>,
    ) -> Result<ReferenceMatchJob, SessionError> {
        // Native main-thread factories stay with the session; only their render halves travel.
        let prepare = auris_core::plugin::PrepareContext::new(
            SAMPLE_RATE,
            state.options.block_frames,
            auris_engine::RENDER_CHANNELS,
        );
        let mut placed = self.hosted.place(&state.candidate, &prepare);
        placed.extend(self.vst3.place(&state.candidate, &prepare));
        let mut instruments = self.hosted.place_instruments(&state.candidate, &prepare);
        instruments.extend(self.vst3.place_instruments(&state.candidate, &prepare));
        let render = Box::new(RenderJob::new(
            state.candidate.clone(),
            state.bank.clone(),
            Arc::clone(&state.registry),
            placed,
            instruments,
        ));
        render.validate_complete()?;
        // Native dirty notifications do not update Project. Serialize the actual instances
        // before deciding a revision bump was only setup; otherwise a preset edited in its
        // own window could be accepted under the old baseline's provenance.
        self.poll();
        self.collect_reference_native_state()?;
        if self.project != state.provenance.original {
            return Err(failure(
                "the project changed while preparing a render; start matching again",
            ));
        }
        state.provenance.revision = self.revision;
        Ok(ReferenceMatchJob { state, render })
    }

    /// Accept a worker result and capture the next render, or return its exact partial best.
    ///
    /// No project edits occur here. Edits, undo, native state changes, another session, or a
    /// changed project folder invalidate the pass before another candidate can be prepared.
    pub fn continue_reference_match(
        &mut self,
        result: ReferenceMatchResult,
    ) -> Result<ReferenceMatchStep, SessionError> {
        let mut state = result.state;
        self.check_reference_provenance(&state.provenance)?;
        if !state.cancelled && state.completed < state.settings.attempts {
            let proposal = state.completed - 1;
            let family = &state.families[(proposal / 2) % state.families.len()];
            let visit = (proposal / 2) / state.families.len();
            let dial = &family[visit % family.len()];
            let occurrence = (visit / family.len()) * 2 + proposal % 2;
            state.candidate = state.best_project.clone();
            dial.adjust(&mut state.candidate, &state.provenance.original, occurrence);
            return Ok(ReferenceMatchStep::Pending(self.reference_job_for(state)?));
        }
        let baseline = state.baseline.expect("a completed pass has a baseline");
        let best = state.best.expect("a completed pass has a best candidate");
        let mut changes = describe_changes(&state.provenance.original, &state.best_project);
        changes.extend(seeds::describe_changes(
            &state.provenance.original,
            &state.best_project,
        ));
        changes.extend(instruments::describe_changes(
            &state.provenance.original,
            &state.best_project,
        ));
        changes.extend(arrangement::describe_changes(
            &state.provenance.original,
            &state.best_project,
        ));
        Ok(ReferenceMatchStep::Complete(ReferenceMatchReport {
            baseline: baseline.evaluation,
            best: best.evaluation,
            baseline_audio: baseline.audio,
            best_audio: best.audio,
            attempts: state.completed,
            failed_attempts: state.failed_attempts,
            cancelled: state.cancelled,
            changes,
            adoption: Box::new(Adoption {
                provenance: state.provenance,
                project: state.best_project,
            }),
        }))
    }

    /// Adopt the exact retained project in one undo step, without recomposition or balancing.
    ///
    /// Returns `false` when the unchanged baseline won. A report from an edited project or a
    /// different session is refused before recording history or changing the live graph.
    pub fn apply_reference_match(
        &mut self,
        report: &ReferenceMatchReport,
    ) -> Result<bool, SessionError> {
        self.check_reference_provenance(&report.adoption.provenance)?;
        if report.adoption.project == report.adoption.provenance.original {
            return Ok(false);
        }
        self.record(Edit::MatchReference);
        self.replace_project(report.adoption.project.clone());
        Ok(true)
    }

    fn check_reference_provenance(&mut self, provenance: &Provenance) -> Result<(), SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        if self.is_recording() {
            return Err(SessionError::RecordingInProgress);
        }
        self.poll();
        self.collect_reference_native_state()?;
        if !Arc::ptr_eq(&self.registry, &provenance.owner)
            || self.revision != provenance.revision
            || self.project != provenance.original
            || self.project_folder() != provenance.folder.as_deref()
        {
            return Err(failure("the project changed; start matching again"));
        }
        Ok(())
    }

    /// Capture all native state atomically; matching cannot accept the save command's
    /// best-effort behavior when a plugin refuses to serialize its actual sound.
    fn collect_reference_native_state(&mut self) -> Result<(), SessionError> {
        let mut snapshot = self.project.clone();
        for strip in std::iter::once(&mut snapshot.master)
            .chain(snapshot.tracks.iter_mut().map(|track| &mut track.mixer))
        {
            for slot in strip.effects.iter_mut().filter(|slot| slot.is_hosted()) {
                let bytes = if slot.effect_id.starts_with(auris_vst3::ID_PREFIX) {
                    self.vst3.save_effect(slot.id)
                } else {
                    self.hosted.save_state(slot.id)
                }
                .ok_or_else(|| {
                    failure(format!(
                        "effect {} could not snapshot its current state",
                        slot.effect_id
                    ))
                })?;
                slot.state.set_hosted_bytes(&bytes);
            }
        }
        for track in &mut snapshot.tracks {
            let Some(instrument) = track
                .kind
                .as_instrument_mut()
                .filter(|instrument| instrument.is_hosted())
            else {
                continue;
            };
            let bytes = if instrument.instrument_id.starts_with(auris_vst3::ID_PREFIX) {
                self.vst3.save_instrument(track.id)
            } else {
                self.hosted.save_instrument_state(track.id)
            }
            .ok_or_else(|| {
                failure(format!(
                    "instrument on {} could not snapshot its current state",
                    track.name
                ))
            })?;
            instrument.instrument_state.set_hosted_bytes(&bytes);
        }
        self.project = snapshot;
        Ok(())
    }
}

fn failure(message: impl Into<String>) -> SessionError {
    SessionError::ReferenceMatch(message.into())
}

enum SearchDial {
    Continuous(Dial),
    Generation(seeds::Dial),
    Instrument(instruments::Dial),
    Arrangement(arrangement::Dial),
}

impl SearchDial {
    fn adjust(&self, project: &mut Project, original: &Project, occurrence: usize) {
        let direction = if occurrence.is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        match self {
            Self::Continuous(dial) => dial.adjust(project, original, direction),
            Self::Generation(dial) => dial.adjust(project, occurrence),
            Self::Instrument(dial) => dial.adjust(project, original, occurrence),
            Self::Arrangement(dial) => dial.adjust(project, original, occurrence),
        }
    }
}

fn search_families(session: &Session, settings: &ReferenceMatchSettings) -> Vec<Vec<SearchDial>> {
    let enumeration = excerpt::enumeration_project(&session.project, settings);
    let mut mix = Vec::new();
    let mut performance = Vec::new();
    for dial in search_dials(&enumeration, settings) {
        let family = if matches!(dial.control, Control::Gain | Control::Pan) {
            &mut mix
        } else {
            &mut performance
        };
        family.push(SearchDial::Continuous(dial));
    }
    let mut families = vec![mix, performance];
    if settings.generation_seeds {
        families.push(
            seeds::dials(&enumeration, settings.seed)
                .into_iter()
                .map(SearchDial::Generation)
                .collect(),
        );
    }
    if settings.instruments {
        families.push(
            instruments::dials(session, &enumeration, settings.seed)
                .into_iter()
                .map(SearchDial::Instrument)
                .collect(),
        );
    }
    if settings.arrangement {
        families.push(
            arrangement::dials(&enumeration)
                .into_iter()
                .map(SearchDial::Arrangement)
                .collect(),
        );
    }
    // Each active family receives two proposals per round regardless of its dimension count.
    // Categorical visits enumerate new choices even when their previous choices did not win.
    let mut rng = Rng::stream(settings.seed, &["render_search_dimensions".into()]);
    for family in families.iter_mut().skip(2) {
        for index in (1..family.len()).rev() {
            family.swap(index, rng.below(index + 1));
        }
    }
    families.retain(|family| !family.is_empty());
    families
}

#[derive(Clone, Copy)]
enum Control {
    Gain,
    Pan,
    Timing,
    Velocity,
    Swell,
    Accent,
    Delay,
    Gate,
}

#[derive(Clone)]
struct Dial {
    track: TrackId,
    control: Control,
    clips: Vec<auris_core::ClipId>,
}

fn search_dials(project: &Project, settings: &ReferenceMatchSettings) -> Vec<Dial> {
    let mut mix = Vec::new();
    let mut performance = Vec::new();
    for track in &project.tracks {
        if track.mixer.mute {
            continue;
        }
        if settings.mix && !track.kind.is_bus() {
            if project
                .automation
                .lane(ParamTarget::TrackGain(track.id))
                .is_none()
                && track.mixer.gain_db.is_finite()
            {
                mix.push(Dial {
                    track: track.id,
                    control: Control::Gain,
                    clips: Vec::new(),
                });
            }
            if project
                .automation
                .lane(ParamTarget::TrackPan(track.id))
                .is_none()
                && track.mixer.pan.is_finite()
            {
                mix.push(Dial {
                    track: track.id,
                    control: Control::Pan,
                    clips: Vec::new(),
                });
            }
        }
        let clips: Vec<_> = track
            .kind
            .as_instrument()
            .map(|instrument| {
                instrument
                    .clips
                    .iter()
                    .filter(|clip| !clip.muted && !clip.notes.is_empty())
                    .map(|clip| clip.id)
                    .collect()
            })
            .unwrap_or_default();
        if settings.performance && !clips.is_empty() {
            for control in [
                Control::Timing,
                Control::Velocity,
                Control::Swell,
                Control::Accent,
                Control::Delay,
                Control::Gate,
            ] {
                performance.push(Dial {
                    track: track.id,
                    control,
                    clips: clips.clone(),
                });
            }
        }
    }
    let mut rng = Rng::stream(settings.seed, &["reference_match".into()]);
    for dials in [&mut mix, &mut performance] {
        for i in (1..dials.len()).rev() {
            dials.swap(i, rng.below(i + 1));
        }
    }
    // Alternate families so a short budget can listen to both kinds of adjustment.
    let mut dials = Vec::with_capacity(mix.len() + performance.len());
    for i in 0..mix.len().max(performance.len()) {
        if let Some(dial) = mix.get(i) {
            dials.push(dial.clone());
        }
        if let Some(dial) = performance.get(i) {
            dials.push(dial.clone());
        }
    }
    dials
}

impl Dial {
    fn adjust(&self, project: &mut Project, original: &Project, direction: f32) {
        let base = original.track(self.track).expect("captured track");
        let track = project.track_mut(self.track).expect("captured track");
        match self.control {
            Control::Gain => {
                track.mixer.gain_db = bounded(
                    track.mixer.gain_db,
                    base.mixer.gain_db,
                    direction,
                    1.5,
                    3.0,
                    -60.0,
                    12.0,
                )
            }
            Control::Pan => {
                track.mixer.pan = bounded(
                    track.mixer.pan,
                    base.mixer.pan,
                    direction,
                    0.15,
                    0.3,
                    -1.0,
                    1.0,
                )
            }
            control => {
                let instrument = track
                    .kind
                    .as_instrument_mut()
                    .expect("instrument performance");
                let source = base.kind.as_instrument().expect("instrument performance");
                // The render retains the complete project. Only the clip identities captured
                // during excerpt enumeration may receive this performance proposal.
                for id in &self.clips {
                    let Some(clip) = instrument.clips.iter_mut().find(|clip| clip.id == *id) else {
                        continue;
                    };
                    let Some(original) = source.clips.iter().find(|clip| clip.id == *id) else {
                        continue;
                    };
                    adjust_performance(clip, original, control, direction);
                }
            }
        }
    }
}

fn bounded(
    value: f32,
    original: f32,
    direction: f32,
    step: f32,
    radius: f32,
    min: f32,
    max: f32,
) -> f32 {
    let low = (original - radius).max(min).min(original);
    let high = (original + radius).min(max).max(original);
    let next = (value + direction * step).clamp(low, high);
    if next == value {
        (value - direction * step).clamp(low, high)
    } else {
        next
    }
}

fn expression(clip: &MidiClip) -> Expression {
    clip.transforms
        .iter()
        .rev()
        .find_map(|transform| match transform {
            NoteTransform::Expression { settings } => Some(settings.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn gate(clip: &MidiClip) -> f32 {
    clip.transforms
        .iter()
        .rev()
        .find_map(|transform| match transform {
            NoteTransform::Gate { amount } => Some(*amount),
            _ => None,
        })
        .unwrap_or(1.0)
}

fn adjust_performance(clip: &mut MidiClip, original: &MidiClip, control: Control, direction: f32) {
    if matches!(control, Control::Gate) {
        if !gate(clip).is_finite() || !gate(original).is_finite() {
            return;
        }
        let amount = bounded(gate(clip), gate(original), direction, 0.05, 0.1, 0.05, 1.0);
        if let Some(NoteTransform::Gate { amount: value }) = clip
            .transforms
            .iter_mut()
            .rev()
            .find(|t| matches!(t, NoteTransform::Gate { .. }))
        {
            *value = amount;
        } else if amount != 1.0 {
            clip.transforms.push(NoteTransform::Gate { amount });
        }
        if amount == 1.0
            && !original
                .transforms
                .iter()
                .any(|t| matches!(t, NoteTransform::Gate { .. }))
        {
            clip.transforms
                .retain(|t| !matches!(t, NoteTransform::Gate { .. }));
        }
        return;
    }
    let mut settings = expression(clip);
    let base = expression(original);
    let (value, original_value, step, radius, min, max) = match control {
        Control::Timing => (&mut settings.timing, base.timing, 0.1, 0.2, 0.0, 1.0),
        Control::Velocity => (&mut settings.velocity, base.velocity, 0.1, 0.2, 0.0, 1.0),
        Control::Swell => (&mut settings.swell, base.swell, 0.1, 0.2, 0.0, 1.0),
        Control::Accent => (&mut settings.accent, base.accent, 0.15, 0.3, -1.0, 1.0),
        Control::Delay => (&mut settings.delay_ms, base.delay_ms, 4.0, 8.0, -50.0, 50.0),
        _ => unreachable!("mix and gate have their own controls"),
    };
    if !value.is_finite() || !original_value.is_finite() {
        return;
    }
    *value = bounded(*value, original_value, direction, step, radius, min, max);
    let had_expression = original
        .transforms
        .iter()
        .any(|t| matches!(t, NoteTransform::Expression { .. }));
    if !had_expression && settings == Expression::default() {
        clip.transforms
            .retain(|t| !matches!(t, NoteTransform::Expression { .. }));
    } else if let Some(NoteTransform::Expression { settings: value }) = clip
        .transforms
        .iter_mut()
        .rev()
        .find(|t| matches!(t, NoteTransform::Expression { .. }))
    {
        *value = settings;
    } else {
        clip.transforms.push(NoteTransform::Expression { settings });
    }
}

fn describe_changes(original: &Project, best: &Project) -> Vec<String> {
    let mut changes = Vec::new();
    for (before, after) in original.tracks.iter().zip(&best.tracks) {
        if before.mixer.gain_db != after.mixer.gain_db {
            changes.push(format!(
                "{}: gain {:+.2} → {:+.2} dB",
                before.name, before.mixer.gain_db, after.mixer.gain_db
            ));
        }
        if before.mixer.pan != after.mixer.pan {
            changes.push(format!(
                "{}: pan {:+.2} → {:+.2}",
                before.name, before.mixer.pan, after.mixer.pan
            ));
        }
        if let (Some(before), Some(after)) =
            (before.kind.as_instrument(), after.kind.as_instrument())
        {
            for (before, after) in before.clips.iter().zip(&after.clips) {
                let a = expression(before);
                let b = expression(after);
                for (name, was, now) in [
                    ("timing", a.timing, b.timing),
                    ("velocity wander", a.velocity, b.velocity),
                    ("swell", a.swell, b.swell),
                    ("accent", a.accent, b.accent),
                    ("delay (ms)", a.delay_ms, b.delay_ms),
                    ("gate", gate(before), gate(after)),
                ] {
                    if was != now {
                        changes.push(format!("{}: {name} {was:+.2} → {now:+.2}", before.name));
                    }
                }
            }
        }
    }
    changes
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod failure_tests;
