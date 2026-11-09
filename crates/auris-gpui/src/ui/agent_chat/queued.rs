//! Unsent messages belong to their conversation, not to the worker's tool-response channel.

use super::*;
use auris_session::conversations::QueuedMessage;

/// Runtime identity survives reordering; reopening a conversation creates fresh UI identities.
#[derive(Clone)]
pub(super) struct QueuedEntry {
    id: u64,
    pub(super) message: QueuedMessage,
}

impl From<QueuedMessage> for QueuedEntry {
    fn from(message: QueuedMessage) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            message,
        }
    }
}

impl std::ops::Deref for QueuedEntry {
    type Target = QueuedMessage;
    fn deref(&self) -> &QueuedMessage {
        &self.message
    }
}

impl std::ops::DerefMut for QueuedEntry {
    fn deref_mut(&mut self) -> &mut QueuedMessage {
        &mut self.message
    }
}

#[derive(Clone)]
struct QueuedTarget {
    conversation: Option<String>,
    id: u64,
}

impl QueuedTarget {
    fn position(&self, chat: &AgentChat) -> Option<usize> {
        if chat.conversations.active_id() != self.conversation.as_deref() {
            return None;
        }
        chat.queued.iter().position(|message| message.id == self.id)
    }
}

impl AgentChat {
    /// Whether the card at `index` still takes a pick: not answered, and not passed over by
    /// something the person typed after it.
    pub(super) fn choices_open(&self, index: usize) -> bool {
        matches!(
            self.entries.get(index),
            Some(ChatEntry::Choices {
                answered: false,
                ..
            })
        ) && !self.entries[index + 1..]
            .iter()
            .any(|entry| matches!(entry, ChatEntry::You { .. }))
    }

    /// Tick or untick one option of an open multiple-choice card.
    pub(super) fn toggle_choice(&mut self, index: usize, option: usize) {
        if !self.choices_open(index) {
            return;
        }
        if let Some(ChatEntry::Choices { card, picked, .. }) = self.entries.get_mut(index)
            && card.multiple
            && option < card.options.len()
        {
            if let Some(at) = picked.iter().position(|&ticked| ticked == option) {
                picked.remove(at);
            } else {
                picked.push(option);
            }
        }
    }
}

impl AurisApp {
    /// Send a card's pick as the person's next message.
    ///
    /// The pick goes through the queue rather than the composer, so a half-written draft, its
    /// attachments and its IME pre-edit stay where they are, and a pick made while the agent is
    /// still finishing its turn waits for the turn to end, ahead of anything queued before it:
    /// the agent asked, and is waiting for exactly this.
    pub(super) fn answer_agent_choices(
        &mut self,
        index: usize,
        picked: Vec<usize>,
        cx: &mut gpui::Context<Self>,
    ) {
        if !self.agent_chat.choices_open(index)
            || !self.agent_queue_available()
            || self.agent_chat.interrupting
        {
            return;
        }
        let Some(ChatEntry::Choices { card, .. }) = self.agent_chat.entries.get(index) else {
            return;
        };
        let Some(text) = card.answer(&picked) else {
            return;
        };
        if self.agent_chat.queued.is_empty() {
            self.agent_chat.queue_paused = false;
        }
        let selection_context = self.agent_selection_context().to_string();
        self.agent_chat.queued.push_front(
            QueuedMessage {
                text,
                attachments: Vec::new(),
                selection_context,
            }
            .into(),
        );
        if let Some(ChatEntry::Choices {
            picked: sent,
            answered,
            ..
        }) = self.agent_chat.entries.get_mut(index)
        {
            *sent = picked;
            *answered = true;
        }
        self.dispatch_queued_agent_message(cx);
        cx.notify();
    }

    fn agent_queue_available(&self) -> bool {
        !self.agent_chat.conversations.unavailable()
            && self.agent_chat.pending_send.is_none()
            && !self.agent_history_clear_pending()
            && self.agent_chat.pending_reload.is_none()
    }

    pub(super) fn agent_composer_can_submit(&self) -> bool {
        if self.agent_chat.input.content().trim().is_empty()
            || self.agent_chat.input.marked().is_some()
            || self.agent_chat.references.midi_loading
        {
            return false;
        }
        if crate::ui::chat_commands::is_control_command(self.agent_chat.input.content().trim()) {
            return !self.agent_operation_busy();
        }
        self.agent_queue_available()
            && (self.agent_chat.busy || !self.agent_operation_busy())
            && self.agent_send_blocker().is_none()
    }

