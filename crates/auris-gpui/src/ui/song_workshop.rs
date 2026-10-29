//! A persistent conversation with inline, independently produced song alternatives.

use super::agent_components;
use super::audio_player::{PlayerState, PlayerTarget};
use super::text_field::{KeyEffect, TextField};
use crate::app::{AurisApp, Pane};
use atelier::{Badge, Button, IconName, Label, ListItem, SectionHeader, TextColor, TextSize, Tone};
use auris_agent::workshop::{Command, Snapshot, Workshop};
use auris_i18n::{Key, messages};
use auris_session::SongCandidate;
use gpui::{AnyElement, Context, FocusHandle, IntoElement, Window, div, prelude::*};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

/// The width the transcript, the cards and the composer share, as in Atelier's compose screen.
const COLUMN_WIDTH: gpui::Pixels = gpui::px(1040.0);

/// A command's label used as a title: without the ellipsis that says the command asks for more.
fn title_of(label: &str) -> String {
    label.trim_end_matches(['…', '.']).to_owned()
}

#[path = "song_workshop/chat_input.rs"]
mod input;
#[path = "song_workshop/preview.rs"]
mod preview;

pub(crate) struct SongWorkshop {
    identity: u64,
    pub(crate) brief: TextField,
    foci: [FocusHandle; 1],
    active_field: Option<usize>,
    _focus_subscriptions: Vec<gpui::Subscription>,
    preferences: auris_session::AgentPreferences,
    models: Vec<super::agent_chat::ModelOption>,
    model_receiver: Option<mpsc::Receiver<Result<String, String>>>,
    model_error: Option<String>,
    runtime: Option<Workshop>,
    snapshot: Option<Snapshot>,
    view_round: Option<usize>,
    candidates: [Option<Arc<SongCandidate>>; 3],
    played_audio: [Option<Arc<auris_session::prelude::AudioBuffer>>; 3],
    saved: [Option<std::path::PathBuf>; 3],
    errors: [Option<String>; 3],
    pub(crate) players: [PlayerState; 3],
    preview_serial: u64,
    preparing: Option<usize>,
    error: Option<String>,
    submitted: bool,
    show_memory: bool,
    scroll: gpui::ScrollHandle,
}

impl SongWorkshop {
    pub(crate) fn busy(&self) -> bool {
        self.submitted
            || self
                .snapshot
                .as_ref()
                .is_none_or(|s| s.planning || !s.progress.is_empty())
    }
    fn send(&self, command: Command) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(|runtime| runtime.send(command))
    }
    fn stop(&mut self) {
        self.send(Command::Stop(None));
        for player in &mut self.players {
            player.stop();
        }
        self.preview_serial += 1;
        self.preparing = None;
    }
    fn view(&mut self, index: usize) {
        for player in &mut self.players {
            player.reset_audio();
        }
        self.preview_serial += 1;
        self.preparing = None;
        self.view_round = Some(index);
        self.played_audio = std::array::from_fn(|_| None);
        self.saved = std::array::from_fn(|_| None);
        self.errors = std::array::from_fn(|_| None);
        self.refresh_candidates();
        self.send(Command::View(index));
    }
    fn refresh_candidates(&mut self) {
        self.candidates = std::array::from_fn(|slot| {
            self.snapshot.as_ref().and_then(|s| {
                let candidate = s
                    .document
                    .rounds
                    .get(self.view_round?)?
                    .candidates
                    .get(slot)?;
                s.candidates.get(&candidate.id).cloned()
            })
        });
    }
    fn audition(&self, index: usize) -> Option<Arc<auris_session::prelude::AudioBuffer>> {
        if let Some(candidate) = &self.candidates[index] {
            return Some(Arc::clone(&candidate.audio));
        }
        let snapshot = self.snapshot.as_ref()?;
        let id = snapshot
            .document
            .rounds
            .get(self.view_round?)?
            .candidates
            .get(index)?
            .id;
        snapshot.previews.get(&id).map(|p| Arc::clone(&p.audio))
    }
    fn text(&self, _: usize) -> &TextField {
        &self.brief
    }
    fn text_mut(&mut self, _: usize) -> &mut TextField {
        &mut self.brief
    }
    pub(crate) fn field(&self) -> Option<&TextField> {
        self.active_field.map(|_| &self.brief)
    }
    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        self.active_field.map(|_| &mut self.brief)
    }
}

impl Drop for SongWorkshop {
    fn drop(&mut self) {
        self.send(Command::Draft(self.brief.content().to_owned()));
    }
}

