//! A window owns independent document entities; only its selected document is painted.

use gpui::{
    App, AppContext, Context, Entity, EntityId, IntoElement, Render, ScrollHandle, WeakEntity,
    Window, div, prelude::*, px,
};

use auris_i18n::Key;

use crate::app::{AurisApp, Pane};
use crate::ui::icons::Icon;
use crate::ui::prompt::PendingAction;
use crate::ui::widgets::{
    ButtonState, ButtonStyle, Latch, button_with_content, icon_button_enabled,
};

/// The documents open in one native window.
pub(crate) struct ProjectTabs {
    pub(crate) projects: Vec<Entity<AurisApp>>,
    pub(crate) active: usize,
    approved: Vec<(EntityId, u64)>,
    pub(crate) scroll: ScrollHandle,
}

impl ProjectTabs {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let asked = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            update_window(asked.clone(), cx, |tabs, window, cx| {
                tabs.start_close_window(window, cx)
            });
            false
        });
        let project = cx.new(AurisApp::new);
        let owner = cx.entity().downgrade();
        project.update(cx, |view, _| {
            view.project_tabs = Some(owner);
            view.project_window = Some(window.window_handle().window_id());
            let _ = view.session.set_audio_suspended(true);
            view.focus_pane(Pane::Arrangement, window);
        });
        Self {
            projects: vec![project],
            active: 0,
            approved: Vec::new(),
            scroll: ScrollHandle::new(),
        }
    }

    pub(crate) fn active(&self) -> Entity<AurisApp> {
        self.projects[self.active].clone()
    }

    pub(crate) fn select(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.projects.iter().position(|view| view.entity_id() == id) else {
            return;
        };
        if index != self.active {
            self.active().update(cx, |view, _| {
                view.session.release_typed_notes();
                view.stop_audition();
                view.release_physical_preview();
                view.drag = None;
                // A menu belongs to the document that opened it. Do not let a hidden tab
                // restore its trigger or activation release when it becomes active again.
                view.close_menu();
                view.close_menu_bar();
                view.menu_focus_return = None;
                view.menu_activation = None;
            });
            self.active = index;
        }
        self.active().update(cx, |view, cx| {
            view.refresh_shared_settings();
            view.appearance = crate::appearance::Appearance::load();
            view.theme = view.appearance.theme();
            view.main_was_active = false;
            view.titled.clear();
            if view.project_chooser.is_some() {
                window.focus(&view.focus);
            } else {
                view.focus_pane(view.last_pane, window);
            }
            cx.notify();
        });
        self.scroll.scroll_to_item(index);
        cx.notify();
    }

    pub(crate) fn add(
        &mut self,
        path: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = cx.new(AurisApp::new);
        let owner = cx.entity().downgrade();
        project.update(cx, |view, cx| {
            view.project_tabs = Some(owner);
            view.project_window = Some(window.window_handle().window_id());
            let _ = view.session.set_audio_suspended(true);
            if let Some(path) = path {
                view.open_project_at(path, cx);
            } else {
                view.show_project_chooser(window, cx);
            }
        });
        let id = project.entity_id();
        self.projects.push(project);
        self.select(id, window, cx);
    }

    pub(crate) fn request_close_tab(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.projects.iter().any(|view| view.entity_id() == id) {
            return;
        }
        self.select(id, window, cx);
        let ready = self.active().update(cx, |view, cx| {
            let ready = view.confirm_discard(PendingAction::CloseTab);
            cx.notify();
            ready
        });
        if ready {
            self.close_tab(id, window, cx);
        }
    }

    pub(crate) fn close_tab(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.projects.iter().position(|view| view.entity_id() == id) else {
            return;
        };
        self.projects[index].update(cx, |view, _| view.save_window_placement());
        if self.projects.len() == 1 {
            // Keep the native window and its close handler; replace the closed document
            // with a fresh start screen so its edits and background work are released.
            self.add(None, window, cx);
        }
        self.projects.remove(index);
        self.active = if index < self.active {
            self.active - 1
        } else {
            self.active.min(self.projects.len() - 1)
        };
        let id = self.active().entity_id();
        self.select(id, window, cx);
    }

    /// A queued confirmation can discard only the revision that was approved.
    pub(crate) fn approve_close_tab(
        &mut self,
        id: EntityId,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self
            .projects
            .iter()
            .find(|project| project.entity_id() == id)
        else {
            return;
        };
        if project.read(cx).session.revision() != revision {
            self.request_close_tab(id, window, cx);
        } else {
            self.close_tab(id, window, cx);
        }
    }

    /// Rechecks revisions on every continuation, including after an asynchronous Save As.
    pub(crate) fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for project in self.projects.clone() {
            let revision = project.read(cx).session.revision();
            if self.approved.contains(&(project.entity_id(), revision)) {
                continue;
            }
            self.select(project.entity_id(), window, cx);
            let ready = project.update(cx, |view, cx| {
                let ready = view.confirm_discard(PendingAction::CloseWindow);
                cx.notify();
                ready
            });
            if !ready {
                return;
            }
            self.approved.push((project.entity_id(), revision));
        }
        self.active()
            .update(cx, |view, _| view.save_window_placement());
        window.remove_window();
    }

    pub(crate) fn start_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.approved.clear();
        self.close_window(window, cx);
    }

    pub(crate) fn approve_close_window(
        &mut self,
        id: EntityId,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.approved.push((id, revision));
        self.close_window(window, cx);
    }
}

