//! The agent panel: a conversation with a language model, beside the song it is about.
//!
//! The UI-free `auris-agent` library runs on a cancellable background thread. Channels carry
//! requests, events and host replies; the window never blocks on model or network work.
//!
//! Editing commands execute against the window's current session. They do not save or
//! reload a project; successful edits appear on repaint and use ordinary undo history.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use auris_i18n::Key;
use auris_session::AgentPreferences;
use gpui::{
    AnyElement, IntoElement, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*, px,
};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::text_field::TextField;
use crate::ui::widgets::{ButtonStyle, button};

mod controls;

/// Maximum transcript rows retained in the panel.
const CHAT_CAPACITY: usize = 500;

/// One line of the conversation, as the panel shows it.
#[derive(Debug, PartialEq)]
pub(crate) enum ChatEntry {
    /// What the person said.
    You(String),
    /// What the model answered.
    Agent(String),
    /// Status produced by the agent runtime, not by the model.
    Status(String),
    /// One tool call: running while `line` is empty, answered or refused once it is not.
    Tool {
        /// The tool's wire name.
        name: String,
        /// Whether it answered rather than refused.
        ok: bool,
        /// The first line of its answer.
        line: String,
        /// The whole answer, shown when the row is clicked open.
        detail: String,
    },
    /// Something went wrong — the worker, the provider, the channel.
    Error(String),
    /// A note from the panel itself, translated when drawn.
    Note(Key),
}

/// One event off the agent's wire, already parsed.
#[derive(Debug, PartialEq)]
pub(crate) enum AgentEvent {
    Permission {
        id: u64,
        tool: String,
        args: serde_json::Value,
    },
    Compacting,
    Compacted {
        ok: bool,
        message: String,
        manual: bool,
        tokens: u64,
    },
    /// A file-free command for the bound session.
    Edit {
        command: serde_json::Value,
    },
    /// Previously completed text turns recovered for this project.
    History {
        /// User and assistant text, oldest first.
        turns: Vec<(String, String)>,
    },
    /// A nonfatal message, which does not end the active turn.
    Notice {
        /// What happened.
        message: String,
    },
    /// The worker is up, and named what answered the phone.
    Ready {
        /// The model the agent resolved to.
        model: String,
    },
    /// A tool was asked.
    Call {
        /// Its wire name.
        tool: String,
    },
    /// A tool answered or refused.
    Result {
        /// Its wire name.
        tool: String,
        /// Whether it answered.
        ok: bool,
        /// The first line of what it said.
        line: String,
        /// Everything it said, for the row's opened form.
        detail: String,
    },
    /// A project file on disk is no longer what the window last read.
    Changed {
        /// The file, resolved and absolute.
        project: PathBuf,
    },
    /// The model's reply; the turn is over.
    Answer {
        /// The reply text.
        text: String,
        /// Prompt tokens the turn's final request carried — the context gauge's needle.
        input_tokens: u64,
        /// Tokens the model wrote across the turn.
        output_tokens: u64,
    },
    /// The turn failed; the worker is still alive.
    Error {
        /// What went wrong.
        message: String,
    },
    /// The background worker has finished.
    Ended,
}

/// Reads one JSON line off the wire into an event, or nothing for a line that is not one.
///
/// Tolerant on purpose: the child is another program, and a line this build does not know is a
/// line to skip, not a reason to tear the conversation down.
pub(crate) fn parse_event(line: &str) -> Option<AgentEvent> {
    let parsed: serde_json::Value = serde_json::from_str(line).ok()?;
    let text = |key: &str| {
        parsed
            .get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Some(match parsed.get("event")?.as_str()? {
        "ended" => AgentEvent::Ended,
        "permission" => AgentEvent::Permission {
            id: parsed.get("id")?.as_u64()?,
            tool: text("tool"),
            args: parsed.get("args")?.clone(),
        },
        "compacting" => AgentEvent::Compacting,
        "compacted" => AgentEvent::Compacted {
            ok: parsed["ok"] == true,
            message: text("message"),
            manual: parsed["manual"] == true,
            tokens: parsed["context_tokens"].as_u64().unwrap_or(0),
        },
        "edit" => AgentEvent::Edit {
            command: parsed.get("command")?.clone(),
        },
        "history" => AgentEvent::History {
            turns: parsed
                .get("turns")?
                .as_array()?
                .iter()
                .filter_map(|turn| {
                    Some((
                        turn.get("user")?.as_str()?.to_string(),
                        turn.get("answer")?.as_str()?.to_string(),
                    ))
                })
                .collect(),
        },
        "notice" => AgentEvent::Notice {
            message: text("message"),
        },
        "ready" => AgentEvent::Ready {
            model: text("model"),
        },
        "call" => AgentEvent::Call { tool: text("tool") },
        "result" => {
            let detail = text("text");
            AgentEvent::Result {
                tool: text("tool"),
                ok: parsed
                    .get("ok")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
                line: detail.lines().next().unwrap_or_default().to_string(),
                detail,
            }
        }
        "changed" => AgentEvent::Changed {
            project: PathBuf::from(text("project")),
        },
        "answer" => {
            let count = |key: &str| {
                parsed
                    .get(key)
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0)
            };
            AgentEvent::Answer {
                text: text("text"),
                input_tokens: count("input_tokens"),
                output_tokens: count("output_tokens"),
            }
        }
        "error" => AgentEvent::Error {
            message: text("message"),
        },
        _ => return None,
    })
}

/// The message as the wire carries it: the person's words, framed with what only the window
/// knows — which project is open in it.
///
/// The frame is one bracketed line the model reads and the transcript never shows; the person's
/// own words stay their own.
pub(crate) fn framed_say(text: &str, project: Option<&Path>) -> String {
    match project {
        Some(path) => format!(
            "[The project open in the window right now: {}]\n{text}",
            path.display()
        ),
        None => text.to_string(),
    }
}

/// Whether two paths name the same file, asked the way the filesystem answers it.
///
/// The agent resolves its side and the session keeps its own; canonicalising both is what makes
/// `Song.auris` written two ways still one file. A path that cannot be canonicalised — deleted
/// between the event and the question — falls back to plain equality.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The context gauge's colour by pressure — picocode's thresholds: red from 85%, yellow
/// from 60%, the accent below.
pub(crate) fn gauge_colour(ratio: f32, theme: &Theme) -> gpui::Hsla {
    if ratio >= 0.85 {
        theme.danger
    } else if ratio >= 0.6 {
        theme.warning
    } else {
        theme.accent
    }
}

/// What the window should do after one event has been absorbed.
#[derive(Debug, PartialEq)]
pub(crate) enum Absorbed {
    /// Nothing beyond repainting.
    Nothing,
    /// Reload this project: the agent rewrote the open document and the window holds nothing
    /// unsaved.
    Reload(PathBuf),
}

/// Which of the panel's text fields is being typed into.
///
/// The model is deliberately not among them: a model is something the provider *has*, so it
/// is picked from the list the provider answers with rather than spelt by hand.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentField {
    /// The message being written.
    Chat,
}

/// One model a provider reported serving.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelOption {
    /// The name the provider answers to.
    pub(crate) name: String,
    /// Its context window, when the provider says.
    pub(crate) context_length: Option<u64>,
}

/// Reads the one line `auris-agent models` prints into the picker's options.
///
/// A free function because it is a decision — what counts as a model, what counts as the
/// provider having failed — and the worker thread should carry none.
pub(crate) fn parse_model_list(line: &str) -> Result<Vec<ModelOption>, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(line).map_err(|error| format!("not JSON: {error}"))?;
    if let Some(error) = parsed.get("error").and_then(|value| value.as_str()) {
        return Err(error.to_string());
    }
    let models = parsed
        .get("models")
        .and_then(|value| value.as_array())
        .ok_or("no models in the answer")?;
    Ok(models
        .iter()
        .filter_map(|model| {
            Some(ModelOption {
                name: model.get("name")?.as_str()?.to_string(),
                context_length: model.get("context_length").and_then(|value| value.as_u64()),
            })
        })
        .collect())
}

/// One cancellable worker owned by the panel. Dropping it stops the conversation.
struct AgentLink {
    worker: auris_agent::Worker,
    inspection: Option<PendingInspection>,
}

struct PendingInspection {
    revision: u64,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: Receiver<Result<auris_session::audio_inspection::Inspection, String>>,
}

impl PendingInspection {
    fn poll(
        &self,
        revision: u64,
        same_document: bool,
    ) -> Option<Result<auris_session::audio_inspection::Inspection, String>> {
        if self.revision != revision || !same_document {
            return Some(Err(
                "The document changed during inspection; request a fresh inspection".into(),
            ));
        }
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Audio inspection worker stopped".into())),
        }
    }
}

