//! Reference-audio comparison of detached project renders, followed by explicit adoption.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use auris_i18n::Key;
use auris_session::audio_evaluation::{AudioEvaluator, ReferenceAudioEvaluator};
use auris_session::clap_evaluation::{ClapAudioEvaluator, ClapTarget};
use auris_session::prelude::AudioBuffer;
use auris_session::{ReferenceMatchReport, ReferenceMatchSettings, ReferenceMatchStep};
use gpui::Context;

use crate::app::AurisApp;
use crate::i18n::error_text;
use crate::ui::prompt::{Prompt, PromptTarget};

#[cfg(test)]
#[path = "reference_match_tests.rs"]
mod tests;
#[path = "reference_match_view.rs"]
mod view;

struct ReferenceSource {
    path: PathBuf,
    audio: Arc<AudioBuffer>,
}

/// The fixed audio objective used throughout one pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MatchObjective {
    #[default]
    AcousticReference,
    ClapReference,
    ClapText,
}

impl MatchObjective {
    fn needs_reference(self) -> bool {
        self != Self::ClapText
    }
    fn uses_clap(self) -> bool {
        self != Self::AcousticReference
    }
    fn label(self) -> Key {
        match self {
            Self::AcousticReference => Key::AudioMatchAcoustic,
            Self::ClapReference => Key::AudioMatchClapReference,
            Self::ClapText => Key::AudioMatchClapText,
        }
    }
}

#[derive(Clone)]
struct MatchSnapshot {
    generation: u64,
    revision: u64,
    settings: ReferenceMatchSettings,
    reference_start: f64,
    reference: Option<Arc<AudioBuffer>>,
    objective: MatchObjective,
    model_directory: Option<PathBuf>,
    text_prompt: String,
}

struct MatchControl {
    snapshot: MatchSnapshot,
    cancel: Arc<AtomicBool>,
    completed: Arc<AtomicUsize>,
    fraction: Arc<AtomicU32>,
    prepared: Arc<AtomicBool>,
}

struct MatchComparison {
    snapshot: MatchSnapshot,
    report: ReferenceMatchReport,
}

struct MatchPreview {
    selection: usize,
    // No ticket yet means the worker is preparing the selected excerpt.
    status: Option<auris_session::OutputPreviewStatus>,
}

/// One window's reference file, controls and worker lifetime.
pub(crate) struct ReferenceMatchState {
    /// Whether the reference-matching sheet claims the screen and keyboard.
    pub(crate) open: bool,
    settings: ReferenceMatchSettings,
    reference_start: f64,
    objective: MatchObjective,
    model_directory: Option<PathBuf>,
    text_prompt: String,
    source: Option<ReferenceSource>,
    loading: Option<u64>,
    generation: u64,
    running: Option<MatchControl>,
    comparison: Option<MatchComparison>,
    error: Option<String>,
    cancelled: bool,
    preview: Option<MatchPreview>,
    preview_generation: u64,
}

impl Default for ReferenceMatchState {
    fn default() -> Self {
        Self {
            open: false,
            settings: ReferenceMatchSettings {
                project_start_seconds: 0.0,
                duration_seconds: 12.0,
                attempts: 32,
                seed: 42,
                mix: true,
                performance: true,
                generation_seeds: true,
                instruments: true,
                arrangement: true,
            },
            reference_start: 0.0,
            objective: MatchObjective::default(),
            model_directory: None,
            text_prompt: String::new(),
            source: None,
            loading: None,
            generation: 0,
            running: None,
            comparison: None,
            error: None,
            cancelled: false,
            preview: None,
            preview_generation: 0,
        }
    }
}