impl AurisApp {
    pub(crate) fn open_song_workshop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let focus = cx.focus_handle().tab_stop(true);
        let subscription = cx.on_focus(&focus, window, |this, _, cx| {
            if let Some(s) = &mut this.song_workshop {
                s.active_field = Some(0);
            }
            cx.notify();
        });
        let runtime = self
            .session
            .project_folder()
            .ok_or_else(|| "No project workspace".to_string())
            .and_then(|folder| {
                Workshop::start(
                    folder.into(),
                    self.session.path().is_none(),
                    self.settings.clone(),
                )
            });
        let (runtime, error) = match runtime {
            Ok(runtime) => (Some(runtime), None),
            Err(e) => (None, Some(e)),
        };
        self.song_workshop = Some(SongWorkshop {
            identity: NEXT.fetch_add(1, Ordering::Relaxed),
            brief: TextField::default(),
            foci: [focus.clone()],
            active_field: Some(0),
            _focus_subscriptions: vec![subscription],
            preferences: self.settings.agent.clone(),
            models: Vec::new(),
            model_receiver: Some(auris_agent::discover_models_background(
                self.settings.agent.clone(),
            )),
            model_error: None,
            runtime,
            snapshot: None,
            view_round: None,
            candidates: std::array::from_fn(|_| None),
            played_audio: std::array::from_fn(|_| None),
            saved: std::array::from_fn(|_| None),
            errors: std::array::from_fn(|_| None),
            players: std::array::from_fn(|_| PlayerState::default()),
            preview_serial: 0,
            preparing: None,
            error,
            submitted: false,
            show_memory: false,
            scroll: gpui::ScrollHandle::new(),
        });
        focus.focus(window);
        cx.notify();
    }

    fn start_song_workshop(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &mut self.song_workshop else {
            return;
        };
        if s.busy() || s.brief.content().trim().is_empty() {
            return;
        }
        if !s.preferences.is_configured() {
            return;
        }
        if !s.send(Command::Send(
            s.brief.content().to_owned(),
            Box::new(s.preferences.clone()),
        )) {
            s.error =
                Some("The song conversation is unavailable. Close and reopen it to retry.".into());
            cx.notify();
            return;
        }
        s.brief = TextField::default();
        s.submitted = true;
        s.error = None;
        cx.notify();
    }

    pub(crate) fn poll_song_workshop(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &mut self.song_workshop else {
            return;
        };
        if let Some(answer) = s.model_receiver.as_ref().and_then(|r| r.try_recv().ok()) {
            s.model_receiver = None;
            if self.menu.as_ref().is_some_and(|menu| menu.entries.iter().any(|entry| matches!(
                entry,
                super::context_menu::MenuEntry::Item(item)
                    if matches!(item.command, super::context_menu::MenuCommand::WorkshopModel(_) | super::context_menu::MenuCommand::WorkshopRefreshModels)
            ))) {
                self.menu = None;
            }
            match answer.and_then(|text| super::agent_chat::parse_model_list(&text)) {
                Ok(models) => s.models = models,
                Err(e) => s.model_error = Some(e),
            }
            cx.notify();
        }
        if let Some(snapshot) = s.runtime.as_ref().and_then(Workshop::poll) {
            let follow_tail =
                f32::from(s.scroll.max_offset().height) + f32::from(s.scroll.offset().y) < 64.0;
            let initial = s.snapshot.is_none();
            let added = s
                .snapshot
                .as_ref()
                .map_or(0, |old| old.document.rounds.len())
                < snapshot.document.rounds.len();
            if initial && s.brief.content().is_empty() {
                s.brief = TextField::new(snapshot.document.draft.clone());
            }
            s.error = snapshot.error.clone();
            if snapshot.document.messages.len()
                > s.snapshot
                    .as_ref()
                    .map_or(0, |old| old.document.messages.len())
                || snapshot.error.is_some()
            {
                s.submitted = false;
            }
            s.snapshot = Some(snapshot);
            if follow_tail {
                s.scroll.scroll_to_bottom();
            }
            if initial || added {
                if let Some(index) = s
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.document.rounds.len().checked_sub(1))
                {
                    s.view(index);
                }
            } else {
                s.refresh_candidates();
            }
            cx.notify();
        }
    }

    fn adopt_song_candidate(&mut self, index: usize, _: &mut Window, cx: &mut Context<Self>) {
        let selection_request = self.t(Key::WorkshopSelectionRequest).to_string();
        if let Some(s) = &mut self.song_workshop
            && let Some(snapshot) = &s.snapshot
            && let Some(round) = s.view_round.and_then(|i| snapshot.document.rounds.get(i))
            && let Some(candidate) = round.candidates.get(index)
        {
            let message = selection_request.replace("{song}", &candidate.direction.title);
            if s.send(Command::Select(
                candidate.id,
                Box::new(s.preferences.clone()),
                message,
            )) {
                s.submitted = true;
                s.scroll.scroll_to_bottom();
            }
        }
        cx.notify();
    }

    fn open_workshop_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &mut self.song_workshop else {
            return;
        };
        if s.busy() || self.agent_chat.busy {
            return;
        }
        let Some(snapshot) = &s.snapshot else {
            return;
        };
        let Some(candidate) = snapshot
            .document
            .selected
            .and_then(|id| snapshot.candidates.get(&id))
            .cloned()
        else {
            return;
        };
        let Some(folder) = self
            .session
            .project_folder()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let unsaved = self.session.path().is_none();
        let revision = self.session.revision();
        let selected = snapshot.document.selected;
        let mut document = snapshot.document.clone();
        document.draft = s.brief.content().to_owned();
        let identity = s.identity;
        s.submitted = true;
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { auris_agent::workshop::handoff(folder, unsaved, document) })
                    .await;
                let _ = view.update_in(cx, |this, window, cx| {
                    let Some(s) = &mut this.song_workshop else {
                        return;
                    };
                    if s.identity != identity {
                        return;
                    }
                    s.submitted = false;
                    if this.session.revision() != revision
                        || s.snapshot.as_ref().and_then(|s| s.document.selected) != selected
                    {
                        s.error = Some(
                            "The selected song changed while opening the editor. Try again.".into(),
                        );
                        cx.notify();
                        return;
                    }
                    match result {
                        Err(e) => s.error = Some(e),
                        Ok((store, id)) => {
                            if let Err(e) = this.session.adopt_song_candidate(&candidate) {
                                s.error = Some(e.to_string());
                                cx.notify();
                                return;
                            }
                            this.song_workshop = None;
                            this.project_chooser = None;
                            this.open_workshop_conversation(store, id);
                            this.resync_selection();
                            this.reset_view();
                            this.focus_pane(Pane::Arrangement, window);
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        cx.notify();
    }

    /// Asks where to keep one draft as a project of its own, without adopting it.
    fn save_song_candidate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &self.song_workshop else {
            return;
        };
        let Some(candidate) = state.candidates[index].clone() else {
            return;
        };
        let identity = state.identity;
        let language = self.language();
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let handle = rfd::AsyncFileDialog::new()
                    .set_title(Key::DialogSaveProject.get(language))
                    .set_file_name(format!(
                        "{}.{}",
                        candidate.idea.title,
                        auris_session::PROJECT_EXTENSION
                    ))
                    .add_filter(
                        Key::FilterProject.get(language),
                        &[auris_session::PROJECT_EXTENSION],
                    )
                    .save_file()
                    .await;
                let Some(handle) = handle else { return };
                let path = handle.path().to_path_buf();
                let _ = view.update(cx, |this, cx| {
                    this.finish_save_song_candidate(identity, index, candidate, path, cx);
                });
            })
            .detach();
    }

    /// Writes a draft at the chosen path off the window thread and marks its card.
    pub(crate) fn finish_save_song_candidate(
        &mut self,
        identity: u64,
        index: usize,
        candidate: Arc<SongCandidate>,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let kept = Arc::clone(&candidate);
            let saved = cx
                .background_executor()
                .spawn(async move { kept.save_as(&path) })
                .await;
            let _ = view.update(cx, |this, cx| {
                let failure = saved
                    .as_ref()
                    .err()
                    .map(|error| this.failure(Key::WorkshopSave, error));
                // A workshop closed, restarted or regenerated meanwhile is not the one asked.
                let Some(state) = this
                    .song_workshop
                    .as_mut()
                    .filter(|state| state.identity == identity)
                else {
                    return;
                };
                if !state.candidates[index]
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &candidate))
                {
                    return;
                }
                match saved {
                    Ok(report) => {
                        state.saved[index] = Some(report.document);
                        state.errors[index] = None;
                    }
                    Err(_) => state.errors[index] = failure,
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Draws the workshop as Atelier's `auris_screens` compose example lays it out: one centred
    /// column of transcript and candidate cards, the composer under it, and a quiet row of
    /// workshop options beneath that. Back leads the title bar, where every screen keeps it.
    pub(crate) fn render_song_workshop(
        &mut self,
        transition: Option<super::project_chooser::WelcomeTransition>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.render_workshop_input(cx);
        let menu = self
            .render_context_menu(window, cx)
            .map(IntoElement::into_any_element);
        let theme = self.theme.clone();
        let spacing = theme.atelier.spacing;
        let Some(s) = &self.song_workshop else {
            return div().into_any_element();
        };
        let mut transcript = div()
            .flex()
            .flex_col()
            .gap(spacing.xl)
            .w_full()
            .max_w(COLUMN_WIDTH);
        if let Some(snapshot) = &s.snapshot {
            if snapshot.document.messages.is_empty() {
                transcript = transcript.child(text_panel(
                    agent_components::conversation_intro(
                        title_of(self.t(Key::WorkshopTitle)),
                        self.t(Key::WorkshopChatHint),
                        &theme,
                    ),
                    &theme,
                ));
            }
            for (i, message) in snapshot.document.messages.iter().enumerate() {
                let entry = agent_components::workshop_message(
                    ("song-workshop-message", i),
                    message.user,
                    message.text.clone(),
                    &theme,
                    window,
                    cx,
                );
                // The person's own words are already in an opaque bubble.
                transcript = transcript.child(if message.user {
                    entry
                } else {
                    text_panel(entry, &theme)
                });
                for (round_index, round) in snapshot
                    .document
                    .rounds
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.after_message == i + 1)
                {
                    if s.view_round != Some(round_index) {
                        transcript = transcript.child(text_panel(
                            Button::new(
                                ("song-round", round_index),
                                round
                                    .candidates
                                    .iter()
                                    .map(|c| c.direction.title.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                            )
                            .ghost()
                            .icon(IconName::History)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(s) = &mut this.song_workshop {
                                        s.view(round_index);
                                    }
                                    cx.notify();
                                },
                            )),
                            &theme,
                        ));
                        continue;
                    }
                    let mut cards = div()
                        .id(("song-candidates", round_index))
                        .w_full()
                        .min_w_0()
                        .flex_shrink_0()
                        .overflow_x_scroll()
                        .flex()
                        .items_start()
                        .gap(spacing.lg);
                    for (index, candidate) in round.candidates.iter().enumerate() {
                        cards = cards.child(self.candidate_card(index, candidate, snapshot, cx));
                    }
                    transcript = transcript.child(cards);
                }
            }
            if snapshot.planning {
                transcript = transcript.child(text_panel(
                    agent_components::thinking_indicator(
                        "song-planning-activity",
                        self.t(Key::WorkshopPlanning),
                    ),
                    &theme,
                ));
            }
            if s.show_memory {
                transcript =
                    transcript.child(text_panel(self.workshop_memory(snapshot, cx), &theme));
            }
        } else {
            transcript = transcript.child(text_panel(
                agent_components::thinking_indicator(
                    "song-loading-activity",
                    self.t(Key::WorkshopLoadingModels),
                ),
                &theme,
            ));
        }
        if let Some(error) = &s.error {
            transcript = transcript.child(text_panel(
                agent_components::error_notice(error.clone()),
                &theme,
            ));
        }
        let has_messages = s
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.document.messages.is_empty());
        let veil = workshop_veil(has_messages, -s.scroll.offset().y);
        let body = div()
            .id("song-workshop-body")
            .debug_selector(|| "song-workshop-body".into())
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&s.scroll)
            // The veil follows the scroll position, which the view reads while drawing; a wheel
            // that moves the transcript has to ask for the next drawing.
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .px(spacing.xxl)
            .py(spacing.xl)
            .flex()
            .flex_col()
            .items_center()
            .child(transcript);
        let selected = s
            .snapshot
            .as_ref()
            .is_some_and(|s| s.document.selected.is_some());
        let memory = s.snapshot.as_ref().is_some_and(|s| s.preferences.enabled);
        let can_open = selected && !s.busy();
        // On a surface of its own for the same reason the transcript's text is: see `text_panel`.
        let options = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(spacing.md)
            .w_full()
            .px(spacing.md)
            .py(spacing.xs)
            .rounded(theme.atelier.shape.radius_lg)
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border_subtle)
            .child(
                div().debug_selector(|| "song-memory".into()).child(
                    atelier::Switch::new("song-memory", memory)
                        .label(self.t(Key::WorkshopMemory))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = &this.song_workshop {
                                s.send(Command::Memory(!memory));
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(
                div().debug_selector(|| "song-memory-view".into()).child(
                    Button::new("song-memory-view", self.t(Key::WorkshopMemoryView))
                        .ghost()
                        .icon(IconName::History)
                        .selected(s.show_memory)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(s) = &mut this.song_workshop {
                                s.show_memory = !s.show_memory;
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(div().flex_1())
            .child(
                div().debug_selector(|| "song-editor".into()).child(
                    Button::new("song-editor", self.t(Key::WorkshopOpenEditor))
                        .disabled(!can_open)
                        .when(!can_open, |button| {
                            button.tooltip(self.t(Key::WorkshopOpenEditorHint))
                        })
                        .on_click(cx.listener(Self::open_workshop_editor_click)),
                ),
            );
        let footer = div()
            .id("song-workshop-footer")
            .debug_selector(|| "song-workshop-footer".into())
            .w_full()
            .max_w(COLUMN_WIDTH + spacing.xxl * 2.0)
            .mx_auto()
            .flex_shrink_0()
            .px(spacing.xxl)
            .pb(spacing.lg)
            .flex()
            .flex_col()
            .gap(spacing.md)
            .child(input)
            .child(options);
        let screen = div()
            .size_full()
            .flex()
            .flex_col()
            .child(body)
            .child(footer);
        let back = div().debug_selector(|| "song-back".into()).child(
            Button::new("song-back", self.t(Key::WorkshopBack))
                .ghost()
                .icon(IconName::ChevronLeft)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.song_workshop = None;
                    this.session.stop_output_preview();
                    this.begin_welcome_transition(false, veil, cx);
                    this.focus.focus(window);
                    cx.notify();
                })),
        );
        let mut title = crate::titlebar::Parts::new(
            "song-workshop-title",
            title_of(self.t(Key::WorkshopTitle)),
        )
        .leading(back);
        if let Some(subtitle) = self.workshop_subtitle() {
            title = title.subtitle(subtitle);
        }
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.atelier.window_fill())
            .text_color(theme.text)
            .font(theme.font.clone())
            .text_sm()
            .track_focus(&self.focus)
            .key_context(crate::actions::KEY_CONTEXT)
            .on_key_down(cx.listener(Self::workshop_key))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                this.move_audio_player(event, cx);
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.audio_player_drag = None),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.audio_player_drag = None),
            )
            .on_action(
                cx.listener(|_, _: &crate::actions::FocusNextPane, window, _| window.focus_next()),
            )
            .on_action(
                cx.listener(|_, _: &crate::actions::FocusPreviousPane, window, _| {
                    window.focus_prev()
                }),
            )
            .on_action(cx.listener(|this, _: &crate::actions::Quit, window, cx| {
                this.quit_asking(window, cx)
            }))
            .child(crate::titlebar::composed(
                window,
                &theme,
                title,
                crate::titlebar::controls(
                    window,
                    &theme,
                    cx.listener(|_, _, window, _| window.remove_window()),
                ),
            ))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    // The metal the start panel floated over stays beneath this screen, behind a
                    // veil of the surface colour: thin while the conversation is empty, thicker
                    // once there is text to read, and solid once the transcript has been
                    // scrolled. While the start panel grows into this screen the travelling
                    // shape is the veil, and it arrives at the veil's thickness.
                    .when(veil < 1.0 || transition.is_some(), |area| {
                        area.children(self.welcome_backdrop(window))
                    })
                    .child(match transition {
                        Some(transition) => self.welcome_morph(transition, window),
                        None => workshop_veil_layer(has_messages, veil, &theme),
                    })
                    .child(match transition {
                        Some(transition) => self.welcome_arrival(transition, screen),
                        None => screen.into_any_element(),
                    }),
            )
            .children(menu)
            .children(crate::titlebar::resize_handles(window))
            .into_any_element()
    }

    /// One alternative: what it is, how far it has got, its player and what can be done with it.
    fn candidate_card(
        &self,
        index: usize,
        candidate: &auris_session::workshop::Candidate,
        snapshot: &Snapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let spacing = theme.atelier.spacing;
        let colors = &theme.atelier.colors;
        let s = self.song_workshop.as_ref().expect("visible workshop");
        let id = candidate.id;
        let ready = s.candidates[index].is_some();
        let audition = s.audition(index);
        let running = snapshot.progress.contains_key(&id);
        let selected = snapshot.document.selected == Some(id);
        let mut badges = div().flex().flex_wrap().gap(spacing.xs);
        if selected {
            badges = badges.child(
                Badge::new(self.t(Key::WorkshopSelected))
                    .tone(Tone::Accent)
                    .icon(IconName::Check),
            );
        }
        if let Some(audio) = audition.as_ref() {
            let seconds = audio.duration_seconds().round() as u64;
            badges = badges.child(Badge::new(format!("{}:{:02}", seconds / 60, seconds % 60)));
            if !ready {
                badges = badges.child(Badge::new(self.t(Key::WorkshopDraftPreview)));
            }
        }
        let mut card = div()
            .id(("song-candidate", index))
            .debug_selector(move || format!("song-candidate-{index}"))
            .flex()
            .flex_col()
            .flex_1()
            .min_w(gpui::px(300.0))
            .gap(spacing.md)
            .p(spacing.lg)
            .rounded(theme.atelier.shape.radius_lg)
            .border_1()
            // The agent's colour marks only work the agent is doing: a card being written. The
            // song the person chose is their choice, and carries the accent.
            // Mixed into the card's own fill rather than laid over it, so the card stays opaque
            // over the metal beneath the transcript.
            .when(running, |card| {
                card.bg(atelier::mix(colors.elevated, colors.agent, 0.12))
                    .border_color(colors.agent.opacity(0.35))
            })
            .when(!running, |card| {
                card.bg(colors.elevated).border_color(if selected {
                    colors.accent
                } else {
                    colors.border
                })
            })
            .child(
                Label::new(candidate.direction.title.clone())
                    .size(TextSize::Large)
                    .heading_weight(),
            );
        if let Some(stage) = snapshot.progress.get(&id) {
            badges = badges.child(
                Badge::new(self.t(Key::WorkshopGenerating))
                    .tone(Tone::Agent)
                    .dot(),
            );
            card = card.child(badges).child(
                Label::new(candidate.direction.description.clone())
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
            );
            let label = self.t(match stage.as_str() {
                "harmony" => Key::WorkshopHarmonyProgress,
                "melody" => Key::WorkshopMelodyProgress,
                "compose" => Key::WorkshopComposing,
                "sing" => Key::WorkshopSinging,
                "render" => Key::WorkshopRendering,
                "render_wait" => Key::WorkshopQueued,
                "stopping" => Key::WorkshopStopping,
                "planning" => Key::WorkshopPlanning,
                "recovering" => Key::WorkshopRecovering,
                _ => Key::WorkshopArrangeProgress,
            });
            card = card.child(
                div()
                    .debug_selector(move || format!("song-candidate-activity-{index}"))
                    .flex()
                    .flex_col()
                    .gap(spacing.sm)
                    .p(spacing.md)
                    .rounded(theme.atelier.shape.radius_md)
                    .bg(colors.surface)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(spacing.sm)
                            .child(atelier::Spinner::new(("song-candidate-spinner", index)))
                            .child(Label::new(label).size(TextSize::Small)),
                    )
                    .child(
                        atelier::ProgressBar::indeterminate(("song-candidate-progress", index))
                            .tone(Tone::Agent),
                    ),
            );
        } else {
            card = card.child(badges).child(
                Label::new(candidate.direction.description.clone())
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
            );
        }
        for error in [
            candidate.error.as_ref(),
            s.errors[index].as_ref(),
            snapshot.preview_errors.get(&id),
        ]
        .into_iter()
        .flatten()
        {
            card = card.child(
                div()
                    .flex()
                    .items_start()
                    .gap(spacing.xs)
                    .text_size(theme.atelier.typography.size_sm)
                    .text_color(theme.danger)
                    .child(atelier::Icon::new(IconName::Alert).color(theme.danger))
                    .child(div().flex_1().min_w_0().child(error.clone())),
            );
        }
        if let Some(saved) = &s.saved[index] {
            card = card.child(
                Label::new(messages::saved(
                    self.language(),
                    &saved.display().to_string(),
                ))
                .size(TextSize::Small)
                .color(TextColor::Faint),
            );
        }
        if candidate.recoveries > 0 {
            card = card.child(
                Label::new(format!(
                    "{} {}/{}",
                    self.t(Key::WorkshopRecoveryCount),
                    candidate.recoveries,
                    auris_agent::workshop::AUTO_RECOVERY_LIMIT
                ))
                .size(TextSize::Small)
                .color(TextColor::Faint),
            );
        }
        if !running
            && !candidate.ready
            && candidate.recoveries >= auris_agent::workshop::AUTO_RECOVERY_LIMIT
        {
            card = card.child(
                Label::new(self.t(Key::WorkshopRecoveryLimit))
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
            );
        }
        card = card.child(
            self.render_audio_player(
                PlayerTarget::Workshop(index),
                audition
                    .as_ref()
                    .map_or(0.0, |audio| audio.duration_seconds()),
                audition.is_some() && s.preparing.is_none(),
                s.preparing == Some(index),
                cx,
            ),
        );
        if audition.as_ref().is_some_and(|latest| {
            s.played_audio[index]
                .as_ref()
                .is_some_and(|played| !Arc::ptr_eq(latest, played))
        }) {
            card = card.child(
                div()
                    .debug_selector(move || format!("song-preview-latest-{index}"))
                    .child(
                        Button::new(
                            ("song-preview-latest", index),
                            self.t(Key::WorkshopLatestPreview),
                        )
                        .ghost()
                        .icon(IconName::Refresh)
                        .disabled(s.preparing.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(state) = &mut this.song_workshop {
                                state.players[index].reset_audio();
                            }
                            this.preview_song_candidate(index, cx);
                        })),
                    ),
            );
        }
        let usable = ready && !running;
        let retry_enabled = running || (!snapshot.planning && snapshot.progress.len() < 3);
        let unfinished = !candidate.ready;
        card.child(
            div()
                .flex()
                .flex_wrap()
                .gap(spacing.sm)
                .child(
                    div()
                        .debug_selector(move || format!("song-adopt-{index}"))
                        .child(
                            Button::new(("song-adopt", index), self.t(Key::WorkshopContinue))
                                .icon(IconName::Check)
                                .disabled(!usable)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.adopt_song_candidate(index, window, cx)
                                })),
                        ),
                )
                .child(
                    div()
                        .debug_selector(move || format!("song-retry-{index}"))
                        .child(
                            Button::new(
                                ("song-retry", index),
                                self.t(if running {
                                    Key::WorkshopCancel
                                } else if unfinished {
                                    Key::WorkshopRecover
                                } else {
                                    Key::WorkshopRetry
                                }),
                            )
                            .ghost()
                            .icon(if running {
                                IconName::Stop
                            } else {
                                IconName::Refresh
                            })
                            .disabled(!retry_enabled)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(s) = &this.song_workshop {
                                        s.send(if running {
                                            Command::Stop(Some(id))
                                        } else if unfinished {
                                            Command::Recover(id, Box::new(s.preferences.clone()))
                                        } else {
                                            Command::Retry(id)
                                        });
                                    }
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .debug_selector(move || format!("song-save-{index}"))
                        .child(
                            Button::new(("song-save", index), self.t(Key::WorkshopSave))
                                .ghost()
                                .icon(IconName::Folder)
                                .disabled(!usable)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.save_song_candidate(index, window, cx)
                                })),
                        ),
                ),
        )
        .into_any_element()
    }

    /// The choices the workshop remembers, each of which can be forgotten.
    fn workshop_memory(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        let spacing = self.theme.atelier.spacing;
        let mut list = div().flex().flex_col().gap(spacing.xs).child(
            SectionHeader::new(self.t(Key::WorkshopMemoryView)).action(
                Button::new("song-forget", self.t(Key::WorkshopForget))
                    .ghost()
                    .icon(IconName::Trash)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(s) = &this.song_workshop {
                            s.send(Command::Forget);
                        }
                        cx.notify();
                    })),
            ),
        );
        for (index, selection) in snapshot.preferences.selections.iter().enumerate() {
            let Some(chosen) = selection.alternatives.get(selection.chosen) else {
                continue;
            };
            let conversation = selection.conversation.clone();
            let candidate = selection.candidate;
            list = list.child(
                ListItem::new(
                    ("song-memory-item", index),
                    format!("{} — {}", chosen.title, chosen.description),
                )
                .icon(IconName::Music)
                .description(selection.context.clone())
                .trailing(
                    Button::new(
                        ("song-forget-choice", index),
                        self.t(Key::WorkshopForgetChoice),
                    )
                    .ghost()
                    .size(atelier::ControlSize::Small)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(s) = &this.song_workshop {
                            s.send(Command::ForgetChoice(conversation.clone(), candidate));
                        }
                        cx.notify();
                    })),
                ),
            );
        }
        list.into_any_element()
    }

    /// How the current round stands, beside the title: how many alternatives, how many still
    /// being written.
    fn workshop_subtitle(&self) -> Option<String> {
        let snapshot = self.song_workshop.as_ref()?.snapshot.as_ref()?;
        let round = snapshot.document.rounds.last()?;
        let total = round.candidates.len();
        let running = round
            .candidates
            .iter()
            .filter(|candidate| snapshot.progress.contains_key(&candidate.id))
            .count();
        Some(messages::workshop_round(self.language(), total, running))
    }

    fn open_workshop_editor_click(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workshop_editor(window, cx);
    }
}

