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
static NEXT_WORKSHOP: AtomicU64 = AtomicU64::new(1);

/// A command's label used as a title: without the ellipsis that says the command asks for more.
fn title_of(label: &str) -> String {
    label.trim_end_matches(['…', '.']).to_owned()
}

fn tool_counts(template: &str, usage: &auris_session::workshop::ToolUsage) -> String {
    template
        .replace("{total}", &usage.total.to_string())
        .replace("{succeeded}", &usage.succeeded.to_string())
        .replace("{failed}", &usage.failed.to_string())
        .replace("{running}", &usage.running().to_string())
        .replace("{interrupted}", &usage.interrupted.to_string())
}

#[path = "song_workshop/chat_input.rs"]
mod input;
mod input_assist;
#[path = "song_workshop/preview.rs"]
mod preview;
mod queue;
mod threads;

pub(crate) struct SongWorkshop {
    pub(crate) identity: u64,
    pub(crate) references: super::agent_chat::music::MusicReferences,
    pub(crate) brief: TextField,
    pub(crate) foci: [FocusHandle; 1],
    pub(crate) active_field: Option<usize>,
    _focus_subscriptions: Vec<gpui::Subscription>,
    preferences: auris_session::AgentPreferences,
    models: Vec<super::agent_chat::ModelOption>,
    model_receiver: Option<mpsc::Receiver<Result<String, String>>>,
    model_error: Option<String>,
    reasoning_picker: Option<super::reasoning_picker::Handle>,
    completions: super::chat_commands::Completions,
    command_feedback: Option<Result<String, String>>,
    input_assist: input_assist::InputAssist,
    input_focused: bool,
    runtime: Option<Workshop>,
    snapshot: Option<Snapshot>,
    candidates: Vec<Option<Arc<SongCandidate>>>,
    played_audio: Vec<Option<Arc<auris_session::prelude::AudioBuffer>>>,
    played_timeline: Vec<Option<Arc<auris_session::AuditionTimeline>>>,
    saved: Vec<Option<std::path::PathBuf>>,
    errors: Vec<Option<String>>,
    pub(crate) players: Vec<PlayerState>,
    preview_serial: u64,
    preparing: Option<usize>,
    error: Option<String>,
    submitted: bool,
    /// Messages written while the director or a composer was still at work, oldest first.
    queued: std::collections::VecDeque<queue::QueuedBrief>,
    /// Set by Stop and by a send the workshop refused, so the queue does not empty itself into
    /// a conversation the user has just asked to be quiet.
    queue_paused: bool,
    /// The queued message the workshop has been handed and not yet acknowledged. Unlike a send
    /// from the composer, the composer is not holding its text, so it stays editable meanwhile.
    in_flight: Option<queue::QueuedBrief>,
    chord_prompt_open: bool,
    show_memory: bool,
    expanded_tools: std::collections::BTreeSet<u64>,
    scroll: gpui::ScrollHandle,
    switching: bool,
    show_archived_threads: bool,
    pending_thread_name: Option<String>,
}

impl SongWorkshop {
    /// Release outdated audition snapshots once playback and preparation are idle.
    pub(super) fn sync_idle_auditions(&mut self) -> bool {
        let mut changed = false;
        for index in 0..self.players.len() {
            if preview::should_refresh_audition(
                self.played_audio[index].as_ref(),
                self.audition(index).as_ref(),
                self.players[index].playing(),
                self.preparing == Some(index),
            ) {
                self.players[index].stop();
                self.played_audio[index] = None;
                self.played_timeline[index] = None;
                changed = true;
            }
        }
        changed
    }
    /// Send feedback only to the exact conversation and candidate that opened the sheet.
    pub(super) fn send_feedback(
        &self,
        identity: u64,
        id: u64,
        feedback: auris_session::workshop::Feedback,
    ) -> bool {
        self.identity == identity
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .document
                    .rounds
                    .iter()
                    .flat_map(|r| &r.candidates)
                    .any(|c| c.id == id && c.ready)
            })
            && !self.switching
            && self.send(Command::Feedback(
                self.snapshot
                    .as_ref()
                    .map_or("default", |s| s.document.thread_id.as_str())
                    .to_owned(),
                id,
                feedback,
            ))
    }
    /// Whether production or a conversation switch is in progress.
    pub(crate) fn busy(&self) -> bool {
        self.switching
            || self.submitted
            || self
                .snapshot
                .as_ref()
                .is_none_or(|s| s.planning || !s.progress.is_empty())
    }
    /// Queue a command for the background conversation owner.
    pub(crate) fn send(&self, command: Command) -> bool {
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
    fn load_candidate_audio(&mut self) {
        for player in &mut self.players {
            player.reset_audio();
        }
        self.preview_serial += 1;
        self.preparing = None;
        self.refresh_candidates();
        if let Some(snapshot) = &self.snapshot {
            for round in 0..snapshot.document.rounds.len() {
                self.send(Command::View(round));
            }
        }
    }
    fn refresh_candidates(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        self.candidates = snapshot
            .document
            .rounds
            .iter()
            .flat_map(|round| &round.candidates)
            .map(|candidate| snapshot.candidates.get(&candidate.id).cloned())
            .collect();
        let count = self.candidates.len().max(3);
        self.candidates.resize_with(count, || None);
        self.players.resize_with(count, PlayerState::default);
        self.played_audio.resize_with(count, || None);
        self.played_timeline.resize_with(count, || None);
        self.saved.resize_with(count, || None);
        self.errors.resize_with(count, || None);
    }
    fn audition(&self, index: usize) -> Option<Arc<auris_session::prelude::AudioBuffer>> {
        if let Some(candidate) = &self.candidates[index] {
            return Some(Arc::clone(&candidate.audio));
        }
        let snapshot = self.snapshot.as_ref()?;
        let id = snapshot
            .document
            .rounds
            .iter()
            .flat_map(|round| &round.candidates)
            .nth(index)?
            .id;
        snapshot.previews.get(&id).map(|p| Arc::clone(&p.audio))
    }
    pub(super) fn audition_timeline(
        &self,
        index: usize,
    ) -> Option<Arc<auris_session::AuditionTimeline>> {
        if let Some(timeline) = &self.played_timeline[index] {
            return Some(Arc::clone(timeline));
        }
        self.latest_timeline(index)
    }
    fn latest_timeline(&self, index: usize) -> Option<Arc<auris_session::AuditionTimeline>> {
        if let Some(candidate) = &self.candidates[index] {
            return Some(Arc::clone(&candidate.timeline));
        }
        let snapshot = self.snapshot.as_ref()?;
        let id = snapshot
            .document
            .rounds
            .iter()
            .flat_map(|round| &round.candidates)
            .nth(index)?
            .id;
        snapshot.previews.get(&id).map(|p| Arc::clone(&p.timeline))
    }
    fn text(&self, _: usize) -> &TextField {
        &self.brief
    }
    fn text_mut(&mut self, _: usize) -> &mut TextField {
        &mut self.brief
    }
    /// Whether the composer's text is the message the workshop is still acknowledging.
    fn brief_locked(&self) -> bool {
        (self.submitted && self.in_flight.is_none()) || self.switching
    }
    pub(crate) fn field(&self) -> Option<&TextField> {
        if self.brief_locked() {
            return None;
        }
        match self.active_field {
            Some(_) => Some(&self.brief),
            None => None,
        }
    }
    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        if self.brief_locked() {
            return None;
        }
        match self.active_field {
            Some(_) => Some(&mut self.brief),
            None => None,
        }
    }
}

impl Drop for SongWorkshop {
    fn drop(&mut self) {
        self.send(Command::ThreadDraft(
            self.snapshot
                .as_ref()
                .map_or("default", |s| s.document.thread_id.as_str())
                .to_owned(),
            self.brief.content().to_owned(),
            self.references.attachments.clone(),
        ));
    }
}

