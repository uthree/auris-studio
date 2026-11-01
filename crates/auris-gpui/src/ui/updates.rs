//! Background update flow and its desktop feedback.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use auris_i18n::Key;
use gpui::{AnyElement, Context, IntoElement, Window, div, prelude::*};

use crate::app::AurisApp;
use crate::ui::prompt::{PendingAction, Prompt, PromptBody, Question};
use crate::update_check::{
    ReadyUpdate, ReleaseAsset, UpdateOutcome, UpdateStatus, check_latest_release, download_update,
    installed_target, prune_update_folder, take_update_result,
};

/// One release operation at a time, with a verified package retained until quit.
#[derive(Debug)]
pub(crate) enum UpdatePhase {
    Idle,
    Checking {
        manual: bool,
    },
    Available {
        current: String,
        latest: String,
    },
    Failed,
    Downloading {
        version: String,
        total: u64,
        received: Arc<AtomicU64>,
        cancelled: Arc<AtomicBool>,
    },
    Ready(ReadyUpdate),
}

impl AurisApp {
    /// Only update dialogs belong above the start screen; editor sheets remain with the editor.
    pub(crate) fn startup_update_prompt_open(&self) -> bool {
        self.prompt
            .as_ref()
            .is_some_and(|prompt| match &prompt.body {
                PromptBody::Ask(
                    Question::RestartForUpdate { .. } | Question::OpenUpdateRelease { .. },
                ) => true,
                PromptBody::Notice(_) => prompt.title.as_ref() == self.t(Key::CmdCheckForUpdates),
                _ => false,
            })
    }

    /// Changes the background update preference and discards pending work when it is disabled.
    pub(crate) fn apply_auto_update(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let was_enabled = self.settings.auto_update;
        self.settings.auto_update = enabled;
        if !enabled {
            let was_updating = !matches!(self.update_phase, UpdatePhase::Idle);
            if let UpdatePhase::Downloading { cancelled, .. } = &self.update_phase {
                cancelled.store(true, Ordering::Relaxed);
            }
            self.update_generation = self.update_generation.wrapping_add(1);
            self.update_phase = UpdatePhase::Idle;
            self.restart_after_update = false;
            if matches!(
                self.prompt.as_ref().map(|prompt| &prompt.body),
                Some(PromptBody::Ask(Question::RestartForUpdate { .. }))
            ) {
                self.cancel_prompt();
            }
            if was_updating {
                self.set_status("");
            }
        }
        if let Err(error) = self.save_settings() {
            log::warn!("could not save settings: {error}");
        }
        if enabled && !was_enabled {
            self.check_for_updates_mode(false, cx);
        }
        cx.notify();
    }

