//! The editor's conversation drawn by Atelier's `ChatView`.
//!
//! [`AgentChat::entries`] stays the model: the worker's events, the saved conversations and the
//! copy-as-Markdown all read it. `ChatView` is the view, and owns what is purely about reading —
//! where the person is scrolled to, which tool calls and reasoning are open, the jump-to-latest
//! button and its count — so the panel scrolls, groups tool calls and answers questions exactly
//! as every other Atelier conversation does.
//!
//! Each frame the visible window of entries is compared with what the view was last given, and
//! only the difference is sent: new entries pushed, changed ones replaced in place, entries that
//! left the front of the window removed. Anything the comparison cannot express as that — a new
//! conversation, a shorter transcript, the window moving back to earlier messages — replaces the
//! whole view, which also shows its end.

use std::rc::Rc;

use atelier::agent::{ChatEntry as ViewEntry, ChatEvent, ChatView, Choice, EntryId, ToolState};
use auris_i18n::Language;
use gpui::{Entity, Subscription};

use super::*;

/// The view's element id. Its rows are `agent-chat-{part}-{entry}`, for example
/// `agent-chat-answer-3`; see `AgentChat::chat_row_id`.
pub(crate) const CHAT_VIEW_ID: &str = "agent-chat";

/// One entry as the view was last given it.
#[derive(Clone, PartialEq)]
struct Shown {
    entry: ChatEntry,
    streaming: bool,
}

/// The `ChatView` and what it currently shows.
pub(crate) struct ChatSync {
    pub(crate) view: Entity<ChatView>,
    /// What each of the view's entries was built from, oldest first. `None` marks an entry
    /// whose view may have moved on by itself (a pick on a choice card) and must be rebuilt.
    rows: Vec<(EntryId, Option<Shown>)>,
    /// The position, counted from the first entry this panel ever held, of `rows[0]`.
    first: usize,
    /// [`AgentChat::transcript_epoch`] when the rows were built.
    epoch: u64,
    language: Language,
    thinking: Option<SharedString>,
    _events: Subscription,
}

impl ChatSync {
    /// Index into [`AgentChat::entries`] of a view entry, if it is still one of them.
    fn index_of(&self, id: EntryId, trimmed: usize) -> Option<usize> {
        let at = self.rows.iter().position(|(row, _)| *row == id)?;
        (self.first + at).checked_sub(trimmed)
    }
}

impl AgentChat {
    /// The element id of one part of the row showing `entries[index]`, for window tests and
    /// for anything that has to point at a row: `answer`, `user`, `tool`, `reasoning`,
    /// `choices`, `copy`.
    #[cfg(test)]
    pub(crate) fn chat_row_id(&self, index: usize, part: &str) -> Option<String> {
        let sync = self.chat.as_ref()?;
        let at = (self.trimmed + index).checked_sub(sync.first)?;
        let (id, _) = sync.rows.get(at)?;
        // `EntryId` keeps its number private; its debug form is the only way to name it.
        let number = format!("{id:?}")
            .trim_start_matches("EntryId(")
            .trim_end_matches(')')
            .to_string();
        Some(format!("{CHAT_VIEW_ID}-{part}-{number}"))
    }

    /// Whether the person has opened the reasoning or tool call showing `entries[index]`.
    #[cfg(test)]
    pub(crate) fn chat_row_open(&self, index: usize, cx: &gpui::App) -> bool {
        let Some(sync) = self.chat.as_ref() else {
            return false;
        };
        let Some(at) = (self.trimmed + index).checked_sub(sync.first) else {
            return false;
        };
        sync.rows
            .get(at)
            .is_some_and(|(id, _)| sync.view.read(cx).is_open(*id))
    }

    /// Entries that arrived while the person was reading further up.
    #[cfg(test)]
    pub(crate) fn unread_entries(&self, cx: &gpui::App) -> usize {
        self.chat
            .as_ref()
            .map_or(0, |sync| sync.view.read(cx).unread())
    }
}