impl AurisApp {
    fn open_workshop_feedback(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(state) = &self.song_workshop else {
            return;
        };
        let Some(candidate) = state.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .document
                .rounds
                .iter()
                .flat_map(|r| &r.candidates)
                .find(|c| c.id == id && c.ready)
        }) else {
            return;
        };
        let feedback = candidate.feedback.as_ref();
        let prompt = super::prompt::Prompt::new(
            self.t(Key::WorkshopFeedbackTitle)
                .replace("{song}", &candidate.direction.title),
            super::prompt::PromptTarget::WorkshopFeedback {
                workshop: state.identity,
                candidate: id,
                rating: feedback.map_or(0, |f| f.rating),
            },
            feedback.map_or_else(String::new, |f| f.comment.clone()),
        )
        .with_feedback_focus(cx);
        self.open_prompt(prompt);
    }
    pub(crate) fn workshop_attach_files(
        &mut self,
        paths: &[std::path::PathBuf],
        cx: &mut Context<Self>,
    ) {
        if self.song_workshop.is_some() {
            self.attach_music_files(paths, cx);
        }
    }
    pub(crate) fn open_song_workshop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.attachment_preview = None;

        let focus = cx.focus_handle().tab_stop(true);
        let subscription = cx.on_focus(&focus, window, |this, _, cx| {
            if let Some(s) = &mut this.song_workshop {
                s.active_field = Some(0);
                s.input_focused = true;
            }
            cx.notify();
        });
        let blur = cx.on_blur(&focus, window, |this, _, cx| {
            if let Some(s) = &mut this.song_workshop {
                s.input_focused = false;
                s.input_assist.pause();
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
        let identity = NEXT_WORKSHOP.fetch_add(1, Ordering::Relaxed);
        self.song_workshop = Some(SongWorkshop {
            identity,
            brief: TextField::default(),
            references: Default::default(),
            foci: [focus.clone()],
            active_field: Some(0),
            _focus_subscriptions: vec![subscription, blur],
            preferences: self.settings.agent.clone(),
            models: Vec::new(),
            model_receiver: Some(auris_agent::discover_models_background(
                self.settings.agent.clone(),
            )),
            model_error: None,
            reasoning_picker: None,
            completions: Default::default(),
            command_feedback: None,
            input_assist: input_assist::InputAssist::new(identity - 1, std::time::Instant::now()),
            input_focused: true,
            runtime,
            snapshot: None,
            candidates: vec![None; 3],
            played_audio: vec![None; 3],
            played_timeline: vec![None; 3],
            saved: vec![None; 3],
            errors: vec![None; 3],
            players: (0..3).map(|_| PlayerState::default()).collect(),
            preview_serial: 0,
            preparing: None,
            error,
            submitted: false,
            queued: Default::default(),
            queue_paused: false,
            in_flight: None,
            chord_prompt_open: false,
            show_memory: false,
            expanded_tools: Default::default(),
            scroll: gpui::ScrollHandle::new(),
            switching: false,
            show_archived_threads: false,
            pending_thread_name: None,
        });
        focus.focus(window);
        cx.notify();
    }

    fn start_song_workshop(&mut self, cx: &mut Context<Self>) {
        if self
            .song_workshop
            .as_ref()
            .is_some_and(|s| s.brief.marked().is_some())
        {
            return;
        }
        if self.workshop_control_command() {
            cx.notify();
            return;
        }
        if self
            .song_workshop
            .as_ref()
            .is_none_or(|s| s.brief_locked() || s.references.midi_loading)
            || !self.commit_midi_draft()
        {
            return;
        }
        if self
            .song_workshop
            .as_ref()
            .is_some_and(|s| s.busy() || !s.queued.is_empty())
        {
            // Busy, or others are already waiting: this one waits its turn behind them.
            self.enqueue_workshop_brief(false);
            self.dispatch_queued_workshop_brief();
            cx.notify();
            return;
        }
        let Some(s) = &mut self.song_workshop else {
            return;
        };
        if s.brief.content().trim().is_empty() {
            return;
        }
        if !s.preferences.is_configured() {
            return;
        }
        if !s
            .references
            .attachments
            .iter()
            .all(|path| s.references.attached_here.contains(path))
        {
            s.references.attachment_error = Some(Key::AgentQueueReattach.get(self.language).into());
            cx.notify();
            return;
        }
        if !s.send(Command::Send(
            s.brief.content().to_owned(),
            Box::new(s.preferences.clone()),
            s.references.attachments.clone(),
        )) {
            s.error =
                Some("The song conversation is unavailable. Close and reopen it to retry.".into());
            cx.notify();
            return;
        }
        s.submitted = true;
        s.error = None;
        s.command_feedback = None;
        s.completions.reset();
        cx.notify();
    }

    fn request_more_song_candidates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = self.t(Key::WorkshopMoreCandidatesRequest).to_owned();
        let Some(s) = &mut self.song_workshop else {
            return;
        };
        if s.busy() {
            return;
        }
        // Keep any feedback already drafted, and let the user refine the request before sending.
        if !s.brief.content().contains(&request) {
            let prefix = if s.brief.content().is_empty() {
                request
            } else {
                format!("{request}\n\n")
            };
            s.brief.replace(0..0, &prefix);
        }
        s.active_field = Some(0);
        s.foci[0].focus(window);
        cx.notify();
    }

    fn workshop_copy_button(
        // atelier: allow — localized candidate action delegates clipboard and feedback to CopyButton.
        &self,
        id: (&'static str, usize),
        label: Key,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = window
            .use_keyed_state(
                gpui::ElementId::NamedChild(Box::new(id.into()), "copy-focus".into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        div()
            .track_focus(&focus)
            .opacity(0.0)
            .group_hover("workshop-copy", |style| style.opacity(1.0))
            .in_focus(|style| style.opacity(1.0))
            .child(
                atelier::CopyButton::new(id, text)
                    .tooltip(self.t(label))
                    .copied_label(self.t(Key::CopiedToClipboard)),
            )
            .into_any_element()
    }

    /// Apply the shared allowance without replacing the workshop's local model choice.
    pub(crate) fn sync_workshop_tool_call_limit(&mut self) {
        if let Some(workshop) = &mut self.song_workshop {
            workshop.preferences.max_tool_calls = self.settings.agent.max_tool_calls;
        }
    }

    pub(crate) fn poll_song_workshop(&mut self, cx: &mut Context<Self>) {
        self.poll_workshop_input_assist(cx);
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
            let changed_thread = s
                .snapshot
                .as_ref()
                .is_some_and(|old| old.document.thread_id != snapshot.document.thread_id);
            if changed_thread {
                for player in &mut s.players {
                    player.reset_audio();
                }
                s.players.clear();
                s.played_audio.clear();
                s.played_timeline.clear();
                s.saved.clear();
                s.errors.clear();
                s.identity = NEXT_WORKSHOP.fetch_add(1, Ordering::Relaxed);
                s.preview_serial = s.preview_serial.wrapping_add(1);
                s.preparing = None;
                s.expanded_tools.clear();
                s.completions.reset();
                s.input_assist.edited();
                s.submitted = false;
                // The queue was written to the conversation that has just been left.
                s.queued.clear();
                s.queue_paused = false;
                s.in_flight = None;
                s.brief = TextField::new(snapshot.document.draft.clone());
                s.references = Default::default();
                s.references.attachments = snapshot.document.draft_attachments.clone();
            }
            if changed_thread || snapshot.error.is_some() {
                s.switching = false;
            }
            let follow_tail =
                f32::from(s.scroll.max_offset().height) + f32::from(s.scroll.offset().y) < 64.0;
            let initial = s.snapshot.is_none() || changed_thread;
            let added = s
                .snapshot
                .as_ref()
                .map_or(0, |old| old.document.rounds.len())
                < snapshot.document.rounds.len();
            if initial && s.brief.content().is_empty() {
                s.brief = TextField::new(snapshot.document.draft.clone());
                s.references.attachments = snapshot.document.draft_attachments.clone();
            }
            s.error = snapshot.error.clone();
            if s.submitted
                && s.in_flight.is_none()
                && snapshot.document.messages.len()
                    > s.snapshot
                        .as_ref()
                        .map_or(0, |old| old.document.messages.len())
            {
                s.brief = TextField::default();
                s.references.attachments.clear();
                s.references.attachment_error = None;
            }
            if snapshot.document.messages.len()
                > s.snapshot
                    .as_ref()
                    .map_or(0, |old| old.document.messages.len())
                || snapshot.error.is_some()
            {
                s.submitted = false;
                if snapshot.error.is_some()
                    && let Some(message) = s.in_flight.take()
                {
                    // Refused rather than lost: back at the head of the line, and held there
                    // until the user has seen why.
                    s.queued.push_front(message);
                    s.queue_paused = true;
                }
                s.in_flight = None;
            }
            s.snapshot = Some(snapshot);
            if follow_tail {
                s.scroll.scroll_to_bottom();
            }
            if initial || added {
                s.load_candidate_audio();
            } else {
                s.refresh_candidates();
            }
            cx.notify();
        }
        if s.sync_idle_auditions() {
            cx.notify();
        }
        if self.dispatch_queued_workshop_brief() {
            cx.notify();
        }
    }

    fn adopt_song_candidate(&mut self, index: usize, _: &mut Window, cx: &mut Context<Self>) {
        let selection_request = self.t(Key::WorkshopSelectionRequest).to_string();
        if let Some(s) = &mut self.song_workshop
            && let Some(snapshot) = &s.snapshot
            && let Some(candidate) = snapshot
                .document
                .rounds
                .iter()
                .flat_map(|round| &round.candidates)
                .nth(index)
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

    fn open_workshop_editor(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.commit_midi_draft() {
            cx.notify();
            return;
        }
        let Some(s) = &mut self.song_workshop else {
            return;
        };
        if s.busy() || self.agent_chat.busy {
            return;
        }
        let Some(snapshot) = &s.snapshot else {
            return;
        };
        if !snapshot
            .document
            .rounds
            .iter()
            .flat_map(|round| &round.candidates)
            .any(|candidate| candidate.id == id && candidate.ready)
            || snapshot.progress.contains_key(&id)
        {
            return;
        }
        let Some(candidate) = snapshot.candidates.get(&id).cloned() else {
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
        document.selected = Some(id);
        document.draft = s.brief.content().to_owned();
        document.draft_attachments = s.references.attachments.clone();
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
                        || s.snapshot
                            .as_ref()
                            .and_then(|s| s.candidates.get(&id))
                            .is_none_or(|current| !Arc::ptr_eq(current, &candidate))
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
                            this.attachment_preview = None;
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
        let project_tabs = self.render_project_tabs(window, cx);
        let paths = self
            .song_workshop
            .as_ref()
            .map(|state| {
                state
                    .references
                    .attachments
                    .iter()
                    .cloned()
                    .chain(
                        state
                            .snapshot
                            .iter()
                            .flat_map(|snapshot| &snapshot.document.messages)
                            .flat_map(|message| message.attachments.iter().cloned()),
                    )
                    .collect()
            })
            .unwrap_or_default();
        self.load_inline_attachments(paths, cx);
        self.sync_auxiliary_windows(cx);
        let chord_prompt = self.workshop_prompt_open();
        if chord_prompt {
            self.reconcile_focus(window, cx);
        }
        if let Some(s) = &mut self.song_workshop {
            if s.chord_prompt_open && !chord_prompt {
                s.active_field = Some(0);
                window.focus(&s.foci[0]);
            }
            s.chord_prompt_open = chord_prompt;
        }
        let input = self.render_workshop_input(window, cx);
        let prompt = chord_prompt
            .then(|| self.render_prompt(cx))
            .flatten()
            .map(IntoElement::into_any_element);
        let attachment_preview = self.render_attachment_preview(true, window, cx);
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
                let entry =
                    div()
                        .flex()
                        .flex_col()
                        .group("workshop-copy")
                        .gap(spacing.xs)
                        .child(entry)
                        .children(message.attachments.iter().enumerate().map(
                            |(file_index, path)| {
                                let preview = self.inline_attachment(
                                    path,
                                    format!("song-inline-attachment-{i}-{file_index}"),
                                    window,
                                    cx,
                                );
                                let path = path.clone();
                                let name = path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned();
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(
                                        Button::new(
                                            gpui::ElementId::Name(
                                                format!("song-attachment-{i}-{file_index}").into(),
                                            ),
                                            name,
                                        )
                                        .ghost()
                                        .icon(IconName::File)
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.open_agent_attachment(path.clone(), cx)
                                            }),
                                        ),
                                    )
                                    .child(preview)
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .when(message.user, |row| row.justify_end())
                                .child(
                                    div()
                                        .rounded(theme.atelier.shape.radius_md)
                                        .when(message.user, |copy| copy.bg(theme.surface))
                                        .child(self.workshop_copy_button(
                                            ("song-copy-message", i),
                                            Key::WorkshopCopyMessage,
                                            message.text.clone(),
                                            window,
                                            cx,
                                        )),
                                ),
                        )
                        .into_any_element();
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
                    let mut usage = auris_session::workshop::ToolUsage::default();
                    for tool in round.candidates.iter().flat_map(|c| c.tools.values()) {
                        usage.total += tool.total;
                        usage.succeeded += tool.succeeded;
                        usage.failed += tool.failed;
                        usage.interrupted += tool.interrupted;
                    }
                    if usage.total > 0 {
                        transcript = transcript.child(text_panel(
                            Label::new(tool_counts(self.t(Key::WorkshopToolCounts), &usage))
                                .size(TextSize::Small),
                            &theme,
                        ));
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
                    let offset: usize = snapshot.document.rounds[..round_index]
                        .iter()
                        .map(|round| round.candidates.len())
                        .sum();
                    for (index, candidate) in round.candidates.iter().enumerate() {
                        cards = cards.child(self.candidate_card(
                            offset + index,
                            candidate,
                            snapshot,
                            window,
                            cx,
                        ));
                    }
                    transcript = transcript.child(cards);
                }
            }
            if !snapshot.document.rounds.is_empty() {
                transcript = transcript.child(text_panel(
                    Button::new("song-more-candidates", self.t(Key::WorkshopMoreCandidates))
                        .disabled(s.busy())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.request_more_song_candidates(window, cx);
                        })),
                    &theme,
                ));
            }
            if snapshot.planning {
                transcript = transcript.child(text_panel(
                    agent_components::thinking_indicator(
                        "song-planning-activity",
                        self.t(Key::WorkshopPlanning),
                        &theme,
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
                    &theme,
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
        let memory = s.snapshot.as_ref().is_some_and(|s| s.preferences.enabled);
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
                atelier::Switch::new("song-memory", memory)
                    .label(self.t(Key::WorkshopMemory))
                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                        if let Some(s) = &this.song_workshop {
                            s.send(Command::Memory(*on));
                        }
                        cx.notify();
                    })),
            )
            .child(
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
            );
        let footer = div()
            .id("song-workshop-footer")
            .max_h(window.viewport_size().height * 0.65)
            .overflow_y_scroll()
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
            .child(attachment_preview)
            .child(options);
        let content = div()
            .h_full()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(body)
            .child(footer);
        let screen = div()
            .size_full()
            .flex()
            .child(self.workshop_sidebar(cx))
            .child(content);
        let back = Button::new("song-back", self.t(Key::WorkshopBack))
            .ghost()
            .icon(IconName::ChevronLeft)
            .on_click(cx.listener(move |this, _, window, cx| {
                if !this.commit_midi_draft() {
                    cx.notify();
                    return;
                }
                this.close_timbre_map();
                this.song_workshop = None;
                this.attachment_preview = None;
                this.session.stop_output_preview();
                this.begin_welcome_transition(false, veil, cx);
                this.focus.focus(window);
                cx.notify();
            }));
        let title = crate::titlebar::Parts::new(
            "song-workshop-title",
            title_of(self.t(Key::WorkshopTitle)),
        )
        .leading(back);
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
            .key_context(
                if chord_prompt
                    || s.input_focused
                    || self.menu.is_some()
                    || self.reference_score_claims_keys()
                {
                    crate::actions::context::PROMPT
                } else {
                    crate::actions::KEY_CONTEXT
                },
            )
            .on_key_down(cx.listener(Self::workshop_key))
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                this.attach_music_files(paths.paths(), cx);
            }))
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
            .on_action(
                cx.listener(|this, _: &crate::actions::NextProjectTab, _, cx| {
                    this.cycle_project_tab(true, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::PreviousProjectTab, _, cx| {
                    this.cycle_project_tab(false, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::actions::CloseProjectTab, _, cx| {
                    this.close_project_tab(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &crate::actions::NewProject, _, cx| {
                crate::project_windows::open_in(None, this.project_window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &crate::actions::OpenProject, window, cx| {
                    this.open_project(window, cx)
                }),
            )
            .child(crate::titlebar::bar(
                window,
                &theme,
                title,
                crate::titlebar::controls(
                    window,
                    &theme,
                    cx.listener(|this, _, window, cx| this.close_project_window(window, cx)),
                ),
            ))
            .children(project_tabs)
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
            .children(prompt)
            .children(crate::titlebar::resize_handles(window))
            .into_any_element()
    }

    /// One alternative: what it is, how far it has got, its player and what can be done with it.
    fn candidate_card(
        &self,
        index: usize,
        candidate: &auris_session::workshop::Candidate,
        snapshot: &Snapshot,
        window: &mut Window,
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
        if running {
            badges = badges.child(
                Badge::new(self.t(Key::WorkshopGenerating))
                    .tone(Tone::Accent)
                    .dot(),
            );
        }
        let description = div()
            .flex()
            .items_start()
            .gap(spacing.xs)
            .group("workshop-copy")
            .text_size(theme.atelier.typography.size_sm)
            .text_color(theme.text_muted)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(agent_components::selectable_text(
                        gpui::SharedString::from(format!("song-concept-{id}")),
                        candidate.direction.description.clone(),
                        window,
                        cx,
                    )),
            )
            .child(self.workshop_copy_button(
                ("song-copy-concept", index),
                Key::WorkshopCopyConcept,
                candidate.direction.description.clone(),
                window,
                cx,
            ));
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
            // Mixed into the card's own fill rather than laid over it, so the card stays opaque
            // over the metal beneath the transcript.
            .when(running, |card| {
                card.bg(atelier::mix(colors.elevated, colors.accent, 0.12))
                    .border_color(colors.accent.opacity(0.35))
            })
            .when(!running, |card| {
                card.bg(colors.elevated).border_color(if selected {
                    colors.accent
                } else {
                    colors.border
                })
            })
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(spacing.xs)
                    .group("workshop-copy")
                    .text_size(theme.atelier.typography.size_lg)
                    .font_weight(theme.atelier.typography.heading_weight)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(agent_components::selectable_text(
                                gpui::SharedString::from(format!("song-title-{id}")),
                                candidate.direction.title.clone(),
                                window,
                                cx,
                            )),
                    )
                    .child(self.workshop_copy_button(
                        ("song-copy-title", index),
                        Key::WorkshopCopyTitle,
                        candidate.direction.title.clone(),
                        window,
                        cx,
                    )),
            )
            .child(badges)
            .child(description);
        if let Some(stage) = snapshot.progress.get(&id) {
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
                    .items_center()
                    .gap(spacing.sm)
                    .p(spacing.md)
                    .rounded(theme.atelier.shape.radius_md)
                    .bg(colors.surface)
                    .child(
                        atelier::Spinner::new(("song-candidate-spinner", index))
                            .color(colors.accent),
                    )
                    .child(Label::new(label).size(TextSize::Small)),
            );
        }
        if !candidate.tools.is_empty() {
            let opened = s.expanded_tools.contains(&id);
            let mut total = auris_session::workshop::ToolUsage::default();
            for usage in candidate.tools.values() {
                total.total += usage.total;
                total.succeeded += usage.succeeded;
                total.failed += usage.failed;
                total.interrupted += usage.interrupted;
            }
            let mut label = self
                .t(Key::AgentToolGroup)
                .replace("{count}", &total.total.to_string());
            for (count, key) in [
                (total.running(), Key::AgentToolGroupRunning),
                (total.failed, Key::AgentToolGroupFailed),
            ] {
                if count > 0 {
                    label.push_str(" · ");
                    label.push_str(&self.t(key).replace("{count}", &count.to_string()));
                }
            }
            let mut tools = div()
                .flex()
                .flex_col()
                .gap(spacing.xs)
                .debug_selector(move || format!("song-candidate-tools-{index}"))
                .child(
                    super::widgets::disclosure(
                        gpui::SharedString::from(format!("song-tools-toggle-{id}")),
                        label,
                        opened,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            if let Some(workshop) = &mut this.song_workshop
                                && !workshop.expanded_tools.remove(&id)
                            {
                                workshop.expanded_tools.insert(id);
                            }
                            cx.notify();
                        }),
                    )
                    .when(total.failed > 0, |row| row.text_color(theme.danger)),
                );
            if opened {
                let mut details = div()
                    .flex()
                    .flex_col()
                    .gap(spacing.xs)
                    .debug_selector(move || format!("song-tools-body-{index}"))
                    .child(
                        Label::new(tool_counts(self.t(Key::WorkshopToolCounts), &total))
                            .size(TextSize::Small),
                    );
                for (name, usage) in &candidate.tools {
                    details = details.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(spacing.sm)
                            .min_w_0()
                            .child(super::icons::icon(
                                super::icons::tool_icon(name),
                                theme.atelier.typography.size_sm,
                                if usage.running() > 0 {
                                    colors.accent
                                } else {
                                    theme.text_muted
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(theme.atelier.typography.size_sm)
                                    .child(div().child(name.clone()))
                                    .child(div().text_color(theme.text_muted).child(tool_counts(
                                        self.t(Key::WorkshopToolCounts),
                                        usage,
                                    ))),
                            ),
                    );
                }
                tools = tools.child(details);
            }
            card = card.child(tools);
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(crate::i18n::agent_error_text(error, self.language(), true)),
                    ),
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
                audition.is_some() && s.preparing.is_none() && !s.switching,
                s.preparing == Some(index),
                None,
                cx,
            ),
        );
        let usable = ready && !running && !s.switching;
        if let Some(song) = &s.candidates[index] {
            let lyrics = song.lyrics();
            if !lyrics.is_empty() {
                card = card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(spacing.sm)
                        .debug_selector(move || format!("song-lyrics-{index}"))
                        .child(
                            Label::new(self.t(Key::WorkshopCandidateLyrics))
                                .size(TextSize::Small)
                                .color(TextColor::Muted),
                        )
                        .child(agent_components::selectable_text(
                            gpui::SharedString::from(format!("song-lyrics-text-{id}")),
                            lyrics,
                            window,
                            cx,
                        )),
                );
            }
        }
        let retry_enabled =
            !s.switching && (running || (!snapshot.planning && snapshot.progress.len() < 3));
        let unfinished = !candidate.ready;
        if let Some(feedback) = &candidate.feedback {
            card = card.child(
                Label::new(format!(
                    "{}{}  {}/5",
                    "★".repeat(usize::from(feedback.rating.min(5))),
                    "☆".repeat(usize::from(5 - feedback.rating.min(5))),
                    feedback.rating
                ))
                .size(TextSize::Small),
            );
            if !feedback.comment.is_empty() {
                card = card.child(
                    div()
                        .text_size(theme.atelier.typography.size_sm)
                        .child(feedback.comment.clone()),
                );
            }
        }
        card.child(
            div()
                .flex()
                .flex_wrap()
                .gap(spacing.sm)
                .child(
                    Button::new(
                        ("song-feedback", index),
                        self.t(if candidate.feedback.is_some() {
                            Key::WorkshopEditFeedback
                        } else {
                            Key::WorkshopRateSong
                        }),
                    )
                    .ghost()
                    .disabled(!candidate.ready || running || s.switching)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_workshop_feedback(id, cx);
                        this.focus.focus(window);
                        cx.notify();
                    })),
                )
                .child(
                    Button::new(("song-adopt", index), self.t(Key::WorkshopContinue))
                        .icon(IconName::Check)
                        .disabled(!usable)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.adopt_song_candidate(index, window, cx)
                        })),
                )
                .child(
                    Button::new(("song-editor", index), self.t(Key::WorkshopOpenEditor))
                        .disabled(!usable || s.busy() || self.agent_chat.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_workshop_editor(id, window, cx);
                        })),
                )
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(s) = &this.song_workshop {
                            s.send(if running {
                                Command::Stop(Some(id))
                            } else if unfinished {
                                Command::Recover(id, Box::new(s.preferences.clone()))
                            } else {
                                Command::Retry(id, Box::new(s.preferences.clone()))
                            });
                        }
                        cx.notify();
                    })),
                )
                .child(
                    Button::new(("song-save", index), self.t(Key::WorkshopSave))
                        .ghost()
                        .icon(IconName::Folder)
                        .disabled(!usable)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.save_song_candidate(index, window, cx)
                        })),
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

    #[gpui::test]
    fn saved_tool_call_limit_updates_an_open_workshop_without_changing_its_model(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.open_song_workshop(window, cx);
            })
        });
        app.update(cx, |this, _| {
            let workshop = this.song_workshop.as_mut().unwrap();
            workshop.preferences.model = "workshop-choice".into();
        });
        app.update(cx, |this, _| {
            let mut prefs = this.settings.agent.clone();
            prefs.max_tool_calls = Some(5000);
            this.agent_apply_preferences_with(prefs, |_| Ok::<(), String>(()))
                .unwrap();
            let prefs = &this.song_workshop.as_ref().unwrap().preferences;
            assert_eq!(prefs.tool_call_limit(), 5000);
            assert_eq!(prefs.model, "workshop-choice");
        });
    }

    #[gpui::test]
    fn feedback_form_accepts_stars_multiline_impressions_and_preserves_failed_submissions(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::{click, paint};
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        app.update(cx, |this, _| {
            let state = this.song_workshop.as_mut().unwrap();
            state.brief = TextField::new("Unsent brief");
            state.snapshot.as_mut().unwrap().document = serde_json::from_value(serde_json::json!({
                "messages":[{"user":false,"text":"Here is your song"}],
                "rounds":[{"after_message":1,"candidates":[{"id":1,"parent":null,"ready":true,"error":null,
                    "direction":{"title":"My song","description":"Piano","instructions":"Compose"}}]}]
            })).unwrap();
        });
        paint(&app, cx);
        click("song-feedback-0", cx);
        paint(&app, cx);
        cx.simulate_keystrokes("secondary-enter");
        paint(&app, cx);
        assert!(cx.debug_bounds("prompt-error").is_some());
        cx.simulate_keystrokes("shift-tab space");
        // A focused control is pressed when Space comes back up, as a native button is.
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("space").unwrap(),
        });
        app.read_with(cx, |this, _| {
            assert!(matches!(
                this.prompt.as_ref().unwrap().target(),
                Some(super::super::prompt::PromptTarget::WorkshopFeedback { rating: 5, .. })
            ))
        });
        click("feedback-star-4", cx);
        click("feedback-comment", cx);
        cx.simulate_input("メロディが好き");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("ドラムを控えめに");
        cx.simulate_keystrokes("secondary-enter");
        app.read_with(cx, |this, _| {
            let prompt = this.prompt.as_ref().unwrap();
            assert!(matches!(
                prompt.target(),
                Some(super::super::prompt::PromptTarget::WorkshopFeedback {
                    rating: 4,
                    candidate: 1,
                    ..
                })
            ));
            assert_eq!(
                prompt.field().unwrap().content(),
                "メロディが好き\nドラムを控えめに"
            );
            assert_eq!(
                this.song_workshop.as_ref().unwrap().brief.content(),
                "Unsent brief"
            );
            assert!(
                this.song_workshop
                    .as_ref()
                    .unwrap()
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .document
                    .rounds[0]
                    .candidates[0]
                    .feedback
                    .is_none()
            );
        });
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| assert!(this.prompt.is_none()));
    }

    #[gpui::test]
    fn workshop_edits_melody_lyrics_and_chords_without_touching_editor_drafts(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::{click, open, paint, resize};
        let (app, cx) = open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
                this.pointer = crate::gestures::PointerGestures::default();
                this.settings.piano_roll_grid = auris_session::PianoRollGrid::Sixteenth;
                this.settings.relative_note_snap = false;
                this.agent_chat
                    .references
                    .attachments
                    .push(std::path::PathBuf::from("editor-only.mid"));
            })
        });
        make_ready(&app, cx);
        resize(&app, cx, gpui::size(gpui::px(1200.), gpui::px(1000.)));
        let before = app.read_with(cx, |this, _| this.project().clone());
        click("music-reference-menu", cx);
        paint(&app, cx);
        crate::harness::choose(
            &app,
            cx,
            &crate::ui::context_menu::MenuCommand::ComposerMelody(false),
        );
        paint(&app, cx);
        let editor = crate::harness::reference_editor(&app, cx);
        crate::harness::show_pitch(&editor, cx, 60);
        let at = crate::harness::roll_point(&editor, cx, auris_session::prelude::Ticks(240), 60);
        crate::harness::click_at(cx, at, crate::harness::creating());
        paint(&app, cx);
        let at = editor.read_with(cx, |app, _| {
            let note = &app.selected_midi_clip().unwrap().notes[0];
            (
                note.start + auris_session::prelude::Ticks(note.length.0 / 2),
                note.pitch,
            )
        });
        let at = crate::harness::roll_point(&editor, cx, at.0, at.1);
        crate::harness::double_click(cx, at);
        paint(&app, cx);
        cx.simulate_input("きみ");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        click("agent-midi-apply", cx);
        app.read_with(cx, |this, _| {
            let paths = &this.song_workshop.as_ref().unwrap().references.attachments;
            assert_eq!(paths.len(), 1);
            let score = auris_session::midi_reference::MidiReference::read(&paths[0]).unwrap();
            assert_eq!(score.tracks[0].notes[0].lyric, "きみ");
            assert_eq!(
                this.agent_chat.references.attachments,
                vec![std::path::PathBuf::from("editor-only.mid")]
            );
        });
        paint(&app, cx);
        click("music-reference-menu", cx);
        paint(&app, cx);
        crate::harness::choose(
            &app,
            cx,
            &crate::ui::context_menu::MenuCommand::ComposerChords,
        );
        paint(&app, cx);
        cx.simulate_input("| C | G | Am | F |");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(
                this.prompt.is_none(),
                "prompt={:?}, text={:?}, status={}, locked={}",
                this.prompt.as_ref().and_then(|p| p.target()),
                this.prompt
                    .as_ref()
                    .and_then(|p| p.field())
                    .map(|f| f.content()),
                this.status,
                this.music_composer_locked()
            );
            let paths = &this.song_workshop.as_ref().unwrap().references.attachments;
            assert_eq!(paths.len(), 2);
            let chords: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&paths[1]).unwrap()).unwrap();
            assert_eq!(chords["chords"][2]["chord"], "Am");
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn workshop_sound_map_returns_the_choice_to_the_song_composer(cx: &mut gpui::TestAppContext) {
        use crate::harness::{click, open, paint};
        let (app, cx) = open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
                this.timbre_map.map = Some(Arc::new(
                    this.session
                        .timbre_map_job()
                        .unwrap()
                        .run(&auris_session::TimbreMapControl::default())
                        .unwrap(),
                ));
            })
        });
        make_ready(&app, cx);
        paint(&app, cx);
        click("music-reference-menu", cx);
        paint(&app, cx);
        crate::harness::choose(
            &app,
            cx,
            &crate::ui::context_menu::MenuCommand::ComposerSound,
        );
        paint(&app, cx);
        let handle = app.read_with(cx, |this, _| {
            this.auxiliary_windows[&crate::auxiliary_window::Surface::TimbreMap]
        });
        let mut utility = gpui::VisualTestContext::from_window(handle.into(), cx);
        click("timbre-all-0", &mut utility);
        paint(&app, &mut utility);
        click("timbre-attach-chat", &mut utility);
        app.read_with(&utility, |this, _| {
            assert!(!this.timbre_map.open);
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.references.attachments.len(), 1);
            assert!(this.agent_chat.references.attachments.is_empty());
            let sound: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&s.references.attachments[0]).unwrap())
                    .unwrap();
            assert_eq!(sound["reference_type"], "instrument_choice");
        });
    }

    #[gpui::test]
    fn workshop_references_survive_a_rejected_submission(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| app.update(cx, |this, cx| this.open_song_workshop(window, cx)));
        make_ready(&app, cx);
        app.update(cx, |this, cx| {
            this.attach_agent_music_reference(&serde_json::json!({"chords": ["C", "G"]}), "chords")
                .unwrap();
            let s = this.song_workshop.as_mut().unwrap();
            s.brief = TextField::new("Use these chords");
            s.preferences.backend = "codex".into();
            s.preferences.model = "test".into();
            this.start_song_workshop(cx);
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.brief.content(), "Use these chords");
            assert_eq!(s.references.attachments.len(), 1);
            assert!(s.error.is_some());
        });
    }

    #[gpui::test]
    fn workshop_midi_file_opens_an_independent_editable_copy(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phrase.mid");
        let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
        let track = b"\0\xff\x05\x02la\0\x92\x40\x64\x83\x60\x82\x40\0\0\xff\x2f\0";
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(track);
        std::fs::write(&path, &bytes).unwrap();
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        app.update(cx, |this, cx| {
            this.attach_music_files(std::slice::from_ref(&path), cx);
            this.edit_midi_attachment(path.clone(), cx);
        });
        cx.run_until_parked();
        app.update(cx, |this, _| {
            assert!(
                this.song_workshop
                    .as_ref()
                    .unwrap()
                    .references
                    .midi_draft
                    .is_some()
            );
            assert!(this.commit_midi_draft());
            let paths = &this.song_workshop.as_ref().unwrap().references.attachments;
            assert_ne!(&paths[0], &path);
            let score = auris_session::midi_reference::MidiReference::read(&paths[0]).unwrap();
            assert_eq!(score.tracks[0].notes[0].lyric, "la");
        });
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

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
                threads: Vec::new(),
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

    fn conversation_snapshot(messages: &[&str], planning: bool) -> Snapshot {
        let document = auris_session::workshop::Document {
            messages: messages
                .iter()
                .map(|text| auris_session::workshop::Message {
                    attachments: Vec::new(),
                    user: true,
                    text: (*text).to_owned(),
                    references: Vec::new(),
                })
                .collect(),
            ..Default::default()
        };
        Snapshot {
            threads: Vec::new(),
            document,
            candidates: Default::default(),
            previews: Default::default(),
            preview_errors: Default::default(),
            progress: Default::default(),
            planning,
            preferences: Default::default(),
            error: None,
        }
    }

    fn sent_text(commands: &mpsc::Receiver<Command>) -> Option<String> {
        match commands.try_recv() {
            Ok(Command::Send(text, _, _)) => Some(text),
            _ => None,
        }
    }

    #[gpui::test]
    fn workshop_messages_wait_their_turn_and_can_jump_the_queue(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        let (commands, snapshots) = app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            let (runtime, commands, snapshots) = Workshop::loopback();
            s.runtime = Some(runtime);
            s.preferences.model = "test".into();
            // The director is still answering an earlier message.
            s.snapshot = Some(conversation_snapshot(&[], true));
            (commands, snapshots)
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-brief", cx);
        cx.simulate_input("make it faster");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("and brighter");
        cx.simulate_keystrokes("enter");
        let queued = |app: &gpui::Entity<AurisApp>, cx: &mut gpui::VisualTestContext| {
            app.read_with(cx, |this, _| {
                let s = this.song_workshop.as_ref().unwrap();
                s.queued.iter().map(|m| m.text.clone()).collect::<Vec<_>>()
            })
        };
        assert_eq!(queued(&app, cx), ["make it faster", "and brighter"]);
        app.read_with(cx, |this, _| {
            assert!(
                this.song_workshop
                    .as_ref()
                    .unwrap()
                    .brief
                    .content()
                    .is_empty()
            );
        });
        assert!(
            commands.try_recv().is_err(),
            "nothing reaches a busy workshop"
        );

        // Typed third, sent first: the secondary modifier stops the round in progress.
        cx.simulate_input("actually, a ballad");
        cx.simulate_keystrokes("secondary-enter");
        assert!(matches!(commands.try_recv(), Ok(Command::Stop(None))));
        assert_eq!(
            queued(&app, cx),
            ["actually, a ballad", "make it faster", "and brighter"]
        );

        // The work stops and the head of the line goes out by itself, while the composer stays
        // free for the next thought.
        snapshots.send(conversation_snapshot(&[], false)).unwrap();
        app.update(cx, |this, cx| this.poll_song_workshop(cx));
        assert_eq!(sent_text(&commands).as_deref(), Some("actually, a ballad"));
        crate::harness::paint(&app, cx);
        cx.simulate_input("x");
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_workshop.as_ref().unwrap().brief.content(), "x");
        });

        // Acknowledged and answered: the next one waits for that round, not merely the send.
        snapshots
            .send(conversation_snapshot(&["actually, a ballad"], true))
            .unwrap();
        app.update(cx, |this, cx| this.poll_song_workshop(cx));
        assert!(commands.try_recv().is_err());
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_workshop.as_ref().unwrap().brief.content(), "x");
        });
        snapshots
            .send(conversation_snapshot(&["actually, a ballad"], false))
            .unwrap();
        app.update(cx, |this, cx| this.poll_song_workshop(cx));
        assert_eq!(sent_text(&commands).as_deref(), Some("make it faster"));
        assert_eq!(queued(&app, cx), ["and brighter"]);
    }

    #[gpui::test]
    fn stopping_the_workshop_holds_the_queue_until_resumed(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        let (commands, snapshots) = app.update(cx, |this, cx| {
            let s = this.song_workshop.as_mut().unwrap();
            let (runtime, commands, snapshots) = Workshop::loopback();
            s.runtime = Some(runtime);
            s.preferences.model = "test".into();
            s.snapshot = Some(conversation_snapshot(&[], true));
            s.brief = TextField::new("later");
            this.start_song_workshop(cx);
            (commands, snapshots)
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-cancel", cx);
        assert!(matches!(commands.try_recv(), Ok(Command::Stop(None))));
        snapshots.send(conversation_snapshot(&[], false)).unwrap();
        app.update(cx, |this, cx| this.poll_song_workshop(cx));
        assert!(
            commands.try_recv().is_err(),
            "Stop is not a cue for the next message"
        );
        crate::harness::paint(&app, cx);
        crate::harness::click("workshop-queue-toggle", cx);
        assert_eq!(sent_text(&commands).as_deref(), Some("later"));
    }

    #[gpui::test]
    fn earlier_rounds_keep_their_cards_players_and_candidate_addresses(
        cx: &mut gpui::TestAppContext,
    ) {
        use auris_agent::workshop::Preview;
        use auris_session::workshop::Round;
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        let commands = app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            let (runtime, commands, _) = Workshop::loopback();
            s.runtime = Some(runtime);
            let mut snapshot = conversation_snapshot(&["First", "Second"], false);
            for round in 0..2 {
                let candidates = (0..2).map(|slot| serde_json::from_value(serde_json::json!({
                    "id": 10 + round * 2 + slot, "parent": null, "ready": false, "error": null,
                    "direction": {"title": format!("Song {round}/{slot}"), "description": "Description", "instructions": "Compose"}
                })).unwrap()).collect();
                snapshot.document.rounds.push(Round { after_message: round + 1, candidates });
            }
            for (index, candidate) in snapshot.document.rounds.iter().flat_map(|r| &r.candidates).enumerate() {
                snapshot.previews.insert(candidate.id, Preview {
                    revision: 1,
                    audio: Arc::new(auris_session::prelude::AudioBuffer::stereo((index + 1) * 100, 100.0)),
                    timeline: Default::default(),
                });
            }
            s.snapshot = Some(snapshot);
            s.refresh_candidates();
            assert_eq!(s.players.len(), 4);
            for index in 0..4 { assert_eq!(s.audition(index).unwrap().duration_seconds(), (index + 1) as f64); }
            commands
        });
        crate::harness::resize(&app, cx, gpui::size(gpui::px(1400.), gpui::px(1800.)));
        for selector in [
            "song-candidate-0",
            "song-candidate-1",
            "song-candidate-2",
            "song-candidate-3",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} should stay expanded"
            );
        }
        assert!(cx.debug_bounds("song-round-0").is_none());
        cx.update(|window, cx| app.update(cx, |this, cx| this.adopt_song_candidate(0, window, cx)));
        assert!(matches!(commands.try_recv(), Ok(Command::Select(10, _, _))));
        cx.update(|window, cx| app.update(cx, |this, cx| this.adopt_song_candidate(3, window, cx)));
        assert!(matches!(commands.try_recv(), Ok(Command::Select(13, _, _))));
    }

    #[gpui::test]
    fn images_are_embedded_before_and_after_sending_without_opening_a_preview(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("cover.png");
        image::DynamicImage::new_rgb8(40, 20).save(&image).unwrap();
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        app.update(cx, |this, cx| {
            this.workshop_attach_files(std::slice::from_ref(&image), cx)
        });
        crate::harness::paint(&app, cx);
        cx.run_until_parked();
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("song-inline-draft-0-image").is_some());
        app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            s.references.attachments.clear();
            let mut snapshot = conversation_snapshot(&["Use this cover"], false);
            snapshot.document.messages[0].attachments.push(image);
            s.snapshot = Some(snapshot);
            assert!(this.attachment_preview.is_none());
        });
        crate::harness::resize(&app, cx, gpui::size(gpui::px(800.), gpui::px(1000.)));
        assert!(
            cx.debug_bounds("song-inline-attachment-0-0-image")
                .is_some()
        );
        app.read_with(cx, |this, _| assert_eq!(this.inline_attachments.len(), 1));
    }

    #[gpui::test]
    fn workshop_files_can_be_attached_previewed_and_removed(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("lyrics.txt");
        std::fs::write(&text, "夜の雨").unwrap();
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        app.update(cx, |this, cx| {
            this.workshop_attach_files(
                &[
                    text.clone(),
                    dir.path().join("cover.png"),
                    dir.path().join("reference.wav"),
                    text.clone(),
                ],
                cx,
            );
            assert_eq!(
                this.song_workshop
                    .as_ref()
                    .unwrap()
                    .references
                    .attachments
                    .len(),
                3
            );
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-draft-attachment-0", cx);
        cx.run_until_parked();
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("attachment-preview").is_some());
        crate::harness::click("close-attachment-preview", cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("song-remove-attachment-0", cx);
        app.read_with(cx, |this, _| {
            let attachments = &this.song_workshop.as_ref().unwrap().references.attachments;
            assert_eq!(attachments.len(), 2);
            assert!(!attachments.contains(&text));
        });
    }

    #[gpui::test]
    fn each_editor_button_opens_its_own_candidate_without_a_prior_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        use auris_session::workshop::{Candidate, Direction, Message, Round};
        let songs: Vec<_> = ["First", "Second"]
            .into_iter()
            .enumerate()
            .map(|(i, title)| {
                Arc::new(
                    SongCandidate::generate(
                        SongIdea {
                            title: title.into(),
                            description: "Bright keys".into(),
                            preset: "chiptune".into(),
                            tempo: 120.,
                            key: "C major".into(),
                            density: 0.5,
                            lead: "piano".into(),
                        },
                        "Bright instrumental",
                        30,
                        i as u64 + 42,
                        &AtomicBool::new(false),
                    )
                    .unwrap(),
                )
            })
            .collect();
        for selected in [None, Some(100)] {
            let (app, cx) = crate::harness::open(cx);
            cx.update(|window, cx| {
                app.update(cx, |this, cx| {
                    this.show_project_chooser(window, cx);
                    this.open_song_workshop(window, cx);
                })
            });
            make_ready(&app, cx);
            let before = app.update(cx, |this, _| {
                let s = this.song_workshop.as_mut().unwrap();
                let snapshot = s.snapshot.as_mut().unwrap();
                snapshot.document.selected = selected;
                snapshot.document.messages.push(Message {
                    user: false,
                    text: "Two finished alternatives".into(),
                    references: vec![],
                    attachments: vec![],
                });
                snapshot.document.rounds.push(Round {
                    after_message: 1,
                    candidates: songs
                        .iter()
                        .enumerate()
                        .map(|(i, song)| Candidate {
                            id: i as u64 + 100,
                            parent: None,
                            ready: true,
                            error: None,
                            recoveries: 0,
                            tools: Default::default(),
                            feedback: None,
                            direction: Direction {
                                title: song.idea.title.clone(),
                                description: song.idea.description.clone(),
                                instructions: String::new(),
                                protected_tracks: vec![],
                            },
                        })
                        .collect(),
                });
                snapshot.candidates = songs
                    .iter()
                    .enumerate()
                    .map(|(i, song)| (i as u64 + 100, Arc::clone(song)))
                    .collect();
                s.refresh_candidates();
                this.project().clone()
            });
            crate::harness::resize(&app, cx, gpui::size(gpui::px(1400.), gpui::px(1100.)));
            assert!(cx.debug_bounds("song-editor").is_none());
            for (card, button) in [
                ("song-candidate-0", "song-editor-0"),
                ("song-candidate-1", "song-editor-1"),
            ] {
                let card = cx.debug_bounds(card).unwrap();
                let button = cx.debug_bounds(button).unwrap();
                assert!(card.contains(&button.center()));
            }
            crate::harness::click("song-editor-1", cx);
            cx.run_until_parked();
            app.read_with(cx, |this, _| {
                assert!(this.song_workshop.is_none());
                assert!(
                    this.project()
                        .creation_brief
                        .as_ref()
                        .unwrap()
                        .contains("Second")
                );
            });
            app.update(cx, |this, _| {
                this.session.undo();
                assert_eq!(this.project(), &before);
            });
        }
    }

    #[gpui::test]
    fn single_candidate_supports_copying_and_requesting_more(cx: &mut gpui::TestAppContext) {
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
            s.brief = TextField::new("もっと静かにしてください");
            let snapshot = s.snapshot.as_mut().unwrap();
            snapshot.document.messages.push(Message {
                references: Vec::new(),
                attachments: Vec::new(),
                user: false,
                text: "この方向で作ります。\nピアノを中心に。".into(),
            });
            snapshot.document.rounds.push(Round {
                after_message: 1,
                candidates: vec![Candidate {
                    id: 1,
                    parent: None,
                    ready: false,
                    error: None,
                    recoveries: 0,
                    tools: Default::default(),
                    feedback: None,
                    direction: Direction {
                        title: "夜の散歩".into(),
                        description: "静かなピアノ\n柔らかな弦楽器".into(),
                        instructions: "Private composer instructions".into(),
                        protected_tracks: vec![],
                    },
                }],
            });
        });
        crate::harness::resize(&app, cx, gpui::size(gpui::px(1200.0), gpui::px(1000.0)));
        assert!(cx.debug_bounds("song-candidate-0").is_some());
        assert!(cx.debug_bounds("song-candidate-1").is_none());
        for (selector, expected) in [
            ("song-copy-title-0", "夜の散歩"),
            ("song-copy-concept-0", "静かなピアノ\n柔らかな弦楽器"),
            (
                "song-copy-message-0",
                "この方向で作ります。\nピアノを中心に。",
            ),
        ] {
            crate::harness::click(selector, cx);
            cx.update(|_, cx| {
                assert_eq!(
                    cx.read_from_clipboard()
                        .and_then(|item| item.text())
                        .as_deref(),
                    Some(expected)
                );
            });
            crate::harness::paint(&app, cx);
        }
        crate::harness::click("song-more-candidates", cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("song-more-candidates", cx);
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            let request = this.t(Key::WorkshopMoreCandidatesRequest);
            assert_eq!(
                s.brief.content(),
                format!("{request}\n\nもっと静かにしてください")
            );
            assert_eq!(s.active_field, Some(0));
            assert_eq!(s.snapshot.as_ref().unwrap().document.rounds.len(), 1);
            assert_eq!(s.snapshot.as_ref().unwrap().document.selected, None);
        });
        for (selector, expected) in [
            ("song-title-1", "夜の散歩"),
            ("song-concept-1", "静かなピアノ\n柔らかな弦楽器"),
        ] {
            crate::harness::select_text(&app, cx, selector);
            cx.simulate_keystrokes("secondary-c");
            cx.update(|_, cx| {
                assert_eq!(
                    cx.read_from_clipboard()
                        .and_then(|item| item.text())
                        .as_deref(),
                    Some(expected)
                );
            });
        }
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
            let snapshot = s.snapshot.as_mut().unwrap();
            snapshot.document.messages.push(Message {
                references: vec![],
                attachments: Vec::new(),
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
                        tools: Default::default(),
                        feedback: None,
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
            snapshot.document.rounds[0].candidates[0].tools.insert(
                "edit_notes".into(),
                auris_session::workshop::ToolUsage {
                    total: 3,
                    succeeded: 1,
                    failed: 1,
                    interrupted: 0,
                },
            );
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
            assert!(cx.debug_bounds("song-candidate-tools-0").is_some());
            assert!(cx.debug_bounds("song-tools-body-0").is_none());
            assert!(cx.debug_bounds("song-candidate-activity-1").is_none());
            assert!(cx.debug_bounds("song-planning-activity").is_some());
        }
        crate::harness::click("song-tools-toggle-0", cx);
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("song-tools-body-0").is_some());
        assert!(cx.debug_bounds("song-tools-body-1").is_none());
        app.update(cx, |this, _| {
            let snapshot = this
                .song_workshop
                .as_mut()
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap();
            snapshot.progress.clear();
            snapshot.document.rounds[0].candidates[0]
                .tools
                .get_mut("edit_notes")
                .unwrap()
                .interrupt();
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("song-candidate-tools-0").is_some());
        assert!(cx.debug_bounds("song-tools-body-0").is_some());
        let expanded_height = cx
            .debug_bounds("song-candidate-tools-0")
            .unwrap()
            .size
            .height;
        crate::harness::click("song-tools-toggle-0", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(
                !this
                    .song_workshop
                    .as_ref()
                    .unwrap()
                    .expanded_tools
                    .contains(&0),
                "tool details should collapse after clicking the disclosure"
            )
        });
        // GPUI retains debug bounds for removed elements; measure the live container instead.
        assert!(
            cx.debug_bounds("song-candidate-tools-0")
                .unwrap()
                .size
                .height
                < expanded_height
        );
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
                references: vec![],
                attachments: Vec::new(),
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
    fn loading_candidate_audio_invalidates_pending_audition(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| app.update(cx, |this, cx| this.open_song_workshop(window, cx)));
        make_ready(&app, cx);
        app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            s.preparing = Some(0);
            let serial = s.preview_serial;
            s.load_candidate_audio();
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
            let snapshot = s.snapshot.as_mut().unwrap();
            snapshot
                .document
                .messages
                .push(auris_session::workshop::Message {
                    references: vec![],
                    attachments: Vec::new(),
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
                    tools: Default::default(),
                    feedback: None,
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
                    timeline: Arc::new(auris_session::AuditionTimeline::from_audio(
                        &first,
                        vec![
                            auris_session::AuditionSection {
                                label: "Intro".into(),
                                instance: 1,
                                start_seconds: 0.0,
                                end_seconds: 0.5,
                            },
                            auris_session::AuditionSection {
                                label: "Chorus".into(),
                                instance: 1,
                                start_seconds: 0.5,
                                end_seconds: 1.0,
                            },
                        ],
                    )),
                    audio: Arc::clone(&first),
                },
            );
            let timeline = &mut snapshot.previews.get_mut(&1).unwrap().timeline;
            Arc::make_mut(timeline).tracks = (0..2)
                .map(|index| auris_session::AuditionTrack {
                    id: index,
                    name: format!("Track {index}"),
                    color: auris_session::prelude::Color::PALETTE[index as usize].0,
                    notes: vec![auris_session::AuditionNote {
                        pitch: 48 + index as u8 * 24,
                        start_seconds: 0.1,
                        end_seconds: 0.4,
                    }],
                })
                .collect();
            this.session.project().clone()
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-preview-0-section-1", cx);
        assert!(cx.debug_bounds("song-preview-0-seek-notes").is_some());
        assert!(cx.debug_bounds("song-preview-0-tracks").is_some());
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.players[0].options(1.0).position_seconds, 0.5);
        });
        crate::harness::click("song-preview-0-seek", cx);
        cx.simulate_keystrokes("home right");
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            // Native AudioPlayer advances by five seconds; this one-second draft reaches its
            // endpoint, so ask for options against a longer duration to observe the position.
            assert!(s.players[0].options(2.0).position_seconds > 0.0);
        });
        let waveform = cx.debug_bounds("song-preview-0-seek").unwrap();
        crate::harness::drag(
            cx,
            waveform.center(),
            gpui::point(waveform.left(), waveform.center().y),
        );
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.players[0].options(1.0).position_seconds, 0.0);
        });
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
                    timeline: Arc::new(auris_session::AuditionTimeline::from_audio(
                        &latest,
                        vec![],
                    )),
                    audio: Arc::clone(&latest),
                },
            );
            // Arrival of new audio does not swap the buffer the user chose to hear.
            assert!(Arc::ptr_eq(s.played_audio[0].as_ref().unwrap(), &first));
            let timeline = s.audition_timeline(0).unwrap();
            assert_eq!(timeline.duration_seconds, 1.0);
            assert_eq!(timeline.sections[1].label, "Chorus");
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
        // The headless harness has no audio callback; stop the finite player state explicitly.
        app.update(cx, |this, cx| {
            this.song_workshop.as_mut().unwrap().players[0].stop();
            this.poll_song_workshop(cx);
        });
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            let state = this.song_workshop.as_ref().unwrap();
            assert!(state.played_audio[0].is_none());
            assert_eq!(state.audition_timeline(0).unwrap().duration_seconds, 2.0);
        });
        crate::harness::click("song-preview-0-play", cx);
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert!(
                s.played_audio[0]
                    .as_ref()
                    .is_some_and(|audio| Arc::ptr_eq(audio, &latest))
            );
            assert_eq!(s.audition_timeline(0).unwrap().duration_seconds, 2.0);
            assert_eq!(this.session.project(), &before);
        });
    }

    #[gpui::test]
    fn workshop_slash_effort_completes_on_tab_and_executes_on_enter(cx: &mut gpui::TestAppContext) {
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
            s.preferences.backend = "rig".into();
            s.preferences.thinking = Some(true);
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-brief", cx);
        cx.simulate_input("/ef");
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("workshop-slash-completions").is_some());
        cx.simulate_keystrokes("tab");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().brief.content(),
                "/effort "
            )
        });
        cx.simulate_input("h");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.preferences.effort, auris_session::ReasoningEffort::High);
            assert_eq!(s.preferences.thinking, None);
            assert!(s.brief.content().is_empty());
            assert!(!s.submitted);
        });
    }

    #[gpui::test]
    fn reasoning_popover_uses_supported_steps_and_keeps_workshop_preferences_local(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        let before = app.read_with(cx, |this, _| this.settings.agent.clone());
        app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            s.preferences.backend = "codex".into();
            s.preferences.model = "fixture".into();
            s.preferences.effort = auris_session::ReasoningEffort::Default;
            s.preferences.fast_mode = false;
            s.models = vec![super::super::agent_chat::ModelOption {
                name: "fixture".into(),
                efforts: Some(vec![
                    auris_session::ReasoningEffort::Low,
                    auris_session::ReasoningEffort::High,
                ]),
                ..Default::default()
            }];
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("workshop-effort", cx);
        crate::harness::paint(&app, cx);
        let popup = cx.debug_bounds("workshop-effort-popup").unwrap();
        assert!(popup.bottom() <= cx.debug_bounds("workshop-effort").unwrap().top());
        cx.simulate_keystrokes("right");
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().preferences.effort,
                auris_session::ReasoningEffort::Low
            )
        });
        cx.simulate_keystrokes("end");
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().preferences.effort,
                auris_session::ReasoningEffort::High
            )
        });
        let slider = cx.debug_bounds("workshop-effort-slider").unwrap();
        crate::harness::press(cx, slider.center());
        crate::harness::release(cx, slider.center());
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().preferences.effort,
                auris_session::ReasoningEffort::Low
            )
        });
        crate::harness::click("reasoning-fast-mode", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.song_workshop.as_ref().unwrap().preferences.fast_mode)
        });
        crate::harness::click("reasoning-reset", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            let prefs = &this.song_workshop.as_ref().unwrap().preferences;
            assert_eq!(prefs.effort, auris_session::ReasoningEffort::Default);
            assert!(!prefs.fast_mode);
            assert_eq!(this.settings.agent, before);
        });
        cx.simulate_keystrokes("escape");
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.song_workshop.is_some()));
    }

    #[gpui::test]
    fn workshop_slash_models_keep_provider_routes_and_global_preferences(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        let before = app.read_with(cx, |this, _| this.settings.agent.clone());
        app.update(cx, |this, _| {
            this.song_workshop.as_mut().unwrap().models = ["codex", "rig"]
                .into_iter()
                .map(|backend| super::super::agent_chat::ModelOption {
                    name: "same-model".into(),
                    connection: Some(auris_agent::ModelConnection {
                        backend: backend.into(),
                        provider: "ollama".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .collect();
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-brief", cx);
        cx.simulate_input("/model ");
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop
                    .as_ref()
                    .unwrap()
                    .completions
                    .dismissed
                    .as_deref(),
                Some("/model ")
            )
        });
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert!(
                s.completions
                    .matches(
                        s.brief.content(),
                        &s.models,
                        super::super::chat_commands::Scope::Workshop
                    )
                    .is_empty()
            );
        });
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_workshop.as_ref().unwrap().active_field, Some(0))
        });
        // Native text input reopens the list and resets its selection.
        cx.simulate_keystrokes("backspace");
        cx.simulate_input(" ");
        cx.simulate_keystrokes("down enter");
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.preferences.backend, "rig");
            assert_eq!(s.preferences.model, "same-model");
            assert!(s.brief.content().is_empty());
            assert_eq!(this.settings.agent, before);
        });
    }

    #[gpui::test]
    fn workshop_slash_click_completes_and_shift_enter_keeps_the_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                this.show_project_chooser(window, cx);
                this.open_song_workshop(window, cx);
            })
        });
        make_ready(&app, cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("song-brief", cx);
        cx.simulate_input("/ef");
        crate::harness::paint(&app, cx);
        crate::harness::click("workshop-slash-option-0", cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.song_workshop.as_ref().unwrap().foci[0].is_focused(window),
                    "Completion must retain input focus"
                );
            })
        });
        cx.simulate_keystrokes("shift-enter");
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.brief.content(), "/effort \n");
            assert!(!s.submitted);
        });
    }

    #[gpui::test]
    fn workshop_slash_help_runs_without_a_model_and_preserves_attachments(
        cx: &mut gpui::TestAppContext,
    ) {
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
            s.preferences = Default::default();
            s.references.attachments.push("draft.txt".into());
            this.start_midi_draft(false);
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("song-brief", cx);
        cx.simulate_input("/help");
        // The send button uses the same local command route as Enter.
        crate::harness::paint(&app, cx);
        crate::harness::click("song-generate", cx);
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            let help = s.command_feedback.as_ref().unwrap().as_ref().unwrap();
            assert!(help.contains("/model"));
            assert!(help.contains("/effort"));
            assert!(!help.contains("/permissions"));
            assert!(
                s.references.midi_draft.is_some(),
                "Local commands keep the melody draft open"
            );
            assert_eq!(
                s.references.attachments,
                vec![std::path::PathBuf::from("draft.txt")]
            );
            assert!(s.brief.content().is_empty());
            assert!(!s.submitted);
        });
    }

    #[gpui::test]
    fn workshop_slash_errors_preserve_input_and_busy_settings(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| app.update(cx, |this, cx| this.open_song_workshop(window, cx)));
        make_ready(&app, cx);
        app.update(cx, |this, cx| {
            let before = this.song_workshop.as_ref().unwrap().preferences.clone();
            for command in [
                "/permissions",
                "/unknown",
                "/effort invalid",
                "/model absent",
            ] {
                this.song_workshop.as_mut().unwrap().brief = TextField::new(command);
                this.start_song_workshop(cx);
                let s = this.song_workshop.as_ref().unwrap();
                assert!(s.command_feedback.as_ref().unwrap().is_err(), "{command}");
                assert_eq!(s.brief.content(), command);
                assert_eq!(s.preferences, before);
                assert!(!s.submitted);
            }
            let s = this.song_workshop.as_mut().unwrap();
            s.submitted = true;
            s.brief = TextField::new("/effort high");
            this.start_song_workshop(cx);
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.preferences, before);
            assert!(s.command_feedback.as_ref().unwrap().is_err());
        });
    }

    #[gpui::test]
    fn workshop_slash_does_not_submit_ime_preedit(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        cx.update(|window, cx| app.update(cx, |this, cx| this.open_song_workshop(window, cx)));
        make_ready(&app, cx);
        app.update(cx, |this, cx| {
            this.song_workshop
                .as_mut()
                .unwrap()
                .brief
                .replace_and_mark(0..0, "/help", None);
            this.start_song_workshop(cx);
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.brief.content(), "/help");
            assert!(s.brief.marked().is_some());
            assert!(s.command_feedback.is_none());
        });
    }

    #[gpui::test]
    fn workshop_input_prediction_accepts_tab_without_submitting(cx: &mut gpui::TestAppContext) {
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
            s.brief = TextField::new("A calm song");
            s.brief.caret_to_end();
            s.input_assist
                .offer(s.brief.content(), &s.preferences, " with piano");
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("workshop-input-suggestion").is_some());
        cx.simulate_keystrokes("tab");
        app.read_with(cx, |this, _| {
            let s = this.song_workshop.as_ref().unwrap();
            assert_eq!(s.brief.content(), "A calm song with piano");
            assert!(!s.submitted);
            assert!(!s.submitted);
        });
    }

    #[gpui::test]
    fn workshop_input_prediction_click_preserves_focus_and_rejects_stale_results(
        cx: &mut gpui::TestAppContext,
    ) {
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
            s.brief = TextField::new("A calm song");
            s.brief.caret_to_end();
            s.input_assist
                .offer(s.brief.content(), &s.preferences, " with piano");
            s.active_field = None;
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("workshop-accept-suggestion", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().brief.content(),
                "A calm song with piano"
            )
        });
        cx.simulate_input(" please");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_workshop.as_ref().unwrap().brief.content(),
                "A calm song with piano please"
            )
        });
        app.update(cx, |this, _| {
            let s = this.song_workshop.as_mut().unwrap();
            s.input_assist
                .offer(s.brief.content(), &s.preferences, " and strings");
            s.preferences.model = "changed-model".into();
            this.accept_workshop_suggestion();
            assert_eq!(
                this.song_workshop.as_ref().unwrap().brief.content(),
                "A calm song with piano please"
            );
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