impl ReferenceMatchState {
    /// Requests cancellation while retaining the worker's slot until it returns.
    pub(crate) fn cancel(&self) {
        if let Some(run) = &self.running {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn busy(&self) -> bool {
        self.running.is_some() || self.loading.is_some()
    }

    fn input_problem(&self) -> Option<Key> {
        if self.objective.uses_clap() && self.model_directory.is_none() {
            Some(Key::AudioMatchModelRequired)
        } else if self.objective == MatchObjective::ClapText && self.text_prompt.trim().is_empty() {
            Some(Key::AudioMatchPromptRequired)
        } else if self.objective.needs_reference() && self.source.is_none() {
            Some(Key::ReferenceMatchMissing)
        } else if !self.settings.mix
            && !self.settings.performance
            && !self.settings.generation_seeds
            && !self.settings.instruments
            && !self.settings.arrangement
        {
            Some(Key::ReferenceMatchNeedScope)
        } else {
            None
        }
    }
}

/// Builds a fixed evaluator on a worker; native models never load in a UI callback.
fn prepare_evaluator(
    snapshot: &MatchSnapshot,
    cancel: Arc<AtomicBool>,
    language: auris_i18n::Language,
) -> Result<Arc<dyn AudioEvaluator>, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Key::SongSearchCancelled.get(language).into());
    }
    let excerpt = || {
        let reference = snapshot
            .reference
            .as_ref()
            .ok_or_else(|| Key::ReferenceMatchMissing.get(language).to_string())?;
        reference_excerpt(
            reference,
            snapshot.reference_start,
            snapshot.settings.duration_seconds,
        )
        .ok_or_else(|| Key::ReferenceMatchShort.get(language).to_string())
    };
    match snapshot.objective {
        MatchObjective::AcousticReference => {
            Ok(Arc::new(ReferenceAudioEvaluator::new(excerpt()?.as_ref())?))
        }
        MatchObjective::ClapReference | MatchObjective::ClapText => {
            let directory = snapshot
                .model_directory
                .as_ref()
                .ok_or_else(|| Key::AudioMatchModelRequired.get(language).to_string())?;
            let target = if snapshot.objective == MatchObjective::ClapText {
                if snapshot.text_prompt.trim().is_empty() {
                    return Err(Key::AudioMatchPromptRequired.get(language).into());
                }
                ClapTarget::Text(snapshot.text_prompt.clone())
            } else {
                ClapTarget::Audio(excerpt()?)
            };
            Ok(Arc::new(ClapAudioEvaluator::load(
                directory, target, cancel,
            )?))
        }
    }
}

/// Copies an exact reference excerpt; callers perform this allocation on a worker.
fn reference_excerpt(audio: &AudioBuffer, start: f64, duration: f64) -> Option<Arc<AudioBuffer>> {
    if !start.is_finite() || start < 0.0 || !duration.is_finite() || duration <= 0.0 {
        return None;
    }
    let from = (start * audio.sample_rate()).round() as usize;
    let frames = (duration * audio.sample_rate()).round() as usize;
    let end = from.checked_add(frames)?;
    if frames == 0 || end > audio.frame_count() {
        return None;
    }
    let channels = audio
        .iter_channels()
        .map(|channel| channel[from..end].to_vec())
        .collect();
    AudioBuffer::from_planar(channels, audio.sample_rate())
        .ok()
        .map(Arc::new)
}

