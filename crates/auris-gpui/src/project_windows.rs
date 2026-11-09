//! Application-wide project routing and exclusive realtime audio ownership.

use std::path::PathBuf;

use gpui::{
    App, AppContext, Context, Entity, EntityId, Global, WindowHandle, WindowId, WindowOptions, px,
    size,
};

use crate::app::AurisApp;
use crate::project_tabs::ProjectTabs;
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
    approved: Vec<(EntityId, u64)>,
    restart: Option<WindowId>,
    restart_committed: bool,
}

impl Global for ProjectWindows {}

pub(crate) fn installed(cx: &App) -> bool {
    cx.has_global::<ProjectWindows>()
}

fn windows(cx: &App) -> Vec<WindowHandle<ProjectTabs>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<ProjectTabs>())
        .collect()
}

fn projects(cx: &App) -> Vec<Entity<AurisApp>> {
    cx.windows()
        .into_iter()
        .flat_map(|window| {
            if let Some(tabs) = window.downcast::<ProjectTabs>() {
                tabs.read(cx)
                    .map(|tabs| tabs.projects.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        })
        .collect()
}

fn activate(project: &Entity<AurisApp>, cx: &mut App) {
    let owner = project.read(cx).project_tabs.clone();
    let id = project.entity_id();
    if let Some(owner) = owner {
        crate::project_tabs::update_window(owner, cx, move |tabs, window, cx| {
            tabs.select(id, window, cx);
            crate::termination::activate(window, cx);
        });
    } else if let Some(id) = project.read(cx).project_window
        && let Some(handle) = cx
            .windows()
            .into_iter()
            .find(|handle| handle.window_id() == id)
    {
        let _ = handle.update(cx, |_, window, cx| crate::termination::activate(window, cx));
    }
}

pub(crate) fn install(cx: &mut App) {
    cx.set_global(ProjectWindows::default());
    cx.on_window_closed(|cx| {
        if projects(cx).is_empty() {
            crate::termination::quit(cx);
        }
    })
    .detach();
}

pub(crate) fn open(path: Option<PathBuf>, cx: &mut App) {
    open_in(path, None, cx);
}

pub(crate) fn open_in(path: Option<PathBuf>, preferred: Option<WindowId>, cx: &mut App) {
    // Opening draws synchronously. Defer until the invoking document is no longer leased.
    cx.defer(move |cx| {
        if let Some(path) = path.as_ref() {
            let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            for project in projects(cx) {
                let found = project.update(cx, |view, _| {
                    view.session.path().is_some_and(|existing| {
                        std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf())
                            == identity
                    }) || view.opening_project.as_ref() == Some(&identity)
                });
                if found {
                    activate(&project, cx);
                    return;
                }
            }
        }
        let target = windows(cx)
            .into_iter()
            .find(|handle| Some(handle.window_id()) == preferred)
            .or_else(|| {
                cx.active_window()
                    .and_then(|window| window.downcast::<ProjectTabs>())
            })
            .or_else(|| windows(cx).first().copied());
        if let Some(target) = target {
            let _ = target.update(cx, |tabs, window, cx| {
                if let Some(path) = path.as_ref() {
                    let reused = tabs.active().update(cx, |view, cx| {
                        if view.project_chooser.is_some()
                            && view.song_workshop.is_none()
                            && view.opening_project.is_none()
                        {
                            view.open_project_at(path.clone(), cx);
                            true
                        } else {
                            false
                        }
                    });
                    if reused {
                        window.activate_window();
                        return;
                    }
                }
                tabs.add(path, window, cx);
                window.activate_window();
            });
            return;
        }
        match create(cx) {
            Ok(handle) => {
                if let Some(path) = path {
                    let _ = handle.update(cx, |tabs, _, cx| {
                        tabs.active()
                            .update(cx, |view, cx| view.open_project_at(path, cx))
                    });
                } else {
                    let _ = handle.update(cx, |tabs, window, cx| {
                        tabs.active()
                            .update(cx, |view, cx| view.show_project_chooser(window, cx))
                    });
                }
            }
            Err(error) => log::error!("could not open project window: {error}"),
        }
    });
}

/// Opens a start screen showing its setup guide, for a request made where none is showing.
pub(crate) fn open_setup_guide(cx: &mut App) {
    match create(cx) {
        Ok(handle) => {
            let _ = handle.update(cx, |tabs, window, cx| {
                tabs.active().update(cx, |view, cx| {
                    view.show_project_chooser(window, cx);
                    if let Some(chooser) = &mut view.project_chooser {
                        chooser.page = crate::ui::project_chooser::ChooserPage::Setup;
                    }
                })
            });
        }
        Err(error) => log::error!("could not open project window: {error}"),
    }
}

pub(crate) fn create(cx: &mut App) -> Result<WindowHandle<ProjectTabs>, String> {
    let remembered = auris_session::Settings::load().window;
    let bounds = crate::opening_window_bounds(remembered, cx);
    crate::native_window::open(
        cx,
        WindowOptions {
            window_bounds: Some(bounds),
            window_min_size: Some(size(px(640.0), px(480.0))),
            focus: true,
            ..crate::titlebar::options("Auris Studio")
        },
        |window, cx| cx.new(|cx| ProjectTabs::new(window, cx)),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn request_quit(cx: &mut App) {
    cx.global_mut::<ProjectWindows>().approved.clear();
    cx.global_mut::<ProjectWindows>().restart = None;
    cx.global_mut::<ProjectWindows>().restart_committed = false;
    advance_quit(cx);
}

pub(crate) fn request_restart(owner: Option<WindowId>, cx: &mut App) {
    cx.global_mut::<ProjectWindows>().approved.clear();
    cx.global_mut::<ProjectWindows>().restart = owner;
    cx.global_mut::<ProjectWindows>().restart_committed = false;
    advance_quit(cx);
}

fn advance_quit(cx: &mut App) {
    cx.defer(|cx| {
        for project in projects(cx) {
            let id = project.entity_id();
            let approved = cx.global::<ProjectWindows>().approved.clone();
            let ready = project.update(cx, |view, cx| {
                if approved.contains(&(id, view.session.revision())) {
                    return true;
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
            if !ready {
                activate(&project, cx);
                return;
            }
        }
        if let Some(owner) = cx.global::<ProjectWindows>().restart {
            cx.global_mut::<ProjectWindows>().restart_committed = true;
            for handle in windows(cx) {
                if handle.window_id() == owner {
                    let _ = handle.update(cx, |tabs, _, cx| {
                        tabs.active()
                            .update(cx, |view, _| view.restart_after_update = true)
                    });
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
        && (cx.global::<ProjectWindows>().restart_committed
            || projects(cx).into_iter().any(|project| {
                let view = project.read(cx);
                view.project_window != own && view.restart_after_update
            }))
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
            open_in(Some(path), self.project_window, cx);
        } else if self.confirm_discard(PendingAction::OpenDropped(path.clone())) {
            self.open_project_at(path, cx);
        }
    }

    pub(crate) fn approve_quit(&mut self, cx: &mut Context<Self>) {
        if !installed(cx) {
            crate::termination::quit(cx);
            return;
        }
        if self.project_window.is_some() {
            let id = cx.entity_id();
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
        for project in projects(cx) {
            if project.entity_id() == cx.entity_id() {
                continue;
            }
            let result = project.update(cx, |view, cx| {
                let was_suspended = view.session.audio_suspended();
                let result = view.session.set_audio_suspended(true);
                if result.is_ok() {
                    view.stop_audition();
                    view.release_physical_preview();
                    if !was_suspended {
                        view.set_status(view.t(auris_i18n::Key::ProjectAudioSuspended));
                    }
                }
                cx.notify();
                result
            });
            match result {
                Ok(()) => {}
                Err(error) => {
                    let message =
                        if matches!(error, auris_session::SessionError::RecordingInProgress) {
                            self.t(auris_i18n::Key::OtherProjectRecording).to_string()
                        } else {
                            crate::i18n::error_text(&error, self.language())
                        };
                    self.set_failed_status(message);
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
                    self.session.audio_suspended(),
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
    fn start_screen_opens_a_saved_project_in_the_same_window(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        let directory = tempfile::tempdir().unwrap();
        let mut saved =
            auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
        saved
            .save_as(&directory.path().join("Reopen.auris"))
            .unwrap();
        let path = saved.path().unwrap().to_path_buf();
        cx.update(|window, cx| first.update(cx, |view, cx| view.show_project_chooser(window, cx)));
        cx.cx.update(|cx| open(Some(path.clone()), cx));
        cx.run_until_parked();
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 1);
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        first.read_with(cx, |view, _| {
            assert!(view.project_chooser.is_none());
            assert_eq!(view.session.path(), Some(path.as_path()));
        });
        cx.cx.update(|cx| assert_eq!(windows(cx).len(), 1));
    }

    #[gpui::test]
    fn tabs_keep_independent_documents_undo_and_audio(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| view.session.add_audio_track("First"));
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        second.update(cx, |view, cx| {
            view.project_chooser = None;
            view.session.add_audio_track("Second");
            assert!(view.acquire_audio(cx));
        });
        first.update(cx, |view, cx| {
            assert!(view.session.audio_suspended());
            assert!(view.acquire_audio(cx));
            assert_eq!(view.project().tracks[1].name, "First");
        });
        second.update(cx, |view, _| {
            assert!(view.session.audio_suspended());
            view.session.undo();
            assert_eq!(view.project().tracks.len(), 1);
        });
        assert_eq!(
            first.read_with(cx, |view, _| view.project().tracks.len()),
            2
        );
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
    }

    #[gpui::test]
    fn opening_a_project_twice_selects_its_existing_tab(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        let directory = tempfile::tempdir().unwrap();
        let path = first.update(cx, |view, _| {
            view.session
                .save_as(&directory.path().join("Same.auris"))
                .unwrap();
            view.session.add_audio_track("Unsaved edits");
            view.session.path().unwrap().to_path_buf()
        });
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        cx.cx.update(|cx| {
            open(Some(path.clone()), cx);
            open(Some(path), cx);
        });
        cx.run_until_parked();
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        assert!(first.read_with(cx, |view, _| view.session.is_dirty()));
        assert_eq!(
            first.read_with(cx, |view, _| view.project().tracks.len()),
            2
        );
    }

    #[gpui::test]
    fn opening_another_saved_project_keeps_the_current_dirty_document(
        cx: &mut gpui::TestAppContext,
    ) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| {
            view.session.add_audio_track("Keep unsaved first")
        });
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        let directory = tempfile::tempdir().unwrap();
        let path = second.update(cx, |view, _| {
            view.project_chooser = None;
            view.session.add_audio_track("Saved second");
            view.session
                .save_as(&directory.path().join("Second.auris"))
                .unwrap();
            view.session.path().unwrap().to_path_buf()
        });
        cx.update(|window, cx| {
            tabs.update(cx, |tabs, cx| {
                tabs.close_tab(second.entity_id(), window, cx)
            })
        });
        cx.cx.update(|cx| open(Some(path), cx));
        cx.run_until_parked();
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
        let loaded = tabs.read_with(cx, |tabs, _| tabs.active());
        assert_ne!(loaded.entity_id(), first.entity_id());
        assert_eq!(
            loaded.read_with(cx, |view, _| view.project().tracks[1].name.clone()),
            "Saved second"
        );
        assert!(first.read_with(cx, |view, _| view.session.is_dirty()));
        assert_eq!(
            first.read_with(cx, |view, _| view.project().tracks[1].name.clone()),
            "Keep unsaved first"
        );
    }

    #[gpui::test]
    fn quit_checks_hidden_dirty_tabs_before_closing_any_window(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| view.session.add_audio_track("Unsaved"));
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        cx.cx.update(request_quit);
        cx.run_until_parked();
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        first.update(cx, |view, _| {
            assert!(matches!(
                view.prompt.as_ref().map(|prompt| &prompt.body),
                Some(crate::ui::prompt::PromptBody::Ask(
                    crate::ui::prompt::Question::Unsaved(PendingAction::Quit)
                ))
            ));
            assert!(view.cancel_prompt());
            assert!(view.session.is_dirty());
        });
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
    }

    #[gpui::test]
    fn a_restart_chosen_in_another_window_is_seen_by_this_ones_quit(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let own = tabs
            .read_with(cx, |tabs, _| tabs.active())
            .read_with(cx, |view, _| view.project_window);
        let second = cx.cx.update(|cx| create(cx).unwrap());
        cx.cx.update(|cx| {
            assert!(!restart_requested_elsewhere(own, cx));
            second
                .update(cx, |tabs, _, cx| {
                    tabs.active()
                        .update(cx, |view, _| view.restart_after_update = true)
                })
                .unwrap();
            assert!(restart_requested_elsewhere(own, cx));
            assert!(!restart_requested_elsewhere(Some(second.window_id()), cx));
        });
    }

    #[gpui::test]
    fn a_cancelled_restart_does_not_restart_on_a_later_window_close(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| view.session.add_audio_track("Unsaved"));
        let own = first.read_with(cx, |view, _| view.project_window);
        cx.cx.update(|cx| request_restart(own, cx));
        cx.run_until_parked();
        assert!(first.read_with(cx, |view, _| view.prompt.is_some()));
        first.update(cx, |view, _| view.cancel_prompt());
        cx.cx
            .update(|cx| assert!(!restart_requested_elsewhere(None, cx)));
    }
}
