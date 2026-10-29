//! Application-wide project routing and exclusive realtime audio ownership.

use std::path::PathBuf;

use gpui::{App, AppContext, Context, Global, WindowHandle, WindowId, WindowOptions, px, size};

use crate::app::{AurisApp, Pane};
use crate::ui::prompt::PendingAction;

pub(crate) enum AuditionRequest {
    Notes {
        track: auris_session::prelude::TrackId,
        pitches: Vec<u8>,
        velocity: f32,
        note_index: Option<usize>,
    },
    Selection(auris_session::NoteSelectionPreview),
}

#[derive(Default)]
pub(crate) struct ProjectWindows {
    approved: Vec<(WindowId, u64)>,
    restart: Option<WindowId>,
}

impl Global for ProjectWindows {}

pub(crate) fn installed(cx: &App) -> bool {
    cx.has_global::<ProjectWindows>()
}

fn windows(cx: &App) -> Vec<WindowHandle<AurisApp>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<AurisApp>())
        .collect()
}

pub(crate) fn install(cx: &mut App) {
    cx.set_global(ProjectWindows::default());
    cx.on_window_closed(|cx| {
        if windows(cx).is_empty() {
            crate::termination::quit(cx);
        }
    })
    .detach();
}

pub(crate) fn open(path: Option<PathBuf>, cx: &mut App) {
    // Opening draws synchronously. Defer until the invoking document is no longer leased.
    cx.defer(move |cx| {
        if let Some(path) = path.as_ref() {
            let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            for handle in windows(cx) {
                let found = handle.update(cx, |view, window, _| {
                    let found = view.session.path().is_some_and(|existing| {
                        std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf())
                            == identity
                    }) || view.opening_project.as_ref() == Some(&identity);
                    if found {
                        window.activate_window();
                    }
                    found
                });
                if matches!(found, Ok(true)) {
                    return;
                }
            }
            for handle in windows(cx) {
                let reused = handle.update(cx, |view, window, cx| {
                    if view.project_chooser.is_some()
                        && view.song_workshop.is_none()
                        && view.opening_project.is_none()
                    {
                        window.activate_window();
                        view.open_project_at(path.clone(), cx);
                        true
                    } else {
                        false
                    }
                });
                if matches!(reused, Ok(true)) {
                    return;
                }
            }
        }
        match create(cx) {
            Ok(handle) => {
                if let Some(path) = path {
                    let _ = handle.update(cx, |view, _, cx| view.open_project_at(path, cx));
                } else {
                    let _ =
                        handle.update(cx, |view, window, cx| view.show_project_chooser(window, cx));
                }
            }
            Err(error) => log::error!("could not open project window: {error}"),
        }
    });
}

fn apply_window_theme(window: &mut gpui::Window, cx: &App) {
    let theme = &cx.global::<crate::theme::Theme>().atelier;
    if cfg!(test) {
        // The headless platform has no native window handle for Atelier's OS backdrop.
        window.set_background_appearance(theme.window_background());
    } else {
        atelier::set_window_background(window, theme);
    }
}

/// Opens a start screen showing its setup guide, for a request made where none is showing.
pub(crate) fn open_setup_guide(cx: &mut App) {
    match create(cx) {
        Ok(handle) => {
            let _ = handle.update(cx, |view, window, cx| {
                view.show_project_chooser(window, cx);
                if let Some(chooser) = &mut view.project_chooser {
                    chooser.page = crate::ui::project_chooser::ChooserPage::Setup;
                }
            });
        }
        Err(error) => log::error!("could not open project window: {error}"),
    }
}