impl Render for ProjectTabs {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context(crate::actions::context::PROJECT_TABS)
            .child(self.active())
    }
}

impl AurisApp {
    pub(crate) fn project_tabs_height(&self) -> gpui::Pixels {
        if self.project_tabs.is_some() {
            self.tab_bar_height
        } else {
            px(0.0)
        }
    }

    pub(crate) fn cycle_project_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.modal_blocks_document_actions() {
            return;
        }
        if let Some(tabs) = self.project_tabs.clone() {
            update_window(tabs, cx, move |tabs, window, cx| {
                let count = tabs.projects.len();
                let index = (tabs.active + if forward { 1 } else { count - 1 }) % count;
                let id = tabs.projects[index].entity_id();
                tabs.select(id, window, cx);
            });
        }
    }

    pub(crate) fn close_project_tab(&mut self, cx: &mut Context<Self>) {
        if self.modal_blocks_document_actions() {
            return;
        }
        if let Some(tabs) = self.project_tabs.clone() {
            let id = cx.entity_id();
            update_window(tabs, cx, move |tabs, window, cx| {
                tabs.request_close_tab(id, window, cx)
            });
        }
    }

    pub(crate) fn close_project_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tabs) = self.project_tabs.clone() {
            update_window(tabs, cx, |tabs, window, cx| {
                tabs.start_close_window(window, cx)
            });
        } else if self.confirm_discard(PendingAction::CloseWindow) {
            self.save_window_placement();
            window.remove_window();
        }
    }

    pub(crate) fn render_project_tabs(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.tab_bar_height = px(0.0);
        let owner = self.project_tabs.as_ref()?.upgrade()?;
        let tabs = owner.read(cx);
        if tabs.projects.len() <= 1 {
            return None;
        }
        let own = cx.entity_id();
        let enabled = !self.modal_blocks_document_actions();
        self.tab_bar_height = window.rem_size() * 2.0;
        let theme = &self.theme;
        let mut row = div()
            .id("project-tab-list")
            .flex()
            .items_center()
            .gap_1()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .track_scroll(&tabs.scroll);
        for (index, project) in tabs.projects.iter().enumerate() {
            let id = project.entity_id();
            let label = if id == own {
                self.project_tab_label()
            } else {
                project.read(cx).project_tab_label()
            };
            let active = index == tabs.active;
            let owner = owner.downgrade();
            let close_owner = owner.clone();
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap_1()
                    .rounded(theme.atelier.shape.radius_sm)
                    .when(active, |tab| tab.bg(theme.surface_raised))
                    .child(
                        button_with_content(
                            ("project-tab", index),
                            div().min_w_0().truncate().child(label.clone()),
                            ButtonStyle::Ghost,
                            ButtonState::available(Latch::Off, enabled),
                            theme.accent,
                            theme,
                            move |_, _, cx| {
                                update_window(owner.clone(), cx, move |tabs, window, cx| {
                                    tabs.select(id, window, cx)
                                })
                            },
                        )
                        .max_w_48()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(if active { theme.text } else { theme.text_muted })
                        .tooltip(crate::ui::tooltip::keyed_tip(
                            if id == own {
                                self.project_tab_tooltip()
                            } else {
                                project.read(cx).project_tab_tooltip()
                            },
                            "",
                            theme,
                        )),
                    )
                    .child(
                        icon_button_enabled(
                            ("close-project-tab", index),
                            Icon::Cross,
                            false,
                            enabled,
                            theme.accent,
                            theme,
                            move |_, _, cx| {
                                update_window(close_owner.clone(), cx, move |tabs, window, cx| {
                                    tabs.request_close_tab(id, window, cx)
                                })
                            },
                        )
                        .tooltip(self.tip(Key::CmdCloseTab, "file.close_tab")),
                    ),
            );
        }
        Some(
            div()
                .id("project-tabs")
                .flex()
                .items_center()
                .h_8()
                .flex_shrink_0()
                .gap_1()
                .px_2()
                .bg(theme.surface_sunken)
                .border_b_1()
                .border_color(theme.border_subtle)
                .child(row)
                .child(
                    icon_button_enabled(
                        "new-project-tab",
                        Icon::Plus,
                        false,
                        enabled,
                        theme.accent,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            crate::project_windows::open_in(None, this.project_window, cx)
                        }),
                    )
                    .tooltip(self.tip(Key::CmdNewProject, "file.new")),
                )
                .into_any_element(),
        )
    }

    fn project_tab_label(&self) -> String {
        let label = if self.project_chooser.is_some() {
            self.t(if self.song_workshop.is_some() {
                Key::WorkshopTitle
            } else {
                Key::ChooseProject
            })
            .to_string()
        } else {
            self.opening_project
                .as_deref()
                .and_then(|path| path.file_stem())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.session.document_title())
        };
        if self.session.is_dirty() {
            format!("{label} •")
        } else {
            label
        }
    }

    fn project_tab_tooltip(&self) -> String {
        let label = self.project_tab_label();
        match self.opening_project.as_deref().or(self.session.path()) {
            Some(path) => format!("{label}\n{}", path.display()),
            None => label,
        }
    }
}

