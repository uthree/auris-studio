//! The song writer and measured mix, with the expensive work off the window thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use auris_i18n::{Key, messages};
use auris_session::prelude::*;
use auris_session::{ComposeBalancePhase, ComposeBalanceStep, ComposeReport};
use gpui::Context;

use crate::app::AurisApp;
use crate::ui::compose_progress::ComposeProgressState;
use crate::ui::prompt::Prompt;

impl AurisApp {
    /// Starts writing a specification, returning whether the request was accepted.
    ///
    /// The sheet stays alive until success so a refused source does not discard its settings.
    /// Hosted plugin construction stays on its owning thread; writing, rendering and measuring
    /// run on workers. Balancing belongs to the same undo step as adopting the score.
    pub(crate) fn compose_spec(
        &mut self,
        spec: &SongSpec,
        close_sheet: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.compose_progress.is_some() {
            return false;
        }
        if self.choosing_export || self.export.as_ref().is_some_and(|job| job.result.is_none()) {
            self.set_status(self.t(Key::ExportAlreadyRunning));
            return false;
        }
        if self.session.is_recording() {
            self.finish_recording();
        }
        self.cancel_auto_sing();
        self.reset_drum_analysis();
        self.stop_audition();
        self.clear_chord_preview();
        self.session.release_typed_notes();
        self.session.stop();
        self.menu = None;
        self.close_menu_bar();
        let revision = self.session.revision();
        let spec = spec.clone();
        self.compose_progress = Some(ComposeProgressState {
            stage: Key::ComposeProgressWriting,
            detail: None,
            progress: None,
            started_at: Instant::now(),
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            let piece = cx
                .background_executor()
                .spawn(async move { compose(&spec) })
                .await;
            let seed = piece.seed;
            if this
                .update(cx, |this, cx| {
                    if let Some(state) = this.compose_progress.as_mut() {
                        state.stage = Key::ComposeProgressPreparing;
                    }
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            // Yield before the owner-thread handoff, so the preparation stage can be drawn.
            cx.background_executor()
                .timer(std::time::Duration::ZERO)
                .await;
            let adopted = this.update(cx, |this, cx| {
                if this.session.revision() != revision {
                    this.reject_composition(this.t(Key::ComposeProgressChanged).to_string(), cx);
                    return None;
                }
                match this.session.compose_without_balance(&piece) {
                    Ok(report) => {
                        this.resync_selection();
                        this.reset_view();
                        let first = this.project().tracks.first().map(|track| track.id);
                        this.selected_track = None;
                        if let Some(track) = first {
                            this.select_track(track);
                        }
                        Some((report, this.session.begin_composed_balance()))
                    }
                    Err(error) => {
                        this.reject_composition(this.failure(Key::CmdComposeSong, &error), cx);
                        None
                    }
                }
            });
            let Ok(Some((mut report, mut next))) = adopted else {
                return;
            };
            let progress = Arc::new(AtomicU32::new(0));
            while let Some(job) = next {
                let pass = job.progress();
                if this
                    .update(cx, |this, cx| {
                        let name = match pass.phase {
                            ComposeBalancePhase::Track(name) => name,
                            ComposeBalancePhase::Mix => this.t(Key::ComposeProgressMix).to_string(),
                            ComposeBalancePhase::Verification => {
                                this.t(Key::ComposeProgressVerification).to_string()
                            }
                        };
                        if let Some(state) = this.compose_progress.as_mut() {
                            state.stage = Key::ComposeProgressBalancing;
                            state.detail =
                                Some(format!("{} / {} · {name}", pass.completed + 1, pass.total));
                            state.progress = Some(Arc::clone(&progress));
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                let worker_progress = Arc::clone(&progress);
                let measured = cx
                    .background_executor()
                    .spawn(async move {
                        let mut update = |fraction: f32| {
                            worker_progress.store(fraction.to_bits(), Ordering::Relaxed);
                        };
                        job.run(&mut RenderProgress::reporting(&mut update))
                    })
                    .await;
                let continued = this.update(cx, |this, _| {
                    measured.and_then(|result| this.session.continue_composed_balance(result))
                });
                match continued {
                    Ok(Ok(ComposeBalanceStep::Pending(job))) => next = Some(job),
                    Ok(Ok(ComposeBalanceStep::Complete(balance))) => {
                        report.balance = Some(balance);
                        next = None;
                    }
                    Ok(Err(auris_session::SessionError::StaleBalance)) => {
                        let _ = this.update(cx, |this, cx| {
                            this.reject_composition(this.t(Key::BalanceChanged).to_string(), cx);
                        });
                        return;
                    }
                    Ok(Err(error)) => {
                        let _ = this.update(cx, |this, cx| {
                            this.finish_composition(report, seed, close_sheet, cx);
                            let message = format!(
                                "{}\n{}",
                                this.t(Key::ComposeProgressBalanceFailed),
                                this.failure(Key::CmdBalanceLevels, &error)
                            );
                            this.set_failed_status(message.clone());
                            this.open_prompt(Prompt::notice(
                                this.t(Key::CmdComposeSong),
                                [message.into()],
                            ));
                            cx.notify();
                        });
                        return;
                    }
                    Err(_) => return,
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.finish_composition(report, seed, close_sheet, cx);
            });
        })
        .detach();
        true
    }

    /// Dismisses the progress layer while preserving the draft after a failed adoption.
    fn reject_composition(&mut self, message: String, cx: &mut Context<Self>) {
        self.compose_progress = None;
        self.set_failed_status(message.clone());
        self.open_prompt(Prompt::notice(
            self.t(Key::CmdComposeSong),
            [message.into()],
        ));
        cx.notify();
    }

    /// Completes either entry point with one consistent status and sheet lifetime.
    fn finish_composition(
        &mut self,
        report: ComposeReport,
        seed: u64,
        close_sheet: bool,
        cx: &mut Context<Self>,
    ) {
        self.compose_progress = None;
        if close_sheet {
            self.song_sheet = None;
            self.lyrics_edit = None;
            self.song_library = None;
        }
        let language = self.language();
        let written = if report.substituted.is_empty() {
            messages::composed_document(language, report.tracks, report.notes, seed)
        } else {
            messages::composed_document_substituted(
                language,
                report.tracks,
                report.notes,
                seed,
                report.substituted.len(),
            )
        };
        self.set_status(match report.balance.as_ref().and_then(|it| it.now_lufs) {
            Some(lufs) => format!("{written} · {}", messages::mixed_to(language, lufs)),
            None => written,
        });
        cx.notify();
    }
}

#[cfg(test)]
#[path = "compose_job_tests.rs"]
mod tests;