pub(crate) fn create(cx: &mut App) -> Result<WindowHandle<AurisApp>, String> {
    let remembered = auris_session::Settings::load().window;
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(crate::opening_window_bounds(remembered, cx)),
                window_min_size: Some(size(px(640.0), px(480.0))),
                focus: true,
                window_background: cx
                    .global::<crate::theme::Theme>()
                    .atelier
                    .window_background(),
                ..crate::titlebar::options("Auris Studio")
            },
            |window, cx| {
                let view = cx.new(AurisApp::new);
                apply_window_theme(window, cx);
                window
                    .observe_global::<crate::theme::Theme>(cx, |window, cx| {
                        apply_window_theme(window, cx);
                    })
                    .detach();
                view
            },
        )
        .map_err(|error| error.to_string())?;
    handle
        .update(cx, |view, window, cx| {
            view.project_window = Some(window.window_handle().window_id());
            let _ = view.session.set_audio_suspended(true);
            view.focus_pane(Pane::Arrangement, window);
            let asked = cx.entity().downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                asked
                    .update(cx, |view, cx| {
                        if view.compose_progress.is_some() {
                            return false;
                        }
                        let close = view.confirm_discard(PendingAction::CloseWindow);
                        if close {
                            view.save_window_placement();
                        }
                        cx.notify();
                        close
                    })
                    .unwrap_or(true)
            });
        })
        .map_err(|error| error.to_string())?;
    Ok(handle)
}

pub(crate) fn request_quit(cx: &mut App) {
    cx.global_mut::<ProjectWindows>().approved.clear();
    cx.global_mut::<ProjectWindows>().restart = None;
    advance_quit(cx);
}

pub(crate) fn request_restart(owner: Option<WindowId>, cx: &mut App) {
    cx.global_mut::<ProjectWindows>().approved.clear();
    cx.global_mut::<ProjectWindows>().restart = owner;
    advance_quit(cx);
}

fn advance_quit(cx: &mut App) {
    cx.defer(|cx| {
        for handle in windows(cx) {
            let id = handle.window_id();
            let approved = cx.global::<ProjectWindows>().approved.clone();
            let ready = handle.update(cx, |view, window, cx| {
                if approved.contains(&(id, view.session.revision())) {
                    return true;
                }
                crate::termination::activate(window, cx);
                if view.compose_progress.is_some() {
                    return false;
                }
                if view.confirm_discard(PendingAction::Quit) {
                    view.save_window_placement();
                    cx.global_mut::<ProjectWindows>()
                        .approved
                        .push((id, view.session.revision()));
                    true
                } else {
                    cx.notify();
                    false
                }
            });
            if !matches!(ready, Ok(true)) {
                return;
            }
        }
        if let Some(owner) = cx.global::<ProjectWindows>().restart {
            for handle in windows(cx) {
                if handle.window_id() == owner {
                    let _ = handle.update(cx, |view, _, _| view.restart_after_update = true);
                }
            }
        }
        crate::termination::quit(cx);
    });
}

/// Whether a project window other than `own` was told to reopen after the update is applied.
///
/// `own` is skipped because it is the window being updated when this is asked, and so cannot be
/// read. Without the window registry there is only the one window, and nothing else to ask.
pub(crate) fn restart_requested_elsewhere(own: Option<WindowId>, cx: &App) -> bool {
    installed(cx)
        && windows(cx)
            .into_iter()
            .filter(|handle| Some(handle.window_id()) != own)
            .any(|handle| handle.read(cx).is_ok_and(|view| view.restart_after_update))
}

impl AurisApp {
    pub(crate) fn refresh_shared_settings(&mut self) {
        let placement = self.settings.window;
        let mut local = self.settings.clone();
        local.window = self.settings_baseline.borrow().window;
        if local != *self.settings_baseline.borrow() {
            return;
        }
        let Ok(settings) = auris_session::Settings::load_for_update() else {
            return;
        };
        if self.settings == settings {
            return;
        }
        self.language = settings.language();
        if self.settings.autosave != settings.autosave {
            self.session.set_autosave(settings.autosave && !cfg!(test));
        }
        if self.settings.audio != settings.audio
            && !self.session.is_recording()
            && let Err(error) = self.session.set_audio_preferences(settings.audio.clone())
        {
            self.report_session_error(&error);
        }
        self.session.set_plugin_paths(&settings.plugin_paths);
        if self.settings.singer_acceleration != settings.singer_acceleration {
            self.session
                .set_singer_acceleration(settings.singer_acceleration);
        }
        if self.settings.japanese_dictionary != settings.japanese_dictionary {
            let _ = self
                .session
                .set_japanese_dictionary(settings.japanese_dictionary.as_deref());
        }
        if !self.agent_operation_busy() && self.agent_chat.preferences() == self.settings.agent {
            self.agent_chat.load_preferences(&settings.agent);
        }
        *self.settings_baseline.borrow_mut() = settings.clone();
        self.settings = settings;
        self.settings.window = placement;
    }