/// Defers until the invoking document's entity lease has ended.
pub(crate) fn update_window(
    owner: WeakEntity<ProjectTabs>,
    cx: &mut App,
    update: impl FnOnce(&mut ProjectTabs, &mut Window, &mut Context<ProjectTabs>) + 'static,
) {
    cx.defer(move |cx| {
        let Some(owner) = owner.upgrade() else { return };
        let Some(id) = owner
            .read(cx)
            .projects
            .first()
            .and_then(|view| view.read(cx).project_window)
        else {
            return;
        };
        if let Some(handle) = cx
            .windows()
            .into_iter()
            .find(|handle| handle.window_id() == id)
        {
            let _ = handle.update(cx, |_, window, cx| {
                owner.update(cx, |tabs, cx| update(tabs, window, cx))
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::harness::{click, paint};

    fn project_window_count(cx: &gpui::App) -> usize {
        cx.windows()
            .into_iter()
            .filter_map(|window| window.downcast::<super::ProjectTabs>())
            .count()
    }

    #[gpui::test]
    fn closing_the_last_tab_returns_to_a_fresh_start_screen(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        let window_id = first.read_with(cx, |view, _| view.project_window);
        first.update(cx, |view, _| view.project_chooser = None);
        paint(&first, cx);
        cx.simulate_keystrokes("secondary-w");
        cx.run_until_parked();

        let start = tabs.read_with(cx, |tabs, _| {
            assert_eq!(tabs.projects.len(), 1);
            tabs.active()
        });
        assert_ne!(start.entity_id(), first.entity_id());
        start.read_with(cx, |view, _| {
            assert_eq!(view.project_window, window_id);
            assert!(view.project_chooser.is_some());
            assert!(view.song_workshop.is_none());
            assert!(view.session.path().is_none());
            assert!(!view.session.is_dirty());
            assert!(view.session.audio_suspended());
        });
        paint(&start, cx);
        click("chooser-empty", cx);
        paint(&start, cx);
        click("chooser-confirm", cx);
        start.read_with(cx, |view, _| {
            assert!(view.project_chooser.is_none());
            assert_eq!(view.project_window, window_id);
        });
        cx.simulate_keystrokes("secondary-w");
        cx.run_until_parked();
        let start = tabs.read_with(cx, |tabs, _| tabs.active());
        paint(&start, cx);
        click("chooser-ai", cx);
        paint(&start, cx);
        click("chooser-confirm", cx);
        start.read_with(cx, |view, _| {
            assert!(view.song_workshop.is_some());
            assert_eq!(view.project_window, window_id);
        });
        cx.cx.update(|cx| assert_eq!(project_window_count(cx), 1));
    }

    #[gpui::test]
    fn cancelling_the_last_tabs_close_keeps_the_document(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| {
            view.project_chooser = None;
            view.session.add_audio_track("Keep unsaved edits");
        });
        paint(&first, cx);
        cx.simulate_keystrokes("secondary-w");
        cx.run_until_parked();
        paint(&first, cx);
        cx.simulate_keystrokes("escape");
        first.read_with(cx, |view, _| {
            assert!(view.prompt.is_none());
            assert!(view.project_chooser.is_none());
            assert!(view.session.is_dirty());
        });
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
    }

    #[gpui::test]
    fn discarding_the_last_tabs_edits_returns_to_start(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| {
            view.project_chooser = None;
            view.session.add_audio_track("Discard these edits");
        });
        paint(&first, cx);
        cx.simulate_keystrokes("secondary-w");
        cx.run_until_parked();
        paint(&first, cx);
        click("prompt-deny", cx);
        cx.run_until_parked();
        let start = tabs.read_with(cx, |tabs, _| tabs.active());
        assert_ne!(start.entity_id(), first.entity_id());
        assert!(start.read_with(cx, |view, _| view.project_chooser.is_some()));
        cx.cx.update(|cx| assert_eq!(project_window_count(cx), 1));
    }

    #[gpui::test]
    fn closing_the_native_window_still_removes_it(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let id = tabs.read_with(cx, |tabs, cx| {
            tabs.active().read(cx).project_window.unwrap()
        });
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.start_close_window(window, cx)));
        cx.run_until_parked();
        cx.cx
            .update(|cx| assert!(cx.windows().iter().all(|window| window.window_id() != id)));
    }

    #[gpui::test]
    fn tab_bar_is_visible_only_while_multiple_tabs_are_open(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        paint(&first, cx);
        assert!(cx.debug_bounds("close-project-tab-0").is_none());
        assert!(cx.debug_bounds("new-project-tab").is_none());
        assert_eq!(
            first.read_with(cx, |view, _| view.project_tabs_height()),
            gpui::px(0.0)
        );

        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        paint(&second, cx);
        assert!(cx.debug_bounds("close-project-tab-0").is_some());
        assert!(cx.debug_bounds("close-project-tab-1").is_some());
        assert!(second.read_with(cx, |view, _| view.project_tabs_height()) > gpui::px(0.0));

        click("close-project-tab-1", cx);
        cx.run_until_parked();
        paint(&first, cx);
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 1);
        // GPUI retains debug bounds for elements from earlier frames, including hidden ones.
        cx.update(|window, cx| {
            first.update(cx, |view, cx| {
                assert!(view.render_project_tabs(window, cx).is_none());
            })
        });
        assert_eq!(
            first.read_with(cx, |view, _| view.project_tabs_height()),
            gpui::px(0.0)
        );
    }

    #[gpui::test]
    fn new_and_cycle_shortcuts_keep_the_first_documents_edits(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| {
            view.session.add_audio_track("Keep this");
            view.timeline.scroll_ticks = auris_session::prelude::Ticks(1234);
            view.agent_chat.input = crate::ui::text_field::TextField::new("First chat draft");
        });
        paint(&first, cx);
        cx.simulate_keystrokes("secondary-n");
        cx.run_until_parked();
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        assert_ne!(first.entity_id(), second.entity_id());
        assert!(second.read_with(cx, |view, _| view.project_chooser.is_some()));
        cx.simulate_keystrokes("ctrl-tab");
        cx.run_until_parked();
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        assert_eq!(
            first.read_with(cx, |view, _| view.timeline.scroll_ticks.0),
            1234
        );
        assert!(first.read_with(cx, |view, _| view.session.is_dirty()));
        assert_eq!(
            first.read_with(cx, |view, _| view.agent_chat.input.content().to_owned()),
            "First chat draft"
        );
        assert!(
            second
                .read_with(cx, |view, _| view.agent_chat.input.content().to_owned())
                .is_empty()
        );
        assert_eq!(
            first.read_with(cx, |view, _| view.project().tracks[1].name.clone()),
            "Keep this"
        );
        first.update(cx, |view, _| {
            view.menu_bar = Some(crate::ui::menu_bar::OpenMenu::at(0));
            view.menu_focus_return = Some(view.focus.clone());
        });
        paint(&first, cx);
        cx.simulate_keystrokes("ctrl-shift-tab");
        cx.run_until_parked();
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            second.entity_id()
        );
        assert!(first.read_with(cx, |view, _| view.menu_bar.is_none()));
        assert!(first.read_with(cx, |view, _| view.menu_focus_return.is_none()));
    }

    #[gpui::test]
    fn closing_a_dirty_tab_can_cancel_then_discard_only_that_document(
        cx: &mut gpui::TestAppContext,
    ) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        second.update(cx, |view, _| {
            view.project_chooser = None;
            view.session.add_audio_track("Unsaved second");
        });
        paint(&second, cx);
        cx.simulate_keystrokes("secondary-w");
        cx.run_until_parked();
        assert!(second.read_with(cx, |view, _| view.prompt.is_some()));
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
        cx.simulate_keystrokes("escape");
        assert!(second.read_with(cx, |view, _| view.prompt.is_none()));
        assert!(second.read_with(cx, |view, _| view.session.is_dirty()));
        cx.simulate_keystrokes("secondary-w");
        cx.run_until_parked();
        paint(&second, cx);
        click("prompt-deny", cx);
        cx.run_until_parked();
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 1);
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        assert!(!first.read_with(cx, |view, _| view.session.is_dirty()));
    }

    #[gpui::test]
    fn native_close_checks_an_inactive_dirty_tab(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| {
            view.session.add_audio_track("Keep native-close edits")
        });
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        assert!(!cx.simulate_close());
        cx.run_until_parked();
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        assert!(first.read_with(cx, |view, _| view.prompt.is_some()));
        cx.simulate_keystrokes("escape");
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
        assert!(first.read_with(cx, |view, _| view.session.is_dirty()));
    }

    #[gpui::test]
    fn a_repeated_close_request_cannot_prompt_or_close_the_dirty_sibling(
        cx: &mut gpui::TestAppContext,
    ) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        second.update(cx, |view, _| {
            view.session.add_audio_track("Keep sibling edits")
        });
        cx.update(|window, cx| {
            tabs.update(cx, |tabs, cx| {
                tabs.close_tab(first.entity_id(), window, cx);
                tabs.request_close_tab(first.entity_id(), window, cx);
            })
        });
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 1);
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            second.entity_id()
        );
        assert!(second.read_with(cx, |view, _| view.prompt.is_none()));
        assert!(second.read_with(cx, |view, _| view.session.is_dirty()));
    }

    #[gpui::test]
    fn a_tab_edit_after_close_approval_is_asked_about_again(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        let revision = first.read_with(cx, |view, _| view.session.revision());
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        first.update(cx, |view, _| view.session.add_audio_track("Later edit"));
        cx.update(|window, cx| {
            tabs.update(cx, |tabs, cx| {
                tabs.approve_close_tab(first.entity_id(), revision, window, cx)
            })
        });
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
        assert!(first.read_with(cx, |view, _| view.prompt.is_some()));
        assert!(first.read_with(cx, |view, _| view.session.is_dirty()));
    }

    #[gpui::test]
    fn editing_an_approved_tab_requires_a_new_window_close_decision(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| {
            view.panels.show(crate::dock::Panel::Mixer);
            view.session.add_audio_track("First edits");
        });
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        second.update(cx, |view, _| view.session.add_audio_track("Second edits"));
        // Paint the confirmation before answering so deferred mixer layout writes precede
        // approval, just as they do when the user answers the sheet.
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.start_close_window(window, cx)));
        paint(&first, cx);
        click("prompt-deny", cx);
        cx.run_until_parked();
        assert!(second.read_with(cx, |view, _| view.prompt.is_some()));
        first.update(cx, |view, _| view.session.add_audio_track("Later edit"));
        let revision = second.read_with(cx, |view, _| view.session.revision());
        second.update(cx, |view, _| view.cancel_prompt());
        cx.update(|window, cx| {
            tabs.update(cx, |tabs, cx| {
                tabs.approve_close_window(second.entity_id(), revision, window, cx)
            })
        });
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        assert!(first.read_with(cx, |view, _| view.prompt.is_some()));
    }

    #[gpui::test]
    fn closing_a_window_checks_each_dirty_tab_and_cancel_keeps_them(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        first.update(cx, |view, _| view.session.add_audio_track("First edits"));
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        second.update(cx, |view, _| {
            view.project_chooser = None;
            view.session.add_audio_track("Second edits");
        });
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.start_close_window(window, cx)));
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        paint(&first, cx);
        click("prompt-deny", cx);
        cx.run_until_parked();
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            second.entity_id()
        );
        assert!(second.read_with(cx, |view, _| view.prompt.is_some()));
        cx.simulate_keystrokes("escape");
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 2);
        assert!(first.read_with(cx, |view, _| view.session.is_dirty()));
        assert!(second.read_with(cx, |view, _| view.session.is_dirty()));
        // A new close attempt must ask again, including about a previously discarded tab.
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.start_close_window(window, cx)));
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            first.entity_id()
        );
        assert!(first.read_with(cx, |view, _| view.prompt.is_some()));
    }

    #[gpui::test]
    fn closing_after_save_keeps_the_selected_sibling_and_its_save_path(
        cx: &mut gpui::TestAppContext,
    ) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        let directory = tempfile::tempdir().unwrap();
        let first_path = first.update(cx, |view, _| {
            view.session
                .save_as(&directory.path().join("First.auris"))
                .unwrap();
            view.session.path().unwrap().to_path_buf()
        });
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        let second_path = second.update(cx, |view, _| {
            view.project_chooser = None;
            view.session
                .save_as(&directory.path().join("Second.auris"))
                .unwrap();
            view.session.path().unwrap().to_path_buf()
        });
        cx.update(|window, cx| {
            tabs.update(cx, |tabs, cx| tabs.close_tab(first.entity_id(), window, cx))
        });
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            second.entity_id()
        );
        assert_ne!(first_path, second_path);
        assert_eq!(
            second.read_with(cx, |view, _| view
                .session
                .path()
                .map(std::path::Path::to_path_buf)),
            Some(second_path)
        );
    }

    #[gpui::test]
    fn saving_from_the_close_prompt_finishes_only_that_tabs_close(cx: &mut gpui::TestAppContext) {
        let (tabs, cx) = crate::harness::open_tabs(cx);
        let first = tabs.read_with(cx, |tabs, _| tabs.active());
        let directory = tempfile::tempdir().unwrap();
        first.update(cx, |view, _| {
            view.session
                .save_as(&directory.path().join("Saved.auris"))
                .unwrap();
            view.session.add_audio_track("Save these edits");
        });
        cx.update(|window, cx| tabs.update(cx, |tabs, cx| tabs.add(None, window, cx)));
        let second = tabs.read_with(cx, |tabs, _| tabs.active());
        cx.update(|window, cx| {
            tabs.update(cx, |tabs, cx| {
                tabs.request_close_tab(first.entity_id(), window, cx)
            })
        });
        paint(&first, cx);
        click("prompt-ok", cx);
        cx.run_until_parked();
        assert!(!first.read_with(cx, |view, _| view.session.is_dirty()));
        assert_eq!(tabs.read_with(cx, |tabs, _| tabs.projects.len()), 1);
        assert_eq!(
            tabs.read_with(cx, |tabs, _| tabs.active()).entity_id(),
            second.entity_id()
        );
    }
}