impl AurisApp {
    /// Opens matching on the current project, without composing or importing a reference track.
    pub(crate) fn open_reference_match(&mut self, cx: &mut Context<Self>) {
        if self.compose_progress.is_some() {
            return;
        }
        if !self.reference_match.open
            && self.reference_match.comparison.is_none()
            && !self.reference_match.busy()
        {
            self.reference_match.settings.project_start_seconds = self
                .project()
                .tempo_map
                .ticks_to_seconds(self.playhead_ticks())
                .0;
        }
        self.reference_match.open = true;
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// Closes the sheet, stops preview audio, and prevents late worker results from reopening it.
    pub(crate) fn close_reference_match(&mut self, cx: &mut Context<Self>) {
        self.reference_match.cancel();
        if self.prompt.as_ref().and_then(Prompt::target) == Some(PromptTarget::AudioMatchText) {
            self.cancel_prompt();
        }
        self.reference_match.open = false;
        self.reference_match.generation = self.reference_match.generation.wrapping_add(1);
        self.reference_match.comparison = None;
        self.stop_reference_preview();
        cx.notify();
    }

    fn match_snapshot_is_current(&self, snapshot: &MatchSnapshot) -> bool {
        self.reference_match.open
            && snapshot.generation == self.reference_match.generation
            && snapshot.revision == self.session.revision()
            && snapshot.settings == self.reference_match.settings
            && snapshot.reference_start == self.reference_match.reference_start
            && snapshot.objective == self.reference_match.objective
            && snapshot.model_directory == self.reference_match.model_directory
            && snapshot.text_prompt == self.reference_match.text_prompt
            && match (&snapshot.reference, &self.reference_match.source) {
                (Some(reference), Some(source)) => Arc::ptr_eq(&source.audio, reference),
                (None, None) => true,
                _ => false,
            }
    }

    fn owns_match_worker(&self, generation: u64) -> bool {
        self.reference_match
            .running
            .as_ref()
            .is_some_and(|run| run.snapshot.generation == generation)
    }

    /// Retires obsolete work and preview audio during normal repaint housekeeping.
    pub(crate) fn poll_reference_match(&mut self) {
        let stale = self
            .reference_match
            .running
            .as_ref()
            .is_some_and(|run| !self.match_snapshot_is_current(&run.snapshot));
        if stale {
            self.reference_match.cancel();
        }
        let stale_comparison = self
            .reference_match
            .comparison
            .as_ref()
            .is_some_and(|result| !self.match_snapshot_is_current(&result.snapshot));
        if (stale || stale_comparison) && self.reference_match.preview.is_some() {
            self.stop_reference_preview();
        }
        if self
            .reference_match
            .preview
            .as_ref()
            .is_some_and(|preview| {
                preview
                    .status
                    .as_ref()
                    .is_some_and(|status| !status.is_active())
            })
        {
            self.reference_match.preview = None;
        }
    }

    fn change_reference_settings(&mut self, update: impl FnOnce(&mut ReferenceMatchState)) {
        let before = (
            self.reference_match.settings.clone(),
            self.reference_match.reference_start,
            self.reference_match.objective,
            self.reference_match.model_directory.clone(),
            self.reference_match.text_prompt.clone(),
        );
        update(&mut self.reference_match);
        if before
            != (
                self.reference_match.settings.clone(),
                self.reference_match.reference_start,
                self.reference_match.objective,
                self.reference_match.model_directory.clone(),
                self.reference_match.text_prompt.clone(),
            )
        {
            self.reference_match.generation = self.reference_match.generation.wrapping_add(1);
            self.reference_match.cancel();
            self.reference_match.error = None;
            self.stop_reference_preview();
        }
    }

    fn choose_audio_match_model(&mut self, cx: &mut Context<Self>) {
        if self.reference_match.busy() {
            return;
        }
        let generation = self.reference_match.generation;
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let directory = rfd::AsyncFileDialog::new()
                .set_title(Key::AudioMatchChooseModel.get(language))
                .pick_folder()
                .await;
            if let Some(directory) = directory {
                let _ = this.update(cx, |this, cx| {
                    if this.reference_match.open && this.reference_match.generation == generation {
                        this.change_reference_settings(|state| {
                            state.model_directory = Some(directory.path().to_path_buf())
                        });
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn edit_audio_match_prompt(&mut self) {
        if !self.reference_match.busy() {
            self.open_prompt(Prompt::new(
                self.t(Key::AudioMatchPrompt),
                PromptTarget::AudioMatchText,
                self.reference_match.text_prompt.clone(),
            ));
        }
    }

    /// Accepts text from the shared editable prompt without altering the document.
    pub(crate) fn set_audio_match_prompt(&mut self, text: String) {
        if self.reference_match.open {
            self.change_reference_settings(|state| state.text_prompt = text);
        }
    }

    fn choose_reference_audio(&mut self, cx: &mut Context<Self>) {
        if self.reference_match.busy() {
            return;
        }
        let generation = self.reference_match.generation;
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let handle = rfd::AsyncFileDialog::new()
                .set_title(Key::ReferenceMatchChoose.get(language))
                .add_filter(
                    Key::ReferenceMatchSource.get(language),
                    auris_session::supported_audio_extensions(),
                )
                .pick_file()
                .await;
            if let Some(handle) = handle {
                let _ = this.update(cx, |this, cx| {
                    if this.reference_match.open && this.reference_match.generation == generation {
                        this.load_reference_audio(handle.path().to_path_buf(), cx);
                    }
                });
            }
        })
        .detach();
    }

    fn load_reference_audio(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.reference_match.busy() {
            return;
        }
        self.stop_reference_preview();
        self.reference_match.generation = self.reference_match.generation.wrapping_add(1);
        let generation = self.reference_match.generation;
        self.reference_match.loading = Some(generation);
        self.reference_match.error = None;
        self.reference_match.comparison = None;
        cx.spawn(async move |this, cx| {
            let shown = path.clone();
            let decoded = cx
                .background_executor()
                .spawn(async move { auris_session::decode_audio(&path, 44_100.0) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.reference_match.loading != Some(generation) {
                    return;
                }
                this.reference_match.loading = None;
                if !this.reference_match.open || this.reference_match.generation != generation {
                    return;
                }
                match decoded {
                    Ok(audio) => {
                        this.reference_match.source = Some(ReferenceSource {
                            path: shown,
                            audio: Arc::new(audio),
                        });
                        this.reference_match.reference_start = 0.0;
                    }
                    Err(error) => {
                        this.reference_match.error = Some(error_text(&error, this.language()))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Captures reference and document identities before preparing any detached render.
    fn start_reference_match(&mut self, cx: &mut Context<Self>) -> bool {
        if self.reference_match.busy() || self.compose_progress.is_some() {
            return false;
        }
        if let Some(problem) = self.reference_match.input_problem() {
            self.reference_match.error = Some(self.t(problem).into());
            cx.notify();
            return false;
        }
        let reference = self
            .reference_match
            .source
            .as_ref()
            .map(|source| Arc::clone(&source.audio));
        self.stop_reference_preview();
        self.reference_match.generation = self.reference_match.generation.wrapping_add(1);
        let snapshot = MatchSnapshot {
            generation: self.reference_match.generation,
            revision: self.session.revision(),
            settings: self.reference_match.settings.clone(),
            reference_start: self.reference_match.reference_start,
            reference,
            objective: self.reference_match.objective,
            model_directory: self.reference_match.model_directory.clone(),
            text_prompt: self.reference_match.text_prompt.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicUsize::new(0));
        let fraction = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let prepared = Arc::new(AtomicBool::new(false));
        self.reference_match.running = Some(MatchControl {
            snapshot: snapshot.clone(),
            cancel: Arc::clone(&cancel),
            completed: Arc::clone(&completed),
            fraction: Arc::clone(&fraction),
            prepared: Arc::clone(&prepared),
        });
        self.reference_match.comparison = None;
        self.reference_match.error = None;
        self.reference_match.cancelled = false;
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let mut snapshot = snapshot;
            let preparation = snapshot.clone();
            let preparation_cancel = Arc::clone(&cancel);
            let evaluator = cx
                .background_executor()
                .spawn(async move { prepare_evaluator(&preparation, preparation_cancel, language) })
                .await;
            let job = this
                .update(cx, |this, cx| {
                    if !this.owns_match_worker(snapshot.generation) {
                        return None;
                    }
                    if cancel.load(Ordering::Relaxed) || !this.match_snapshot_is_current(&snapshot)
                    {
                        this.reference_match.running = None;
                        this.reference_match.cancelled = true;
                        cx.notify();
                        return None;
                    }
                    let job = evaluator.and_then(|evaluator| {
                        this.session
                            .begin_reference_match(snapshot.settings.clone(), evaluator)
                            .map_err(|error| error_text(&error, this.language()))
                    });
                    match job {
                        Ok(job) => {
                            prepared.store(true, Ordering::Relaxed);
                            snapshot.revision = this.session.revision();
                            if let Some(run) = &mut this.reference_match.running {
                                run.snapshot = snapshot.clone();
                            }
                            Some(job)
                        }
                        Err(error) => {
                            this.reference_match.error = Some(error);
                            this.reference_match.running = None;
                            cx.notify();
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some(mut job) = job else {
                return;
            };
            loop {
                completed.store(job.progress().completed, Ordering::Relaxed);
                fraction.store(
                    (job.progress().completed as f32 / snapshot.settings.attempts as f32).to_bits(),
                    Ordering::Relaxed,
                );
                let worker_cancel = Arc::clone(&cancel);
                let worker_fraction = Arc::clone(&fraction);
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        job.run(&worker_cancel, &mut |value| {
                            worker_fraction.store(value.to_bits(), Ordering::Relaxed)
                        })
                    })
                    .await;
                let next = this
                    .update(cx, |this, cx| {
                        if !this.owns_match_worker(snapshot.generation) {
                            return None;
                        }
                        if !this.match_snapshot_is_current(&snapshot) {
                            this.reference_match.running = None;
                            if this.reference_match.open {
                                this.reference_match.error =
                                    Some(this.t(Key::ReferenceMatchChanged).into());
                            }
                            cx.notify();
                            return None;
                        }
                        let next = match result
                            .and_then(|result| this.session.continue_reference_match(result))
                        {
                            Ok(ReferenceMatchStep::Pending(next)) => {
                                snapshot.revision = this.session.revision();
                                if let Some(run) = &mut this.reference_match.running {
                                    run.snapshot = snapshot.clone();
                                }
                                Some(next)
                            }
                            Ok(ReferenceMatchStep::Complete(report)) => {
                                this.reference_match.cancelled = report.cancelled;
                                this.reference_match.comparison = Some(MatchComparison {
                                    snapshot: snapshot.clone(),
                                    report,
                                });
                                this.reference_match.running = None;
                                None
                            }
                            Err(error) => {
                                this.reference_match.running = None;
                                if cancel.load(Ordering::Relaxed) {
                                    this.reference_match.cancelled = true;
                                } else {
                                    this.reference_match.error =
                                        Some(error_text(&error, this.language()));
                                }
                                None
                            }
                        };
                        cx.notify();
                        next
                    })
                    .ok()
                    .flatten();
                let Some(next) = next else {
                    break;
                };
                job = next;
            }
        })
        .detach();
        cx.notify();
        true
    }

    fn stop_reference_preview(&mut self) {
        if let Some(status) = self
            .reference_match
            .preview
            .as_ref()
            .and_then(|preview| preview.status.as_ref())
        {
            status.stop();
        }
        self.session.stop_output_preview();
        self.reference_match.preview = None;
        self.reference_match.preview_generation =
            self.reference_match.preview_generation.wrapping_add(1);
    }

    fn preview_reference_match(&mut self, selection: usize, cx: &mut Context<Self>) {
        let state = &self.reference_match;
        if !state.open || selection > 2 || (selection == 0 && !state.objective.needs_reference()) {
            return;
        }
        if selection > 0
            && state
                .comparison
                .as_ref()
                .is_none_or(|result| !self.match_snapshot_is_current(&result.snapshot))
        {
            return;
        }
        let audio = match selection {
            0 => state
                .source
                .as_ref()
                .map(|source| Arc::clone(&source.audio)),
            1 => state
                .comparison
                .as_ref()
                .map(|result| Arc::clone(&result.report.baseline_audio)),
            _ => state
                .comparison
                .as_ref()
                .map(|result| Arc::clone(&result.report.best_audio)),
        };
        let Some(audio) = audio else {
            return;
        };
        let generation = state.generation;
        let revision = self.session.revision();
        let start = state.reference_start;
        let duration = state.settings.duration_seconds;
        let rate = self.session.sample_rate();
        let language = self.language();
        self.stop_reference_preview();
        let preview_generation = self.reference_match.preview_generation;
        self.reference_match.error = None;
        self.reference_match.preview = Some(MatchPreview {
            selection,
            status: None,
        });
        cx.spawn(async move |this, cx| {
            let prepared = cx
                .background_executor()
                .spawn(async move {
                    let audio = if selection == 0 {
                        reference_excerpt(&audio, start, duration)
                            .ok_or_else(|| Key::ReferenceMatchShort.get(language).to_string())?
                    } else {
                        audio
                    };
                    auris_session::prepare_output_preview(&audio, rate)
                        .map_err(|error| error_text(&error, language))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.reference_match.open
                    || this.reference_match.generation != generation
                    || this.reference_match.preview_generation != preview_generation
                    || this.session.revision() != revision
                {
                    if this.reference_match.preview_generation == preview_generation {
                        this.reference_match.preview = None;
                        cx.notify();
                    }
                    return;
                }
                let result = prepared.and_then(|buffer| {
                    this.session
                        .play_output_preview(buffer)
                        .map_err(|error| error_text(&error, this.language()))
                });
                match result {
                    Ok(status) => {
                        this.reference_match.preview = Some(MatchPreview {
                            selection,
                            status: Some(status),
                        });
                    }
                    Err(error) => {
                        this.reference_match.preview = None;
                        this.reference_match.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_reference_match(&mut self, cx: &mut Context<Self>) -> bool {
        if self.reference_match.busy() {
            return false;
        }
        let Some(comparison) = self.reference_match.comparison.as_ref() else {
            return false;
        };
        if !self.match_snapshot_is_current(&comparison.snapshot) {
            self.reference_match.error = Some(self.t(Key::ReferenceMatchChanged).into());
            cx.notify();
            return false;
        }
        let result = self.session.apply_reference_match(&comparison.report);
        self.stop_reference_preview();
        match result {
            Ok(changed) => {
                self.close_reference_match(cx);
                self.set_status(self.t(if changed {
                    Key::ReferenceMatchApplied
                } else {
                    Key::ReferenceMatchUnchanged
                }));
                cx.notify();
                changed
            }
            Err(error) => {
                self.reference_match.error = Some(error_text(&error, self.language()));
                cx.notify();
                false
            }
        }
    }
}