impl AurisApp {
    /// The conversation view, brought up to date with the transcript.
    pub(super) fn agent_chat_view(
        &mut self,
        transcript_start: usize,
        thinking: Option<SharedString>,
        cx: &mut gpui::Context<Self>,
    ) -> Entity<ChatView> {
        let language = self.language();
        let mut sync = match self.agent_chat.chat.take() {
            Some(sync) => sync,
            None => {
                let view = cx.new(|_| ChatView::new(CHAT_VIEW_ID).anonymous());
                let events = cx.subscribe(&view, |this, _, event: &ChatEvent, cx| {
                    this.agent_chat_event(event, cx);
                });
                ChatSync {
                    view,
                    rows: Vec::new(),
                    first: 0,
                    epoch: u64::MAX,
                    language,
                    thinking: None,
                    _events: events,
                }
            }
        };
        let entries = &self.agent_chat.entries;
        let len = entries.len();
        let start = transcript_start.min(len);
        let trimmed = self.agent_chat.trimmed;
        let first = trimmed + start;
        let busy = self.agent_chat.busy;
        let shown = |index: usize| Shown {
            entry: entries[index].clone(),
            streaming: entry_is_streaming(busy, index, len, &entries[index]),
        };
        let replace = sync.epoch != self.agent_chat.transcript_epoch
            || sync.language != language
            || first < sync.first
            || sync.first + sync.rows.len() > trimmed + len;
        let view = sync.view.clone();
        if replace {
            let rows: Vec<Shown> = (start..len).map(shown).collect();
            let built: Vec<ViewEntry> = (start..len)
                .zip(&rows)
                .map(|(index, row)| self.agent_view_entry(trimmed + index, row, cx))
                .collect();
            let ids = view.update(cx, |chat, cx| {
                chat.replace(built, cx);
                chat.entries().map(|(id, _)| id).collect::<Vec<_>>()
            });
            sync.rows = ids.into_iter().zip(rows.into_iter().map(Some)).collect();
            sync.first = first;
            sync.epoch = self.agent_chat.transcript_epoch;
            sync.language = language;
            sync.thinking = None;
        } else {
            // Entries that left the front of the window, oldest first.
            let gone: Vec<EntryId> = sync
                .rows
                .drain(..(first - sync.first).min(sync.rows.len()))
                .map(|(id, _)| id)
                .collect();
            sync.first = first;
            let mut changed = Vec::new();
            for (offset, (id, last)) in sync.rows.iter_mut().enumerate() {
                let index = start + offset;
                let streaming = entry_is_streaming(busy, index, len, &entries[index]);
                let same = last.as_ref().is_some_and(|last| {
                    last.streaming == streaming && last.entry == entries[index]
                });
                if !same {
                    let row = shown(index);
                    changed.push((*id, self.agent_view_entry(trimmed + index, &row, cx)));
                    *last = Some(row);
                }
            }
            let pushed: Vec<Shown> = (start + sync.rows.len()..len).map(shown).collect();
            let built: Vec<ViewEntry> = (start + sync.rows.len()..len)
                .zip(&pushed)
                .map(|(index, row)| self.agent_view_entry(trimmed + index, row, cx))
                .collect();
            let jump = std::mem::take(&mut self.agent_chat.jump_pending);
            if !gone.is_empty() || !changed.is_empty() || !built.is_empty() || jump {
                let ids = view.update(cx, |chat, cx| {
                    for id in gone {
                        chat.remove(id, cx);
                    }
                    for (id, entry) in changed {
                        chat.update_entry(id, |shown| *shown = entry, cx);
                    }
                    let ids: Vec<EntryId> = built.into_iter().map(|e| chat.push(e, cx)).collect();
                    if jump {
                        chat.jump_to_latest(cx);
                    }
                    ids
                });
                sync.rows
                    .extend(ids.into_iter().zip(pushed.into_iter().map(Some)));
            }
        }
        self.agent_chat.jump_pending = false;
        if sync.thinking != thinking {
            sync.thinking = thinking.clone();
            view.update(cx, |chat, cx| chat.set_thinking(thinking, cx));
        }
        self.agent_chat.chat = Some(sync);
        view
    }

    /// What the person did in the view: so far, only answering a question.
    fn agent_chat_event(&mut self, event: &ChatEvent, cx: &mut gpui::Context<Self>) {
        let ChatEvent::Chose { entry, picked } = event else {
            return;
        };
        let Some(sync) = self.agent_chat.chat.as_mut() else {
            return;
        };
        let index = sync.index_of(*entry, self.agent_chat.trimmed);
        // The card has already shown the pick as sent. Whether it was is the transcript's to
        // say — a refused answer reopens the card — so the row is rebuilt from it.
        if let Some(row) = sync.rows.iter_mut().find(|(id, _)| id == entry) {
            row.1 = None;
        }
        if let Some(index) = index {
            self.answer_agent_choices(index, picked.clone(), cx);
        }
        cx.notify();
    }