    pub(crate) fn poll_pending_audition(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.pending_audition.take() else {
            return;
        };
        if !self.acquire_audio(cx) {
            self.stop_audition();
            return;
        }
        match request {
            AuditionRequest::Notes {
                track,
                pitches,
                velocity,
                note_index,
            } => self.sound(track, pitches, velocity, note_index),
            AuditionRequest::Selection(preview) => {
                self.session.play_note_selection_preview(&preview)
            }
        }
    }

    pub(crate) fn settings_saver(
        &self,
    ) -> impl FnOnce(&auris_session::Settings) -> Result<(), auris_session::SessionError> + use<>
    {
        let baseline = self.settings_baseline.clone();
        move |settings| persist_settings(settings, &baseline)
    }

    pub(crate) fn save_settings(&self) -> Result<(), auris_session::SessionError> {
        persist_settings(&self.settings, &self.settings_baseline)
    }

    pub(crate) fn open_project_window(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if installed(cx) {
            open(Some(path), cx);
        } else if self.confirm_discard(PendingAction::OpenDropped(path.clone())) {
            self.open_project_at(path, cx);
        }
    }

    pub(crate) fn approve_quit(&mut self, cx: &mut Context<Self>) {
        if !installed(cx) {
            crate::termination::quit(cx);
            return;
        }
        if let Some(id) = self.project_window {
            cx.global_mut::<ProjectWindows>()
                .approved
                .push((id, self.session.revision()));
        }
        advance_quit(cx);
    }

    /// Switch only for an explicit audio command, never because focus changed.
    pub(crate) fn acquire_audio(&mut self, cx: &mut Context<Self>) -> bool {
        if !installed(cx) || !self.session.audio_suspended() {
            return true;
        }
        for handle in windows(cx) {
            if Some(handle.window_id()) == self.project_window {
                continue;
            }
            let result = handle.update(cx, |view, _, cx| {
                let was_suspended = view.session.audio_suspended();
                let result = view.session.set_audio_suspended(true);
                if result.is_ok() {
                    view.stop_audition();
                    view.release_physical_preview();
                    view.clear_chord_preview();
                    if !was_suspended {
                        view.set_status(view.t(auris_i18n::Key::ProjectAudioSuspended));
                    }
                }
                cx.notify();
                result
            });
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let message =
                        if matches!(error, auris_session::SessionError::RecordingInProgress) {
                            self.t(auris_i18n::Key::OtherProjectRecording).to_string()
                        } else {
                            crate::i18n::error_text(&error, self.language())
                        };
                    self.set_failed_status(message);
                    return false;
                }
                Err(error) => {
                    log::warn!("could not transfer realtime audio: {error}");
                    return false;
                }
            }
        }
        // Preferences may have been edited in another project's settings window.
        let preferences = auris_session::Settings::load().audio;
        let result = self
            .session
            .set_audio_preferences(preferences)
            .and_then(|()| self.session.set_audio_suspended(false));
        match result {
            Ok(()) => {
                self.set_status(crate::app::audio_line(
                    &self.session.audio_status(),
                    self.language(),
                ));
                true
            }
            Err(error) => {
                self.report_session_error(&error);
                false
            }
        }
    }
}