    pub(super) fn enqueue_agent_message(&mut self) -> bool {
        if !self.commit_agent_midi_draft() {
            return false;
        }
        if !self.agent_composer_can_submit()
            || crate::ui::chat_commands::is_control_command(self.agent_chat.input.content().trim())
        {
            return false;
        }
        if let Err(error) = auris_agent::validate_attachments(
            &self.settings.agent,
            &self.agent_chat.references.attachments,
        ) {
            self.agent_chat.references.attachment_error = Some(error);
            return false;
        }
        if self.agent_chat.queued.is_empty() {
            self.agent_chat.queue_paused = false;
        }
        self.agent_chat.queued.push_back(
            QueuedMessage {
                text: self.agent_chat.input.content().trim().to_owned(),
                attachments: std::mem::take(&mut self.agent_chat.references.attachments),
                selection_context: self.agent_selection_context().to_string(),
            }
            .into(),
        );
        self.agent_chat.input = TextField::default();
        self.agent_chat.references.attachment_error = None;
        true
    }

    fn queued_attachments_available(&self, message: &QueuedMessage) -> bool {
        message
            .attachments
            .iter()
            .all(|path| self.agent_chat.references.attached_here.contains(path))
    }

    pub(super) fn dispatch_queued_agent_message(&mut self, cx: &mut gpui::Context<Self>) {
        if self.agent_chat.queue_paused
            || self.agent_operation_busy()
            || self.agent_chat.interrupting
            || self.agent_chat.controls.compacting
            || self.agent_chat.controls.review.is_some()
            || self.agent_chat.queued.is_empty()
            || !self.agent_queue_available()
        {
            return;
        }
        if !self.settings.agent.is_configured() {
            self.agent_chat.queue_paused = true;
            cx.notify();
            return;
        }
        let Some(message) = self.agent_chat.queued.front() else {
            return;
        };
        if !self.queued_attachments_available(message) {
            self.agent_chat.references.attachment_error =
                Some(self.t(Key::AgentQueueReattach).into());
            self.agent_chat.queue_paused = true;
            cx.notify();
            return;
        }
        if let Err(error) =
            auris_agent::validate_attachments(&self.settings.agent, &message.attachments)
        {
            self.agent_chat.references.attachment_error = Some(error);
            self.agent_chat.queue_paused = true;
            cx.notify();
            return;
        }
        if !self.start_agent_history_load() {
            return;
        }
        let Some(message) = self.agent_chat.queued.front().cloned() else {
            return;
        };
        let message = message.message;
        let submission = PendingAgentSend {
            token: self.agent_document_token(),
            text: message.text,
            attachments: message.attachments,
            preferences: self.settings.agent.clone(),
            selection_context: serde_json::json!({
                "selection_at_submission": message.selection_context,
                "current_revision": self.session.revision(),
                "queued_message": "This selection was captured when the message was queued and may predate completed turns. Verify track/clip IDs and note indices against the current document before editing."
            }).to_string(),
        };
        // Automatic dispatch must leave the next draft, selection and IME pre-edit alone.
        let draft = std::mem::take(&mut self.agent_chat.input);
        let attachments = std::mem::take(&mut self.agent_chat.references.attachments);
        if self.send_agent_message(submission) {
            self.agent_chat.queued.pop_front();
        } else {
            self.agent_chat.queue_paused = true;
        }
        self.agent_chat.input = draft;
        self.agent_chat.references.attachments = attachments;
        cx.notify();
    }