    /// One transcript entry as the view draws it. `key` is the entry's position counted from
    /// the first this panel ever held, so ids built from it never repeat.
    fn agent_view_entry(
        &self,
        key: usize,
        shown: &Shown,
        cx: &mut gpui::Context<Self>,
    ) -> ViewEntry {
        let language = self.language();
        match &shown.entry {
            ChatEntry::You { text, attachments } if attachments.is_empty() => {
                ViewEntry::user(text.clone())
            }
            ChatEntry::You { text, attachments } => {
                let app = cx.entity().downgrade();
                let text: SharedString = text.clone().into();
                let files: Rc<Vec<(PathBuf, SharedString)>> = Rc::new(
                    attachments
                        .iter()
                        .map(|path| {
                            let full = path.to_string_lossy().into_owned();
                            // A saved conversation may have originated on another platform.
                            let name = full
                                .rsplit(['/', '\\'])
                                .next()
                                .unwrap_or(&full)
                                .replace(['\r', '\n'], " ");
                            (path.clone(), name.into())
                        })
                        .collect(),
                );
                ViewEntry::custom(move |_, _| {
                    let chips = files.iter().enumerate().map(|(n, (path, name))| {
                        let app = app.clone();
                        let path = path.clone();
                        atelier::agent::ContextChip::new(
                            SharedString::from(format!("agent-message-attachment-{key}-{n}")),
                            atelier::IconName::File,
                            name.clone(),
                        )
                        .on_click(move |_, _, cx| {
                            app.update(cx, |this, cx| this.open_agent_attachment(path.clone(), cx))
                                .ok();
                        })
                    });
                    agent_components::user_message_with_attachments(
                        SharedString::from(format!("agent-message-{key}")),
                        text.clone(),
                        chips,
                    )
                })
            }
            ChatEntry::Agent(text) => {
                let mut entry = ViewEntry::assistant(text.clone());
                if let ViewEntry::Assistant { streaming, .. } = &mut entry {
                    *streaming = shown.streaming;
                }
                entry
            }
            ChatEntry::Reasoning(text) => {
                let summary = match log_preview(text) {
                    preview if preview.is_empty() => Key::AgentReasoningLog.get(language).into(),
                    preview => format!("{} — {preview}", Key::AgentReasoningLog.get(language)),
                };
                ViewEntry::reasoning(summary, text.clone())
            }
            ChatEntry::RestoredContext(detail) => {
                ViewEntry::reasoning(Key::AgentRestoredContext.get(language), detail.clone())
            }
            ChatEntry::Status(text) => ViewEntry::status(text.clone()),
            ChatEntry::Tool {
                name,
                args,
                ok,
                line,
                detail,
                images,
            } => {
                let (summary, state) = if line.is_empty() {
                    (
                        Key::AgentPhaseTool.get(language).to_string(),
                        ToolState::Running,
                    )
                } else if *ok {
                    (line.clone(), ToolState::Succeeded)
                } else {
                    (line.clone(), ToolState::Failed)
                };
                let mut sections = Vec::new();
                if !args.is_empty() {
                    sections.push(format!("{}\n{args}", Key::AgentToolArguments.get(language)));
                }
                if !detail.is_empty() {
                    sections.push(format!("{}\n{detail}", Key::AgentToolResult.get(language)));
                }
                let icon = crate::ui::icons::tool_icon(name).atelier_name();
                if !images.is_empty() {
                    return self.pictured_tool_entry(
                        key,
                        name.clone().into(),
                        summary.into(),
                        state,
                        icon,
                        (!sections.is_empty()).then(|| sections.join("\n\n").into()),
                        images.clone(),
                        cx,
                    );
                }
                let entry = ViewEntry::tool(name.clone(), summary, state).icon(icon);
                if sections.is_empty() {
                    entry
                } else {
                    entry.detail(sections.join("\n\n"))
                }
            }
            ChatEntry::Error(message) => {
                ViewEntry::error(crate::i18n::agent_error_text(message, language, false))
            }
            ChatEntry::Note(key) if *key == Key::AgentEnded => ViewEntry::error(key.get(language)),
            ChatEntry::Note(key) => ViewEntry::status(key.get(language)),
            ChatEntry::Choices {
                card,
                picked,
                answered,
            } => {
                let options = card.options.iter().map(|option| {
                    let choice = Choice::new(option.label.clone());
                    match option.detail.clone() {
                        Some(detail) => choice.detail(detail),
                        None => choice,
                    }
                });
                let mut entry = ViewEntry::choices(card.question.clone(), options);
                if card.multiple {
                    entry = entry.multiple();
                }
                if let ViewEntry::Choices {
                    picked: shown,
                    answered: sent,
                    ..
                } = &mut entry
                {
                    shown.clone_from(picked);
                    *sent = *answered;
                }
                entry
            }
        }
    }
}

