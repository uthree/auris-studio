//! Background search of a song-sheet snapshot, with explicit adoption of its exact winner.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

use auris_i18n::Key;
use auris_session::composition_search::{
    CandidateOutcome, SearchResult, TerminationReason, search_composition_with_progress,
};
use auris_session::prelude::{Composition, SongSpec};
use gpui::Context;

use crate::app::AurisApp;
use crate::ui::compose_sheet::{SearchSettings, SearchViewStatus, song_spec};

#[derive(Clone)]
struct SearchSnapshot {
    revision: u64,
    song: SongSpec,
    settings: SearchSettings,
}

struct RunningSearch {
    generation: u64,
    snapshot: SearchSnapshot,
    cancel: Arc<AtomicBool>,
    completed: Arc<AtomicUsize>,
    best_density: Arc<AtomicU64>,
}

struct CompletedSearch {
    snapshot: SearchSnapshot,
    result: SearchResult<SongSpec, Composition>,
}

/// Search settings and one window's independent task/result lifetime.
#[derive(Default)]
pub(crate) struct CompositionSearchState {
    /// Whether the search controls are disclosed in the song sheet.
    pub(crate) open: bool,
    /// Parameters for the next search; captured separately by every active run.
    pub(crate) settings: SearchSettings,
    initialized: bool,
    generation: u64,
    running: Option<RunningSearch>,
    result: Option<CompletedSearch>,
    error: Option<String>,
}

impl CompositionSearchState {
    /// Abandons a result while retaining the worker's slot until its current attempt finishes.
    pub(crate) fn dismiss(&mut self) {
        if let Some(run) = &self.running {
            run.cancel.store(true, Ordering::Relaxed);
        }
        self.generation = self.generation.wrapping_add(1);
        self.result = None;
        self.error = None;
        self.open = false;
        self.initialized = false;
    }
}

impl AurisApp {
    /// Discloses search beside the ordinary song settings.
    pub(crate) fn toggle_song_search(&mut self, cx: &mut Context<Self>) {
        let Some(dials) = self.song_sheet.as_ref() else {
            return;
        };
        if !self.composition_search.initialized {
            self.composition_search.settings = SearchSettings::for_song(&song_spec(dials));
            self.composition_search.initialized = true;
        }
        self.composition_search.open = !self.composition_search.open;
        cx.notify();
    }

    fn search_snapshot_is_current(&self, snapshot: &SearchSnapshot) -> bool {
        self.session.revision() == snapshot.revision
            && self.composition_search.settings == snapshot.settings
            && self
                .song_sheet
                .as_ref()
                .is_some_and(|dials| song_spec(dials) == snapshot.song)
    }

    /// Cancels stale work on the regular repaint tick without changing the document.
    pub(crate) fn poll_song_search(&mut self) {
        if self.song_sheet.is_none() {
            let state = &self.composition_search;
            if state.initialized
                || state.open
                || state.result.is_some()
                || state.error.is_some()
                || state
                    .running
                    .as_ref()
                    .is_some_and(|run| run.generation == state.generation)
            {
                self.composition_search.dismiss();
            }
            return;
        }
        if self.composition_search.initialized
            && let Some(dials) = &self.song_sheet
        {
            self.composition_search
                .settings
                .reconcile(&song_spec(dials));
        }
        let stale = self.composition_search.running.as_ref().is_some_and(|run| {
            run.generation == self.composition_search.generation
                && !self.search_snapshot_is_current(&run.snapshot)
        });
        if stale {
            let run = self
                .composition_search
                .running
                .as_ref()
                .expect("a stale task exists");
            run.cancel.store(true, Ordering::Relaxed);
            // Invalidate this result once, even if the sheet is later changed back before the
            // cancelled attempt finishes. Keep its separate worker token until completion.
            self.composition_search.generation = self.composition_search.generation.wrapping_add(1);
            self.composition_search.error = Some(self.t(Key::SongSearchChanged).into());
        }
    }