pub(crate) fn persist_settings(
    settings: &auris_session::Settings,
    baseline: &std::cell::RefCell<auris_session::Settings>,
) -> Result<(), auris_session::SessionError> {
    settings.save_changes(&baseline.borrow())?;
    *baseline.borrow_mut() = settings.clone();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn projects_keep_independent_documents_and_transfer_audio(cx: &mut gpui::TestAppContext) {
        let (first, cx) = crate::harness::open(cx);
        let second = cx.update(|window, cx| {
            install(cx);
            first.update(cx, |view, _| {
                view.project_window = Some(window.window_handle().window_id());
                view.session.set_audio_suspended(true).unwrap();
                view.session.add_audio_track("First");
            });
            create(cx).unwrap()
        });
        cx.cx.update(|cx| {
            second
                .update(cx, |view, _, cx| {
                    view.session.add_audio_track("Second");
                    assert!(view.acquire_audio(cx));
                })
                .unwrap();
            first.update(cx, |view, cx| {
                assert!(view.session.audio_suspended());
                assert!(view.acquire_audio(cx));
                assert_eq!(view.project().tracks.len(), 2);
                assert_eq!(view.project().tracks[1].name, "First");
            });
            second
                .update(cx, |view, _, _| {
                    assert!(view.session.audio_suspended());
                    assert_eq!(view.project().tracks.len(), 2);
                    assert_eq!(view.project().tracks[1].name, "Second");
                    view.session.undo();
                    assert_eq!(view.project().tracks.len(), 1);
                })
                .unwrap();
            assert_eq!(first.read(cx).project().tracks.len(), 2);
        });
    }

    #[gpui::test]
    fn a_restart_chosen_in_another_window_is_seen_by_this_ones_quit(cx: &mut gpui::TestAppContext) {
        let (first, cx) = crate::harness::open(cx);
        let own = cx.update(|window, cx| {
            install(cx);
            let own = window.window_handle().window_id();
            first.update(cx, |view, _| view.project_window = Some(own));
            own
        });
        let second = cx.cx.update(|cx| create(cx).unwrap());
        cx.cx.update(|cx| {
            assert!(!restart_requested_elsewhere(Some(own), cx));
            second
                .update(cx, |view, _, _| view.restart_after_update = true)
                .unwrap();
            assert!(restart_requested_elsewhere(Some(own), cx));
            // The asking window's own flag is its caller's to read, not this function's.
            let other = second.window_id();
            assert!(!restart_requested_elsewhere(Some(other), cx));
        });
    }

    #[gpui::test]
    fn closing_one_project_keeps_the_other_window(cx: &mut gpui::TestAppContext) {
        let (first, cx) = crate::harness::open(cx);
        let second = cx.cx.update(|cx| {
            install(cx);
            create(cx).unwrap()
        });
        cx.cx.update(|cx| {
            second
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        });
        cx.run_until_parked();
        cx.cx.update(|cx| {
            assert_eq!(windows(cx).len(), 1);
            assert_eq!(first.read(cx).project().tracks.len(), 1);
        });
    }

    #[gpui::test]
    fn opening_the_same_project_activates_its_existing_window(cx: &mut gpui::TestAppContext) {
        let (first, cx) = crate::harness::open(cx);
        let directory = tempfile::tempdir().unwrap();
        let path = cx.update(|window, cx| {
            install(cx);
            first.update(cx, |view, _| {
                view.project_window = Some(window.window_handle().window_id());
                view.session
                    .save_as(&directory.path().join("Same.auris"))
                    .unwrap();
                view.session.add_audio_track("Unsaved edits");
                view.session.path().unwrap().to_path_buf()
            })
        });
        cx.cx.update(|cx| {
            open(Some(path.clone()), cx);
            open(Some(path), cx);
        });
        cx.run_until_parked();
        cx.cx.update(|cx| {
            assert_eq!(windows(cx).len(), 1);
            assert!(first.read(cx).session.is_dirty());
            assert_eq!(first.read(cx).project().tracks.len(), 2);
        });
    }

    #[gpui::test]
    fn quit_checks_other_dirty_projects_before_closing_any_window(cx: &mut gpui::TestAppContext) {
        let (first, cx) = crate::harness::open(cx);
        let second = cx.update(|window, cx| {
            install(cx);
            first.update(cx, |view, _| {
                view.project_window = Some(window.window_handle().window_id())
            });
            let second = create(cx).unwrap();
            second
                .update(cx, |view, _, _| {
                    view.session.add_audio_track("Unsaved");
                })
                .unwrap();
            request_quit(cx);
            second
        });
        cx.run_until_parked();
        cx.cx.update(|cx| {
            assert_eq!(windows(cx).len(), 2);
            second
                .update(cx, |view, _, _| {
                    assert!(matches!(
                        view.prompt.as_ref().map(|prompt| &prompt.body),
                        Some(crate::ui::prompt::PromptBody::Ask(
                            crate::ui::prompt::Question::Unsaved(PendingAction::Quit)
                        ))
                    ));
                    assert!(view.session.is_dirty());
                    assert!(view.cancel_prompt());
                    assert!(view.session.is_dirty());
                })
                .unwrap();
        });
    }
}
