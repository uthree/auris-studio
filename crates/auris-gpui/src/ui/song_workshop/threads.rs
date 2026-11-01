//! Project-local song conversations and their reversible sidebar actions.

use super::*;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::prompt::{Prompt, PromptTarget};
use atelier::{Edge, Sidebar};

impl SongWorkshop {
    /// Reject a rename sheet that belongs to a conversation view which has since changed.
    pub(in crate::ui) fn rename_thread(&self, identity: u64, title: String) -> bool {
        self.identity == identity
            && self
                .pending_thread_name
                .as_ref()
                .is_some_and(|id| self.send(Command::RenameThread(id.clone(), title)))
    }
}

impl AurisApp {
    fn request_workshop_thread(&mut self, command: Command, cx: &mut Context<Self>) {
        if self
            .song_workshop
            .as_ref()
            .is_none_or(|s| s.busy() || s.references.midi_loading)
            || !self.commit_midi_draft()
        {
            return;
        }
        let unavailable = self.t(Key::WorkshopThreadUnavailable).to_owned();
        let Some(state) = &mut self.song_workshop else {
            return;
        };
        let id = state
            .snapshot
            .as_ref()
            .map_or("default", |s| s.document.thread_id.as_str())
            .to_owned();
        let switches = matches!(&command, Command::NewThread | Command::OpenThread(_))
            || matches!(&command, Command::ArchiveThread(thread, true) if *thread == id);
        if state.send(Command::ThreadDraft(
            id,
            state.brief.content().to_owned(),
            state.references.attachments.clone(),
        )) && state.send(command)
        {
            state.switching = switches;
        } else {
            state.error = Some(unavailable);
        }
        cx.notify();
    }

    /// Ask for the name of the exact sidebar conversation selected by the user.
    pub(crate) fn rename_workshop_thread(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(state) = &mut self.song_workshop else {
            return;
        };
        if state.busy() {
            return;
        }
        let title = state
            .snapshot
            .as_ref()
            .and_then(|s| s.threads.iter().find(|t| t.id == id))
            .map_or_else(String::new, |t| t.title.clone());
        state.pending_thread_name = Some(id);
        let identity = state.identity;
        self.open_prompt(Prompt::new(
            title_of(self.t(Key::WorkshopRenameThread)),
            PromptTarget::WorkshopThreadName { workshop: identity },
            title,
        ));
        cx.notify();
    }

    /// Archive or restore a sidebar conversation without removing its generated songs.
    pub(crate) fn archive_workshop_thread(
        &mut self,
        id: String,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        self.request_workshop_thread(Command::ArchiveThread(id, archived), cx);
    }