    /// Starts one quiet check shortly after launch and repeats while the app remains open.
    pub(crate) fn start_auto_updates(&mut self, cx: &mut Context<Self>) {
        cx.background_executor()
            .spawn(async { prune_update_folder() })
            .detach();
        if let Some(UpdateOutcome::Failure(detail)) = take_update_result() {
            log::warn!("previous update failed: {detail}");
            let message = self.t(Key::UpdateInstallFailed).to_owned();
            self.set_failed_status(&message);
            if self.prompt.is_none() {
                self.open_prompt(Prompt::notice(
                    self.t(Key::CmdCheckForUpdates),
                    [message.into()],
                ));
            }
            cx.notify();
        }
        cx.spawn(async move |this, cx| {
            let mut delay = Duration::from_secs(1);
            loop {
                cx.background_executor().timer(delay).await;
                if this
                    .update(cx, |this, cx| {
                        if this.settings.auto_update {
                            this.check_for_updates_mode(false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
                delay = Duration::from_secs(6 * 60 * 60);
            }
        })
        .detach();
    }

    pub(crate) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.check_for_updates_mode(true, cx);
    }

    fn check_for_updates_mode(&mut self, manual: bool, cx: &mut Context<Self>) {
        if let UpdatePhase::Ready(update) = &self.update_phase {
            if manual {
                let version = update.latest.clone();
                self.show_ready_prompt(version);
                cx.notify();
            }
            return;
        }
        if let UpdatePhase::Checking { manual: was_manual } = &mut self.update_phase {
            if manual && !*was_manual {
                *was_manual = true;
                self.set_status(self.t(Key::UpdateChecking));
                cx.notify();
            }
            return;
        }
        if matches!(self.update_phase, UpdatePhase::Downloading { .. }) {
            return;
        }
        self.update_phase = UpdatePhase::Checking { manual };
        self.update_generation = self.update_generation.wrapping_add(1);
        let generation = self.update_generation;
        if manual {
            self.set_status(self.t(Key::UpdateChecking));
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { check_latest_release() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_update_check(result, generation, cx)
            });
        })
        .detach();
    }

    fn finish_update_check(
        &mut self,
        result: Result<UpdateStatus, crate::update_check::CheckError>,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        if self.update_generation != generation {
            return;
        }
        let UpdatePhase::Checking { manual } = &self.update_phase else {
            return;
        };
        let manual = *manual;
        self.update_phase = UpdatePhase::Idle;
        let title = self.t(Key::CmdCheckForUpdates).to_owned();
        match result {
            Ok(UpdateStatus::Available {
                current,
                latest,
                asset,
            }) => {
                if let (Some(asset), Some(target)) = (asset, installed_target()) {
                    self.start_update_download(asset, latest, target, manual, cx);
                    return;
                }
                let message = self
                    .t(Key::UpdateAvailable)
                    .replace("{current}", &current)
                    .replace("{version}", &latest);
                self.set_status(&message);
                self.update_phase = UpdatePhase::Available {
                    current: current.clone(),
                    latest: latest.clone(),
                };
                if manual && self.prompt.is_none() {
                    self.open_prompt(Prompt::ask(
                        title,
                        Question::OpenUpdateRelease { current, latest },
                    ));
                }
            }
            Ok(UpdateStatus::Current(version)) if manual => {
                let message = self.t(Key::UpdateCurrent).replace("{version}", &version);
                self.set_status(&message);
                if self.prompt.is_none() {
                    self.open_prompt(Prompt::notice(title, [message.into()]));
                }
            }
            Ok(UpdateStatus::NoRelease) if manual => {
                let message = self.t(Key::UpdateNoRelease).to_owned();
                self.set_status(&message);
                if self.prompt.is_none() {
                    self.open_prompt(Prompt::notice(title, [message.into()]));
                }
            }
            Ok(_) => {}
            Err(error) => {
                log::warn!("update check failed: {error}");
                if manual {
                    let message = self.t(Key::UpdateFailed).to_owned();
                    self.set_failed_status(&message);
                    if self.prompt.is_none() {
                        self.open_prompt(Prompt::notice(title, [message.into()]));
                    }
                }
            }
        }
        cx.notify();
    }

    fn start_update_download(
        &mut self,
        asset: ReleaseAsset,
        latest: String,
        target: crate::update_check::InstallTarget,
        manual: bool,
        cx: &mut Context<Self>,
    ) {
        self.update_generation = self.update_generation.wrapping_add(1);
        let generation = self.update_generation;
        let received = Arc::new(AtomicU64::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        self.update_phase = UpdatePhase::Downloading {
            version: latest.clone(),
            total: asset.size,
            received: Arc::clone(&received),
            cancelled: Arc::clone(&cancelled),
        };
        self.set_status(
            self.t(Key::UpdateDownloading)
                .replace("{version}", &latest)
                .replace("{percent}", "0"),
        );
        cx.notify();
        let worker_received = Arc::clone(&received);
        let worker_cancelled = Arc::clone(&cancelled);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    download_update(&asset, &latest, target, &worker_received, &worker_cancelled)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.update_generation != generation
                    || !matches!(this.update_phase, UpdatePhase::Downloading { .. })
                {
                    return;
                }
                match result {
                    Ok(ready) => {
                        let version = ready.latest.clone();
                        this.update_phase = UpdatePhase::Ready(ready);
                        this.set_status(this.t(Key::UpdateReady).replace("{version}", &version));
                        this.show_ready_prompt(version);
                    }
                    Err(error) => {
                        log::warn!("update download failed: {error}");
                        this.update_phase = UpdatePhase::Failed;
                        this.set_failed_status(this.t(Key::UpdateDownloadFailed));
                        if manual {
                            this.set_failed_status(this.t(Key::UpdateDownloadFailed));
                            if this.prompt.is_none() {
                                this.open_prompt(Prompt::notice(
                                    this.t(Key::CmdCheckForUpdates),
                                    [this.t(Key::UpdateDownloadFailed).into()],
                                ));
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let keep_polling = this
                    .update(cx, |this, cx| {
                        if this.update_generation != generation {
                            return false;
                        }
                        let UpdatePhase::Downloading {
                            version,
                            total,
                            received,
                            ..
                        } = &this.update_phase
                        else {
                            return false;
                        };
                        let percent = received.load(Ordering::Relaxed).saturating_mul(100) / *total;
                        let message = this
                            .t(Key::UpdateDownloading)
                            .replace("{version}", version)
                            .replace("{percent}", &percent.to_string());
                        if this.status != message {
                            this.set_status(message);
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_polling {
                    break;
                }
            }
        })
        .detach();
    }

    fn show_ready_prompt(&mut self, version: String) {
        if self.prompt.is_none() {
            self.open_prompt(Prompt::ask(
                self.t(Key::CmdCheckForUpdates),
                Question::RestartForUpdate { version },
            ));
        }
    }

    /// Release guidance stays visible on the start screen even after a dialog is dismissed.
    pub(crate) fn render_startup_update(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, String)> {
        let (message, action) = match &self.update_phase {
            UpdatePhase::Idle => return None,
            UpdatePhase::Checking { .. } => (self.t(Key::UpdateChecking).to_owned(), None),
            UpdatePhase::Available { current, latest } => (
                self.t(Key::UpdateAvailable)
                    .replace("{current}", current)
                    .replace("{version}", latest),
                Some(
                    atelier::Button::new("startup-update-release", self.t(Key::UpdateOpenRelease))
                        .on_click(|_, _, cx| cx.open_url(crate::update_check::RELEASE_PAGE)),
                ),
            ),
            UpdatePhase::Downloading {
                version,
                total,
                received,
                ..
            } => (
                self.t(Key::UpdateDownloading)
                    .replace("{version}", version)
                    .replace(
                        "{percent}",
                        &(received.load(Ordering::Relaxed).saturating_mul(100) / (*total).max(1))
                            .to_string(),
                    ),
                None,
            ),
            UpdatePhase::Ready(update) => (
                self.t(Key::UpdateReady)
                    .replace("{version}", &update.latest),
                Some(
                    atelier::Button::new("startup-update-restart", self.t(Key::UpdateRestartNow))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.restart_for_update(window, cx)),
                        ),
                ),
            ),
            UpdatePhase::Failed => (
                self.t(Key::UpdateDownloadFailed).to_owned(),
                Some(
                    atelier::Button::new("startup-update-retry", self.t(Key::CmdCheckForUpdates))
                        .on_click(cx.listener(|this, _, _, cx| this.check_for_updates(cx))),
                ),
            ),
        };
        let theme = &self.theme;
        let notice = div()
            .id("startup-update")
            .debug_selector(|| "startup-update".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(theme.atelier.spacing.sm)
            .px(theme.atelier.spacing.xxl)
            .pb(theme.atelier.spacing.md)
            .text_size(theme.atelier.typography.size_sm)
            .text_color(theme.text)
            .child(message.clone())
            .children(action)
            .into_any_element();
        Some((notice, message))
    }

    /// Use the same save guard as Quit before requesting an update restart.
    pub(crate) fn restart_for_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.update_phase, UpdatePhase::Ready(_)) {
            return;
        }
        if crate::project_windows::installed(cx) {
            crate::project_windows::request_restart(self.project_window, cx);
            return;
        }
        crate::termination::activate(window, cx);

        if self.confirm_discard(PendingAction::RestartForUpdate) {
            self.complete_update_restart(cx);
        }
        cx.notify();
    }

    pub(crate) fn complete_update_restart(&mut self, cx: &mut Context<Self>) {
        if matches!(self.update_phase, UpdatePhase::Ready(_)) {
            self.restart_after_update = true;
            self.save_window_placement();
            crate::termination::quit(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, open, paint, resize};
    use gpui::{TestAppContext, px, size};

    #[gpui::test]
    fn startup_shows_a_release_fallback_and_keeps_project_actions_reachable(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.update_phase = UpdatePhase::Checking { manual: false };
                this.finish_update_check(
                    Ok(UpdateStatus::Available {
                        current: "0.15.1".into(),
                        latest: "0.16.0".into(),
                        asset: None,
                    }),
                    this.update_generation,
                    cx,
                );
                assert!(this.prompt.is_none());
            });
        });
        resize(&app, cx, size(px(720.0), px(600.0)));
        assert!(cx.debug_bounds("startup-update-release").is_some());
        click("startup-update-release", cx);
        assert_eq!(
            cx.opened_url().as_deref(),
            Some(crate::update_check::RELEASE_PAGE)
        );
        click("chooser-confirm", cx);
        app.read_with(cx, |this, _| assert!(this.project_chooser.is_none()));
    }

    #[gpui::test]
    fn ready_update_prompt_can_be_dismissed_on_both_start_screens(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| this.show_project_chooser(window, cx));
        });
        for workshop in [false, true] {
            cx.update(|window, cx| {
                app.update(cx, |this, cx| {
                    if workshop {
                        this.open_song_workshop(window, cx);
                    }
                    this.update_phase = UpdatePhase::Ready(ReadyUpdate {
                        latest: "0.16.0".into(),
                        package: "unused-package".into(),
                        sha256: "unused".into(),
                        target: crate::update_check::InstallTarget {
                            app: "unused-app".into(),
                            helper: "unused-helper".into(),
                        },
                    });
                    this.show_ready_prompt("0.16.0".into());
                    cx.notify();
                });
            });
            paint(&app, cx);
            assert!(cx.debug_bounds("prompt-ok").is_some());
            cx.simulate_keystrokes("escape");
            paint(&app, cx);
            app.read_with(cx, |this, _| {
                assert!(this.prompt.is_none());
                assert!(matches!(this.update_phase, UpdatePhase::Ready(_)));
                assert!(!this.restart_after_update);
            });
            if !workshop {
                assert!(cx.debug_bounds("startup-update-restart").is_some());
            }
        }
    }

    #[gpui::test]
    fn quiet_offline_checks_and_stale_results_do_not_interrupt_startup(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            this.update_phase = UpdatePhase::Checking { manual: false };
            this.finish_update_check(
                Err(crate::update_check::CheckError::Request("offline".into())),
                this.update_generation,
                cx,
            );
            assert!(this.prompt.is_none());
            assert!(matches!(this.update_phase, UpdatePhase::Idle));
            this.update_generation = this.update_generation.wrapping_add(1);
            this.finish_update_check(
                Ok(UpdateStatus::Available {
                    current: "0.15.1".into(),
                    latest: "0.16.0".into(),
                    asset: None,
                }),
                this.update_generation.wrapping_sub(1),
                cx,
            );
            assert!(matches!(this.update_phase, UpdatePhase::Idle));
        });
    }
}