    fn prioritize_queued_agent_message(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        if !self.agent_queue_available() || self.agent_chat.interrupting {
            return;
        }
        let Some(message) = self.agent_chat.queued.get(index) else {
            return;
        };
        if !self.queued_attachments_available(message) {
            self.agent_chat.references.attachment_error =
                Some(self.t(Key::AgentQueueReattach).into());
            return;
        }
        if let Some(message) = self.agent_chat.queued.remove(index) {
            self.agent_chat.queued.push_front(message);
        }
        self.agent_chat.queue_paused = false;
        if self.agent_chat.busy {
            // Startup can be interrupted before the worker even receives its first request.
            // Carry that request with the priority message as well as checkpointing model work.
            let request = self
                .agent_chat
                .entries
                .iter()
                .rev()
                .find_map(|entry| match entry {
                    ChatEntry::You { text, .. } => Some(text.clone()),
                    _ => None,
                });
            if let Some(message) = self.agent_chat.queued.front_mut() {
                message.selection_context = serde_json::json!({
                    "selection_at_submission": message.selection_context,
                    "interrupted_request": request,
                    "continuation": "The current message is the user's newest instruction. The preceding request was interrupted and may have made partial edits. Inspect the current document before continuing."
                }).to_string();
            }
            if let Some(link) = self.agent_chat.link.as_mut() {
                link.worker.cancel();
                link.job = None;
                link.inspection = None;
                link.listening = None;
                link.sound_search = None;
                self.agent_chat.controls.pending = None;
                self.agent_chat.controls.review = None;
                self.agent_chat.controls.compacting = false;
                self.agent_chat.restore_pending_focus = false;
                self.agent_chat.loaded_history = None;
                self.agent_chat.history_project = None;
                self.agent_chat.history_load = None;
                // Keep the receiver until native interruption and shutdown have completed.
                self.agent_chat.interrupting = true;
            } else {
                self.agent_stop(cx);
                self.agent_chat.queue_paused = false;
            }
        }
        cx.notify();
    }

    pub(super) fn submit_agent_message_now(&mut self, cx: &mut gpui::Context<Self>) {
        if self.enqueue_agent_message() {
            self.prioritize_queued_agent_message(self.agent_chat.queued.len() - 1, cx);
        }
    }

    fn edit_queued_agent_message(&mut self, index: usize) {
        if !self.agent_chat.input.content().is_empty()
            || !self.agent_chat.references.attachments.is_empty()
        {
            return;
        }
        let Some(message) = self.agent_chat.queued.get(index) else {
            return;
        };
        if !self.queued_attachments_available(message) {
            self.agent_chat.references.attachment_error =
                Some(self.t(Key::AgentQueueReattach).into());
            return;
        }
        if let Some(message) = self.agent_chat.queued.remove(index) {
            let message = message.message;
            self.agent_chat.input = TextField::new(message.text);
            self.agent_chat.references.attachments = message.attachments;
            self.focus_agent_field(AgentField::Chat);
        }
    }

