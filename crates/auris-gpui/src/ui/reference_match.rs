//! Reference-audio comparison of detached project renders, followed by explicit adoption.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use auris_i18n::Key;
use auris_session::audio_evaluation::ReferenceAudioEvaluator;
use auris_session::prelude::AudioBuffer;
use auris_session::{ReferenceMatchReport, ReferenceMatchSettings, ReferenceMatchStep};
use gpui::Context;

use crate::app::AurisApp;
use crate::i18n::error_text;

#[cfg(test)]
#[path = "reference_match_tests.rs"]
mod tests;
#[path = "reference_match_view.rs"]
mod view;

struct ReferenceSource {
    path: PathBuf,
    audio: Arc<AudioBuffer>,
}

#[derive(Clone)]
struct MatchSnapshot {
    generation: u64,
    revision: u64,
    settings: ReferenceMatchSettings,
    reference_start: f64,
    reference: Arc<AudioBuffer>,
}

struct MatchControl {
    snapshot: MatchSnapshot,
    cancel: Arc<AtomicBool>,
    completed: Arc<AtomicUsize>,
    fraction: Arc<AtomicU32>,
}

struct MatchComparison {
    snapshot: MatchSnapshot,
    report: ReferenceMatchReport,
}

/// One window's reference file, controls and worker lifetime.
pub(crate) struct ReferenceMatchState {
    /// Whether the reference-matching sheet claims the screen and keyboard.
    pub(crate) open: bool,
    settings: ReferenceMatchSettings,
    reference_start: f64,
    source: Option<ReferenceSource>,
    loading: Option<u64>,
    generation: u64,
    running: Option<MatchControl>,
    comparison: Option<MatchComparison>,
    error: Option<String>,
    cancelled: bool,
    previewing: bool,
    preview_generation: u64,
}

impl Default for ReferenceMatchState {
    fn default() -> Self {
        Self {
            open: false,
            settings: ReferenceMatchSettings {
                project_start_seconds: 0.0,
                duration_seconds: 12.0,
                attempts: 8,
                seed: 42,
                mix: true,
                performance: true,
            },
            reference_start: 0.0,
            source: None,
            loading: None,
            generation: 0,
            running: None,
            comparison: None,
            error: None,
            cancelled: false,
            previewing: false,
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
            && self
                .reference_match
                .source
                .as_ref()
                .is_some_and(|source| Arc::ptr_eq(&source.audio, &snapshot.reference))
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
        if (stale || stale_comparison) && self.reference_match.previewing {
            self.stop_reference_preview();
        }
    }

    fn change_reference_settings(&mut self, update: impl FnOnce(&mut ReferenceMatchState)) {
        let before = (
            self.reference_match.settings.clone(),
            self.reference_match.reference_start,
        );
        update(&mut self.reference_match);
        if before
            != (
                self.reference_match.settings.clone(),
                self.reference_match.reference_start,
            )
        {
            self.reference_match.generation = self.reference_match.generation.wrapping_add(1);
            self.reference_match.cancel();
            self.reference_match.error = None;
            self.stop_reference_preview();
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
        let Some(source) = &self.reference_match.source else {
            self.reference_match.error = Some(self.t(Key::ReferenceMatchMissing).into());
            cx.notify();
            return false;
        };
        if !self.reference_match.settings.mix && !self.reference_match.settings.performance {
            self.reference_match.error = Some(self.t(Key::ReferenceMatchNeedScope).into());
            cx.notify();
            return false;
        }
        let reference = Arc::clone(&source.audio);
        self.stop_reference_preview();
        self.reference_match.generation = self.reference_match.generation.wrapping_add(1);
        let snapshot = MatchSnapshot {
            generation: self.reference_match.generation,
            revision: self.session.revision(),
            settings: self.reference_match.settings.clone(),
            reference_start: self.reference_match.reference_start,
            reference,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicUsize::new(0));
        let fraction = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        self.reference_match.running = Some(MatchControl {
            snapshot: snapshot.clone(),
            cancel: Arc::clone(&cancel),
            completed: Arc::clone(&completed),
            fraction: Arc::clone(&fraction),
        });
        self.reference_match.comparison = None;
        self.reference_match.error = None;
        self.reference_match.cancelled = false;
        let language = self.language();
        cx.spawn(async move |this, cx| {
            let mut snapshot = snapshot;
            let preparation = snapshot.clone();
            let prepared = cx
                .background_executor()
                .spawn(async move {
                    let excerpt = reference_excerpt(
                        &preparation.reference,
                        preparation.reference_start,
                        preparation.settings.duration_seconds,
                    )
                    .ok_or_else(|| Key::ReferenceMatchShort.get(language).to_string())?;
                    ReferenceAudioEvaluator::new(&excerpt)
                })
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
                    let job = prepared.and_then(|evaluator| {
                        this.session
                            .begin_reference_match(snapshot.settings.clone(), Arc::new(evaluator))
                            .map_err(|error| error_text(&error, this.language()))
                    });
                    match job {
                        Ok(job) => {
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
        self.session.stop_output_preview();
        self.reference_match.previewing = false;
        self.reference_match.preview_generation =
            self.reference_match.preview_generation.wrapping_add(1);
    }

    fn preview_reference_match(&mut self, selection: usize, cx: &mut Context<Self>) {
        let state = &self.reference_match;
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
                    return;
                }
                let result = prepared.and_then(|buffer| {
                    this.session
                        .play_output_preview(buffer)
                        .map_err(|error| error_text(&error, this.language()))
                });
                match result {
                    Ok(()) => this.reference_match.previewing = true,
                    Err(error) => this.reference_match.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
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
                self.reference_match.comparison = None;
                self.reference_match.generation = self.reference_match.generation.wrapping_add(1);
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