/// How thick the surface-coloured veil over the metal is, from 0 (bare metal) to 1 (solid).
///
/// An empty conversation is the welcome continued, so the metal shows through. Once there is a
/// transcript the veil thickens enough to read over, and scrolling it — which is reading it —
/// takes the veil the rest of the way to solid over the first [`VEIL_SCROLL`] of travel.
pub(crate) fn workshop_veil(has_messages: bool, scrolled: gpui::Pixels) -> f32 {
    if !has_messages {
        return EMPTY_VEIL;
    }
    let travel = (f32::from(scrolled) / f32::from(VEIL_SCROLL)).clamp(0.0, 1.0);
    atelier::motion::lerp(LOG_VEIL, 1.0, travel)
}

/// An opaque surface under text in the transcript.
///
/// The transcript sits over the metal behind a thin veil, which is there to let the welcome show
/// through, not to be read on. Whatever carries words gets a surface of its own, as the person's
/// bubbles, the candidate cards and the composer already have, so text never lies on the metal.
fn text_panel(content: impl IntoElement, theme: &crate::theme::Theme) -> AnyElement {
    let atelier = &theme.atelier;
    div()
        .w_full()
        .min_w_0()
        .px(atelier.spacing.lg)
        .py(atelier.spacing.md)
        .rounded(atelier.shape.radius_lg)
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border_subtle)
        .child(content)
        .into_any_element()
}