    /// Starts one bounded run on a worker and keeps the original arrangement untouched.
    pub(crate) fn start_song_search(&mut self, cx: &mut Context<Self>) -> bool {
        if self.composition_search.running.is_some() || self.compose_progress.is_some() {
            return false;
        }
        let Some(dials) = self.song_sheet.as_ref() else {
            return false;
        };
        let song = song_spec(dials);
        self.composition_search.settings.reconcile(&song);
        let request = self.composition_search.settings.to_request(&song);
        let validation = request
            .validate()
            .map_err(|error| error.to_string())
            .and_then(|()| {
                self.session
                    .validate_song_lyrics(&song)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = validation {
            self.composition_search.error = Some(error);
            cx.notify();
            return false;
        }
        let snapshot = SearchSnapshot {
            revision: self.session.revision(),
            song,
            settings: self.composition_search.settings.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicUsize::new(0));
        let best_density = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
        self.composition_search.generation = self.composition_search.generation.wrapping_add(1);
        let generation = self.composition_search.generation;
        self.composition_search.result = None;
        self.composition_search.error = None;
        self.composition_search.running = Some(RunningSearch {
            generation,
            snapshot: snapshot.clone(),
            cancel: Arc::clone(&cancel),
            completed: Arc::clone(&completed),
            best_density: Arc::clone(&best_density),
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut best_fitness = f64::NEG_INFINITY;
                    search_composition_with_progress(
                        &request,
                        || cancel.load(Ordering::Relaxed),
                        |attempt| {
                            if let CandidateOutcome::Success(evaluation) = &attempt.outcome
                                && evaluation.fitness > best_fitness
                            {
                                best_fitness = evaluation.fitness;
                                if let Some(metric) = evaluation
                                    .metrics
                                    .iter()
                                    .find(|metric| metric.name == "notes_per_bar")
                                {
                                    best_density.store(metric.value.to_bits(), Ordering::Relaxed);
                                }
                            }
                            completed.fetch_add(1, Ordering::Relaxed);
                        },
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Cancellation is cooperative: the old composition may still be finishing
                // after its sheet closes. Only its own completion can release this worker slot.
                if this
                    .composition_search
                    .running
                    .as_ref()
                    .is_none_or(|run| run.generation != generation)
                {
                    return;
                }
                this.composition_search.running = None;
                if this.composition_search.generation != generation {
                    cx.notify();
                    return;
                }
                if !this.search_snapshot_is_current(&snapshot) {
                    this.composition_search.error = Some(this.t(Key::SongSearchChanged).into());
                } else {
                    this.composition_search.error = None;
                    match result {
                        Ok(result) => {
                            this.composition_search.result =
                                Some(CompletedSearch { snapshot, result })
                        }
                        Err(error) => this.composition_search.error = Some(error.to_string()),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        true
    }

    /// Stops after the current attempt and keeps any valid partial winner available.
    pub(crate) fn cancel_song_search(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &self.composition_search.running {
            run.cancel.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    /// Hands the evaluated score to the normal undoable composition/playback pipeline.
    pub(crate) fn apply_song_search(&mut self, cx: &mut Context<Self>) -> bool {
        if self.composition_search.running.is_some() || self.compose_progress.is_some() {
            return false;
        }
        let Some(completed) = &self.composition_search.result else {
            return false;
        };
        if !self.search_snapshot_is_current(&completed.snapshot) {
            self.composition_search.error = Some(self.t(Key::SongSearchChanged).into());
            cx.notify();
            return false;
        }
        let Some(best) = &completed.result.best else {
            return false;
        };
        self.compose_generated_score(best.score.clone(), true, true, cx)
    }

    /// A presentation snapshot with no score cloning or mutation of task state.
    pub(crate) fn song_search_view_status(&self) -> SearchViewStatus {
        let state = &self.composition_search;
        let running = state.running.as_ref();
        let completed = state.result.as_ref();
        let best = completed.and_then(|completed| completed.result.best.as_ref());
        let best_density = running
            .and_then(|run| {
                let density = f64::from_bits(run.best_density.load(Ordering::Relaxed));
                density.is_finite().then_some(density)
            })
            .or_else(|| {
                best.and_then(|best| {
                    best.evaluation
                        .metrics
                        .iter()
                        .find(|metric| metric.name == "notes_per_bar")
                        .map(|metric| metric.value)
                })
            });
        let snapshot = running
            .map(|run| &run.snapshot)
            .or_else(|| completed.map(|run| &run.snapshot));
        let stale = snapshot.is_some_and(|snapshot| !self.search_snapshot_is_current(snapshot));
        SearchViewStatus {
            running: running.is_some(),
            cancelling: running.is_some_and(|run| run.cancel.load(Ordering::Relaxed)),
            completed: running.map_or_else(
                || completed.map_or(0, |run| run.result.history.len()),
                |run| run.completed.load(Ordering::Relaxed),
            ),
            budget: snapshot.map_or(state.settings.attempt_budget, |snapshot| {
                snapshot.settings.attempt_budget
            }),
            best_density,
            target: snapshot.map(|snapshot| snapshot.settings.target_notes_per_bar),
            best_candidate: best.map(|best| best.candidate.id),
            cancelled: completed
                .is_some_and(|run| run.result.termination == TerminationReason::Cancelled),
            stale,
            error: state.error.clone(),
            can_apply: running.is_none()
                && best.is_some()
                && !stale
                && self.compose_progress.is_none(),
        }
    }
}

#[cfg(test)]
#[path = "song_search_tests.rs"]
mod tests;