impl Drop for PendingInspection {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl AgentLink {
    fn send(&self, wire: &str) -> Result<(), String> {
        self.worker.send(wire)
    }
}

/// Everything the agent panel is, apart from its pixels.
pub(crate) struct AgentChat {
    controls: controls::Controls,
    policy: auris_session::agent_policy::Policy,
    auto_compact_percent: Option<u8>,
    /// Requested Ollama context, independent of the model's architectural maximum.
    pub(crate) context_tokens: u32,
    pub(crate) output_tokens: u32,
    /// Ollama thinking override.
    pub(crate) thinking: Option<bool>,
    /// The transcript, oldest first.
    pub(crate) entries: Vec<ChatEntry>,
    /// The message being written.
    pub(crate) input: TextField,
    /// The model picked from the provider's list, while the settings section is open.
    pub(crate) chosen_model: String,
    /// The base URL being edited.
    pub(crate) url_field: TextField,
    /// The API key variable being edited.
    pub(crate) key_env_field: TextField,
    /// Whether the provider under edit is the OpenAI-compatible one.
    pub(crate) provider_openai: bool,
    /// What the provider last answered the model question with.
    pub(crate) models: Vec<ModelOption>,
    /// Whether that question is in flight.
    pub(crate) fetching_models: bool,
    /// What went wrong the last time it was asked, shown where the list would be.
    pub(crate) models_error: Option<String>,
    /// Whether the model picker is dropped open.
    pub(crate) model_menu: bool,
    /// Prompt tokens the last turn carried — the context gauge's needle.
    pub(crate) tokens_in: u64,
    /// Tokens the model has written across the conversation.
    pub(crate) tokens_out: u64,
    /// The chosen model's context window, when its listing said.
    pub(crate) context_window: Option<u64>,
    /// The transcript rows clicked open to their full text.
    pub(crate) expanded: std::collections::BTreeSet<usize>,
    /// Running tool rows by wire name, so a result never scans the transcript.
    open_tools: std::collections::BTreeMap<String, usize>,
    /// The wire a model listing comes back on.
    pub(crate) models_rx: Option<Receiver<Result<String, String>>>,
    /// Which field holds the keyboard, if any.
    pub(crate) focused: Option<AgentField>,
    /// Whether the settings section is showing.
    pub(crate) configuring: bool,
    /// Whether the settings fields have been seeded from the saved preferences.
    ///
    /// The form can legitimately contain an empty model while its provider and URL are being
    /// edited, so emptiness cannot stand in for this bit of lifecycle state.
    preferences_loaded: bool,
    /// Whether a message is in flight and unanswered.
    pub(crate) busy: bool,
    /// What the child said it resolved to, for the header.
    pub(crate) model_label: String,
    /// A project the agent rewrote while the window held unsaved edits, awaiting the button.
    pub(crate) pending_reload: Option<PathBuf>,
    /// Writes collected until the current turn ends, so a whole turn is one undo step.
    turn_project: Option<PathBuf>,
    /// The document whose folder and model history the running child belongs to.
    bound_project: Option<PathBuf>,
    /// Audio files selected by the user for the next message.
    attachments: Vec<PathBuf>,
    /// A different project produced by the last turn, ready to open explicitly.
    produced_project: Option<PathBuf>,
    /// The next child should replace its persisted text memory with an empty conversation.
    fresh_history: bool,
    /// Apply changed provider settings after the current reply has finished.
    restart_after_turn: bool,
    /// Where the transcript is scrolled to.
    pub(crate) scroll: gpui::ScrollHandle,
    link: Option<AgentLink>,
}

impl Default for AgentChat {
    fn default() -> Self {
        Self {
            controls: controls::Controls::default(),
            policy: Default::default(),
            auto_compact_percent: None,
            context_tokens: 32768,
            output_tokens: 4096,
            thinking: None,
            entries: Vec::new(),
            input: TextField::new(String::new()),
            chosen_model: String::new(),
            url_field: TextField::new(String::new()),
            key_env_field: TextField::new(String::new()),
            provider_openai: false,
            models: Vec::new(),
            fetching_models: false,
            models_error: None,
            model_menu: false,
            tokens_in: 0,
            tokens_out: 0,
            context_window: None,
            expanded: std::collections::BTreeSet::new(),
            open_tools: std::collections::BTreeMap::new(),
            models_rx: None,
            focused: None,
            configuring: false,
            preferences_loaded: false,
            busy: false,
            model_label: String::new(),
            pending_reload: None,
            turn_project: None,
            bound_project: None,
            attachments: Vec::new(),
            produced_project: None,
            fresh_history: false,
            restart_after_turn: false,
            scroll: gpui::ScrollHandle::new(),
            link: None,
        }
    }
}

impl AgentChat {
    /// Appends one transcript row, keeping indexes coherent and the newest row visible.
    fn push_entry(&mut self, entry: ChatEntry) -> usize {
        if self.entries.len() >= CHAT_CAPACITY {
            self.entries.remove(0);
            self.expanded = self
                .expanded
                .iter()
                .filter_map(|index| index.checked_sub(1))
                .collect();
            self.open_tools.retain(|_, index| {
                let Some(shifted) = index.checked_sub(1) else {
                    return false;
                };
                *index = shifted;
                true
            });
        }
        let index = self.entries.len();
        self.entries.push(entry);
        self.scroll.scroll_to_bottom();
        index
    }

    /// Whether one of this panel's fields is being typed into.
    pub(crate) fn typing(&self) -> bool {
        self.focused.is_some()
    }

    /// The field the keyboard is in, mutably.
    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        Some(match self.focused? {
            AgentField::Chat => &mut self.input,
        })
    }

    /// The field the keyboard is in.
    pub(crate) fn field(&self) -> Option<&TextField> {
        Some(match self.focused? {
            AgentField::Chat => &self.input,
        })
    }

    /// Copies the saved preferences into the settings section's fields.
    pub(crate) fn load_preferences(&mut self, prefs: &AgentPreferences) {
        self.policy = prefs.policy.clone();
        self.auto_compact_percent = prefs.auto_compact_percent;
        self.context_tokens = prefs.context_tokens.unwrap_or(32768);
        self.output_tokens = prefs.output_tokens.unwrap_or(4096);
        self.thinking = prefs.thinking;
        self.provider_openai = prefs.provider.trim() == "openai";
        self.chosen_model = prefs.model.trim().to_string();
        self.url_field = TextField::new(prefs.url.clone());
        self.key_env_field = TextField::new(prefs.api_key_env.clone());
        self.preferences_loaded = true;
    }

    /// Seeds an unopened settings form without overwriting edits on a later repaint.
    fn load_preferences_once(&mut self, prefs: &AgentPreferences) {
        if !self.preferences_loaded {
            self.load_preferences(prefs);
        }
    }

    /// The settings section's fields, read back out as preferences.
    pub(crate) fn preferences(&self) -> AgentPreferences {
        AgentPreferences {
            policy: self.policy.clone(),
            auto_compact_percent: self.auto_compact_percent,
            context_tokens: Some(self.context_tokens),
            output_tokens: Some(self.output_tokens),
            thinking: self.thinking,
            provider: match self.provider_openai {
                true => "openai".to_string(),
                false => "ollama".to_string(),
            },
            model: self.chosen_model.trim().to_string(),
            url: self.url_field.content().trim().to_string(),
            api_key_env: self.key_env_field.content().trim().to_string(),
        }
    }

    /// The share of the chosen model's context window the last turn filled, when known.
    pub(crate) fn context_ratio(&self) -> Option<f32> {
        let window = self.context_window?;
        (window > 0).then(|| (self.tokens_in as f32 / window as f32).min(1.0))
    }

    /// Takes one event into the transcript, and says what the window should do about it.
    ///
    /// Plain data in, plain instruction out — the whole reload policy is here, where a unit
    /// test can hold it, and the window's only job is to obey the answer.
    pub(crate) fn absorb(
        &mut self,
        event: AgentEvent,
        open: Option<&Path>,
        dirty: bool,
    ) -> Absorbed {
        match event {
            AgentEvent::Permission { .. } | AgentEvent::Edit { .. } => {}
            AgentEvent::Compacting => {
                self.controls.compacting = true;
            }
            AgentEvent::Compacted {
                ok,
                message,
                manual,
                tokens,
            } => {
                self.controls.compacting = false;
                if manual {
                    self.busy = false;
                }
                if ok {
                    self.tokens_in = tokens;
                    self.push_entry(ChatEntry::Status(message));
                } else {
                    self.push_entry(ChatEntry::Error(message));
                }
            }
            AgentEvent::History { turns } => {
                let current = match self.entries.last() {
                    Some(ChatEntry::You(text)) => Some(text.clone()),
                    _ => None,
                };
                self.entries.clear();
                self.open_tools.clear();
                self.expanded.clear();
                for (user, answer) in turns {
                    self.push_entry(ChatEntry::You(user));
                    self.push_entry(ChatEntry::Agent(answer));
                }
                if let Some(current) = current {
                    self.push_entry(ChatEntry::You(current));
                }
            }
            AgentEvent::Notice { message } => {
                self.push_entry(ChatEntry::Error(message));
            }
            AgentEvent::Ready { model } => {
                self.model_label = model;
            }
            AgentEvent::Call { tool } => {
                let index = self.push_entry(ChatEntry::Tool {
                    name: tool.clone(),
                    ok: true,
                    line: String::new(),
                    detail: String::new(),
                });
                self.open_tools.insert(tool, index);
            }
            AgentEvent::Result {
                tool,
                ok,
                line,
                detail,
            } => {
                // The call pushed a running row; this fills it in. A result with no matching
                // call — a build mismatch, a dropped line — becomes its own row rather than
                // being lost.
                let line = if line.is_empty() {
                    "done".to_string()
                } else {
                    line
                };
                let open_row = self
                    .open_tools
                    .remove(&tool)
                    .and_then(|index| self.entries.get_mut(index));
                match open_row {
                    Some(ChatEntry::Tool {
                        ok: row_ok,
                        line: row_line,
                        detail: row_detail,
                        ..
                    }) => {
                        *row_ok = ok;
                        *row_line = line;
                        *row_detail = detail;
                        self.scroll.scroll_to_bottom();
                    }
                    _ => {
                        self.push_entry(ChatEntry::Tool {
                            name: tool,
                            ok,
                            line,
                            detail,
                        });
                    }
                }
            }
            AgentEvent::Changed { project } => {
                if self.busy {
                    if open.is_some_and(|open| same_file(&project, open)) {
                        self.turn_project = Some(project);
                    } else {
                        self.produced_project = Some(project);
                    }
                    return Absorbed::Nothing;
                }
                if let Some(open) = open
                    && same_file(&project, open)
                {
                    if dirty {
                        self.pending_reload = Some(project);
                        self.push_entry(ChatEntry::Note(Key::AgentReloadOffer));
                    } else {
                        return Absorbed::Reload(project);
                    }
                } else {
                    self.produced_project = Some(project);
                }
            }
            AgentEvent::Answer {
                text,
                input_tokens,
                output_tokens,
            } => {
                self.busy = false;
                // The input count is a level, the output a tally: the next turn's prompt
                // carries everything again, so the last report is the gauge's whole truth.
                if input_tokens > 0 {
                    self.tokens_in = input_tokens;
                }
                self.tokens_out += output_tokens;
                self.push_entry(ChatEntry::Agent(text));
                return self.finish_reload(open, dirty);
            }
            AgentEvent::Error { message } => {
                self.busy = false;
                self.push_entry(ChatEntry::Error(message));
                return self.finish_reload(open, dirty);
            }
            AgentEvent::Ended => {
                self.controls = Default::default();
                self.busy = false;
                self.link = None;
                self.push_entry(ChatEntry::Note(Key::AgentEnded));
                return self.finish_reload(open, dirty);
            }
        }
        Absorbed::Nothing
    }