    /// The messages waiting for the turn to end, as Atelier's `MessageQueue`.
    ///
    /// The queue names a message by where it stood when it was drawn, and a turn can finish
    /// between paint and click and send the first one; each handler therefore resolves the
    /// message it was drawn for by identity, through a [`QueuedTarget`], before acting on it.
    pub(super) fn agent_queue_view(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let enabled = self.agent_queue_available() && !self.agent_chat.interrupting;
        let can_edit = self.agent_chat.input.content().is_empty()
            && self.agent_chat.references.attachments.is_empty();
        let conversation = self.agent_chat.conversations.active_id().map(str::to_owned);
        let targets: std::rc::Rc<Vec<QueuedTarget>> = std::rc::Rc::new(
            self.agent_chat
                .queued
                .iter()
                .map(|message| QueuedTarget {
                    conversation: conversation.clone(),
                    id: message.id,
                })
                .collect(),
        );
        let messages = self.agent_chat.queued.iter().map(|message| {
            let queued = atelier::agent::QueuedMessage::new(message.text.clone());
            if message.attachments.is_empty() {
                queued
            } else {
                queued.attachment(
                    self.t(Key::AgentQueuedAttachments)
                        .replace("{count}", &message.attachments.len().to_string()),
                )
            }
        });
        let resolve = |targets: &std::rc::Rc<Vec<QueuedTarget>>| {
            let targets = targets.clone();
            move |this: &Self, index: usize| {
                targets
                    .get(index)
                    .and_then(|target| target.position(&this.agent_chat))
            }
        };
        let (now, edit, remove) = (resolve(&targets), resolve(&targets), resolve(&targets));
        atelier::agent::MessageQueue::new("agent-queue")
            .messages(messages)
            .paused(self.agent_chat.queue_paused)
            .disabled(!enabled)
            .editable(can_edit)
            .on_pause(cx.listener(|this, paused: &bool, _, cx| {
                this.agent_chat.queue_paused = *paused;
                if !*paused {
                    this.agent_chat.history_error_project = None;
                }
                cx.notify();
            }))
            .on_send_now(cx.listener(move |this, index: &usize, _, cx| {
                if let Some(index) = now(this, *index) {
                    this.prioritize_queued_agent_message(index, cx);
                }
                cx.notify();
            }))
            .on_edit(cx.listener(move |this, index: &usize, _, cx| {
                if let Some(index) = edit(this, *index) {
                    this.edit_queued_agent_message(index);
                }
                cx.notify();
            }))
            .on_remove(cx.listener(move |this, index: &usize, _, cx| {
                if let Some(index) = remove(this, *index) {
                    this.agent_chat.queued.remove(index);
                }
                cx.notify();
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, open, paint};

    #[test]
    fn queue_actions_follow_the_displayed_message_after_automatic_dispatch() {
        let mut chat = AgentChat::default();
        let message = QueuedMessage {
            text: "identical requests still have distinct identities".into(),
            ..Default::default()
        };
        chat.queued.push_back(message.clone().into());
        let second = QueuedEntry::from(message);
        let mut target = QueuedTarget {
            conversation: None,
            id: second.id,
        };
        chat.queued.push_back(second);
        assert_eq!(target.position(&chat), Some(1));
        chat.queued.pop_front();
        assert_eq!(target.position(&chat), Some(0));
        target.conversation = Some("another conversation".into());
        assert_eq!(target.position(&chat), None);
        target.conversation = None;
        chat.queued.pop_front();
        assert_eq!(target.position(&chat), None);
    }

    #[gpui::test]
    #[ignore = "requires a signed-in Codex or Claude CLI and AURIS_QUEUE_TEST_MODEL; runs frontier inference"]
    fn installed_frontier_backend_queues_and_interrupts_without_losing_the_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let model =
            std::env::var("AURIS_QUEUE_TEST_MODEL").expect("explicit frontier model required");
        let backend = std::env::var("AURIS_QUEUE_TEST_BACKEND").unwrap_or_else(|_| "codex".into());
        assert!(matches!(backend.as_str(), "codex" | "claude"));
        let folder = tempfile::tempdir().unwrap();
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.session
                .save_as(&folder.path().join("Queue review.auris"))
                .unwrap();
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.settings.agent.backend = backend;
            this.settings.agent.model = model;
            this.settings.agent.effort = ReasoningEffort::Low;
            this.settings.agent.policy.mode = auris_session::agent_policy::Mode::ReadOnly;
            this.initialize_conversations();
        });
        let poll_until = |cx: &mut gpui::VisualTestContext, ready: &dyn Fn(&AurisApp) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                app.update(cx, |this, cx| this.drain_agent(cx));
                app.read_with(cx, |this, _| {
                    let errors: Vec<_> = this
                        .agent_chat
                        .entries
                        .iter()
                        .filter_map(|entry| match entry {
                            ChatEntry::Error(message) => Some(message.as_str()),
                            _ => None,
                        })
                        .collect();
                    assert!(errors.is_empty(), "frontier backend errors: {errors:?}");
                });
                if app.read_with(cx, |this, _| ready(this)) {
                    break;
                }
                assert!(Instant::now() < deadline, "frontier queue test timed out");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        poll_until(cx, &|this| {
            this.agent_chat.conversations.active_id().is_some()
        });
        let score = app.read_with(cx, |this, _| this.project().clone());
        app.update(cx, |this, _| {
            this.agent_chat.input = TextField::new("This is a messaging UI test. Do not call tools or change the project. Reply exactly FIRST_OK.");
            this.agent_send();
            assert!(this.agent_chat.busy);
            this.agent_chat.input = TextField::new("Do not call tools. Reply exactly SECOND_OK.");
            this.agent_send();
            assert_eq!(this.agent_chat.queued.len(), 1);
            this.agent_chat.input = TextField::new("Preserve this unsent draft");
        });
        poll_until(cx, &|this| {
            !this.agent_chat.busy && this.agent_chat.queued.is_empty()
        });
        app.update(cx, |this, _| {
            let answers: String = this.agent_chat.entries.iter().filter_map(|entry| match entry { ChatEntry::Agent(text) => Some(text.as_str()), _ => None }).collect();
            assert!(answers.contains("FIRST_OK") && answers.contains("SECOND_OK"), "{answers}");
            assert_eq!(this.agent_chat.input.content(), "Preserve this unsent draft");
            this.agent_chat.input = TextField::new("Do not call tools or modify the project. Write a numbered list of 200 short imaginary song titles.");
            this.agent_send();
        });
        poll_until(cx, &|this| {
            !matches!(
                this.agent_chat.phase,
                AgentPhase::Waiting | AgentPhase::Idle
            )
        });
        app.update(cx, |this, cx| {
            this.agent_chat.input =
                TextField::new("Do not call tools. Reply exactly AFTER_PRIORITY_OK.");
            this.agent_send();
            this.agent_chat.input = TextField::new(
                "Stop listing song titles. Do not call tools. Reply exactly INTERRUPT_OK.",
            );
            this.submit_agent_message_now(cx);
            assert!(this.agent_chat.interrupting);
            this.agent_chat.input = TextField::new("Preserve this draft after interruption");
        });
        poll_until(cx, &|this| {
            !this.agent_chat.busy && this.agent_chat.queued.is_empty()
        });
        app.read_with(cx, |this, _| {
            let answers: String = this
                .agent_chat
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    ChatEntry::Agent(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let priority = answers.find("INTERRUPT_OK").expect("priority reply");
            let queued = answers.find("AFTER_PRIORITY_OK").expect("queued reply");
            assert!(priority < queued);
            assert_eq!(
                this.agent_chat.input.content(),
                "Preserve this draft after interruption"
            );
            assert_eq!(this.project(), &score);
            assert!(
                !this
                    .agent_chat
                    .entries
                    .iter()
                    .any(|entry| matches!(entry, ChatEntry::Error(_)))
            );
        });
        app.update(cx, |this, cx| this.agent_stop(cx));
        if let Some(flush) = app.update(cx, |this, _| this.flush_conversation_storage()) {
            flush.recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }

    #[gpui::test]
    fn busy_composer_queues_in_order_without_touching_the_running_transcript(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.settings.agent.backend = "codex".into();
            this.settings.agent.model = "frontier-fixture".into();
            this.agent_chat.busy = true;
            this.agent_chat
                .entries
                .push(ChatEntry::user("Original request".into()));
            this.agent_chat.input = TextField::new("Keep the melody");
            this.agent_send();
            this.agent_chat.input = TextField::new("Then soften the drums");
            this.agent_send();
            assert!(this.agent_chat.busy);
            assert!(this.agent_chat.link.is_none());
            assert_eq!(this.agent_chat.entries.len(), 1);
            assert_eq!(
                this.agent_chat
                    .queued
                    .iter()
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>(),
                ["Keep the melody", "Then soften the drums"]
            );
            assert!(this.agent_chat.input.content().is_empty());
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("agent-send").is_some());
        assert!(cx.debug_bounds("agent-stop").is_some());
        assert!(cx.debug_bounds("agent-queue-message-1").is_some());
        click("agent-queue-send-now-1", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.agent_chat.queued.front().unwrap().text,
                "Then soften the drums"
            );
            assert!(!this.agent_chat.queue_paused);
        });
        // No worker is constructed in this test; exercise only the real queue controls.
        click("agent-queue-pause", cx);
        paint(&app, cx);
        click("agent-queue-edit-0", cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.queue_paused);
            assert_eq!(this.agent_chat.input.content(), "Then soften the drums");
            assert_eq!(this.agent_chat.queued.len(), 1);
        });
    }

    #[gpui::test]
    fn stop_and_errors_pause_the_queue_and_untrusted_saved_attachments_cannot_dispatch(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent.backend = "claude".into();
            this.settings.agent.model = "frontier-fixture".into();
            this.agent_chat.queued.push_back(
                QueuedMessage {
                    text: "Continue".into(),
                    ..Default::default()
                }
                .into(),
            );
            this.agent_stop(cx);
            this.dispatch_queued_agent_message(cx);
            assert_eq!(this.agent_chat.queued.len(), 1);
            assert!(this.agent_chat.link.is_none());
            this.agent_chat.queue_paused = false;
            this.agent_chat.absorb(
                AgentEvent::Error {
                    message: "fixture failure".into(),
                },
                None,
                false,
            );
            assert!(this.agent_chat.queue_paused);
            this.agent_chat.queue_paused = false;
            this.agent_chat
                .queued
                .front_mut()
                .unwrap()
                .attachments
                .push(PathBuf::from("private-reference.png"));
            this.dispatch_queued_agent_message(cx);
            assert!(this.agent_chat.queue_paused);
            assert!(this.agent_chat.link.is_none());
            assert!(this.agent_chat.references.attachment_error.is_some());
            assert_eq!(this.agent_chat.queued.len(), 1);
        });
    }
}