    /// Project-local conversation navigation, including reversible management actions.
    pub(super) fn workshop_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = &self.song_workshop else {
            return div().into_any_element();
        };
        let spacing = self.theme.atelier.spacing;
        let busy = state.busy() || state.references.midi_loading;
        let active = state
            .snapshot
            .as_ref()
            .map(|s| s.document.thread_id.as_str());
        let mut list = div()
            .id("workshop-thread-list")
            .flex()
            .flex_col()
            .gap(spacing.xs)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if let Some(snapshot) = &state.snapshot {
            for thread in snapshot.threads.iter().filter(|t| {
                !t.archived || state.show_archived_threads || Some(t.id.as_str()) == active
            }) {
                let id = thread.id.clone();
                let menu_id = id.clone();
                let archived = thread.archived;
                let title = if thread.title.is_empty() {
                    self.t(Key::WorkshopUntitledThread).to_owned()
                } else {
                    thread.title.clone()
                };
                let menu_title = title.clone();
                let item = ListItem::new(
                    gpui::SharedString::from(format!("workshop-thread-{id}")),
                    title,
                )
                .no_icon_lane()
                .selected(Some(id.as_str()) == active)
                .disabled(busy)
                .trailing(
                    Button::icon_only(
                        gpui::SharedString::from(format!("workshop-thread-menu-{id}")),
                        IconName::More,
                    )
                    .ghost()
                    .size(atelier::ControlSize::Small)
                    .disabled(busy)
                    .tooltip(self.t(Key::WorkshopThreadActions))
                    .on_click(cx.listener(
                        move |this, event: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_menu(
                                ContextMenu::new(event.position(), menu_title.clone())
                                    .item(
                                        this.t(Key::WorkshopRenameThread),
                                        MenuCommand::WorkshopThreadRename(menu_id.clone()),
                                    )
                                    .item(
                                        this.t(if archived {
                                            Key::WorkshopRestoreThread
                                        } else {
                                            Key::WorkshopArchiveThread
                                        }),
                                        MenuCommand::WorkshopThreadArchive(
                                            menu_id.clone(),
                                            !archived,
                                        ),
                                    ),
                            );
                            cx.notify();
                        },
                    )),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this
                        .song_workshop
                        .as_ref()
                        .and_then(|s| s.snapshot.as_ref())
                        .is_some_and(|s| s.document.thread_id == id)
                    {
                        return;
                    }
                    this.request_workshop_thread(Command::OpenThread(id.clone()), cx);
                    if let Some(state) = &mut this.song_workshop {
                        state.active_field = Some(0);
                        state.foci[0].focus(window);
                    }
                }));
                list = list.child(if archived {
                    item.description(self.t(Key::WorkshopArchivedThreads))
                } else {
                    item
                });
            }
        }
        // Atelier's `Sidebar` is chrome: under Glass it is the translucent tint over the blurred
        // window, with its own trailing hairline, so it must not get a background of its own.
        Sidebar::new(Edge::Left)
            .id("workshop-sidebar")
            .w_56()
            .h_full()
            .gap(spacing.sm)
            .p(spacing.md)
            .child(SectionHeader::new(self.t(Key::WorkshopThreads)))
            .child(
                Button::new("workshop-new-thread", self.t(Key::WorkshopNewThread))
                    .icon(IconName::Plus)
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.request_workshop_thread(Command::NewThread, cx);
                        if let Some(state) = &mut this.song_workshop {
                            state.active_field = Some(0);
                            state.foci[0].focus(window);
                        }
                    })),
            )
            .child(list)
            .child(
                Button::new(
                    "workshop-show-archived",
                    self.t(Key::WorkshopArchivedThreads),
                )
                .ghost()
                .selected(state.show_archived_threads)
                .icon(IconName::History)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(state) = &mut this.song_workshop {
                        state.show_archived_threads = !state.show_archived_threads;
                    }
                    cx.notify();
                })),
            )
            .when(busy, |sidebar| {
                sidebar.child(
                    Label::new(self.t(Key::WorkshopThreadBusy))
                        .size(TextSize::Small)
                        .color(TextColor::Muted),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, paint};

    fn wait_for(
        app: &gpui::Entity<AurisApp>,
        cx: &mut gpui::VisualTestContext,
        ready: impl Fn(&SongWorkshop) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update(cx, |this, cx| this.poll_song_workshop(cx));
            paint(app, cx);
            if app.read_with(cx, |this, _| {
                this.song_workshop.as_ref().is_some_and(&ready)
            }) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "workshop owner did not publish the expected conversation"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[gpui::test]
    fn sidebar_creates_reopens_renames_archives_and_restores_without_losing_drafts(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Workshop::start(
            auris_session::workshop::Store::new(directory.path().into()),
            Default::default(),
        )
        .unwrap();
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        app.update(cx, |this, _| {
            let state = this.song_workshop.as_mut().unwrap();
            state.runtime = Some(runtime);
            state.snapshot = None;
            state.model_receiver = None;
        });
        wait_for(&app, cx, |s| s.snapshot.is_some());
        assert!(cx.debug_bounds("workshop-sidebar").is_some());
        app.update(cx, |this, _| {
            this.song_workshop.as_mut().unwrap().brief = TextField::new("Original draft")
        });
        click("workshop-new-thread", cx);
        wait_for(&app, cx, |s| {
            s.snapshot
                .as_ref()
                .is_some_and(|p| p.document.thread_id != "default")
        });
        let second = app.read_with(cx, |this, _| {
            let state = this.song_workshop.as_ref().unwrap();
            assert!(state.brief.content().is_empty());
            assert_eq!(state.snapshot.as_ref().unwrap().threads.len(), 2);
            state.snapshot.as_ref().unwrap().document.thread_id.clone()
        });
        app.update(cx, |this, _| {
            this.song_workshop.as_mut().unwrap().brief = TextField::new("Second draft")
        });
        click("workshop-thread-default", cx);
        wait_for(&app, cx, |s| {
            s.snapshot
                .as_ref()
                .is_some_and(|p| p.document.thread_id == "default")
        });
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().brief.content(),
                "Original draft"
            )
        });
        click("workshop-thread-menu-default", cx);
        paint(&app, cx);
        crate::harness::choose(
            &app,
            cx,
            &MenuCommand::WorkshopThreadRename("default".into()),
        );
        paint(&app, cx);
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("First song");
        cx.simulate_keystrokes("enter");
        wait_for(&app, cx, |s| {
            s.snapshot
                .as_ref()
                .is_some_and(|p| p.document.title == "First song")
        });
        click("workshop-thread-menu-default", cx);
        paint(&app, cx);
        crate::harness::choose(
            &app,
            cx,
            &MenuCommand::WorkshopThreadArchive("default".into(), true),
        );
        wait_for(&app, cx, |s| {
            s.snapshot
                .as_ref()
                .is_some_and(|p| p.document.thread_id == second)
        });
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().brief.content(),
                "Second draft"
            )
        });
        click("workshop-show-archived", cx);
        paint(&app, cx);
        click("workshop-thread-menu-default", cx);
        paint(&app, cx);
        crate::harness::choose(
            &app,
            cx,
            &MenuCommand::WorkshopThreadArchive("default".into(), false),
        );
        wait_for(&app, cx, |s| {
            s.snapshot
                .as_ref()
                .is_some_and(|p| p.threads.iter().any(|t| t.id == "default" && !t.archived))
        });
        app.update(cx, |this, _| {
            this.song_workshop.as_mut().unwrap().runtime = None;
        });
    }
}