    fn finish_reload(&mut self, open: Option<&Path>, dirty: bool) -> Absorbed {
        match self.turn_project.take() {
            Some(project) => self.absorb(AgentEvent::Changed { project }, open, dirty),
            None => Absorbed::Nothing,
        }
    }
}

/// Start a background worker with explicit settings and history location.
fn spawn_link(
    prefs: &AgentPreferences,
    folder: Option<&Path>,
    fresh_history: bool,
) -> Result<AgentLink, String> {
    auris_agent::Worker::spawn(prefs.clone(), folder.map(Path::to_path_buf), fresh_history).map(
        |worker| AgentLink {
            worker,
            inspection: None,
        },
    )
}

/// Fetch provider models off the UI thread.
fn spawn_model_listing(prefs: &AgentPreferences) -> Receiver<Result<String, String>> {
    auris_agent::list_models_background(prefs.clone())
}

impl AurisApp {
    /// Frames the selection using live command IDs and zero-based note indices.
    fn agent_selection_context(&self) -> serde_json::Value {
        let project = self.project();
        let clips: Vec<_> = project
            .tracks
            .iter()
            .flat_map(|track| {
                track.kind.note_clips().into_iter().flatten().enumerate()
                .filter(|(_, clip)| self.selected_clips.contains(&clip.id))
                .map(|(index, clip)| serde_json::json!({
                    "track": track.name, "clip": index + 1, "id": clip.id.0,
                    "primary": self.selected_clip == Some(clip.id),
                    "start_bar": project.signatures.bar_of(clip.start),
                    "start_tick": clip.start.raw(), "end_tick": (clip.start + clip.length).raw()
                }))
            })
            .collect();
        let notes: Vec<_> = self
            .selected_clip
            .and_then(|id| project.midi_clip(id))
            .map(|(_, clip)| {
                clip.notes
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| self.selected_notes.contains(index))
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default();
        serde_json::json!({
            "selected_track": self.selected_track.and_then(|id| project.track(id)).map(|track| &track.name),
            "selected_track_id": self.selected_track.map(|id| id.0),
            "selected_clip_id": self.selected_clip.map(|id| id.0),
            "selected_clips": clips, "selected_note_indices": notes,
            "playhead_tick": self.session.playhead().raw(),
            "playhead_bar": project.signatures.bar_of(self.session.playhead()),
            "loop_region_ticks": project.loop_region.map(|(start, end)| (start.raw(), end.raw())),
            "addressing": "Use stable numeric track and clip IDs. Note indices are zero-based storage indices as read_notes reports. The selection is context, not authorization to change unselected material."
        })
    }

    /// Starts fresh model history and rebinds the next child to the current document.
    pub(crate) fn agent_reset_conversation(&mut self) {
        self.agent_chat.controls = Default::default();
        self.agent_chat.link = None;
        self.agent_chat.busy = false;
        self.agent_chat.bound_project = None;
        self.agent_chat.fresh_history = false;
        self.agent_chat.restart_after_turn = false;
        self.agent_chat.produced_project = None;
        self.agent_chat.turn_project = None;
        self.agent_chat.pending_reload = None;
        self.agent_chat.open_tools.clear();
        self.agent_chat.model_label.clear();
        self.agent_chat.tokens_in = 0;
        self.agent_chat.tokens_out = 0;
        if !self.agent_chat.entries.is_empty() {
            self.agent_chat
                .push_entry(ChatEntry::Note(Key::AgentConversationReset));
        }
    }

    /// Stops the worker and checks for completed writes, which remain undoable.
    fn agent_stop(&mut self, cx: &mut gpui::Context<Self>) {
        self.agent_chat.controls.pending = None;
        self.agent_chat.controls.permits.clear();
        self.agent_chat.controls.compacting = false;
        self.agent_chat.link = None;
        self.agent_chat.busy = false;
        if self.session.externally_modified()
            && let Some(path) = self.session.path().map(Path::to_path_buf)
        {
            self.agent_chat.turn_project = Some(path);
        }
        let open = self.session.path().map(Path::to_path_buf);
        if let Absorbed::Reload(path) = self
            .agent_chat
            .finish_reload(open.as_deref(), self.session.is_dirty())
        {
            self.accept_agent_changes(path, cx);
        }
        self.agent_chat.open_tools.clear();
        self.agent_chat.push_entry(ChatEntry::Note(Key::AgentEnded));
        cx.notify();
    }

    /// Sends what is in the input field, starting the agent if it is not running.
    ///
    /// The window saves first, so the model reads the document as it stands — the other half
    /// of the bargain is in [`AgentChat::absorb`], where the model's writes come back.
    pub(crate) fn agent_send(&mut self) {
        let text = self.agent_chat.input.content().trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.agent_control_command(&text) {
            return;
        }
        if self.agent_chat.busy {
            return;
        }
        if self.agent_chat.pending_reload.is_some() {
            self.agent_chat
                .push_entry(ChatEntry::Note(Key::AgentResolveFirst));
            return;
        }
        if self.agent_chat.configuring {
            // The settings section is open, and Enter means "go with the form as it stands":
            // a model picked but never applied still counts. The second live run picked one,
            // pressed Enter, and watched this branch's predecessor wipe the pick by loading
            // the saved (empty) preferences back over the form.
            let formed = self.agent_chat.preferences();
            if formed.is_configured() && formed != self.settings.agent {
                self.agent_apply_settings();
            }
        }
        if !self.settings.agent.is_configured() {
            // Said out loud, not merely implied by the settings opening: the first live run
            // pressed Enter here, and a message that silently goes nowhere reads as a broken
            // send rather than as a missing model. The typed text stays put for after, and so
            // does the form — resetting it here is what ate the picked model.
            self.agent_chat.configuring = true;
            if !matches!(
                self.agent_chat.entries.last(),
                Some(ChatEntry::Note(Key::AgentNotConfigured))
            ) {
                self.agent_chat
                    .push_entry(ChatEntry::Note(Key::AgentNotConfigured));
            }
            return;
        }
        if self.agent_chat.link.is_some()
            && self.agent_chat.bound_project.as_deref() != self.session.path()
        {
            self.agent_reset_conversation();
        }
        if self.agent_chat.link.is_none() {
            let folder = self
                .session
                .path()
                .and_then(Path::parent)
                .map(Path::to_path_buf);
            match spawn_link(
                &self.settings.agent,
                folder.as_deref(),
                self.agent_chat.fresh_history,
            ) {
                Ok(link) => {
                    self.agent_chat.link = Some(link);
                    self.agent_chat.bound_project = self.session.path().map(Path::to_path_buf);
                    self.agent_chat.fresh_history = false;
                }
                Err(error) => {
                    self.agent_chat.push_entry(ChatEntry::Error(error));
                    return;
                }
            }
        }

        let framed = format!(
            "[Window context: {}]\n{}",
            self.agent_selection_context(),
            framed_say(&text, self.session.path())
        );
        let wire =
            serde_json::json!({ "say": framed, "display": text, "audio": self.agent_chat.attachments, "policy": self.settings.agent.policy, "auto_compact_percent": self.settings.agent.auto_compact_percent.unwrap_or(85) }).to_string();
        if let Some(link) = self.agent_chat.link.as_mut()
            && let Err(error) = link.send(&wire.to_string())
        {
            self.agent_chat
                .push_entry(ChatEntry::Error(error.to_string()));
            self.agent_chat.link = None;
            return;
        }
        self.agent_chat.push_entry(ChatEntry::You(text));
        self.agent_chat.busy = true;
        self.agent_chat.input = TextField::new(String::new());
        self.agent_chat.attachments.clear();
    }

    /// Apply one request to the bound document without touching its saved file.
    fn agent_edit(&mut self, command: serde_json::Value) -> Result<String, String> {
        if self.agent_chat.bound_project.as_deref() != self.session.path() {
            return Err("The open document changed; start a new conversation".into());
        }
        self.check_agent_edit(&command)?;
        serde_json::from_value::<auris_session::live_agent::Command>(command)
            .map_err(|error| error.to_string())
            .and_then(|command| {
                self.session
                    .agent_command_with_plugin_paths(command, &self.settings.plugin_paths)
            })
    }

    fn start_agent_inspection(&mut self, command: &serde_json::Value) -> Result<(), String> {
        if self.agent_chat.bound_project.as_deref() != self.session.path() {
            return Err("The open document changed; start a new conversation".into());
        }
        self.check_agent_edit(command)?;
        let auris_session::live_agent::Command::InspectAudio {
            start_bar,
            bars,
            track,
        } = serde_json::from_value(command.clone()).map_err(|e| e.to_string())?
        else {
            return Err("Expected inspect_audio".into());
        };
        let job = self.session.audio_inspection_job(start_bar, bars, track)?;
        let link = self.agent_chat.link.as_mut().ok_or("The agent stopped")?;
        if link.inspection.is_some() {
            return Err("An inspection is already running".into());
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("auris-audio-inspection".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    job.run(&worker_cancel)
                }))
                .unwrap_or_else(|_| Err("Audio inspection worker panicked".into()));
                let _ = sender.send(result);
            })
            .map_err(|e| e.to_string())?;
        link.inspection = Some(PendingInspection {
            revision: self.session.revision(),
            cancel,
            receiver,
        });
        Ok(())
    }

    fn poll_agent_inspection(&mut self) {
        let Some(link) = self.agent_chat.link.as_mut() else {
            return;
        };
        let Some(pending) = link.inspection.as_ref() else {
            return;
        };
        let Some(result) = pending.poll(
            self.session.revision(),
            self.agent_chat.bound_project.as_deref() == self.session.path(),
        ) else {
            return;
        };
        link.inspection = None;
        let wire = match result {
            Ok(report) => serde_json::json!({"event":"edit_result","ok":true,"inspection":report}),
            Err(error) => serde_json::json!({"event":"edit_result","ok":false,"text":error}),
        };
        let _ = link.send(&wire.to_string());
    }

    /// Drains the agent's channel, obeying what each event asks for.
    ///
    /// Called from the repaint tick, beside `Session::poll` — the same shape as everything
    /// else another thread writes and this one reads.
    pub(crate) fn drain_agent(&mut self, cx: &mut gpui::Context<Self>) {
        // The model listing first: one answer, then the channel is spent.
        if let Some(receiver) = self.agent_chat.models_rx.as_ref()
            && let Ok(answer) = receiver.try_recv()
        {
            self.agent_chat.models_rx = None;
            self.agent_chat.fetching_models = false;
            match answer.and_then(|line| parse_model_list(&line)) {
                Ok(models) => {
                    // The chosen model's window rides in on its listing — the gauge has no
                    // other way to learn it.
                    if let Some(chosen) = models
                        .iter()
                        .find(|option| option.name == self.agent_chat.chosen_model)
                    {
                        self.agent_chat.context_window = chosen.context_length;
                    }
                    self.agent_chat.models = models;
                    self.agent_chat.models_error = None;
                }
                Err(error) => self.agent_chat.models_error = Some(error),
            }
            cx.notify();
        }
        // A live command must not join or finish the user's in-progress undo transaction.
        if self.drag.is_some() {
            return;
        }
        self.poll_agent_inspection();
        loop {
            let Some(link) = self.agent_chat.link.as_ref() else {
                return;
            };
            let Ok(value) = link.worker.try_recv() else {
                return;
            };
            let Some(event) = parse_event(&value.to_string()) else {
                continue;
            };
            if let AgentEvent::Permission { id, tool, args } = event {
                self.agent_permission(id, tool, args);
                cx.notify();
                continue;
            }
            if let AgentEvent::Edit { command } = event {
                if command["action"] == "inspect_audio" {
                    if let Err(error) = self.start_agent_inspection(&command)
                        && let Some(link) = &self.agent_chat.link
                    {
                        let _ = link.send(
                            &serde_json::json!({"event":"edit_result","ok":false,"text":error})
                                .to_string(),
                        );
                    }
                    continue;
                }
                let revision = self.session.revision();
                let result = self.agent_edit(command);
                let ok = result.is_ok();
                let text = result.unwrap_or_else(|error| error);
                let wire = serde_json::json!({"event": "edit_result", "ok": ok, "text": text});
                if let Some(link) = self.agent_chat.link.as_mut()
                    && let Err(error) = link.send(&wire.to_string())
                {
                    self.agent_chat
                        .push_entry(ChatEntry::Error(error.to_string()));
                    self.agent_chat.link = None;
                    self.agent_chat.busy = false;
                }
                if self.session.revision() != revision {
                    self.resync_selection();
                    self.cancel_auto_sing();
                    self.invalidate_sung_previews();
                    self.reset_drum_analysis();
                }
                cx.notify();
                continue;
            }
            let open = self.session.path().map(Path::to_path_buf);
            let dirty = self.session.is_dirty();
            match self.agent_chat.absorb(event, open.as_deref(), dirty) {
                Absorbed::Nothing => cx.notify(),
                Absorbed::Reload(path) => {
                    self.accept_agent_changes(path, cx);
                }
            }
            if !self.agent_chat.busy && self.agent_chat.restart_after_turn {
                self.agent_chat.link = None;
                self.agent_chat.restart_after_turn = false;
            }
        }
    }

    /// Throws the model list away and asks the provider again, with the form as it stands.
    pub(crate) fn agent_refresh_models(&mut self) {
        // One question at a time: a second press while one is out would park another
        // worker thread behind the same server, and a server that is not
        // answering would collect one per click.
        if self.agent_chat.fetching_models {
            return;
        }
        self.agent_chat.models.clear();
        self.agent_chat.models_error = None;
        self.agent_chat.fetching_models = true;
        self.agent_chat.model_menu = false;
        self.agent_chat.models_rx = Some(spawn_model_listing(&self.agent_chat.preferences()));
    }

    /// Reloads the project the agent rewrote, once the person says so.
    pub(crate) fn agent_reload(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.agent_chat.busy
            && let Some(path) = self.agent_chat.pending_reload.clone()
        {
            self.accept_agent_changes(path, cx);
        }
    }

    pub(crate) fn accept_agent_changes(&mut self, path: PathBuf, cx: &mut gpui::Context<Self>) {
        if !self
            .session
            .path()
            .is_some_and(|open| same_file(open, &path))
        {
            return;
        }
        match self.session.reload_external_changes() {
            Ok(missing) => {
                self.reset_drum_analysis();
                self.agent_chat.pending_reload = None;
                self.external_change = None;
                self.resync_selection();
                self.cancel_auto_sing();
                self.invalidate_sung_previews();
                self.agent_chat
                    .push_entry(ChatEntry::Note(Key::AgentReloaded));
                for path in missing {
                    self.agent_chat.push_entry(ChatEntry::Error(format!(
                        "Missing asset: {}",
                        path.display()
                    )));
                }
            }
            Err(error) => {
                self.agent_chat.pending_reload = Some(path);
                self.agent_chat
                    .push_entry(ChatEntry::Error(crate::i18n::error_text(
                        &error,
                        self.language(),
                    )));
            }
        }
        cx.notify();
    }

    /// Writes a complete form straight into the shared preferences, leaving the section open.
    ///
    /// Picking a model is a whole decision, unlike a half-typed URL: it takes effect the
    /// moment it is made, and the Apply button remains for the text fields. An incomplete
    /// form is left alone — nothing is saved until there is a model to save.
    pub(crate) fn agent_write_through(&mut self) {
        let formed = self.agent_chat.preferences();
        if !formed.is_configured() || formed == self.settings.agent {
            return;
        }
        self.settings.agent = formed;
        if let Err(error) = self.settings.save() {
            log::warn!("the agent settings did not save: {error}");
        }
        // The child read its configuration at spawn; the next message spawns a fresh one.
        if self.agent_chat.busy {
            self.agent_chat.restart_after_turn = true;
        } else {
            self.agent_chat.link = None;
        }
        self.agent_chat.model_label = String::new();
    }

    /// Writes the settings section back to the shared preferences and restarts the wire.
    ///
    /// The child read its configuration at spawn, so a change means a new child; dropping the
    /// link is enough, because the next message spawns one.
    pub(crate) fn agent_apply_settings(&mut self) {
        self.settings.agent = self.agent_chat.preferences();
        if let Err(error) = self.settings.save() {
            log::warn!("the agent settings did not save: {error}");
        }
        if self.agent_chat.busy {
            self.agent_chat.restart_after_turn = true;
        } else {
            self.agent_chat.link = None;
        }
        self.agent_chat.model_label = String::new();
        self.agent_chat.configuring = false;
        self.agent_chat.focused = None;
    }

    /// Answers for a key while one of the agent panel's fields holds the keyboard.
    ///
    /// The characters come through the platform's input handler like every other field's; this
    /// sees what that leaves out. Enter in the chat field sends.
    pub(crate) fn agent_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let Some(focused) = self.agent_chat.focused else {
            return false;
        };
        let key = event.keystroke.key.as_str();
        let composing = self
            .agent_chat
            .field()
            .is_some_and(|field| field.marked().is_some());
        if !composing && self.agent_chat.controls.pending.is_some() {
            if key == "escape" {
                self.agent_approval(controls::Approval::Deny);
                return true;
            }
            if key == "enter" && event.keystroke.modifiers.secondary() {
                self.agent_approval(if event.keystroke.modifiers.shift {
                    controls::Approval::Always
                } else {
                    controls::Approval::Once
                });
                return true;
            }
        }
        if !composing && key == "tab" && event.keystroke.modifiers.shift {
            self.agent_mode(self.settings.agent.policy.mode.next());
            return true;
        }
        if !composing {
            match (key, focused) {
                ("escape", _) => {
                    self.agent_chat.focused = None;
                    return true;
                }
                ("enter", AgentField::Chat) => {
                    self.agent_submit(window, cx);
                    return true;
                }
                _ => {}
            }
        }
        let shift = event.keystroke.modifiers.shift;
        let secondary = event.keystroke.modifiers.secondary();
        self.agent_chat.field_mut().is_some_and(|field| {
            field.apply_key_with_clipboard(key, shift, secondary, false, cx)
                != crate::ui::text_field::KeyEffect::Ignored
        })
    }

    /// The send button and Enter share validation before editing the live session.
    fn agent_submit(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) {
        if self.agent_chat.input.marked().is_some() {
            return;
        }
        self.agent_send();
    }

    /// Puts the keyboard into one of the panel's fields.
    pub(crate) fn focus_agent_field(&mut self, field: AgentField) {
        // One field in the window types at a time, and the library's box is the other panel
        // field this could be left fighting with.
        self.library_search_focused = false;
        self.agent_chat.focused = Some(field);
    }

    /// Renders the agent panel.
    pub(crate) fn render_agent_chat(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        // The persistent model picker uses the saved provider even before settings opens.
        self.agent_chat
            .load_preferences_once(&self.settings.agent.clone());
        // When the panel first opens, ask the provider what it serves —
        // once, and only until an answer or a refusal lands; the refresh button asks again.
        if self.agent_chat.models.is_empty()
            && self.agent_chat.models_error.is_none()
            && !self.agent_chat.fetching_models
            && self.agent_chat.models_rx.is_none()
        {
            self.agent_refresh_models();
        }

        let entries: Vec<AnyElement> = self
            .agent_chat
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| self.chat_row(index, entry, &theme, window, cx))
            .collect();
        let rows = entries;
        let busy = self.agent_chat.busy;
        let pending_reload = self.agent_chat.pending_reload.is_some();
        let model_label = match self.agent_chat.model_label.is_empty() {
            true => self.settings.agent.model.clone(),
            false => self.agent_chat.model_label.clone(),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.0))
            .min_w_0()
            .bg(theme.surface_sunken)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .min_h(Metrics::PANEL_HEADER_HEIGHT)
                    .flex_wrap()
                    .px_2()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(div().child(self.t(Key::AgentPanel)))
                    .when(self.agent_chat.produced_project.is_some(), |this| {
                        this.child(button(
                            "agent-open-result",
                            self.t(Key::AgentOpenResult),
                            ButtonStyle::Normal,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                if let Some(path) = this.agent_chat.produced_project.clone()
                                    && this.confirm_discard(
                                        crate::ui::prompt::PendingAction::OpenDropped(path.clone()),
                                    )
                                {
                                    this.open_project_at(path, cx);
                                }
                            }),
                        ))
                    })
                    .child(button(
                        "agent-new-conversation",
                        self.t(Key::AgentNewConversation),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.agent_stop(cx);
                            // A conflict must remain available after clearing model history.
                            let pending = this.agent_chat.pending_reload.clone();
                            this.agent_reset_conversation();
                            this.agent_chat.pending_reload = pending;
                            this.agent_chat.fresh_history = true;
                            this.agent_chat.entries.clear();
                            if let Some(folder) = this.session.project_folder() {
                                let history = folder.join(".auris-conversation.json");
                                if let Err(error) = std::fs::remove_file(&history)
                                    && error.kind() != std::io::ErrorKind::NotFound
                                {
                                    this.agent_chat
                                        .push_entry(ChatEntry::Error(error.to_string()));
                                }
                            }
                            cx.notify();
                        }),
                    ))
                    .when(busy, |this| {
                        this.child(button(
                            "agent-stop",
                            self.t(Key::AgentStop),
                            ButtonStyle::Normal,
                            false,
                            theme.warning,
                            &theme,
                            cx.listener(|this, _, _, cx| this.agent_stop(cx)),
                        ))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_color(theme.text_faint)
                            .child(model_label),
                    )
                    .when(pending_reload, |this| {
                        this.child(button(
                            "agent-reload",
                            self.t(Key::AgentReload),
                            ButtonStyle::Normal,
                            true,
                            theme.warning,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.agent_reload(cx);
                                cx.notify();
                            }),
                        ))
                    })
                    .child(button(
                        "agent-configure",
                        self.t(Key::AgentConfigure),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.open_settings(cx)),
                    )),
            )
            .child(self.agent_controls(cx))
            .child(self.agent_model_picker(cx))
            .child(
                self.scrolling(
                    ScrollPanel::Agent,
                    div()
                        .id("agent-lines")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .p_1()
                        .gap_1()
                        .overflow_y_scroll()
                        .when(self.agent_chat.controls.rules_open, |this| {
                            this.child(self.agent_rules(cx))
                        })
                        .children(rows)
                        .when(busy, |this| {
                            this.child(div().px_1p5().text_xs().text_color(theme.text_faint).child(
                                self.t(if self.agent_chat.controls.compacting {
                                    Key::AgentCompacting
                                } else if self.agent_chat.controls.pending.is_some() {
                                    Key::AgentAwaitingApproval
                                } else {
                                    Key::AgentWorking
                                }),
                            ))
                        })
                        .when(self.agent_chat.entries.is_empty() && !busy, |this| {
                            this.child(
                                div()
                                    .p_2()
                                    .text_xs()
                                    .text_color(theme.text_faint)
                                    .child(self.t(Key::AgentPlaceholder)),
                            )
                        }),
                    cx,
                ),
            )
            .child(self.agent_approval_view(cx))
            .child(self.agent_gauge_row(&theme))
            .child(self.agent_input_row(cx))
    }

    /// The context gauge and token counters, over the input the way picocode sets its status
    /// bar: `↑ prompt ↓ written`, a bar filling the chosen model's window, and the percentage.
    ///
    /// Nothing is drawn before the first turn — a gauge reading zero over an empty transcript
    /// is furniture — and the bar itself only appears when the model's listing said how big
    /// the window is, because a bar with an invented ceiling would be a number wearing a lie.
    fn agent_gauge_row(&self, theme: &Theme) -> AnyElement {
        if self.agent_chat.tokens_in == 0 && self.agent_chat.tokens_out == 0 {
            return div().into_any_element();
        }
        let ratio = self.agent_chat.context_ratio();
        let mut row = div()
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .px_2()
            .py_0p5()
            .border_t_1()
            .border_color(theme.border_subtle)
            .text_xs()
            .text_color(theme.text_faint)
            .child(format!(
                "↑ {} ↓ {}",
                self.agent_chat.tokens_in, self.agent_chat.tokens_out
            ));
        if let Some(ratio) = ratio {
            const GAUGE_WIDTH: f32 = 96.0;
            row = row
                .child(
                    div()
                        .w(px(GAUGE_WIDTH))
                        .h(px(5.0))
                        .rounded_full()
                        .bg(theme.surface_raised)
                        .child(
                            div()
                                .w(px(GAUGE_WIDTH * ratio))
                                .h_full()
                                .rounded_full()
                                .bg(gauge_colour(ratio, theme)),
                        ),
                )
                .child(format!("{:>3.0}%", ratio * 100.0));
        }
        row.into_any_element()
    }

    /// One transcript row. A tool row with an answer opens to the whole of it on a click —
    /// the loop's log, kept where the loop is shown.
    fn chat_row(
        &self,
        index: usize,
        entry: &ChatEntry,
        theme: &Theme,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let (colour, text): (gpui::Hsla, String) = match entry {
            ChatEntry::You(text) => (theme.accent_text, text.clone()),
            ChatEntry::Agent(text) => (theme.text, text.clone()),
            ChatEntry::Status(text) => (theme.text_muted, text.clone()),
            ChatEntry::Tool { name, ok, line, .. } => {
                let mark = match (*ok, line.is_empty()) {
                    (_, true) => "…",
                    (true, false) => "✓",
                    (false, false) => "✗",
                };
                (theme.text_muted, format!("{mark} {name}  {line}"))
            }
            ChatEntry::Error(message) => (theme.danger, message.clone()),
            ChatEntry::Note(key) => (theme.warning, self.t(*key).to_string()),
        };
        let bordered = matches!(entry, ChatEntry::You(_));
        let opened = self.agent_chat.expanded.contains(&index);
        let openable = matches!(entry, ChatEntry::Tool { detail, .. } if !detail.is_empty());
        let detail = match entry {
            ChatEntry::Tool { detail, .. } if opened => Some(detail.clone()),
            _ => None,
        };
        let body: AnyElement = match entry {
            ChatEntry::Agent(_) => super::agent_markdown::render(
                SharedString::from(format!("agent-markdown-{index}")),
                SharedString::from(text),
                theme,
                window,
                cx,
            ),
            _ => div().child(text).into_any_element(),
        };
        div()
            .id(("agent-line", index))
            .debug_selector(move || format!("agent-line-{index}"))
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_1p5()
            .py_0p5()
            .text_xs()
            .text_color(colour)
            .when(bordered, |this| {
                this.border_l_2().border_color(theme.accent)
            })
            .when(openable, |this| {
                this.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if !this.agent_chat.expanded.remove(&index) {
                            this.agent_chat.expanded.insert(index);
                        }
                        cx.notify();
                    }),
                )
            })
            .child(body)
            .when_some(
                match entry {
                    ChatEntry::Agent(text) => Some(text.clone()),
                    _ => None,
                },
                |this, text| {
                    this.child(
                        div().flex().justify_end().mt_1().child(
                            button(
                                ("agent-copy", index),
                                self.t(Key::AgentCopyMarkdown),
                                ButtonStyle::Ghost,
                                false,
                                theme.accent,
                                theme,
                                cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        text.clone(),
                                    ));
                                }),
                            )
                            .cursor_default(),
                        ),
                    )
                },
            )
            .when_some(detail, |this, detail| {
                // Line by line rather than one string: a div's text collapses the newlines a
                // tool's tables are drawn with.
                this.child(
                    div()
                        .mt_0p5()
                        .p_1()
                        .rounded(Metrics::RADIUS_SM)
                        .bg(theme.surface_raised)
                        .text_color(theme.text_muted)
                        .flex()
                        .flex_col()
                        .children(
                            detail
                                .lines()
                                .map(|line| div().child(line.to_string()))
                                .collect::<Vec<_>>(),
                        ),
                )
            })
            .into_any_element()
    }

    /// The settings section: provider, model, URL, key variable, apply.
    fn agent_model_picker(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let labelled = |label: String, control: AnyElement, theme: &Theme| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(96.0))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(control))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_b_1()
            .border_color(theme.border)
            .child(labelled(
                self.t(Key::AgentModelLabel).to_string(),
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(self.dropdown(
                        "agent-model",
                        match self.agent_chat.chosen_model.is_empty() {
                            true => self.t(Key::AgentChooseModel).to_string(),
                            false => self.agent_chat.chosen_model.clone(),
                        },
                        self.agent_chat.model_menu,
                        &theme,
                        |this, _| {
                            this.agent_chat.model_menu = !this.agent_chat.model_menu;
                        },
                        cx,
                    )))
                    .child(button(
                        "agent-models-refresh",
                        self.t(Key::AgentModelsFetch),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.agent_refresh_models();
                            cx.notify();
                        }),
                    ))
                    .into_any_element(),
                &theme,
            ))
            .when(self.agent_chat.fetching_models, |this| {
                this.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(theme.text_faint)
                        .child(self.t(Key::AgentModelsFetching)),
                )
            })
            .when_some(self.agent_chat.models_error.clone(), |this, error| {
                this.child(div().px_1().text_xs().text_color(theme.danger).child(error))
            })
            .when(self.agent_chat.model_menu, |this| {
                let names: Vec<String> = self
                    .agent_chat
                    .models
                    .iter()
                    .map(|option| match option.context_length {
                        Some(window) => format!("{}  ({}k)", option.name, window / 1024),
                        None => option.name.clone(),
                    })
                    .collect();
                this.child(self.option_rows(
                    "agent-model-option",
                    &names,
                    &theme,
                    |this, chosen, _| {
                        if let Some(option) = this.agent_chat.models.get(chosen) {
                            this.agent_chat.chosen_model = option.name.clone();
                            this.agent_chat.context_window = option.context_length;
                            this.agent_chat.tokens_in = 0;
                            this.agent_chat.tokens_out = 0;
                        }
                        this.agent_chat.model_menu = false;
                        // The pick counts the moment it is made — no Apply between the
                        // menu and the setting.
                        this.agent_write_through();
                    },
                    cx,
                ))
            })
            .into_any_element()
    }

    /// A closed dropdown: the current choice and an arrow, opening on a click.
    ///
    /// Not a popup window — the options render as rows underneath, pushing the section down,
    /// which is all a two-item provider list and a one-server model list need.
    fn dropdown(
        &self,
        id: &'static str,
        current: String,
        open: bool,
        theme: &Theme,
        toggle: impl Fn(&mut Self, &mut gpui::Context<Self>) + 'static,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .h(Metrics::CONTROL_HEIGHT)
            .px_1p5()
            .rounded(Metrics::RADIUS_SM)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(match open {
                true => theme.accent,
                false => theme.border_subtle,
            })
            .cursor_pointer()
            .text_xs()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_color(theme.text)
                    .child(current),
            )
            .child(
                div()
                    .text_color(theme.text_muted)
                    .child(if open { "▴" } else { "▾" }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    toggle(this, cx);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The rows an open dropdown shows, each picking by its position in the list.
    fn option_rows(
        &self,
        id: &'static str,
        names: &[String],
        theme: &Theme,
        pick: impl Fn(&mut Self, usize, &mut gpui::Context<Self>) + Clone + 'static,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let mut list = div()
            .id((id, usize::MAX))
            .flex()
            .flex_col()
            .max_h(px(160.0))
            .overflow_y_scroll()
            .ml(px(96.0 + 8.0))
            .rounded(Metrics::RADIUS_SM)
            .border_1()
            .border_color(theme.border_subtle)
            .bg(theme.surface_raised);
        for (index, name) in names.iter().enumerate() {
            let pick = pick.clone();
            list = list.child(
                div()
                    .id((id, index))
                    .debug_selector(move || format!("{id}-{index}"))
                    .px_1p5()
                    .py_0p5()
                    .text_xs()
                    .text_color(theme.text)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            pick(this, index, cx);
                            cx.notify();
                        }),
                    )
                    .child(name.clone()),
            );
        }
        list.into_any_element()
    }

    /// The message field and its border, at the bottom of the panel.
    fn agent_input_row(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let attachments = self.agent_chat.attachments.clone();
        let focused = self.agent_chat.focused == Some(AgentField::Chat);
        let empty = self.agent_chat.input.content().is_empty();
        let placeholder = self.t(Key::AgentPlaceholder).to_string();
        let can_send = !self.agent_chat.busy
            && !self.agent_chat.input.content().trim().is_empty()
            && self.agent_chat.input.marked().is_none();
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .p_1()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(button(
                        "agent-attach-audio",
                        self.t(Key::AgentAttachAudio),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            let language = this.language();
                            cx.spawn(async move |this, cx| {
                                let files = rfd::AsyncFileDialog::new()
                                    .set_title(Key::AgentAttachAudio.get(language))
                                    .add_filter(
                                        "Audio",
                                        &["wav", "mp3", "flac", "ogg", "aac", "aiff", "m4a"],
                                    )
                                    .pick_files()
                                    .await;
                                if let Some(files) = files {
                                    let _ = this.update(cx, |this, cx| {
                                        for file in files {
                                            let path = file.path().to_path_buf();
                                            if !this.agent_chat.attachments.contains(&path) {
                                                this.agent_chat.attachments.push(path);
                                            }
                                        }
                                        cx.notify();
                                    });
                                }
                            })
                            .detach();
                        }),
                    ))
                    .children(attachments.iter().enumerate().map(|(index, path)| {
                        button(
                            ("agent-attachment", index),
                            format!(
                                "{} ×",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ),
                            ButtonStyle::Normal,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                if index < this.agent_chat.attachments.len() {
                                    this.agent_chat.attachments.remove(index);
                                }
                                cx.notify();
                            }),
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(self.panel_field(
                        "agent-input",
                        AgentField::Chat,
                        focused,
                        empty,
                        placeholder,
                        &theme,
                        cx,
                    )))
                    .child(
                        button(
                            "agent-send",
                            self.t(Key::AgentSend),
                            if can_send {
                                ButtonStyle::Primary
                            } else {
                                ButtonStyle::Normal
                            },
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, window, cx| {
                                if can_send {
                                    this.agent_submit(window, cx);
                                    cx.notify();
                                }
                            }),
                        )
                        .flex_shrink_0()
                        .cursor_default()
                        .when(!can_send, |this| {
                            this.text_color(theme.text_faint).opacity(0.5)
                        }),
                    ),
            )
            .into_any_element()
    }

    /// One of the panel's one-line fields, drawn the way the library's search box is.
    #[allow(clippy::too_many_arguments)]
    fn panel_field(
        &mut self,
        id: &'static str,
        field: AgentField,
        focused: bool,
        show_placeholder: bool,
        placeholder: String,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let value = match field {
            AgentField::Chat => &self.agent_chat.input,
        };
        let text = value.content().to_string();
        let selection = value.selection();
        let marked = value.marked();
        let view = cx.entity();
        let handle = self.focus.clone();

        div()
            .id(id)
            // The id again, as a name a test can find the field by — the same line every
            // button gets in `widgets`, compiled to nothing outside `cargo test`.
            .debug_selector(move || id.to_string())
            .flex()
            .items_center()
            .h(Metrics::CONTROL_HEIGHT)
            .px_1p5()
            .rounded(Metrics::RADIUS_SM)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(match focused {
                true => theme.accent,
                false => theme.border_subtle,
            })
            .cursor_text()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(match focused {
                        true => crate::ui::prompt::editable_text(
                            text.clone().into(),
                            selection,
                            marked,
                            handle,
                            view,
                            theme.clone(),
                        )
                        .into_any_element(),
                        false => crate::ui::prompt::field_text(text.clone(), theme.text)
                            .into_any_element(),
                    })
                    .when(show_placeholder && text.is_empty(), |this| {
                        this.child(
                            crate::ui::prompt::field_text(placeholder, theme.text_faint)
                                .absolute()
                                .inset_0(),
                        )
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.focus_agent_field(field);
                    cx.notify();
                }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::prelude::{Note, Ticks};

    #[gpui::test]
    fn copy_answer_keeps_all_markdown_after_resizing(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let answer = "## 再生結果\n\n- **WAV:** `C:/Music/preview.wav`\n\n  次の段落。";
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "test-model".into();
            this.agent_chat.entries = vec![ChatEntry::Agent(answer.into())];
        });
        crate::harness::resize(&app, cx, gpui::size(px(900.), px(600.)));
        crate::harness::click("agent-copy-0", cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some(answer)
            );
        });
    }

    #[gpui::test]
    fn send_button_preserves_drafts_and_opens_configuration(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent = Default::default();
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.input = TextField::new(" ");
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-send", cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.entries.is_empty()));

        app.update(cx, |this, _| {
            this.agent_chat.input = TextField::new("再生してみて。");
            this.agent_chat.busy = true;
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-send", cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.entries.is_empty()));

        app.update(cx, |this, _| this.agent_chat.busy = false);
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-send", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.input.content(), "再生してみて。");
            assert!(this.agent_chat.configuring);
            assert_eq!(
                this.agent_chat.entries,
                vec![ChatEntry::Note(Key::AgentNotConfigured)]
            );
        });
    }

    #[gpui::test]
    fn markdown_transcript_keeps_long_answers_scrollable(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let answer = "## 再生結果\n\n- **WAV:** `C:/Music/preview.wav`\n- 長さ 00:08、2ch\n\n説明の段落です。\n\n";
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "test-model".into();
            this.agent_chat.configuring = false;
            this.agent_chat.entries = vec![ChatEntry::Agent(answer.into())];
        });
        crate::harness::paint(&app, cx);
        let short = cx.debug_bounds("agent-line-0").unwrap().size.height;
        app.update(cx, |this, _| {
            // A fresh identity avoids the Markdown renderer's asynchronous update debounce.
            this.agent_chat.entries = vec![
                ChatEntry::You("再生してみて。".into()),
                ChatEntry::Agent(answer.repeat(40)),
                ChatEntry::Tool {
                    name: "preview".into(),
                    ok: true,
                    line: "finished".into(),
                    detail: "Audio preview details".into(),
                },
            ];
        });
        crate::harness::paint(&app, cx);
        let long = cx.debug_bounds("agent-line-1").unwrap().size.height;
        // Structural overflow, not font metrics or a pixel snapshot: more content must
        // occupy more space, and scrolling must make the following tool reachable.
        assert!(
            long > short * 10.,
            "the answer was compressed: {short:?} -> {long:?}"
        );
        app.update(cx, |this, _| {
            let view = this.scroll_view(ScrollPanel::Agent);
            assert!(view.max_offset > view.viewport);
            this.set_scroll_offset(ScrollPanel::Agent, -view.max_offset);
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-line-2", cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.expanded.contains(&2)));
    }

    #[gpui::test]
    fn markdown_list_body_has_visible_bounds(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "test-model".into();
            this.agent_chat.configuring = false;
            this.agent_chat.entries = vec![ChatEntry::Agent(
                "設定\n\n- **テンポ:** 150 BPM\n- **拍子:** 4/4\n\n改善点\n\n1. 弦楽器を強化\n2. ドラムを追加".into(),
            )];
        });
        crate::harness::paint(&app, cx);
        for selector in [
            "agent-markdown-0-1-0-0-block",
            "agent-markdown-0-1-1-0-block",
            "agent-markdown-0-3-0-0-block",
            "agent-markdown-0-3-1-0-block",
        ] {
            let body = cx.debug_bounds(selector).expect("list body is rendered");
            let message = cx.debug_bounds("agent-line-0").unwrap();
            assert!(
                body.size.width > message.size.width / 2. && body.size.height > gpui::px(0.),
                "list body collapsed: {selector}: {body:?}; message: {message:?}"
            );
        }
    }

    #[gpui::test]
    fn markdown_list_long_body_wraps(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "test-model".into();
            this.agent_chat.configuring = false;
            this.agent_chat.entries = vec![
                ChatEntry::Agent("- **設定:** 短い本文".into()),
                ChatEntry::Agent(format!(
                    "- **設定:** {}",
                    "弦楽器とドラムを強化します。".repeat(30)
                )),
            ];
        });
        crate::harness::paint(&app, cx);
        let short = cx.debug_bounds("agent-line-0").unwrap().size.height;
        let long = cx.debug_bounds("agent-line-1").unwrap().size.height;
        assert!(
            long > short * 3.,
            "list text does not wrap: {short:?} -> {long:?}"
        );
    }

    #[gpui::test]
    fn markdown_list_continuation_paragraphs_stack_vertically(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "test-model".into();
            this.agent_chat.configuring = false;
            this.agent_chat.entries = vec![
                ChatEntry::Agent("- first".into()),
                ChatEntry::Agent(format!("- first{}", "\n\n  next".repeat(30))),
            ];
        });
        crate::harness::paint(&app, cx);
        let short = cx.debug_bounds("agent-line-0").unwrap().size.height;
        let long = cx.debug_bounds("agent-line-1").unwrap().size.height;
        assert!(
            long > short * 10.,
            "list paragraphs share a horizontal row: {short:?} -> {long:?}"
        );
    }

    #[gpui::test]
    fn live_agent_edits_an_unsaved_document_and_undo_restores_it(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            // This editing test must not inherit modes saved by settings-window tests.
            this.settings.agent.policy = Default::default();
            let before = this.project().clone();
            assert!(this.session.path().is_none());
            let event = parse_event(r#"{"event":"edit","command":{"action":"add_track","name":"Agent lead","kind":"instrument"}}"#).unwrap();
            let AgentEvent::Edit { command } = event else { panic!() };
            this.agent_edit(command).unwrap();
            assert!(this.project().tracks.iter().any(|track| track.name == "Agent lead"));
            assert!(this.session.is_dirty());
            assert!(this.session.path().is_none());
            this.session.undo();
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn live_agent_refuses_a_request_bound_to_a_different_document(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            let before = this.project().clone();
            this.agent_chat.bound_project = Some(PathBuf::from("previous.auris"));
            assert!(
                this.agent_edit(
                    serde_json::json!({"action":"add_track", "name":"Wrong", "kind":"instrument"})
                )
                .is_err()
            );
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn pending_changes_block_send_before_saving_or_starting_a_model(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.agent_chat.pending_reload = Some(PathBuf::from("pending.auris"));
            this.agent_chat.input = TextField::new("change the bass");
            let before = this.project().clone();
            this.agent_send();
            assert_eq!(this.project(), &before);
            assert_eq!(this.agent_chat.input.content(), "change the bass");
            assert!(this.agent_chat.link.is_none());
            assert!(!this.agent_chat.busy);
            assert_eq!(
                this.agent_chat.entries.last(),
                Some(&ChatEntry::Note(Key::AgentResolveFirst))
            );
        });
    }

    #[test]
    fn inspection_discards_changed_documents_and_cancels_when_dropped() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pending = PendingInspection {
            revision: 7,
            receiver,
            cancel: cancel.clone(),
        };
        assert!(pending.poll(7, true).is_none());
        assert!(pending.poll(8, true).unwrap().is_err());
        assert!(pending.poll(7, false).unwrap().is_err());
        drop(pending);
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn a_turn_collects_tool_writes_and_offers_conflicts_only_when_finished() {
        let path = PathBuf::from("song.auris");
        let mut chat = AgentChat {
            busy: true,
            ..Default::default()
        };
        for _ in 0..3 {
            assert_eq!(
                chat.absorb(
                    AgentEvent::Changed {
                        project: path.clone()
                    },
                    Some(&path),
                    false
                ),
                Absorbed::Nothing
            );
        }
        let other = PathBuf::from("another-song.auris");
        chat.absorb(
            AgentEvent::Changed {
                project: other.clone(),
            },
            Some(&path),
            false,
        );
        assert_eq!(chat.produced_project, Some(other));
        assert_eq!(
            chat.absorb(
                AgentEvent::Error {
                    message: "provider stopped".into()
                },
                Some(&path),
                true
            ),
            Absorbed::Nothing
        );
        assert!(!chat.busy);
        assert_eq!(chat.pending_reload, Some(path));
        assert!(chat.turn_project.is_none());
    }

    #[gpui::test]
    fn selection_context_uses_live_command_note_indices(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            let track = this
                .session
                .add_default_instrument_track("Selected lead")
                .unwrap();
            let clip = this
                .session
                .add_midi_clip(track, "Idea", Ticks::ZERO, Ticks::QUARTER * 4)
                .unwrap();
            this.session
                .add_note(clip, Note::new(72, Ticks::QUARTER, Ticks::QUARTER))
                .unwrap();
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
            this.selected_track = Some(track);
            this.selected_clip = Some(clip);
            this.selected_clips.insert(clip);
            this.selected_notes.insert(0);
            let context = this.agent_selection_context();
            assert_eq!(context["selected_track"], "Selected lead");
            assert_eq!(context["selected_note_indices"], serde_json::json!([0]));
            assert_eq!(context["selected_clips"][0]["clip"], 1);
        });
    }

    #[test]
    fn resumed_history_keeps_the_message_just_submitted() {
        let mut chat = AgentChat::default();
        chat.push_entry(ChatEntry::You("continue".into()));
        chat.absorb(
            AgentEvent::History {
                turns: vec![("old request".into(), "old answer".into())],
            },
            None,
            false,
        );
        assert_eq!(
            chat.entries,
            vec![
                ChatEntry::You("old request".into()),
                ChatEntry::Agent("old answer".into()),
                ChatEntry::You("continue".into())
            ]
        );
    }

    #[gpui::test]
    fn agent_chat_accepts_clipboard_shortcuts(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels = Default::default();
            this.settings.agent.model = "offline-fixture".to_string();
            this.agent_chat.configuring = true;
            this.agent_chat.models_error = Some("offline fixture".to_string());
            this.panels.show(crate::dock::Panel::Agent);
        });
        crate::harness::paint(&app, cx);
        {
            let (selector, field, text) =
                ("agent-input", AgentField::Chat, "make the bass quieter");
            crate::harness::click(selector, cx);
            crate::harness::paint(&app, cx);
            cx.update(|_, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.into()));
            });
            cx.simulate_keystrokes("secondary-a secondary-v");
            app.read_with(cx, |this, _| {
                assert_eq!(this.agent_chat.focused, Some(field));
                assert_eq!(this.agent_chat.field().unwrap().content(), text);
            });
            cx.simulate_keystrokes("secondary-a secondary-c secondary-x");
            app.read_with(cx, |this, _| {
                assert_eq!(this.agent_chat.field().unwrap().content(), "");
            });
            cx.update(|_, cx| {
                assert_eq!(
                    cx.read_from_clipboard()
                        .and_then(|item| item.text())
                        .as_deref(),
                    Some(text)
                );
            });
            cx.simulate_keystrokes("secondary-v");
            app.read_with(cx, |this, _| {
                assert_eq!(this.agent_chat.field().unwrap().content(), text);
            });
        }
    }

    #[test]
    fn repaint_seeding_does_not_replace_an_unconfigured_form_in_progress() {
        let saved = AgentPreferences {
            provider: "ollama".to_string(),
            model: String::new(),
            url: "http://saved.invalid".to_string(),
            api_key_env: String::new(),
            ..Default::default()
        };
        let mut chat = AgentChat::default();
        chat.load_preferences_once(&saved);

        chat.provider_openai = true;
        chat.url_field = TextField::new("https://being-typed.invalid/v1".to_string());
        chat.key_env_field = TextField::new("MY_AGENT_KEY".to_string());
        chat.load_preferences_once(&saved);

        assert!(chat.provider_openai);
        assert_eq!(chat.url_field.content(), "https://being-typed.invalid/v1");
        assert_eq!(chat.key_env_field.content(), "MY_AGENT_KEY");
    }

    #[test]
    fn runtime_preferences_roundtrip_through_the_panel() {
        let mut chat = AgentChat::default();
        let preferences = AgentPreferences {
            context_tokens: Some(65536),
            output_tokens: Some(16384),
            thinking: Some(false),
            model: "local-tools".into(),
            ..Default::default()
        };
        chat.load_preferences(&preferences);
        assert_eq!(chat.context_tokens, 65536);
        assert_eq!(chat.output_tokens, 16384);
        assert_eq!(chat.thinking, Some(false));
        let saved = chat.preferences();
        assert_eq!(saved.context_tokens, preferences.context_tokens);
        assert_eq!(saved.output_tokens, preferences.output_tokens);
        assert_eq!(saved.thinking, preferences.thinking);
        chat.load_preferences(&AgentPreferences::default());
        assert_eq!(chat.context_tokens, 32768);
        assert_eq!(chat.thinking, None);
    }

    #[test]
    fn the_wire_is_read_tolerantly() {
        assert_eq!(
            parse_event(r#"{"event":"ready","provider":"ollama","model":"m"}"#),
            Some(AgentEvent::Ready {
                model: "m".to_string()
            })
        );
        assert_eq!(
            parse_event(
                r#"{"event":"result","tool":"analyze","ok":true,"text":"The mix — x\nmore"}"#
            ),
            Some(AgentEvent::Result {
                tool: "analyze".to_string(),
                ok: true,
                line: "The mix — x".to_string(),
                detail: "The mix — x\nmore".to_string()
            })
        );
        // Token counts ride on the answer; an older agent's answer without them still reads.
        assert_eq!(
            parse_event(r#"{"event":"answer","text":"done","input_tokens":12,"output_tokens":3}"#),
            Some(AgentEvent::Answer {
                text: "done".to_string(),
                input_tokens: 12,
                output_tokens: 3
            })
        );
        // A line this build does not know, and a line that is not JSON: skipped, not fatal.
        assert_eq!(parse_event(r#"{"event":"novel"}"#), None);
        assert_eq!(parse_event("garbage"), None);
    }

    #[test]
    fn a_call_row_is_filled_in_by_its_result() {
        let mut chat = AgentChat::default();
        chat.absorb(
            AgentEvent::Call {
                tool: "compose".to_string(),
            },
            None,
            false,
        );
        assert!(matches!(
            chat.entries.last(),
            Some(ChatEntry::Tool { line, .. }) if line.is_empty()
        ));
        chat.absorb(
            AgentEvent::Result {
                tool: "compose".to_string(),
                ok: true,
                line: "Wrote X".to_string(),
                detail: "Wrote X\nand the summary".to_string(),
            },
            None,
            false,
        );
        assert_eq!(chat.entries.len(), 1, "the result fills the call's row");
        assert!(matches!(
            chat.entries.last(),
            Some(ChatEntry::Tool { ok: true, line, detail, .. })
                if line == "Wrote X" && detail.contains("summary")
        ));
    }

    #[test]
    fn unmatched_empty_results_are_finished_rows() {
        let mut chat = AgentChat::default();
        chat.absorb(
            AgentEvent::Result {
                tool: "compose".to_string(),
                ok: false,
                line: String::new(),
                detail: String::new(),
            },
            None,
            false,
        );

        assert!(matches!(
            chat.entries.last(),
            Some(ChatEntry::Tool { ok: false, line, .. }) if line == "done"
        ));
    }

    #[test]
    fn the_transcript_is_bounded_and_tool_indexes_survive_eviction() {
        let mut chat = AgentChat::default();
        for index in 0..CHAT_CAPACITY {
            chat.push_entry(ChatEntry::Agent(index.to_string()));
        }
        chat.absorb(
            AgentEvent::Call {
                tool: "compose".to_string(),
            },
            None,
            false,
        );
        chat.absorb(
            AgentEvent::Result {
                tool: "compose".to_string(),
                ok: true,
                line: "written".to_string(),
                detail: "written".to_string(),
            },
            None,
            false,
        );

        assert_eq!(chat.entries.len(), CHAT_CAPACITY);
        assert!(matches!(
            chat.entries.last(),
            Some(ChatEntry::Tool { line, .. }) if line == "written"
        ));
    }

    #[test]
    fn a_change_to_the_open_project_reloads_only_while_nothing_is_unsaved() {
        let root = std::env::temp_dir().join(format!("auris-chat-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Song.auris");
        std::fs::write(&file, "{}").unwrap();

        let changed = || AgentEvent::Changed {
            project: file.clone(),
        };
        let mut chat = AgentChat::default();

        // Clean window, same file: reload without asking.
        assert_eq!(
            chat.absorb(changed(), Some(&file), false),
            Absorbed::Reload(file.clone())
        );
        // Dirty window: the offer, not the deed — unsaved work is never thrown out quietly.
        assert_eq!(chat.absorb(changed(), Some(&file), true), Absorbed::Nothing);
        assert_eq!(chat.pending_reload, Some(file.clone()));
        // A different open project: none of this window's business.
        let other = root.join("Other.auris");
        assert_eq!(
            chat.absorb(changed(), Some(&other), false),
            Absorbed::Nothing
        );
        // No project open at all: likewise.
        assert_eq!(chat.absorb(changed(), None, false), Absorbed::Nothing);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_model_listing_is_read_and_a_refusal_is_carried_whole() {
        let listed = parse_model_list(
            r#"{"models":[{"name":"gpt-oss:20b","context_length":131072},{"name":"gemma4:e2b"}]}"#,
        )
        .unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "gpt-oss:20b");
        assert_eq!(listed[0].context_length, Some(131072));
        assert_eq!(listed[1].context_length, None);
        let refused = parse_model_list(r#"{"error":"nobody answered at :11434"}"#).unwrap_err();
        assert!(refused.contains("11434"), "{refused}");
        assert!(parse_model_list("garbage").is_err());
    }

    #[test]
    fn the_gauge_reads_pressure_the_way_picocode_does() {
        let mut chat = AgentChat {
            tokens_in: 32_768,
            ..Default::default()
        };
        assert_eq!(chat.context_ratio(), None, "no window, no bar");
        chat.context_window = Some(131_072);
        assert_eq!(chat.context_ratio(), Some(0.25));

        let theme = Theme::default();
        assert_eq!(gauge_colour(0.25, &theme), theme.accent);
        assert_eq!(gauge_colour(0.6, &theme), theme.warning);
        assert_eq!(gauge_colour(0.9, &theme), theme.danger);
    }

    #[test]
    fn the_frame_names_the_open_project_and_only_that() {
        let framed = framed_say("make it louder", Some(Path::new("C:/Songs/X/X.auris")));
        assert!(framed.starts_with('['), "{framed}");
        assert!(framed.contains("X.auris"), "{framed}");
        assert!(framed.ends_with("make it louder"), "{framed}");
        assert_eq!(framed_say("hello", None), "hello");
    }

    #[test]
    fn an_answer_or_an_error_puts_the_panel_back_at_rest() {
        let mut chat = AgentChat {
            busy: true,
            ..Default::default()
        };
        chat.absorb(
            AgentEvent::Answer {
                text: "done".to_string(),
                input_tokens: 1200,
                output_tokens: 40,
            },
            None,
            false,
        );
        assert!(!chat.busy);
        assert_eq!(
            (chat.tokens_in, chat.tokens_out),
            (1200, 40),
            "the gauge reads the turn's usage"
        );
        chat.busy = true;
        chat.absorb(
            AgentEvent::Error {
                message: "the provider hung up".to_string(),
            },
            None,
            false,
        );
        assert!(!chat.busy);
        assert!(matches!(chat.entries.last(), Some(ChatEntry::Error(_))));
    }
}