/// The veil over an empty conversation.
const EMPTY_VEIL: f32 = 0.4;
/// The veil over a transcript that has not been scrolled.
const LOG_VEIL: f32 = 0.78;
/// How far the transcript scrolls before the veil is solid.
const VEIL_SCROLL: gpui::Pixels = gpui::px(160.0);

/// The veil itself. Its first thickening, when the first message arrives, eases in rather than
/// jumping; after that it follows the scroll position directly.
fn workshop_veil_layer(has_messages: bool, veil: f32, theme: &crate::theme::Theme) -> AnyElement {
    let layer = div().absolute().inset_0().bg(theme.surface);
    if !has_messages {
        return layer.opacity(veil).into_any_element();
    }
    gpui::AnimationExt::with_animation(
        layer,
        "workshop-veil-thickens",
        gpui::Animation::new(theme.atelier.motion.normal).with_easing(atelier::motion::ease_out),
        move |layer, delta| layer.opacity(atelier::motion::lerp(EMPTY_VEIL, veil, delta)),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_metal_shows_through_an_empty_conversation_and_is_veiled_as_the_log_is_read() {
        let empty = workshop_veil(false, gpui::px(0.0));
        let unread = workshop_veil(true, gpui::px(0.0));
        let partway = workshop_veil(true, VEIL_SCROLL / 2.0);
        let read = workshop_veil(true, VEIL_SCROLL * 3.0);
        assert!(empty < unread && unread < partway && partway < read);
        assert!(
            empty <= 0.5,
            "an empty conversation leaves the metal visible"
        );
        assert_eq!(read, 1.0, "a scrolled transcript reads on a plain surface");
        // Scrolling an empty conversation, if it ever overflowed, keeps the welcome.
        assert_eq!(workshop_veil(false, VEIL_SCROLL * 3.0), empty);
    }
    use auris_session::SongIdea;
    use std::sync::atomic::AtomicBool;

    fn make_ready(app: &gpui::Entity<AurisApp>, cx: &mut gpui::VisualTestContext) {
        app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            s.runtime = None;
            s.model_receiver = None;
            s.snapshot = Some(Snapshot {
                document: Default::default(),
                candidates: Default::default(),
                previews: Default::default(),
                preview_errors: Default::default(),
                progress: Default::default(),
                planning: false,
                preferences: Default::default(),
                error: None,
            });
        });
    }

    #[gpui::test]
    fn candidates_stay_horizontal_and_identify_active_workers(cx: &mut gpui::TestAppContext) {
        use auris_session::workshop::{Candidate, Direction, Message, Round};

        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            s.view_round = Some(0);
            let snapshot = s.snapshot.as_mut().unwrap();
            snapshot.document.messages.push(Message {
                user: false,
                text: "Compare these songs".into(),
            });
            snapshot.document.rounds.push(Round {
                after_message: 1,
                candidates: (0..3)
                    .map(|id| Candidate {
                        id,
                        parent: None,
                        ready: false,
                        error: None,
                        recoveries: 0,
                        direction: Direction {
                            title: format!("Candidate {id}"),
                            description: "Piano and strings".into(),
                            instructions: String::new(),
                            protected_tracks: Vec::new(),
                        },
                    })
                    .collect(),
            });
            snapshot.progress.insert(0, "melody".into());
            snapshot.planning = true;
        });
        for width in [1200.0, 800.0] {
            crate::harness::resize(&app, cx, gpui::size(gpui::px(width), gpui::px(800.0)));
            let a = cx.debug_bounds("song-candidate-0").unwrap();
            let b = cx.debug_bounds("song-candidate-1").unwrap();
            let c = cx.debug_bounds("song-candidate-2").unwrap();
            assert_eq!(a.origin.y, b.origin.y);
            assert_eq!(b.origin.y, c.origin.y);
            assert!(a.right() < b.left() && b.right() < c.left());
            assert!(a.size.width >= gpui::px(300.0));
            assert!(cx.debug_bounds("song-candidate-activity-0").is_some());
            assert!(cx.debug_bounds("song-candidate-activity-1").is_none());
            assert!(cx.debug_bounds("song-planning-activity").is_some());
        }
    }

    #[gpui::test]
    fn the_agents_answers_are_rendered_as_markdown(cx: &mut gpui::TestAppContext) {
        use auris_session::workshop::Message;

        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        app.update(cx, |this, _| {
            let snapshot = this
                .song_workshop
                .as_mut()
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap();
            snapshot.document.messages.push(Message {
                user: false,
                text: "Plan\n\n- **Tempo:** 150 BPM\n- **Key:** C minor".into(),
            });
        });
        crate::harness::resize(&app, cx, gpui::size(gpui::px(1200.0), gpui::px(800.0)));
        // The same blocks the editor's transcript draws for a list: one per item, not one line of
        // asterisks and dashes.
        for selector in [
            "workshop-markdown-song-workshop-message-0-1-0-0-block",
            "workshop-markdown-song-workshop-message-0-1-1-0-block",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector} is drawn");
        }
    }

    #[gpui::test]
    fn workshop_body_and_composer_stay_visible_at_narrow_width(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        crate::harness::resize(&app, cx, gpui::size(gpui::px(900.0), gpui::px(600.0)));

        let body = cx
            .debug_bounds("song-workshop-body")
            .expect("the workshop body remains visible");
        let footer = cx
            .debug_bounds("song-workshop-footer")
            .expect("the workshop composer footer remains visible");
        let brief = cx
            .debug_bounds("song-brief")
            .expect("the workshop composer remains reachable");
        assert!(body.bottom() <= footer.top());
        assert!(brief.left() >= footer.left());
        assert!(brief.right() <= footer.right());
        assert!(brief.bottom() <= footer.bottom());
    }

    #[gpui::test]
    fn single_composer_accepts_japanese_and_shift_enter_without_changing_score(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let before = app.read_with(cx, |this, _| this.session.project().clone());
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("song-brief", cx);
        cx.simulate_input("静かなピアノ曲。歌詞は");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("さくらさいた");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().brief.content(),
                "静かなピアノ曲。歌詞は\nさくらさいた"
            );
            assert_eq!(this.session.project(), &before);
        });
    }

    #[gpui::test]
    fn changing_comparison_invalidates_pending_audition(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| app.update(cx, |this, cx| this.open_song_workshop(window, cx)));
        make_ready(&app, cx);
        app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            s.preparing = Some(0);
            let serial = s.preview_serial;
            s.view(1);
            assert!(s.preparing.is_none());
            assert_ne!(serial, s.preview_serial);
        });
    }

    #[gpui::test]
    fn unfinished_music_can_play_and_refresh_without_becoming_adoptable(
        cx: &mut gpui::TestAppContext,
    ) {
        use auris_session::{
            prelude::AudioBuffer,
            workshop::{Candidate, Direction, Round},
        };
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        let tone = |frames| {
            let mut audio = AudioBuffer::stereo(frames, 22_050.0);
            for channel in audio.channels_mut() {
                for (i, sample) in channel.iter_mut().enumerate() {
                    *sample = (i as f32 * 0.1).sin() * 0.1;
                }
            }
            Arc::new(audio)
        };
        let first = tone(22_050);
        let latest = tone(44_100);
        let before = app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            s.view_round = Some(0);
            let snapshot = s.snapshot.as_mut().unwrap();
            snapshot
                .document
                .messages
                .push(auris_session::workshop::Message {
                    user: false,
                    text: "Writing this candidate".into(),
                });
            snapshot.document.rounds.push(Round {
                after_message: 1,
                candidates: vec![Candidate {
                    id: 1,
                    parent: None,
                    ready: false,
                    error: None,
                    recoveries: 0,
                    direction: Direction {
                        title: "Draft".into(),
                        description: "A melody being written".into(),
                        instructions: "Continue".into(),
                        protected_tracks: vec![],
                    },
                }],
            });
            snapshot.progress.insert(1, "melody".into());
            snapshot.previews.insert(
                1,
                auris_agent::workshop::Preview {
                    revision: 1,
                    audio: Arc::clone(&first),
                },
            );
            this.session.project().clone()
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-preview-0-play", cx);
        cx.run_until_parked();
        app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            assert!(
                s.played_audio[0]
                    .as_ref()
                    .is_some_and(|audio| Arc::ptr_eq(audio, &first))
            );
            s.snapshot.as_mut().unwrap().previews.insert(
                1,
                auris_agent::workshop::Preview {
                    revision: 2,
                    audio: Arc::clone(&latest),
                },
            );
            // Arrival of new audio does not swap the buffer the user chose to hear.
            assert!(Arc::ptr_eq(s.played_audio[0].as_ref().unwrap(), &first));
            assert!(s.candidates[0].is_none());
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-adopt-0", cx);
        app.read_with(cx, |this, _| {
            assert!(
                this.song_workshop
                    .as_ref()
                    .unwrap()
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .document
                    .selected
                    .is_none()
            )
        });
        crate::harness::click("song-preview-latest-0", cx);
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert!(
                s.played_audio[0]
                    .as_ref()
                    .is_some_and(|audio| Arc::ptr_eq(audio, &latest))
            );
            assert_eq!(this.session.project(), &before);
        });
    }

    #[gpui::test]
    fn model_selection_does_not_change_global_settings(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let before = app.read_with(cx, |this, _| this.settings.agent.clone());
        cx.update(|window, cx| app.update(cx, |this, cx| this.open_song_workshop(window, cx)));
        make_ready(&app, cx);
        app.update(cx, |this, _| {
            this.song_workshop.as_mut().unwrap().models =
                vec![super::super::agent_chat::ModelOption {
                    name: "test-model".into(),
                    ..Default::default()
                }];
            this.choose_workshop_model(0);
            assert_eq!(
                this.song_workshop.as_ref().unwrap().preferences.model,
                "test-model"
            );
            assert_eq!(this.settings.agent, before);
        });
    }
    #[gpui::test]
    fn an_unchosen_draft_is_kept_as_a_project_without_touching_the_open_one(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| app.update(cx, |app, cx| app.open_song_workshop(window, cx)));
        make_ready(&app, cx);
        let candidate = Arc::new(
            SongCandidate::generate(
                SongIdea {
                    title: "Second".into(),
                    description: "Bright keys".into(),
                    preset: "chiptune".into(),
                    tempo: 120.0,
                    key: "C major".into(),
                    density: 0.5,
                    lead: "piano".into(),
                },
                "Bright instrumental",
                30,
                42,
                &AtomicBool::new(false),
            )
            .unwrap(),
        );
        let root = tempfile::tempdir().unwrap();
        let chosen = root.path().join("Second.auris");
        let before = app.read_with(cx, |app, _| app.session.project().clone());
        let identity = app.update(cx, |app, _| {
            let state = app.song_workshop.as_mut().unwrap();
            state.candidates[1] = Some(Arc::clone(&candidate));
            state.identity
        });
        for _ in 0..2 {
            let (candidate, chosen) = (Arc::clone(&candidate), chosen.clone());
            app.update(cx, |app, cx| {
                app.finish_save_song_candidate(identity, 1, candidate, chosen, cx)
            });
            cx.run_until_parked();
        }
        let document = root.path().join("Second/Second.auris");
        assert!(document.is_file());
        app.read_with(cx, |app, _| {
            let state = app.song_workshop.as_ref().unwrap();
            assert_eq!(state.saved[1].as_deref(), Some(document.as_path()));
            // The second save named the same folder, and was refused rather than replacing it.
            assert!(state.errors[1].is_some());
            assert!(state.saved[0].is_none() && state.saved[2].is_none());
            assert_eq!(app.session.project(), &before);
        });
    }
    #[gpui::test]
    fn a_sheet_left_behind_the_chooser_does_not_take_the_workshop_text(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                // What the recovery offer at launch leaves: a question with no field, open
                // underneath a chooser that draws nothing of it.
                app.open_prompt(crate::ui::prompt::Prompt::ask(
                    "Remove",
                    crate::ui::prompt::Question::RemovePluginPath("plugins".into()),
                ));
                app.show_project_chooser(window, cx);
                app.open_song_workshop(window, cx);
            })
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-brief", cx);
        cx.simulate_input("夜の雨");
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.song_workshop.as_ref().unwrap().brief.content(),
                "夜の雨"
            );
            assert!(app.prompt.is_some());
        });
    }
}