impl AurisApp {
    /// A tool call that answered with pictures: Atelier's own `ToolCall` row, with the pictures
    /// under it where the detail would open, so the log shows what the model was shown.
    ///
    /// `ChatView` has no pictures on a tool entry, so this row is drawn by the panel, and keeps
    /// its own open state: a custom row is invisible to the view's toggles. It stands outside
    /// the view's tool groups for the same reason.
    #[allow(clippy::too_many_arguments)]
    fn pictured_tool_entry(
        &self,
        key: usize,
        name: SharedString,
        summary: SharedString,
        state: ToolState,
        icon: atelier::IconName,
        detail: Option<SharedString>,
        images: Vec<ToolImage>,
        cx: &mut gpui::Context<Self>,
    ) -> ViewEntry {
        let app = cx.entity().downgrade();
        let chat = self
            .agent_chat
            .chat
            .as_ref()
            .map(|sync| sync.view.downgrade());
        ViewEntry::custom(move |_, cx| {
            let open = app
                .upgrade()
                .is_some_and(|app| app.read(cx).agent_chat.open_pictured_tools.contains(&key));
            let mut call = atelier::agent::ToolCall::new(
                SharedString::from(format!("{CHAT_VIEW_ID}-pictured-tool-{key}")),
                name.clone(),
                summary.clone(),
                state,
            )
            .icon(icon)
            .open(open);
            if let Some(detail) = detail.clone() {
                let app = app.clone();
                let chat = chat.clone();
                call = call.detail(detail).on_toggle(move |_, _, cx| {
                    app.update(cx, |this, cx| {
                        let open = &mut this.agent_chat.open_pictured_tools;
                        if !open.remove(&key) {
                            open.insert(key);
                        }
                        cx.notify();
                    })
                    .ok();
                    if let Some(chat) = chat.as_ref().and_then(|chat| chat.upgrade()) {
                        chat.update(cx, |_, cx| cx.notify());
                    }
                });
            }
            let theme = atelier::ActiveTheme::theme(cx).clone();
            tool_pictures(
                SharedString::from(format!("agent-tool-images-{key}")),
                call.into_any_element(),
                &images,
                &theme,
            )
        })
    }
}

/// A tool row with its pictures under it, indented as Atelier indents a tool call's detail.
pub(super) fn tool_pictures(
    selector: SharedString,
    row: gpui::AnyElement,
    images: &[ToolImage],
    theme: &atelier::Theme,
) -> gpui::AnyElement {
    let bound = crate::ui::attachment_preview::InlinePlace::Message.image_bound();
    let spacing = &theme.spacing;
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(row)
        .child(
            div()
                .debug_selector(move || selector.to_string())
                .mx(spacing.sm)
                .mb(spacing.xs)
                .flex()
                .flex_wrap()
                .gap(spacing.sm)
                .min_w_0()
                .children(images.iter().map(|image| {
                    crate::ui::attachment_preview::framed_picture(&image.0, bound, theme)
                })),
        )
        .into_any_element()
}

/// The selector of one part of the row showing `entries[index]`, as a window test clicks it.
#[cfg(test)]
pub(crate) fn row(
    app: &Entity<AurisApp>,
    cx: &mut gpui::VisualTestContext,
    index: usize,
    part: &str,
) -> &'static str {
    app.read_with(cx, |this, _| this.agent_chat.chat_row_id(index, part))
        .unwrap_or_else(|| panic!("no row {index} on screen"))
        .leak()
}

/// Where the part of the row showing `entries[index]` was drawn.
#[cfg(test)]
pub(crate) fn bounds(
    app: &Entity<AurisApp>,
    cx: &mut gpui::VisualTestContext,
    index: usize,
    part: &str,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    let selector = row(app, cx, index, part);
    cx.debug_bounds(selector)
}

/// Where the answer showing `entries[index]` was drawn: the span of its Markdown blocks, since
/// an answer's own box carries no selector.
#[cfg(test)]
pub(crate) fn answer_bounds(
    app: &Entity<AurisApp>,
    cx: &mut gpui::VisualTestContext,
    index: usize,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    let answer = row(app, cx, index, "answer");
    let mut span: Option<gpui::Bounds<gpui::Pixels>> = None;
    let mut misses = 0;
    for block in 0.. {
        let selector: &'static str = format!("{answer}-block-{block}").leak();
        match cx.debug_bounds(selector) {
            Some(bounds) => {
                misses = 0;
                span = Some(span.map_or(bounds, |span| span.union(&bounds)));
            }
            None if misses > 64 => break,
            None => misses += 1,
        }
    }
    span
}
