//! The agent panel: a conversation with a language model, beside the song it is about.
//!
//! The UI-free `auris-agent` library runs on a cancellable background thread. Channels carry
//! requests, events and host replies; the window never blocks on model or network work.
//!
//! Editing commands execute against the window's current session. They do not save or
//! reload a project; successful edits appear on repaint and use ordinary undo history.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use auris_i18n::Key;
use auris_session::{AgentPreferences, ReasoningEffort};
use gpui::{
    AnyElement, IntoElement, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*,
    px, relative,
};

use crate::app::{AurisApp, Pane};
use crate::theme::{Metrics, Theme};
use crate::ui::icons::{Icon, icon};
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::text_field::TextField;
use crate::ui::widgets::{
    ButtonState, ButtonStyle, bounded_button_enabled, bounded_picker_label, button, button_enabled,
    disclosure,
};

pub(crate) mod controls;

/// Maximum transcript rows retained in the panel.
const CHAT_CAPACITY: usize = 500;

/// A model-selector-local action carrying the standard key that requested navigation.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = auris_agent_model, no_json)]
pub(crate) struct NavigateAgentModel {
    key: &'static str,
}

/// Bindings installed independently of the editable application keymap.
///
/// The selector context wins before window actions such as Down-to-select-the-next-track while
/// the model control owns focus. Tab deliberately remains unbound so normal focus traversal can
/// continue after dismissing the menu.
pub(crate) fn model_key_bindings() -> [gpui::KeyBinding; 7] {
    [
        gpui::KeyBinding::new(
            "up",
            NavigateAgentModel { key: "up" },
            Some("AurisAgentModel"),
        ),
        gpui::KeyBinding::new(
            "down",
            NavigateAgentModel { key: "down" },
            Some("AurisAgentModel"),
        ),
        gpui::KeyBinding::new(
            "home",
            NavigateAgentModel { key: "home" },
            Some("AurisAgentModel"),
        ),
        gpui::KeyBinding::new(
            "end",
            NavigateAgentModel { key: "end" },
            Some("AurisAgentModel"),
        ),
        gpui::KeyBinding::new(
            "enter",
            NavigateAgentModel { key: "enter" },
            Some("AurisAgentModel"),
        ),
        gpui::KeyBinding::new(
            "space",
            NavigateAgentModel { key: "space" },
            Some("AurisAgentModel"),
        ),
        gpui::KeyBinding::new(
            "escape",
            NavigateAgentModel { key: "escape" },
            Some("AurisAgentModel"),
        ),
    ]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AgentPhase {
    #[default]
    Idle,
    Waiting,
    Prefill,
    Thinking,
    Decode,
    Tool,
}

#[derive(Default)]
struct SpeedMeter {
    samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    fn record(&mut self, tokens: u64) {
        self.record_at(Instant::now(), tokens);
    }

    fn record_at(&mut self, now: Instant, tokens: u64) {
        self.samples.push_back((now, tokens));
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(10))
        {
            self.samples.pop_front();
        }
    }

    fn rate(&self) -> Option<f64> {
        let (first, _) = self.samples.front()?;
        let (last, _) = self.samples.back()?;
        let span = last.duration_since(*first);
        if span < Duration::from_millis(500) {
            return None;
        }
        let tokens: u64 = self.samples.iter().skip(1).map(|(_, tokens)| tokens).sum();
        Some(tokens as f64 / span.as_secs_f64())
    }

    fn reset(&mut self) {
        self.samples.clear();
    }
}

fn stream_token_estimate(text: &str) -> u64 {
    let ascii = text.chars().filter(char::is_ascii).count();
    let non_ascii = text.chars().count().saturating_sub(ascii);
    u64::try_from(ascii.div_ceil(4) + non_ascii)
        .unwrap_or(u64::MAX)
        .max(1)
}

fn log_preview(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = line.chars();
    let preview: String = chars.by_ref().take(72).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

/// One line of the conversation, as the panel shows it.
#[derive(Debug, PartialEq)]
pub(crate) enum ChatEntry {
    /// What the person said.
    You(String),
    /// What the model answered.
    Agent(String),
    /// The model's reasoning log, collapsed until explicitly opened.
    Reasoning(String),
    /// Status produced by the agent runtime, not by the model.
    Status(String),
    /// Compacted history restored as reference context, never as a new user instruction.
    RestoredContext(String),
    /// One tool call: running while `line` is empty, answered or refused once it is not.
    Tool {
        /// The tool's wire name.
        name: String,
        /// The JSON arguments the model supplied.
        args: String,
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
        /// Compacted older context, shown separately from user-authored turns.
        summary: String,
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
    /// The current stage of a model request.
    Phase {
        /// Provider-independent phase name.
        phase: AgentPhase,
    },
    /// One streamed fragment of model reasoning.
    ReasoningDelta {
        /// The fragment text.
        text: String,
    },
    /// One streamed fragment of the visible answer.
    TextDelta {
        /// The fragment text.
        text: String,
    },
    /// A tool was asked.
    Call {
        /// Rig's unique id for this invocation, independent of the tool name.
        call_id: String,
        /// Its wire name.
        tool: String,
        /// Its provider-produced JSON arguments, formatted for the disclosure row.
        args: String,
    },
    /// A tool answered or refused.
    Result {
        /// Rig's unique id for the invocation this completes.
        call_id: String,
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
            summary: text("summary"),
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
        "phase" => AgentEvent::Phase {
            phase: match parsed.get("phase")?.as_str()? {
                "waiting" => AgentPhase::Waiting,
                "prefill" => AgentPhase::Prefill,
                "thinking" => AgentPhase::Thinking,
                "decode" => AgentPhase::Decode,
                "tool" => AgentPhase::Tool,
                "idle" => AgentPhase::Idle,
                _ => return None,
            },
        },
        "reasoning_delta" => AgentEvent::ReasoningDelta { text: text("text") },
        "text_delta" => AgentEvent::TextDelta { text: text("text") },
        "call" => {
            let tool = text("tool");
            AgentEvent::Call {
                call_id: parsed
                    .get("call_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("legacy:{tool}")),
                tool,
                args: parsed
                    .get("args")
                    .and_then(|args| {
                        args.as_str()
                            .and_then(|args| serde_json::from_str::<serde_json::Value>(args).ok())
                            .as_ref()
                            .map_or_else(
                                || serde_json::to_string_pretty(args).ok(),
                                |args| serde_json::to_string_pretty(args).ok(),
                            )
                    })
                    .unwrap_or_default(),
            }
        }
        "result" => {
            let detail = text("text");
            let tool = text("tool");
            AgentEvent::Result {
                call_id: parsed
                    .get("call_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("legacy:{tool}")),
                tool,
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

/// Semantic colour for a translated panel note.
///
/// Most notes are neutral guidance. Only a completed action, a recoverable condition, or a
/// terminal failure spends one of the stronger signal colours.
fn note_colour(key: Key, theme: &Theme) -> gpui::Hsla {
    match key {
        Key::AgentReloaded | Key::AgentConversationReset => theme.playing,
        Key::AgentReloadOffer
        | Key::AgentResolveFirst
        | Key::AgentNotConfigured
        | Key::AgentCompactEmpty => theme.warning,
        Key::AgentEnded => theme.danger,
        _ => theme.text_muted,
    }
}

/// A tool cancellation is terminal without pretending the tool itself failed.
fn tool_mark(ok: bool, line: &str) -> &'static str {
    if line.is_empty() {
        "…"
    } else if line == "stopped" {
        "■"
    } else if ok {
        "✓"
    } else {
        "✗"
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

/// The state a hidden Agent panel reports on its switch in the window chrome.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum AgentPanelStatus {
    /// Nothing is running and no result needs attention.
    #[default]
    Idle,
    /// A turn is running.
    Running,
    /// The turn is paused for a permission decision.
    Pending,
    /// The last turn completed successfully.
    Completed,
    /// The last turn stopped or failed.
    Failed,
}

impl AgentPanelStatus {
    /// Gives live states precedence over the last settled result.
    fn from_state(busy: bool, pending: bool, settled: Option<Self>) -> Self {
        if pending {
            Self::Pending
        } else if busy {
            Self::Running
        } else {
            settled.unwrap_or_default()
        }
    }

    /// Stable selector suffix for visual tests and assistive inspection.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// A shape as well as a colour, so the four states do not rely on colour vision.
    pub(crate) fn mark(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Running => "…",
            Self::Pending => "?",
            Self::Completed => "✓",
            Self::Failed => "!",
        }
    }

    /// Localized words used by the panel switch tooltip.
    pub(crate) fn label(self) -> Key {
        match self {
            Self::Idle => Key::AgentPanel,
            Self::Running => Key::AgentWorking,
            Self::Pending => Key::AgentAwaitingApproval,
            Self::Completed => Key::AgentCompleted,
            Self::Failed => Key::AgentFailed,
        }
    }
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
    sound_search: Option<PendingSoundSearch>,
}

/// Identity of the document for which an asynchronous panel operation was started.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AgentDocumentToken {
    project: Option<PathBuf>,
    revision: u64,
}

impl AgentDocumentToken {
    fn matches(&self, project: Option<&Path>, revision: u64) -> bool {
        self.revision == revision && self.project.as_deref() == project
    }
}

struct PendingInspection {
    revision: u64,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: Receiver<Result<auris_session::audio_inspection::Inspection, String>>,
}

struct PendingSoundSearch {
    revision: u64,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: Receiver<Result<String, String>>,
}

struct PendingHistoryClear {
    project: PathBuf,
    receiver: Receiver<Result<(), String>>,
}

struct PendingHistoryLoad {
    token: AgentDocumentToken,
    receiver: Receiver<Result<auris_agent::HistorySnapshot, String>>,
}

/// One immutable composer submission waiting for persisted history to load.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingAgentSend {
    token: AgentDocumentToken,
    text: String,
    attachments: Vec<PathBuf>,
    preferences: AgentPreferences,
    selection_context: String,
}

impl PendingHistoryLoad {
    fn poll(&self) -> Option<Result<auris_agent::HistorySnapshot, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("Conversation storage worker stopped".into()))
            }
        }
    }
}

impl PendingHistoryClear {
    fn poll(&self) -> Option<Result<(), String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("Conversation storage worker stopped".into()))
            }
        }
    }
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

impl PendingSoundSearch {
    fn poll(&self, revision: u64, same_document: bool) -> Option<Result<String, String>> {
        if self.revision != revision || !same_document {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return Some(Err(
                "The document changed during sound search; search again".into(),
            ));
        }
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Sound search worker stopped".into())),
        }
    }
}

impl Drop for PendingSoundSearch {
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
    /// Legacy Ollama thinking override.
    pub(crate) thinking: Option<bool>,
    /// Provider-native reasoning effort.
    pub(crate) effort: ReasoningEffort,
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
    /// Whether the provider has answered at least once, even with a valid empty list.
    pub(crate) models_loaded: bool,
    /// Whether the model picker is dropped open.
    pub(crate) model_menu: bool,
    /// The option reached by arrow keys while the model picker is open.
    model_highlighted: usize,
    /// Keeps the highlighted model visible in a long provider list.
    model_scroll: gpui::ScrollHandle,
    /// Keeps the compact header, permission controls and model section reachable in a short dock.
    controls_scroll: gpui::ScrollHandle,
    /// Non-tab-stop focus ancestors used to reveal each section in `controls_scroll`.
    header_section_focus: Option<gpui::FocusHandle>,
    controls_section_focus: Option<gpui::FocusHandle>,
    model_section_focus: Option<gpui::FocusHandle>,
    /// Stable keyboard focus for the model selector, allocated on its first render.
    model_focus: Option<gpui::FocusHandle>,
    /// Stable keyboard focus for the message editor, allocated on its first render.
    input_focus: Option<gpui::FocusHandle>,
    /// Stable target for cancelling a queued or running turn.
    stop_focus: Option<gpui::FocusHandle>,
    /// Prompt tokens the last turn carried — the context gauge's needle.
    pub(crate) tokens_in: u64,
    /// Tokens the model has written across the conversation.
    pub(crate) tokens_out: u64,
    /// The provider-independent stage of the active request.
    phase: AgentPhase,
    /// Rolling speed estimate for streamed model output.
    speed: SpeedMeter,
    /// Whether the current turn has already drawn streamed visible text.
    streamed_answer: bool,
    /// The chosen model's context window, when its listing said.
    pub(crate) context_window: Option<u64>,
    /// The transcript rows clicked open to their full text.
    pub(crate) expanded: std::collections::BTreeSet<usize>,
    /// Running tool rows by unique call id, so parallel calls sharing a name remain distinct.
    open_tools: std::collections::BTreeMap<String, usize>,
    /// The wire a model listing comes back on.
    pub(crate) models_rx: Option<Receiver<Result<String, String>>>,
    /// A serialized deletion waiting behind any final history write by the cancelled worker.
    history_clear: Option<PendingHistoryClear>,
    /// A saved transcript being read independently of model/provider startup.
    history_load: Option<PendingHistoryLoad>,
    /// The saved project whose history has already populated this transcript.
    history_project: Option<PathBuf>,
    /// The exact displayed history to hand to the next worker for this saved project.
    loaded_history: Option<auris_agent::HistorySnapshot>,
    /// A project whose persisted transcript could not be read until it is reset.
    history_error_project: Option<PathBuf>,
    /// The exact composer submission to send once saved history has arrived.
    pending_send: Option<PendingAgentSend>,
    /// Which field holds the keyboard, if any.
    pub(crate) focused: Option<AgentField>,
    /// Restore approval shortcuts once when a hidden panel is deliberately shown again.
    pub(crate) restore_pending_focus: bool,
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
    pub(crate) fresh_history: bool,
    /// Apply changed provider settings after the current reply has finished.
    restart_after_turn: bool,
    /// Where the transcript is scrolled to.
    pub(crate) scroll: gpui::ScrollHandle,
    /// Transcript changes that arrived while the reader was away from the tail.
    pub(crate) unread_entries: usize,
    /// Whether transcript changes should continue following the tail.
    follow_tail: bool,
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
            effort: ReasoningEffort::Default,
            entries: Vec::new(),
            input: TextField::new(String::new()),
            chosen_model: String::new(),
            url_field: TextField::new(String::new()),
            key_env_field: TextField::new(String::new()),
            provider_openai: false,
            models: Vec::new(),
            fetching_models: false,
            models_error: None,
            models_loaded: false,
            model_menu: false,
            model_highlighted: 0,
            model_scroll: gpui::ScrollHandle::new(),
            controls_scroll: gpui::ScrollHandle::new(),
            header_section_focus: None,
            controls_section_focus: None,
            model_section_focus: None,
            model_focus: None,
            input_focus: None,
            stop_focus: None,
            tokens_in: 0,
            tokens_out: 0,
            phase: AgentPhase::Idle,
            speed: SpeedMeter::default(),
            streamed_answer: false,
            context_window: None,
            expanded: std::collections::BTreeSet::new(),
            open_tools: std::collections::BTreeMap::new(),
            models_rx: None,
            history_clear: None,
            history_load: None,
            history_project: None,
            loaded_history: None,
            history_error_project: None,
            pending_send: None,
            focused: None,
            restore_pending_focus: false,
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
            unread_entries: 0,
            follow_tail: true,
            link: None,
        }
    }
}

impl AgentChat {
    /// Whether a model tool call is waiting for an explicit user decision.
    pub(crate) fn has_pending_approval(&self) -> bool {
        self.controls.pending.is_some()
    }

    /// Whether the transcript is at, or close enough to resume following, its tail.
    fn is_near_tail(&self) -> bool {
        let remaining =
            f32::from(self.scroll.max_offset().height) + f32::from(self.scroll.offset().y);
        remaining <= f32::from(Metrics::CONTROL_HEIGHT) * 2.0
    }

    fn should_follow_tail(&mut self) -> bool {
        self.follow_tail = self.is_near_tail();
        self.follow_tail
    }

    /// Records one transcript mutation without taking the reader away from older content.
    fn finish_transcript_change(&mut self, follow_tail: bool) {
        self.follow_tail = follow_tail;
        if follow_tail {
            self.unread_entries = 0;
            self.scroll.scroll_to_bottom();
        } else {
            self.unread_entries = self.unread_entries.saturating_add(1);
        }
    }

    /// Explicitly resumes automatic tail following and clears the new-message count.
    pub(crate) fn jump_to_latest(&mut self) {
        self.follow_tail = true;
        self.unread_entries = 0;
        self.scroll.scroll_to_bottom();
    }

    /// Replaces the transcript with persisted turns and presents their newest exchange.
    fn replace_history(&mut self, summary: String, turns: Vec<(String, String)>) {
        self.entries.clear();
        self.open_tools.clear();
        self.expanded.clear();
        // A new document's transcript must not inherit the old document's scroll geometry.
        self.scroll = gpui::ScrollHandle::new();
        self.follow_tail = true;
        self.unread_entries = 0;
        if !summary.trim().is_empty() {
            self.push_entry(ChatEntry::RestoredContext(summary));
        }
        for (user, answer) in turns {
            self.push_entry(ChatEntry::You(user));
            self.push_entry(ChatEntry::Agent(answer));
        }
        self.jump_to_latest();
    }

    /// Appends one transcript row, keeping indexes coherent and following only a nearby tail.
    fn push_entry(&mut self, entry: ChatEntry) -> usize {
        let follow_tail = self.should_follow_tail();
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
        self.finish_transcript_change(follow_tail);
        index
    }

    /// Whether one of this panel's fields is being typed into.
    pub(crate) fn typing(&self) -> bool {
        self.focused.is_some()
    }

    /// The message editor's actual focus target, once the panel has been painted.
    pub(crate) fn input_focus(&self) -> Option<&gpui::FocusHandle> {
        self.input_focus.as_ref()
    }

    /// The field the keyboard is in, mutably.
    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        if self.pending_send.is_some() {
            return None;
        }
        Some(match self.focused? {
            AgentField::Chat => &mut self.input,
        })
    }

    /// The field the keyboard is in.
    pub(crate) fn field(&self) -> Option<&TextField> {
        if self.pending_send.is_some() {
            return None;
        }
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
        self.effort = prefs.effort;
        self.provider_openai = prefs.provider.trim() == "openai";
        self.chosen_model = prefs.model.trim().to_string();
        self.url_field = TextField::new(prefs.url.clone());
        self.key_env_field = TextField::new(prefs.api_key_env.clone());
        // A reply started for the previous provider or URL must never repopulate this form.
        // Dropping the receiver is cancellation from the UI's point of view; the short-lived
        // worker will observe its disconnected sender when it finishes.
        self.models_rx = None;
        self.fetching_models = false;
        self.models.clear();
        self.models_error = None;
        self.models_loaded = false;
        self.model_menu = false;
        self.context_window = None;
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
            effort: self.effort,
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

    /// Whether opening or repainting the panel should start its one automatic model query.
    fn needs_model_listing(&self) -> bool {
        !self.models_loaded && !self.fetching_models && self.models_rx.is_none()
    }

    /// Whether the model selector has a catalogue it can truthfully open.
    fn model_selector_enabled(&self) -> bool {
        !self.busy
            && !self.fetching_models
            && self.models_error.is_none()
            && !self.models.is_empty()
    }

    /// Applies the one terminal answer from a model-listing worker.
    fn accept_model_listing(&mut self, answer: Result<String, String>) {
        self.fetching_models = false;
        self.models_loaded = true;
        // The old ceiling belongs to the old catalogue. Keeping it through a missing model or a
        // failed refresh makes the gauge look measured when the provider no longer supports it.
        self.context_window = None;
        match answer.and_then(|line| parse_model_list(&line)) {
            Ok(models) => {
                if let Some(chosen) = models
                    .iter()
                    .find(|option| option.name == self.chosen_model)
                {
                    self.context_window = chosen.context_length;
                }
                self.models = models;
                self.models_error = None;
            }
            Err(error) => self.models_error = Some(error),
        }
    }

    /// State shown on the dock switch while this panel is closed.
    pub(crate) fn panel_status(&self) -> AgentPanelStatus {
        let settled = self.entries.iter().rev().find_map(|entry| match entry {
            ChatEntry::Agent(_) => Some(AgentPanelStatus::Completed),
            ChatEntry::Error(_) | ChatEntry::Note(Key::AgentEnded) => {
                Some(AgentPanelStatus::Failed)
            }
            ChatEntry::Tool { ok, line, .. } if !line.is_empty() => Some(if *ok {
                AgentPanelStatus::Completed
            } else {
                AgentPanelStatus::Failed
            }),
            ChatEntry::You(_)
            | ChatEntry::Note(
                Key::AgentConversationReset | Key::AgentStopped | Key::AgentSendCancelled,
            ) => Some(AgentPanelStatus::Idle),
            _ => None,
        });
        AgentPanelStatus::from_state(
            self.busy || self.history_clear.is_some() || self.pending_send.is_some(),
            self.controls.pending.is_some(),
            settled,
        )
    }

    /// Converts every still-running tool row into a terminal row.
    fn finish_open_tools(&mut self, line: &str, detail: &str) {
        let follow_tail = self.should_follow_tail();
        let mut changed = false;
        for index in std::mem::take(&mut self.open_tools).into_values() {
            if let Some(ChatEntry::Tool {
                ok,
                line: row_line,
                detail: row_detail,
                ..
            }) = self.entries.get_mut(index)
            {
                changed = true;
                *ok = false;
                *row_line = line.to_string();
                if !detail.is_empty() {
                    *row_detail = detail.to_string();
                }
            }
        }
        if changed {
            self.finish_transcript_change(follow_tail);
        }
    }

    fn append_streamed(&mut self, reasoning: bool, text: String) {
        if text.is_empty() {
            return;
        }
        let follow_tail = self.should_follow_tail();
        let appended = match self.entries.last_mut() {
            Some(ChatEntry::Reasoning(existing)) if reasoning => {
                existing.push_str(&text);
                true
            }
            Some(ChatEntry::Agent(existing)) if !reasoning => {
                existing.push_str(&text);
                true
            }
            _ => false,
        };
        if !appended {
            self.push_entry(if reasoning {
                ChatEntry::Reasoning(text)
            } else {
                ChatEntry::Agent(text)
            });
        } else {
            self.finish_transcript_change(follow_tail);
        }
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
            AgentEvent::History { summary, turns } => {
                let current = match self.entries.last() {
                    Some(ChatEntry::You(text)) => Some(text.clone()),
                    _ => None,
                };
                self.replace_history(summary, turns);
                if let Some(current) = current {
                    self.push_entry(ChatEntry::You(current));
                    self.jump_to_latest();
                }
            }
            AgentEvent::Notice { message } => {
                self.push_entry(ChatEntry::Status(message));
            }
            AgentEvent::Ready { model } => {
                self.model_label = model;
            }
            AgentEvent::Phase { phase } => {
                self.phase = phase;
            }
            AgentEvent::ReasoningDelta { text } => {
                self.phase = AgentPhase::Thinking;
                self.speed.record(stream_token_estimate(&text));
                self.append_streamed(true, text);
            }
            AgentEvent::TextDelta { text } => {
                self.phase = AgentPhase::Decode;
                self.speed.record(stream_token_estimate(&text));
                self.streamed_answer = true;
                self.append_streamed(false, text);
            }
            AgentEvent::Call {
                call_id,
                tool,
                args,
            } => {
                self.phase = AgentPhase::Tool;
                let index = self.push_entry(ChatEntry::Tool {
                    name: tool.clone(),
                    args,
                    ok: true,
                    line: String::new(),
                    detail: String::new(),
                });
                self.open_tools.insert(call_id, index);
            }
            AgentEvent::Result {
                call_id,
                tool,
                ok,
                line,
                detail,
            } => {
                let follow_tail = self.should_follow_tail();
                // The call pushed a running row; this fills it in. A result with no matching
                // call — a build mismatch, a dropped line — becomes its own row rather than
                // being lost.
                let line = if line.is_empty() {
                    if ok { "done" } else { "failed" }.to_string()
                } else {
                    line
                };
                let open_row = self
                    .open_tools
                    .remove(&call_id)
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
                        self.finish_transcript_change(follow_tail);
                    }
                    _ => {
                        self.push_entry(ChatEntry::Tool {
                            name: tool,
                            args: String::new(),
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
                self.phase = AgentPhase::Idle;
                self.speed.reset();
                // The input count is a level, the output a tally: the next turn's prompt
                // carries everything again, so the last report is the gauge's whole truth.
                if input_tokens > 0 {
                    self.tokens_in = input_tokens;
                }
                self.tokens_out += output_tokens;
                if !self.streamed_answer && !text.is_empty() {
                    self.push_entry(ChatEntry::Agent(text));
                }
                self.streamed_answer = false;
                return self.finish_reload(open, dirty);
            }
            AgentEvent::Error { message } => {
                self.busy = false;
                self.finish_open_tools("failed", &message);
                self.phase = AgentPhase::Idle;
                self.speed.reset();
                self.streamed_answer = false;
                self.push_entry(ChatEntry::Error(message));
                return self.finish_reload(open, dirty);
            }
            AgentEvent::Ended => {
                self.controls = Default::default();
                self.busy = false;
                self.phase = AgentPhase::Idle;
                self.speed.reset();
                self.streamed_answer = false;
                self.link = None;
                self.finish_open_tools("stopped", "");
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
    history: Option<auris_agent::HistorySnapshot>,
) -> Result<AgentLink, String> {
    auris_agent::Worker::spawn(
        prefs.clone(),
        folder.map(Path::to_path_buf),
        fresh_history,
        history,
    )
    .map(|worker| AgentLink {
        worker,
        inspection: None,
        sound_search: None,
    })
}

/// Fetch provider models off the UI thread.
fn spawn_model_listing(prefs: &AgentPreferences) -> Receiver<Result<String, String>> {
    auris_agent::list_models_background(prefs.clone())
}

impl AurisApp {
    fn agent_document_token(&self) -> AgentDocumentToken {
        AgentDocumentToken {
            project: self.session.path().map(Path::to_path_buf),
            revision: self.session.revision(),
        }
    }

    /// Detaches every path-scoped Agent operation after Save As changes the document identity.
    pub(crate) fn agent_document_saved_from(&mut self, previous: Option<&Path>) {
        if previous == self.session.path() {
            return;
        }
        let has_path_scoped_state = self.agent_chat.link.is_some()
            || self.agent_chat.busy
            || self.agent_chat.pending_send.is_some()
            || self.agent_chat.history_load.is_some()
            || self.agent_chat.history_clear.is_some()
            || self.agent_chat.history_project.is_some()
            || self.agent_chat.loaded_history.is_some()
            || self.agent_chat.history_error_project.is_some()
            || self.agent_chat.controls.pending.is_some()
            || !self.agent_chat.entries.is_empty();
        if !has_path_scoped_state {
            return;
        }

        // The composer belongs to the document the user just renamed, not to the old history
        // file. Preserve it while dropping the worker receiver so queued old-path events cannot
        // be observed under the new path.
        let attachments = std::mem::take(&mut self.agent_chat.attachments);
        self.agent_reset_conversation();
        self.agent_chat.attachments = attachments;
    }

    fn agent_history_clear_pending(&self) -> bool {
        self.agent_chat
            .history_clear
            .as_ref()
            .is_some_and(|pending| self.session.path() == Some(pending.project.as_path()))
    }

    /// Whether a model turn or conversation-storage operation owns the panel controls.
    pub(crate) fn agent_operation_busy(&self) -> bool {
        self.agent_chat.busy
            || self.agent_chat.pending_send.is_some()
            || self.agent_chat.controls.pending.is_some()
            || self.agent_history_clear_pending()
    }

    /// Whether Stop can abandon work without interrupting an irreversible history deletion.
    fn agent_cancelable(&self) -> bool {
        self.agent_chat.busy
            || self.agent_chat.pending_send.is_some()
            || self.agent_chat.controls.pending.is_some()
    }

    /// Starts reading this saved project's transcript before any model provider is contacted.
    fn start_agent_history_load(&mut self) -> bool {
        let token = self.agent_document_token();
        let Some(project) = token.project.as_ref() else {
            return true;
        };
        if self.agent_chat.history_project.as_ref() == Some(project)
            && (self.agent_chat.link.is_some() || self.agent_chat.loaded_history.is_some())
        {
            return true;
        }
        if self.agent_chat.history_error_project.as_ref() == Some(project) {
            return false;
        }
        if self
            .agent_chat
            .history_load
            .as_ref()
            .is_some_and(|pending| pending.token == token)
        {
            return false;
        }
        let Some(folder) = self.session.project_folder() else {
            return true;
        };
        self.agent_chat.loaded_history = None;
        self.agent_chat.history_load = Some(PendingHistoryLoad {
            token,
            receiver: auris_agent::load_history_background(
                folder.join(".auris-conversation.json"),
                self.agent_chat.fresh_history,
            ),
        });
        false
    }

    /// Applies one history answer only to the exact document snapshot that requested it.
    fn poll_agent_history_load(&mut self, cx: &mut gpui::Context<Self>) {
        let answer = self
            .agent_chat
            .history_load
            .as_ref()
            .and_then(PendingHistoryLoad::poll);
        let Some(answer) = answer else { return };
        let pending = self.agent_chat.history_load.take().unwrap();
        if !pending
            .token
            .matches(self.session.path(), self.session.revision())
        {
            if self.agent_chat.pending_send.take().is_some() {
                self.agent_chat.push_entry(ChatEntry::Error(
                    "The document changed while conversation history was loading. Review the draft and send it again."
                        .into(),
                ));
                cx.notify();
            }
            return;
        }
        let project = pending.token.project.expect("saved history has a project");
        match answer {
            Ok(snapshot) => {
                let summary = snapshot.summary().unwrap_or_default().to_string();
                let turns = snapshot.turns();
                self.agent_chat.replace_history(summary, turns);
                self.agent_chat.history_project = Some(project);
                self.agent_chat.loaded_history = Some(snapshot);
                self.agent_chat.history_error_project = None;
                self.agent_chat.fresh_history = false;
                if let Some(message) = self.agent_chat.pending_send.take() {
                    self.send_agent_message(message);
                }
            }
            Err(error) => {
                // A repaint-speed retry loop would only repeat the same disk error. New
                // Conversation or an explicit Send clears this marker and retries the read.
                self.agent_chat.history_error_project = Some(project);
                self.agent_chat.loaded_history = None;
                self.agent_chat.pending_send = None;
                self.agent_chat.push_entry(ChatEntry::Error(error));
            }
        }
        cx.notify();
    }

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
        let had_transcript = !self.agent_chat.entries.is_empty();
        self.agent_chat.controls = Default::default();
        self.agent_chat.restore_pending_focus = false;
        self.agent_chat.link = None;
        self.agent_chat.busy = false;
        self.agent_chat.phase = AgentPhase::Idle;
        self.agent_chat.speed.reset();
        self.agent_chat.streamed_answer = false;
        self.agent_chat.bound_project = None;
        self.agent_chat.history_clear = None;
        self.agent_chat.history_load = None;
        self.agent_chat.history_project = None;
        self.agent_chat.loaded_history = None;
        self.agent_chat.history_error_project = None;
        self.agent_chat.pending_send = None;
        self.agent_chat.fresh_history = false;
        self.agent_chat.restart_after_turn = false;
        self.agent_chat.produced_project = None;
        self.agent_chat.turn_project = None;
        self.agent_chat.pending_reload = None;
        self.agent_chat.attachments.clear();
        self.agent_chat.entries.clear();
        self.agent_chat.expanded.clear();
        self.agent_chat.open_tools.clear();
        self.agent_chat.model_label.clear();
        self.agent_chat.tokens_in = 0;
        self.agent_chat.tokens_out = 0;
        self.agent_chat.scroll = gpui::ScrollHandle::new();
        self.agent_chat.unread_entries = 0;
        self.agent_chat.follow_tail = true;
        if had_transcript {
            self.agent_chat
                .push_entry(ChatEntry::Note(Key::AgentConversationReset));
        }
    }

    /// Stops the current worker and permanently clears this project's conversation history.
    pub(crate) fn start_new_agent_conversation(&mut self, cx: &mut gpui::Context<Self>) {
        if self.agent_history_clear_pending() {
            return;
        }
        self.agent_stop(cx);
        self.agent_chat.history_load = None;
        self.agent_chat.history_error_project = None;
        self.agent_chat.pending_send = None;
        let project = self.session.path().map(Path::to_path_buf);
        let history = self
            .session
            .project_folder()
            .map(|folder| folder.join(".auris-conversation.json"));
        if let (Some(project), Some(history)) = (project, history) {
            self.agent_chat.history_clear = Some(PendingHistoryClear {
                project,
                receiver: auris_agent::clear_history_background(history),
            });
        } else {
            self.finish_new_agent_conversation();
        }
        cx.notify();
    }

    fn finish_new_agent_conversation(&mut self) {
        // A conflict must remain available after clearing model history.
        let pending = self.agent_chat.pending_reload.clone();
        self.agent_reset_conversation();
        self.agent_chat.pending_reload = pending;
        self.agent_chat.fresh_history = true;
        self.agent_chat.entries.clear();
    }

    /// Cancels either a queued submission or the running worker, then returns to its draft.
    fn agent_cancel(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.agent_chat.pending_send.take().is_some() {
            // Dropping the receiver detaches this panel from the read. The storage thread may
            // finish independently, but its stale answer can no longer start the queued turn.
            self.agent_chat.history_load = None;
            self.agent_chat
                .push_entry(ChatEntry::Note(Key::AgentSendCancelled));
        } else {
            self.agent_stop(cx);
        }
        self.focus_agent_field(AgentField::Chat);
        if let Some(focus) = self.agent_chat.input_focus.as_ref() {
            window.focus(focus);
        }
        cx.notify();
    }

    /// Stops the worker and checks for completed writes, which remain undoable.
    fn agent_stop(&mut self, cx: &mut gpui::Context<Self>) {
        self.agent_chat.controls.pending = None;
        self.agent_chat.restore_pending_focus = false;
        self.agent_chat.controls.permits.clear();
        self.agent_chat.controls.compacting = false;
        self.agent_chat.link = None;
        self.agent_chat.busy = false;
        self.agent_chat.phase = AgentPhase::Idle;
        self.agent_chat.speed.reset();
        self.agent_chat.streamed_answer = false;
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
        self.agent_chat.finish_open_tools("stopped", "");
        self.agent_chat
            .push_entry(ChatEntry::Note(Key::AgentStopped));
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
        if self.agent_operation_busy() {
            return;
        }
        if self.agent_control_command(&text) {
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
            if formed.is_configured()
                && formed != self.settings.agent
                && let Err(error) = self.agent_apply_settings()
            {
                let message = crate::i18n::error_text(&error, self.language());
                self.agent_chat.push_entry(ChatEntry::Error(message));
                return;
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
        let project = self.session.path().map(Path::to_path_buf);
        if self.agent_chat.history_error_project == project {
            // The visible Send button is the explicit retry after an earlier read failure.
            self.agent_chat.history_error_project = None;
        }
        let message = PendingAgentSend {
            token: self.agent_document_token(),
            text,
            attachments: self.agent_chat.attachments.clone(),
            preferences: self.settings.agent.clone(),
            selection_context: self.agent_selection_context().to_string(),
        };
        if !self.start_agent_history_load() {
            if self.agent_chat.history_load.is_some() {
                self.agent_chat.pending_send = Some(message);
            }
            return;
        }
        self.send_agent_message(message);
    }

    /// Sends one already-validated composer snapshot without consulting the live editor again.
    fn send_agent_message(&mut self, message: PendingAgentSend) {
        if !message
            .token
            .matches(self.session.path(), self.session.revision())
        {
            self.agent_chat.push_entry(ChatEntry::Error(
                "The document changed before the message could be sent. Review the draft and send it again."
                    .into(),
            ));
            return;
        }
        if self.agent_chat.link.is_none() {
            let folder = message
                .token
                .project
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf);
            let history = if message.token.project.is_some() {
                let Some(snapshot) = self.agent_chat.loaded_history.clone() else {
                    self.agent_chat.history_project = None;
                    self.agent_chat.push_entry(ChatEntry::Error(
                        "Conversation history is not ready. Send again after it reloads.".into(),
                    ));
                    return;
                };
                Some(snapshot)
            } else {
                None
            };
            match spawn_link(
                &message.preferences,
                folder.as_deref(),
                self.agent_chat.fresh_history,
                history,
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
            message.selection_context,
            framed_say(&message.text, message.token.project.as_deref())
        );
        let wire = serde_json::json!({
            "say": framed,
            "display": &message.text,
            "audio": &message.attachments,
            "policy": message.preferences.policy,
            "auto_compact_percent": message.preferences.auto_compact_percent.unwrap_or(85)
        })
        .to_string();
        if let Some(link) = self.agent_chat.link.as_mut()
            && let Err(error) = link.send(&wire)
        {
            self.agent_chat
                .push_entry(ChatEntry::Error(error.to_string()));
            self.agent_chat.link = None;
            return;
        }
        self.agent_chat.push_entry(ChatEntry::You(message.text));
        self.agent_chat.loaded_history = None;
        // Sending is an explicit return to the live exchange. A reader who scrolls away again
        // before the reply arrives will still stop following on that next transcript change.
        self.agent_chat.jump_to_latest();
        self.agent_chat.busy = true;
        self.agent_chat.phase = AgentPhase::Waiting;
        self.agent_chat.speed.reset();
        self.agent_chat.streamed_answer = false;
        self.agent_chat.input = TextField::new(String::new());
        self.agent_chat.attachments.clear();
    }

    /// Apply one request to the bound document without touching its saved file.
    fn agent_edit(&mut self, command: serde_json::Value) -> Result<String, String> {
        if self.agent_chat.bound_project.as_deref() != self.session.path() {
            return Err("The open document changed; start a new conversation".into());
        }
        if command["action"] == "list_instruments" {
            return Err(
                "list_instruments is unavailable in the live window; use a focused search_instruments query"
                    .into(),
            );
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

    fn start_agent_sound_search(&mut self, command: &serde_json::Value) -> Result<(), String> {
        use auris_session::{SoundSearch, live_agent::Command};
        if self.agent_chat.bound_project.as_deref() != self.session.path() {
            return Err("The open document changed; start a new conversation".into());
        }
        self.check_agent_edit(command)?;
        let (request, refresh) =
            match serde_json::from_value(command.clone()).map_err(|e| e.to_string())? {
                Command::SearchInstruments {
                    query,
                    limit,
                    offset,
                    refresh,
                    filter,
                } => (
                    SoundSearch::Text {
                        query,
                        limit,
                        offset,
                        filter,
                    },
                    refresh,
                ),
                Command::SimilarInstruments { id, limit, filter } => {
                    (SoundSearch::Similar { id, limit, filter }, false)
                }
                _ => return Err("Expected a sound search command".into()),
            };
        let job = self.session.sound_library_job(&self.settings.plugin_paths);
        let link = self.agent_chat.link.as_mut().ok_or("The agent stopped")?;
        if link.sound_search.is_some() {
            return Err("A sound search is already running".into());
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("auris-sound-search".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    job.run_isolated(request, refresh, &worker_cancel)
                }))
                .unwrap_or_else(|_| Err("Sound search worker panicked".into()));
                let _ = sender.send(result);
            })
            .map_err(|e| e.to_string())?;
        link.sound_search = Some(PendingSoundSearch {
            revision: self.session.revision(),
            cancel,
            receiver,
        });
        Ok(())
    }

    fn poll_agent_sound_search(&mut self) {
        let Some(link) = self.agent_chat.link.as_mut() else {
            return;
        };
        let Some(pending) = link.sound_search.as_ref() else {
            return;
        };
        let Some(result) = pending.poll(
            self.session.revision(),
            self.agent_chat.bound_project.as_deref() == self.session.path(),
        ) else {
            return;
        };
        link.sound_search = None;
        let wire = serde_json::json!({"event":"edit_result","ok":result.is_ok(),"text":result.unwrap_or_else(|e|e)});
        let _ = link.send(&wire.to_string());
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
        self.poll_agent_history_load(cx);
        let history_answer = self
            .agent_chat
            .history_clear
            .as_ref()
            .and_then(PendingHistoryClear::poll);
        if let Some(answer) = history_answer {
            let pending = self.agent_chat.history_clear.take().unwrap();
            match answer {
                Ok(()) if self.session.path() == Some(pending.project.as_path()) => {
                    self.finish_new_agent_conversation();
                }
                Ok(()) => {}
                Err(error) if self.session.path() == Some(pending.project.as_path()) => {
                    if self.agent_chat.history_project.as_ref() != Some(&pending.project) {
                        self.agent_chat.history_error_project = Some(pending.project.clone());
                    }
                    self.agent_chat.push_entry(ChatEntry::Error(error));
                }
                Err(_) => {}
            }
            cx.notify();
        }
        if self.panels.is_open(crate::dock::Panel::Agent) && !self.agent_history_clear_pending() {
            self.start_agent_history_load();
        }
        // The model listing first: one answer, then the channel is spent.
        let model_answer =
            self.agent_chat
                .models_rx
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(answer) => Some(answer),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Model-listing worker stopped".to_string()))
                    }
                });
        if let Some(answer) = model_answer {
            self.agent_chat.models_rx = None;
            self.agent_chat.accept_model_listing(answer);
            cx.notify();
        }
        // A live command must not join or finish the user's in-progress undo transaction.
        if self.drag.is_some() {
            return;
        }
        self.poll_agent_inspection();
        self.poll_agent_sound_search();
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
                if matches!(
                    command["action"].as_str(),
                    Some("search_instruments" | "similar_instruments")
                ) {
                    if let Err(error) = self.start_agent_sound_search(&command)
                        && let Some(link) = &self.agent_chat.link
                    {
                        let _ = link.send(
                            &serde_json::json!({"event":"edit_result","ok":false,"text":error})
                                .to_string(),
                        );
                    }
                    continue;
                }
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
                    let error = error.to_string();
                    self.agent_chat.finish_open_tools("failed", &error);
                    self.agent_chat.push_entry(ChatEntry::Error(error));
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
        if self.agent_operation_busy() || self.agent_chat.fetching_models {
            return;
        }
        self.agent_chat.models.clear();
        self.agent_chat.models_error = None;
        self.agent_chat.context_window = None;
        self.agent_chat.models_loaded = false;
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
    fn agent_write_through_with<E>(
        &mut self,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) -> Result<(), E> {
        let formed = self.agent_chat.preferences();
        if !formed.is_configured() || formed == self.settings.agent {
            return Ok(());
        }
        if self.persist_agent_preferences_with(formed, save)? {
            self.restart_agent_after_preferences_change();
        }
        Ok(())
    }

    /// Writes the settings section back to the shared preferences and restarts the wire.
    ///
    /// The child read its configuration at spawn, so a change means a new child; dropping the
    /// link is enough, because the next message spawns one.
    pub(crate) fn agent_apply_settings(&mut self) -> Result<(), auris_session::SessionError> {
        self.agent_apply_settings_with(|settings| settings.save())
    }

    fn agent_apply_settings_with<E>(
        &mut self,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) -> Result<(), E> {
        let formed = self.agent_chat.preferences();
        if self.persist_agent_preferences_with(formed, save)? {
            self.restart_agent_after_preferences_change();
        }
        self.finish_agent_settings_form();
        Ok(())
    }

    /// Persists preferences supplied by the separate Settings window, then mirrors them into
    /// the Agent panel. A failed write leaves both the live settings and panel form untouched.
    pub(crate) fn agent_apply_preferences_with<E>(
        &mut self,
        preferences: AgentPreferences,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) -> Result<(), E> {
        let changed = self.persist_agent_preferences_with(preferences.clone(), save)?;
        self.agent_chat.load_preferences(&preferences);
        if changed {
            self.restart_agent_after_preferences_change();
        }
        self.finish_agent_settings_form();
        Ok(())
    }

    fn persist_agent_preferences_with<E>(
        &mut self,
        preferences: AgentPreferences,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) -> Result<bool, E> {
        if preferences == self.settings.agent {
            return Ok(false);
        }
        let previous = std::mem::replace(&mut self.settings.agent, preferences);
        if let Err(error) = save(&self.settings) {
            self.settings.agent = previous;
            return Err(error);
        }
        Ok(true)
    }

    fn restart_agent_after_preferences_change(&mut self) {
        if self.agent_chat.busy {
            self.agent_chat.restart_after_turn = true;
        } else {
            self.agent_chat.link = None;
        }
        self.agent_chat.model_label = String::new();
    }

    fn finish_agent_settings_form(&mut self) {
        self.agent_chat.configuring = false;
        self.agent_chat.focused = None;
    }

    /// Answers for a key while one of the agent panel's fields holds the keyboard.
    ///
    /// The characters come through the platform's input handler like every other field's; this
    /// sees what that leaves out. Enter sends; Shift+Enter inserts a line break.
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
        if composing && key == "tab" {
            // Keep Tab inside the native IME while it owns a pre-edit. Falling through would
            // dispatch the window's focus-navigation binding and strand the candidate session.
            return true;
        }
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
        if self.agent_chat.pending_send.is_some() && !matches!(key, "tab" | "escape") {
            // History loading owns an immutable composer snapshot. Native text input is also
            // blocked by `field_mut`, while this catches keys that edit the field directly.
            return true;
        }
        let completions = if !composing
            && focused == AgentField::Chat
            && self.agent_chat.pending_send.is_none()
        {
            controls::slash_matches(self.agent_chat.input.content())
        } else {
            Vec::new()
        };
        if !completions.is_empty() {
            let selected = self.agent_chat.controls.slash_selected % completions.len();
            match key {
                "up" => {
                    self.agent_chat.controls.slash_selected =
                        (selected + completions.len() - 1) % completions.len();
                    return true;
                }
                "down" => {
                    self.agent_chat.controls.slash_selected = (selected + 1) % completions.len();
                    return true;
                }
                "tab" => {
                    self.accept_agent_completion(&completions[selected].fill);
                    return true;
                }
                "enter" if completions[selected].fill != self.agent_chat.input.content() => {
                    self.accept_agent_completion(&completions[selected].fill);
                    return true;
                }
                _ => {}
            }
        }
        if !composing && key == "tab" {
            if let Some(field) = self.agent_chat.field_mut() {
                field.unmark();
            }
            self.agent_chat.focused = None;
            if event.keystroke.modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            return true;
        }
        if !composing {
            match (key, focused) {
                ("escape", _) => {
                    if let Some(field) = self.agent_chat.field_mut() {
                        field.unmark();
                    }
                    self.agent_chat.focused = None;
                    window.focus(self.panes.handle(Pane::Agent));
                    return true;
                }
                ("enter", AgentField::Chat) => {
                    if event.keystroke.modifiers.shift {
                        self.agent_chat.input.insert("\n");
                    } else {
                        self.agent_submit(window, cx);
                    }
                    return true;
                }
                _ => {}
            }
        }
        let shift = event.keystroke.modifiers.shift;
        let secondary = event.keystroke.modifiers.secondary();
        let effect = self
            .agent_chat
            .field_mut()
            .map(|field| field.apply_key_with_clipboard(key, shift, secondary, true, cx));
        if effect == Some(crate::ui::text_field::KeyEffect::Changed) {
            self.agent_chat.controls.slash_selected = 0;
        }
        effect.is_some_and(|effect| effect != crate::ui::text_field::KeyEffect::Ignored)
    }

    fn accept_agent_completion(&mut self, fill: &str) {
        let length = self.agent_chat.input.content().len();
        self.agent_chat.input.replace(0..length, fill);
        self.agent_chat.controls.slash_selected = 0;
        self.focus_agent_field(AgentField::Chat);
    }

    /// The send button and Enter share validation before editing the live session.
    fn agent_submit(&mut self, window: &mut Window, _cx: &mut gpui::Context<Self>) {
        if self.agent_chat.input.marked().is_some() {
            return;
        }
        self.agent_send();
        if self.agent_chat.pending_send.is_some() {
            self.agent_chat.focused = None;
            if let Some(focus) = self.agent_chat.stop_focus.as_ref() {
                window.focus(focus);
            }
        }
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
        let composer_locked = self.agent_chat.pending_send.is_some();
        let cancelable = self.agent_cancelable();
        let input_focus = self
            .agent_chat
            .input_focus
            .get_or_insert_with(|| {
                cx.focus_handle()
                    .tab_index(Pane::Agent.tab_index() + 2)
                    .tab_stop(true)
            })
            .clone()
            .tab_stop(!composer_locked);
        let stop_focus = self
            .agent_chat
            .stop_focus
            .get_or_insert_with(|| {
                cx.focus_handle()
                    .tab_index(Pane::Agent.tab_index() + 3)
                    .tab_stop(true)
            })
            .clone()
            .tab_stop(cancelable);
        if !composer_locked && input_focus.is_focused(window) {
            self.focus_agent_field(AgentField::Chat);
        } else if !composer_locked
            && self.agent_chat.focused == Some(AgentField::Chat)
            && self.pane_focused(Pane::Agent, window, cx)
        {
            // Several controls express "return to the composer" without receiving a Window.
            // Move that logical request onto the real input target on the following frame.
            window.focus(&input_focus);
        }
        // Arrival while hidden cannot keep the input handle focused: `reconcile_focus` correctly
        // releases every hidden field. Showing the panel is the explicit return, so restore its
        // advertised approval keys exactly once. A later click into another pane clears the
        // ordinary field focus without this repaint stealing it back.
        if self.agent_chat.restore_pending_focus && self.agent_chat.controls.pending.is_some() {
            self.agent_chat.restore_pending_focus = false;
            self.focus_agent_field(AgentField::Chat);
            window.focus(&input_focus);
        }
        // The persistent model picker uses the saved provider even before settings opens.
        self.agent_chat
            .load_preferences_once(&self.settings.agent.clone());
        // When the panel first opens, ask the provider what it serves —
        // once, and only until an answer or a refusal lands; the refresh button asks again.
        if self.agent_chat.needs_model_listing() {
            self.agent_refresh_models();
        }
        if self.agent_operation_busy() || !self.agent_chat.model_selector_enabled() {
            self.agent_chat.model_menu = false;
        }

        let header_section_focus = self
            .agent_chat
            .header_section_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let controls_section_focus = self
            .agent_chat
            .controls_section_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let model_section_focus = self
            .agent_chat
            .model_section_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        for (index, focus) in [
            &header_section_focus,
            &controls_section_focus,
            &model_section_focus,
        ]
        .into_iter()
        .enumerate()
        {
            if focus.contains_focused(window, cx) {
                self.agent_chat.controls_scroll.scroll_to_item(index);
                break;
            }
        }

        let entries: Vec<AnyElement> = self
            .agent_chat
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| self.chat_row(index, entry, &theme, window, cx))
            .collect();
        let rows = entries;
        let operation_busy = self.agent_operation_busy();
        let pending_reload = self.agent_chat.pending_reload.is_some();
        let model_label = match self.agent_chat.model_label.is_empty() {
            true => self.settings.agent.model.clone(),
            false => self.agent_chat.model_label.clone(),
        };
        let controls_max_offset = self.agent_chat.controls_scroll.max_offset().height;
        let controls_offset = self.agent_chat.controls_scroll.offset().y;
        let controls_overflow = controls_max_offset > px(0.0);
        let controls_cue = if controls_offset >= px(-1.0) {
            "↓"
        } else if -controls_offset >= controls_max_offset - px(1.0) {
            "↑"
        } else {
            "↕"
        };

        div()
            .id("agent-panel")
            .debug_selector(|| "agent-panel".to_string())
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.0))
            .min_w_0()
            .overflow_hidden()
            .bg(theme.surface_sunken)
            // Model options stop propagation; any other click in the panel is outside the
            // selector and dismisses it before carrying on to its own control.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    if this.agent_chat.model_menu {
                        this.agent_chat.model_menu = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .id("agent-panel-controls")
                    .debug_selector(|| "agent-panel-controls".to_string())
                    .flex()
                    .flex_col()
                    .relative()
                    .flex_shrink()
                    .min_h_0()
                    .max_h(px(180.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.agent_chat.controls_scroll)
                    .child(
                        div()
                            .track_focus(&header_section_focus)
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
                                                crate::ui::prompt::PendingAction::OpenDropped(
                                                    path.clone(),
                                                ),
                                            )
                                        {
                                            this.open_project_at(path, cx);
                                        }
                                    }),
                                ))
                            })
                            .child(bounded_button_enabled(
                                "agent-new-conversation",
                                self.t(Key::AgentNewConversation),
                                ButtonStyle::Normal,
                                ButtonState::available(false, !operation_busy),
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.open_prompt(crate::ui::prompt::Prompt::ask(
                                        this.t(Key::AgentNewConversationTitle),
                                        crate::ui::prompt::Question::NewAgentConversation,
                                    ));
                                    cx.notify();
                                }),
                            ))
                            .when(cancelable, |this| {
                                this.child(
                                    button(
                                        "agent-stop",
                                        self.t(Key::AgentStop),
                                        ButtonStyle::Normal,
                                        false,
                                        theme.warning,
                                        &theme,
                                        cx.listener(|this, _, window, cx| {
                                            this.agent_cancel(window, cx)
                                        }),
                                    )
                                    .track_focus(&stop_focus),
                                )
                            })
                            .child(
                                div()
                                    .id("agent-header-model")
                                    .flex_1()
                                    .min_w_0()
                                    .h(Metrics::CONTROL_HEIGHT)
                                    .text_color(theme.text_faint)
                                    .child(bounded_picker_label(model_label.clone()))
                                    .when(!model_label.is_empty(), |this| {
                                        this.tooltip(crate::ui::tooltip::keyed_tip(
                                            model_label,
                                            "",
                                            &theme,
                                        ))
                                    }),
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
                            .child(button_enabled(
                                "agent-configure",
                                self.t(Key::AgentConfigure),
                                ButtonStyle::Normal,
                                ButtonState::available(false, !operation_busy),
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.agent_chat.model_menu = false;
                                    this.open_settings_tab(
                                        crate::settings_window::SettingsTab::Agent,
                                        cx,
                                    );
                                }),
                            )),
                    )
                    .child(
                        div()
                            .track_focus(&controls_section_focus)
                            .child(self.agent_controls(cx)),
                    )
                    .child(
                        div()
                            .track_focus(&model_section_focus)
                            .child(self.agent_model_picker(cx)),
                    )
                    .when(controls_overflow, |this| {
                        this.child(
                            div()
                                .id("agent-controls-scroll-cue")
                                .debug_selector(|| "agent-controls-scroll-cue".to_string())
                                .absolute()
                                .right(px(2.0))
                                .bottom(px(2.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(16.0))
                                .rounded_full()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.surface_raised)
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(controls_cue)
                                .tooltip(crate::ui::tooltip::keyed_tip(
                                    self.t(Key::AgentControlsScroll),
                                    "",
                                    &theme,
                                )),
                        )
                    }),
            )
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
                        .when(operation_busy, |this| {
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
                        .when(
                            self.agent_chat.entries.is_empty() && !operation_busy,
                            |this| {
                                this.child(
                                    div()
                                        .p_2()
                                        .text_xs()
                                        .text_color(theme.text_faint)
                                        .child(self.t(Key::AgentPlaceholder)),
                                )
                            },
                        ),
                    cx,
                ),
            )
            .when(self.agent_chat.unread_entries > 0, |this| {
                let unread = self.agent_chat.unread_entries;
                this.child(div().flex().justify_center().px_2().py_0p5().child(button(
                    "agent-jump-latest",
                    format!("{} ({unread})", self.t(Key::AgentJumpLatest)),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.agent_chat.jump_to_latest();
                        cx.notify();
                    }),
                )))
            })
            .child(self.agent_approval_view(cx))
            .child(self.agent_status_row(&theme))
            .child(self.agent_slash_completions(cx))
            .child(self.agent_input_row(input_focus, cx))
    }

    /// The context gauge and token counters, over the input the way picocode sets its status
    /// bar: `↑ prompt ↓ written`, a bar filling the chosen model's window, and the percentage.
    ///
    /// Nothing is drawn before the first turn — a gauge reading zero over an empty transcript
    /// is furniture — and the bar itself only appears when the model's listing said how big
    /// the window is, because a bar with an invented ceiling would be a number wearing a lie.
    fn agent_status_row(&self, theme: &Theme) -> AnyElement {
        let ratio = self.agent_chat.context_ratio();
        let phase = if self.agent_chat.controls.compacting {
            Key::AgentCompacting
        } else if self.agent_chat.controls.pending.is_some() {
            Key::AgentAwaitingApproval
        } else {
            match self.agent_chat.phase {
                AgentPhase::Idle => Key::AgentPhaseIdle,
                AgentPhase::Waiting => Key::AgentPhaseWaiting,
                AgentPhase::Prefill => Key::AgentPhasePrefill,
                AgentPhase::Thinking => Key::AgentPhaseThinking,
                AgentPhase::Decode => Key::AgentPhaseDecode,
                AgentPhase::Tool => Key::AgentPhaseTool,
            }
        };
        let active = self.agent_chat.busy || self.agent_chat.controls.compacting;
        let mut counters = div().flex().items_center().gap_2().min_w_0().child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_color(if active {
                            theme.accent
                        } else {
                            theme.text_faint
                        })
                        .child("●"),
                )
                .child(self.t(phase)),
        );
        if let Some(rate) = self.agent_chat.speed.rate().filter(|_| active) {
            counters = counters.child(format!("{:>3.0} tok/s", rate.max(1.0)));
        }
        if self.agent_chat.tokens_in > 0 || self.agent_chat.tokens_out > 0 {
            counters = counters.child(format!(
                "↑ {} ↓ {}",
                self.agent_chat.tokens_in, self.agent_chat.tokens_out
            ));
        }
        let mut row = div()
            .id("agent-gauge")
            .debug_selector(|| "agent-gauge".to_string())
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_0p5()
            .border_t_1()
            .border_color(theme.border_subtle)
            .text_xs()
            .text_color(theme.text_faint)
            .child(counters);
        if let Some(ratio) = ratio {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .id("agent-gauge-meter")
                            .debug_selector(|| "agent-gauge-meter".to_string())
                            .flex_1()
                            .min_w_0()
                            .h(px(5.0))
                            .rounded_full()
                            .bg(theme.surface_raised)
                            .child(
                                div()
                                    .w(relative(ratio))
                                    .h_full()
                                    .rounded_full()
                                    .bg(gauge_colour(ratio, theme)),
                            ),
                    )
                    .child(format!("{:>3.0}%", ratio * 100.0)),
            );
        }
        row.into_any_element()
    }

    /// One transcript row. A tool row with an answer opens to the whole of it when activated —
    /// by pointer or keyboard — so the loop's log stays where the loop is shown.
    fn chat_row(
        &self,
        index: usize,
        entry: &ChatEntry,
        theme: &Theme,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let opened = self.agent_chat.expanded.contains(&index);
        let openable = matches!(entry, ChatEntry::Reasoning(_))
            || matches!(entry, ChatEntry::Tool { args, detail, .. } if !args.is_empty() || !detail.is_empty())
            || matches!(entry, ChatEntry::RestoredContext(detail) if !detail.is_empty());
        let chevron = if opened { "▾" } else { "▸" };
        let (colour, text): (gpui::Hsla, String) = match entry {
            ChatEntry::You(text) => (theme.accent_text, text.clone()),
            ChatEntry::Agent(text) => (theme.text, text.clone()),
            ChatEntry::Reasoning(reasoning) => (
                theme.text_muted,
                format!(
                    "{chevron} ✳ {}{}",
                    self.t(Key::AgentReasoningLog),
                    match log_preview(reasoning) {
                        preview if preview.is_empty() => String::new(),
                        preview => format!(" — {preview}"),
                    }
                ),
            ),
            ChatEntry::Status(text) => (theme.text_muted, text.clone()),
            ChatEntry::RestoredContext(_) => (
                theme.text_muted,
                self.t(Key::AgentRestoredContext).to_string(),
            ),
            ChatEntry::Tool { name, ok, line, .. } => {
                let mark = tool_mark(*ok, line);
                let disclosure = if openable {
                    format!("{chevron} ")
                } else {
                    String::new()
                };
                (
                    theme.text_muted,
                    format!("{disclosure}{mark} {name}  {line}"),
                )
            }
            ChatEntry::Error(message) => (theme.danger, message.clone()),
            ChatEntry::Note(key) => (note_colour(*key, theme), self.t(*key).to_string()),
        };
        let bordered = matches!(entry, ChatEntry::You(_));
        let details = match entry {
            ChatEntry::Reasoning(text) if opened => vec![(None, text.clone())],
            ChatEntry::Tool { args, detail, .. } if opened => {
                let mut sections = Vec::new();
                if !args.is_empty() {
                    sections.push((
                        Some(self.t(Key::AgentToolArguments).to_string()),
                        args.clone(),
                    ));
                }
                if !detail.is_empty() {
                    sections.push((
                        Some(self.t(Key::AgentToolResult).to_string()),
                        detail.clone(),
                    ));
                }
                sections
            }
            ChatEntry::RestoredContext(detail) if opened => vec![(None, detail.clone())],
            _ => Vec::new(),
        };
        let body: AnyElement = match entry {
            ChatEntry::Agent(_) => super::agent_markdown::render(
                SharedString::from(format!("agent-markdown-{index}")),
                SharedString::from(text),
                theme,
                window,
                cx,
            ),
            ChatEntry::Reasoning(_) => disclosure(
                SharedString::from(format!("agent-reasoning-{index}")),
                text,
                opened,
                theme,
                cx.listener(move |this, _, _, cx| {
                    if !this.agent_chat.expanded.remove(&index) {
                        this.agent_chat.expanded.insert(index);
                    }
                    cx.notify();
                }),
            )
            .into_any_element(),
            ChatEntry::Tool { .. } if openable => disclosure(
                SharedString::from(format!("agent-tool-result-{index}")),
                text,
                opened,
                theme,
                cx.listener(move |this, _, _, cx| {
                    if !this.agent_chat.expanded.remove(&index) {
                        this.agent_chat.expanded.insert(index);
                    }
                    cx.notify();
                }),
            )
            .into_any_element(),
            ChatEntry::RestoredContext(_) => disclosure(
                SharedString::from(format!("agent-restored-context-{index}")),
                text,
                opened,
                theme,
                cx.listener(move |this, _, _, cx| {
                    if !this.agent_chat.expanded.remove(&index) {
                        this.agent_chat.expanded.insert(index);
                    }
                    cx.notify();
                }),
            )
            .into_any_element(),
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
            .when(!details.is_empty(), |this| {
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
                        .children(details.into_iter().enumerate().map(
                            |(section, (label, detail))| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .when(section > 0, |this| this.mt_1())
                                    .when_some(label, |this, label| {
                                        this.child(div().text_color(theme.text_faint).child(label))
                                    })
                                    .children(
                                        detail
                                            .lines()
                                            .map(|line| div().child(line.to_string()))
                                            .collect::<Vec<_>>(),
                                    )
                            },
                        )),
                )
            })
            .into_any_element()
    }

    /// Opens the model menu at the current choice, ready for arrow-key navigation.
    fn open_agent_model_menu(&mut self) {
        if self.agent_operation_busy() || !self.agent_chat.model_selector_enabled() {
            return;
        }
        if let Some(field) = self.agent_chat.field_mut() {
            field.unmark();
        }
        self.agent_chat.focused = None;
        self.library_search_focused = false;
        self.agent_chat.model_highlighted = self
            .agent_chat
            .models
            .iter()
            .position(|option| option.name == self.agent_chat.chosen_model)
            .unwrap_or(0);
        self.agent_chat.model_menu = true;
        self.agent_chat
            .model_scroll
            .scroll_to_item(self.agent_chat.model_highlighted);
    }

    /// Applies one model choice immediately, just like a pointer selection.
    fn choose_agent_model(&mut self, index: usize) {
        let result = self.choose_agent_model_with(index, |settings| settings.save());
        if let Err(error) = result {
            let message = crate::i18n::error_text(&error, self.language());
            self.agent_chat.models_error = Some(message);
        }
    }

    fn choose_agent_model_with<E>(
        &mut self,
        index: usize,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.agent_operation_busy() {
            return Ok(());
        }
        let Some(option) = self.agent_chat.models.get(index).cloned() else {
            self.agent_chat.model_menu = false;
            return Ok(());
        };
        let previous_context_window = self.agent_chat.context_window;
        let previous = (
            std::mem::replace(&mut self.agent_chat.chosen_model, option.name),
            previous_context_window,
            std::mem::replace(&mut self.agent_chat.tokens_in, 0),
            std::mem::replace(&mut self.agent_chat.tokens_out, 0),
            std::mem::replace(&mut self.agent_chat.model_highlighted, index),
            std::mem::replace(&mut self.agent_chat.model_menu, false),
        );
        self.agent_chat.context_window = option.context_length;
        // The pick counts the moment it is made — no Apply between the menu and the setting.
        if let Err(error) = self.agent_write_through_with(save) {
            (
                self.agent_chat.chosen_model,
                self.agent_chat.context_window,
                self.agent_chat.tokens_in,
                self.agent_chat.tokens_out,
                self.agent_chat.model_highlighted,
                self.agent_chat.model_menu,
            ) = previous;
            return Err(error);
        }
        self.agent_chat.models_error = None;
        Ok(())
    }

    /// Handles the keys belonging to the focused model selector.
    fn agent_model_menu_key(&mut self, key: &str) -> bool {
        if self.agent_operation_busy() || !self.agent_chat.model_selector_enabled() {
            self.agent_chat.model_menu = false;
            return false;
        }
        if !self.agent_chat.model_menu {
            if matches!(key, "enter" | "space" | " " | "up" | "down") {
                self.open_agent_model_menu();
                return true;
            }
            return false;
        }
        let count = self.agent_chat.models.len();
        match key {
            "escape" => self.agent_chat.model_menu = false,
            // Dismiss, then let gpui continue ordinary focus traversal.
            "tab" => {
                self.agent_chat.model_menu = false;
                return false;
            }
            "enter" | "space" | " " if count > 0 => {
                self.choose_agent_model(self.agent_chat.model_highlighted)
            }
            "up" if count > 0 => {
                self.agent_chat.model_highlighted =
                    self.agent_chat.model_highlighted.saturating_sub(1);
            }
            "down" if count > 0 => {
                self.agent_chat.model_highlighted =
                    (self.agent_chat.model_highlighted + 1).min(count - 1);
            }
            "home" if count > 0 => self.agent_chat.model_highlighted = 0,
            "end" if count > 0 => self.agent_chat.model_highlighted = count - 1,
            _ => {}
        }
        if self.agent_chat.model_menu {
            self.agent_chat
                .model_scroll
                .scroll_to_item(self.agent_chat.model_highlighted);
        }
        true
    }

    /// The settings section: provider, model, URL, key variable, apply.
    fn agent_model_picker(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let operation_busy = self.agent_operation_busy();
        let refresh_enabled = !operation_busy && !self.agent_chat.fetching_models;
        let model_enabled = !operation_busy && self.agent_chat.model_selector_enabled();
        let configured_model = !self.agent_chat.chosen_model.is_empty();
        let model_focus = self
            .agent_chat
            .model_focus
            .get_or_insert_with(|| {
                cx.focus_handle()
                    .tab_index(Pane::Agent.tab_index() + 1)
                    .tab_stop(true)
            })
            .clone()
            .tab_stop(model_enabled);
        let labelled = |label: String, control: AnyElement, theme: &Theme| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(theme.text_muted).child(label))
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
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .w_full()
                    .min_w_0()
                    .child(div().flex_1().min_w_0().child(self.model_dropdown(
                        match self.agent_chat.chosen_model.is_empty() {
                            true => self.t(Key::AgentChooseModel).to_string(),
                            false => self.agent_chat.chosen_model.clone(),
                        },
                        self.agent_chat.model_menu,
                        model_enabled,
                        configured_model,
                        model_focus,
                        cx,
                    )))
                    .child(button_enabled(
                        "agent-models-refresh",
                        self.t(Key::AgentModelsFetch),
                        ButtonStyle::Normal,
                        ButtonState::available(false, refresh_enabled),
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
            .when(
                self.agent_chat.models_loaded
                    && self.agent_chat.models.is_empty()
                    && self.agent_chat.models_error.is_none()
                    && !self.agent_chat.fetching_models,
                |this| {
                    this.child(
                        div()
                            .id("agent-models-empty")
                            .debug_selector(|| "agent-models-empty".to_string())
                            .px_1()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(self.t(Key::AgentModelsEmpty)),
                    )
                },
            )
            .when(self.agent_chat.model_menu && model_enabled, |this| {
                let names: Vec<String> = self
                    .agent_chat
                    .models
                    .iter()
                    .map(|option| match option.context_length {
                        Some(window) => format!("{}  ({}k)", option.name, window / 1024),
                        None => option.name.clone(),
                    })
                    .collect();
                this.child(self.model_option_rows(&names, &theme, cx))
            })
            .into_any_element()
    }

    /// A closed dropdown: the current choice and an arrow, opening on a click.
    ///
    /// Not a popup window — the options render as rows underneath, pushing the section down,
    /// which is all a two-item provider list and a one-server model list need.
    fn model_dropdown(
        &self,
        current: String,
        open: bool,
        enabled: bool,
        configured: bool,
        focus: gpui::FocusHandle,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let id = "agent-model";
        let theme = &self.theme;
        let pointer_focus = focus.clone();
        let tooltip = current.clone();
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .track_focus(&focus)
            .key_context("AurisAgentModel")
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .h(Metrics::CONTROL_HEIGHT)
            .px_1p5()
            .rounded(Metrics::RADIUS_SM)
            .bg(if open {
                theme.surface_hover
            } else {
                theme.surface_sunken
            })
            .border_1()
            .border_color(match open {
                true => theme.accent,
                false => theme.border_subtle,
            })
            .text_xs()
            .when(enabled, |this| {
                this.tab_index(0)
                    .focus(|this| this.border_color(theme.selection))
                    .hover(|this| {
                        this.bg(theme.surface_hover).border_color(if open {
                            theme.accent
                        } else {
                            theme.border
                        })
                    })
            })
            // Keep a configured name readable even when the provider supplied no usable list;
            // the quiet chevron and absent hover/focus response carry the disabled state.
            .when(!enabled, |this| this.opacity(0.78))
            .child(
                div()
                    .id("agent-model-current")
                    .debug_selector(|| "agent-model-current".to_string())
                    .flex_1()
                    .min_w_0()
                    .text_color(if configured {
                        theme.text
                    } else {
                        theme.text_faint
                    })
                    .child(bounded_picker_label(current)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .w(Metrics::DROPDOWN_INDICATOR_WIDTH)
                    .flex_shrink_0()
                    .child(icon(
                        if open {
                            Icon::ChevronUp
                        } else {
                            Icon::ChevronDown
                        },
                        Metrics::DROPDOWN_INDICATOR_SIZE,
                        if enabled {
                            theme.text_muted
                        } else {
                            theme.text_faint
                        },
                    )),
            )
            .when(enabled, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        window.focus(&pointer_focus);
                        if this.agent_chat.model_menu {
                            this.agent_chat.model_menu = false;
                        } else {
                            this.open_agent_model_menu();
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "tab" && this.agent_chat.model_menu {
                        this.agent_chat.model_menu = false;
                        cx.notify();
                    }
                }))
                .on_action(cx.listener(
                    |this, event: &NavigateAgentModel, _, cx| {
                        if this.agent_model_menu_key(event.key) {
                            cx.stop_propagation();
                        }
                        cx.notify();
                    },
                ))
            })
            .tooltip(crate::ui::tooltip::keyed_tip(tooltip, "", theme))
            .into_any_element()
    }

    /// The rows an open dropdown shows, each picking by its position in the list.
    fn model_option_rows(
        &self,
        names: &[String],
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let id = "agent-model-option";
        let mut list = div()
            .id((id, usize::MAX))
            .flex()
            .flex_col()
            .max_h(px(160.0))
            .overflow_y_scroll()
            .track_scroll(&self.agent_chat.model_scroll)
            .w_full()
            .min_w_0()
            .rounded(Metrics::RADIUS_SM)
            .border_1()
            .border_color(theme.border_subtle)
            .bg(theme.surface_raised)
            .on_mouse_down(
                MouseButton::Left,
                |_: &MouseDownEvent, _, cx: &mut gpui::App| {
                    cx.stop_propagation();
                },
            );
        for (index, name) in names.iter().enumerate() {
            let tooltip = name.clone();
            list = list.child(
                div()
                    .id((id, index))
                    .debug_selector(move || format!("{id}-{index}"))
                    .flex()
                    .items_center()
                    .h(Metrics::CONTROL_HEIGHT)
                    .px_1p5()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme.text)
                    .when(self.agent_chat.model_highlighted == index, |this| {
                        this.bg(theme.accent_soft)
                    })
                    .hover(|this| this.bg(theme.surface_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.choose_agent_model(index);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(bounded_picker_label(name.clone())),
                    )
                    .tooltip(crate::ui::tooltip::keyed_tip(tooltip, "", theme)),
            );
        }
        list.into_any_element()
    }

    /// A compact secondary control whose options expand beneath the control row.
    fn dropdown(
        &self,
        id: &'static str,
        current: String,
        open: bool,
        theme: &Theme,
        toggle: impl Fn(&mut Self, &mut gpui::Context<Self>) + 'static,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        disclosure(
            id,
            current,
            open,
            theme,
            cx.listener(move |this, _, _, cx| {
                toggle(this, cx);
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// The keyboard-accessible choices for an expanded secondary control.
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
            .gap_1()
            .p_1()
            .max_h(px(160.0))
            .overflow_y_scroll()
            .rounded(Metrics::RADIUS_SM)
            .border_1()
            .border_color(theme.border_subtle)
            .bg(theme.surface_raised);
        for (index, name) in names.iter().enumerate() {
            let pick = pick.clone();
            list = list.child(
                button(
                    (id, index),
                    name.clone(),
                    ButtonStyle::Ghost,
                    false,
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        pick(this, index, cx);
                        cx.notify();
                    }),
                )
                .w_full(),
            );
        }
        list.into_any_element()
    }

    fn agent_slash_completions(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let matches = controls::slash_matches(self.agent_chat.input.content());
        if matches.is_empty() {
            return div().into_any_element();
        }
        let selected = self.agent_chat.controls.slash_selected % matches.len();
        let theme = &self.theme;
        div()
            .id("agent-slash-completions")
            .debug_selector(|| "agent-slash-completions".into())
            .flex()
            .flex_col()
            .max_h(px(180.0))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.surface_raised)
            .children(matches.into_iter().enumerate().map(|(index, candidate)| {
                let fill = candidate.fill.clone();
                div()
                    .id(("agent-slash-option", index))
                    .debug_selector(move || format!("agent-slash-option-{index}"))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .when(index == selected, |this| this.bg(theme.surface_hover))
                    .hover(|this| this.bg(theme.surface_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.accept_agent_completion(&fill);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .w(px(112.0))
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.text)
                            .child(candidate.label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.text_faint)
                            .child(self.t(candidate.description)),
                    )
            }))
            .into_any_element()
    }

    /// The message field and its border, at the bottom of the panel.
    fn agent_input_row(
        &mut self,
        input_focus: gpui::FocusHandle,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let attachments = self.agent_chat.attachments.clone();
        let focused = self.agent_chat.focused == Some(AgentField::Chat);
        let empty = self.agent_chat.input.content().is_empty();
        let placeholder = self.t(Key::AgentPlaceholder).to_string();
        let operation_busy = self.agent_operation_busy();
        let composer_locked = self.agent_chat.pending_send.is_some();
        let can_send = !operation_busy
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
                    .w_full()
                    .min_w_0()
                    .child(button_enabled(
                        "agent-attach-audio",
                        self.t(Key::AgentAttachAudio),
                        ButtonStyle::Normal,
                        ButtonState::available(false, !operation_busy),
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            let language = this.language();
                            let token = this.agent_document_token();
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
                                        if this.agent_operation_busy()
                                            || !token.matches(
                                                this.session.path(),
                                                this.session.revision(),
                                            )
                                        {
                                            return;
                                        }
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
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.to_string_lossy().into_owned());
                        let remove_label =
                            self.t(Key::AgentRemoveAttachment).replace("{name}", &name);
                        button_enabled(
                            ("agent-attachment", index),
                            "",
                            ButtonStyle::Normal,
                            ButtonState::available(false, !operation_busy),
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                if index < this.agent_chat.attachments.len() {
                                    this.agent_chat.attachments.remove(index);
                                }
                                cx.notify();
                            }),
                        )
                        .w_full()
                        .max_w_full()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .w_full()
                                .min_w_0()
                                .child(
                                    div().flex_1().min_w_0().child(
                                        crate::ui::widgets::bounded_picker_label(name.clone()),
                                    ),
                                )
                                .child(
                                    div()
                                        .id(("agent-attachment-remove", index))
                                        .debug_selector(move || {
                                            format!("agent-attachment-remove-{index}")
                                        })
                                        .flex_shrink_0()
                                        .child("×"),
                                ),
                        )
                        .tooltip(crate::ui::tooltip::keyed_tip(remove_label, "", &theme))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_1()
                    .when(composer_locked, |this| {
                        this.child(
                            div()
                                .id("agent-send-waiting-history")
                                .debug_selector(|| "agent-send-waiting-history".to_string())
                                .w_full()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(self.t(Key::AgentSendWaitingHistory)),
                        )
                    })
                    .child(div().w_full().min_w_0().child(self.panel_field(
                        "agent-input",
                        AgentField::Chat,
                        focused,
                        empty,
                        placeholder,
                        input_focus,
                        !composer_locked,
                        &theme,
                        cx,
                    )))
                    .child(
                        button_enabled(
                            "agent-send",
                            self.t(Key::AgentSend),
                            if can_send {
                                ButtonStyle::Primary
                            } else {
                                ButtonStyle::Normal
                            },
                            ButtonState::available(false, can_send),
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, window, cx| {
                                this.agent_submit(window, cx);
                                cx.notify();
                            }),
                        )
                        .flex_shrink_0()
                        .cursor_default(),
                    ),
            )
            .into_any_element()
    }

    /// The growing multi-line message composer.
    #[allow(clippy::too_many_arguments)]
    fn panel_field(
        &mut self,
        id: &'static str,
        field: AgentField,
        focused: bool,
        show_placeholder: bool,
        placeholder: String,
        focus: gpui::FocusHandle,
        enabled: bool,
        theme: &Theme,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let value = match field {
            AgentField::Chat => &self.agent_chat.input,
        };
        let text = value.content().to_string();
        let selection = value.selection();
        let composing = value.marked().is_some();
        let marked = value.marked();
        let view = cx.entity();
        let pointer_focus = focus.clone();
        let minimum = crate::ui::text_area::area_height("", 2, 6);
        let maximum = crate::ui::text_area::area_height("a\na\na\na\na\na", 2, 6);

        div()
            .id(id)
            // The id again, as a name a test can find the field by — the same line every
            // button gets in `widgets`, compiled to nothing outside `cargo test`.
            .debug_selector(move || id.to_string())
            .track_focus(&focus)
            .flex()
            .relative()
            .min_h(minimum)
            .max_h(maximum)
            .overflow_hidden()
            .rounded(Metrics::RADIUS_SM)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(match focused && enabled {
                true => theme.accent,
                false => theme.border_subtle,
            })
            .when(enabled, |this| this.cursor_text())
            .when(!enabled, |this| this.cursor_default().opacity(0.62))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    // Normal-flow prose gives the wrapper its auto-growing height. It is
                    // transparent because the canvas above it paints selection, pre-edit and
                    // caret; unlike a fixed height derived from `\n`, it also counts soft wraps.
                    .child(
                        div()
                            .w_full()
                            .min_h(minimum)
                            .max_h(maximum)
                            .overflow_hidden()
                            .px_1p5()
                            .py_1()
                            .text_xs()
                            .line_height(crate::ui::text_area::AREA_LINE_HEIGHT)
                            .text_color(gpui::transparent_black())
                            .child(text.clone()),
                    )
                    .child(div().absolute().inset_0().child(
                        crate::ui::text_area::editable_wrapped_area(
                            text.clone().into(),
                            selection,
                            marked,
                            focused && enabled,
                            focus,
                            view,
                            theme.clone(),
                        ),
                    ))
                    .when(show_placeholder && text.is_empty(), |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(5.0))
                                .left(px(6.0))
                                .text_xs()
                                .text_color(theme.text_faint)
                                .child(placeholder),
                        )
                    }),
            )
            .when(enabled, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        if focused && !composing {
                            let text = this.agent_chat.input.content().to_string();
                            if let Some(offset) = crate::ui::text_area::wrapped_area_offset_at(
                                window,
                                &text,
                                event.position,
                            ) {
                                this.agent_chat
                                    .input
                                    .place_caret(offset, event.modifiers.shift);
                            }
                        }
                        this.focus_agent_field(field);
                        window.focus(&pointer_focus);
                        cx.notify();
                    }),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::agent_policy::Mode;
    use auris_session::prelude::{Note, Ticks};

    fn release_key(key: &str, cx: &mut gpui::VisualTestContext) {
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
        });
    }

    fn saved_history_snapshot(
        path: &Path,
        summary: &str,
        turns: &[(&str, &str)],
    ) -> auris_agent::HistorySnapshot {
        let turns: Vec<_> = turns
            .iter()
            .map(|(user, answer)| serde_json::json!({ "user": user, "answer": answer }))
            .collect();
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "summary": summary,
                "turns": turns,
            }))
            .unwrap(),
        )
        .unwrap();
        auris_agent::load_history_background(path.to_path_buf(), false)
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap()
    }

    #[gpui::test]
    fn approval_mode_is_a_dropdown(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_error = Some("offline fixture".into());
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-mode-menu").is_some());
        assert!(cx.debug_bounds("agent-mode-option-1").is_none());
        crate::harness::click("agent-mode-menu", cx);
        assert!(cx.debug_bounds("agent-mode-option-1").is_some());
        crate::harness::click("agent-mode-option-1", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.policy.mode, Mode::Edit)
        });
    }

    #[gpui::test]
    fn slash_candidates_are_visible_and_click_to_complete(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.input = TextField::new("/ef");
            this.focus_agent_field(AgentField::Chat);
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-slash-completions").is_some());
        crate::harness::click("agent-slash-option-0", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.input.content(), "/effort ")
        });
    }

    #[gpui::test]
    fn reasoning_and_tool_details_start_collapsed(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.entries = vec![
                ChatEntry::Reasoning("private working notes".into()),
                ChatEntry::Tool {
                    name: "inspect".into(),
                    args: "{\n  \"bar\": 2\n}".into(),
                    ok: true,
                    line: "done".into(),
                    detail: "measured".into(),
                },
            ];
        });
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.expanded.is_empty()));
        crate::harness::click("agent-line-0", cx);
        crate::harness::click("agent-line-1", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.agent_chat.expanded.iter().copied().collect::<Vec<_>>(),
                vec![0, 1]
            );
        });
    }

    #[gpui::test]
    fn copy_answer_keeps_all_markdown_after_resizing(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let answer = "## 再生結果\n\n- **WAV:** `C:/Music/preview.wav`\n\n  次の段落。";
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "test-model".into();
            this.agent_chat.push_entry(ChatEntry::Agent(answer.into()));
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
                    args: "{}".into(),
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
        crate::harness::click("agent-tool-result-2", cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.expanded.contains(&2)));
    }

    #[gpui::test]
    fn incoming_rows_follow_only_a_reader_near_the_transcript_tail(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "offline-fixture".into();
            this.agent_chat.configuring = false;
            this.agent_chat.entries = (0..80)
                .map(|index| ChatEntry::Agent(format!("row {index}: {}", "detail ".repeat(12))))
                .collect();
        });
        crate::harness::paint(&app, cx);
        app.update(cx, |this, _| {
            let view = this.scroll_view(ScrollPanel::Agent);
            assert!(view.max_offset > view.viewport);
            this.set_scroll_offset(ScrollPanel::Agent, -view.max_offset / 2.0);
        });
        crate::harness::paint(&app, cx);
        let before = app.read_with(cx, |this, _| this.scroll_view(ScrollPanel::Agent).offset);

        app.update(cx, |this, _| {
            this.agent_chat
                .push_entry(ChatEntry::Agent("new while reading".into()));
            assert_eq!(this.agent_chat.unread_entries, 1);
        });
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            let view = this.scroll_view(ScrollPanel::Agent);
            assert!(
                (view.offset - before).abs() < 1.0,
                "reader was forced to the tail"
            );
        });

        assert!(cx.debug_bounds("agent-jump-latest").is_some());
        crate::harness::click("agent-jump-latest", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            let view = this.scroll_view(ScrollPanel::Agent);
            assert!((view.offset + view.max_offset).abs() < 1.0);
            assert_eq!(this.agent_chat.unread_entries, 0);
        });
    }

    #[gpui::test]
    fn tool_result_disclosure_tabs_both_ways_and_answers_enter_and_space(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "test-model".into();
            this.agent_chat.models_loaded = true;
            this.agent_chat.entries = vec![ChatEntry::Tool {
                name: "inspect_audio".into(),
                args: String::new(),
                ok: true,
                line: "finished".into(),
                detail: "Full tool result".into(),
            }];
        });
        crate::harness::paint(&app, cx);

        crate::harness::click("agent-tool-result-0", cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.expanded.contains(&0)));
        cx.simulate_keystrokes("shift-tab tab");
        release_key("space", cx);
        app.read_with(cx, |this, _| {
            assert!(!this.agent_chat.expanded.contains(&0))
        });
        release_key("enter", cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.expanded.contains(&0)));
    }

    #[test]
    fn translated_notes_use_neutral_success_warning_and_error_signals() {
        let theme = Theme::default();
        assert_eq!(
            note_colour(Key::AgentPermissionHelp, &theme),
            theme.text_muted
        );
        assert_eq!(note_colour(Key::AgentReloaded, &theme), theme.playing);
        assert_eq!(note_colour(Key::AgentCompactEmpty, &theme), theme.warning);
        assert_eq!(note_colour(Key::AgentEnded, &theme), theme.danger);
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
    fn live_agent_refuses_the_legacy_synchronous_instrument_scan(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.policy = Default::default();
            let error = this
                .agent_edit(serde_json::json!({
                    "action": "list_instruments",
                    "query": null,
                    "offset": 0,
                    "refresh": false
                }))
                .unwrap_err();
            assert!(error.contains("search_instruments"), "{error}");
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
    fn sound_search_cancels_on_document_change_and_when_the_agent_stops() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pending = PendingSoundSearch {
            revision: 7,
            receiver,
            cancel: cancel.clone(),
        };

        assert!(pending.poll(7, true).is_none());
        assert!(pending.poll(8, true).unwrap().is_err());
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        drop(pending);
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn document_tokens_reject_other_projects_and_revisions() {
        let token = AgentDocumentToken {
            project: Some(PathBuf::from("Song.auris")),
            revision: 7,
        };
        assert!(token.matches(Some(Path::new("Song.auris")), 7));
        assert!(!token.matches(Some(Path::new("Other.auris")), 7));
        assert!(!token.matches(Some(Path::new("Song.auris")), 8));
        assert!(!token.matches(None, 7));
    }

    #[gpui::test]
    fn history_results_apply_only_to_the_requesting_document_revision(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let root =
            std::env::temp_dir().join(format!("auris-agent-history-load-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        app.update(cx, |this, cx| {
            this.session.save_as(&root.join("History.auris")).unwrap();
            let stale = this.agent_document_token();
            let (sender, receiver) = std::sync::mpsc::channel();
            this.agent_chat.history_load = Some(PendingHistoryLoad {
                token: stale,
                receiver,
            });
            this.session
                .add_default_instrument_track("revision bump")
                .unwrap();
            sender.send(Err("stale request result".into())).unwrap();
            this.poll_agent_history_load(cx);
            assert!(this.agent_chat.entries.is_empty());
            assert!(this.agent_chat.history_project.is_none());

            let current = this.agent_document_token();
            let history_path = this
                .session
                .project_folder()
                .unwrap()
                .join(".auris-conversation.json");
            let snapshot = saved_history_snapshot(
                &history_path,
                "Earlier choices are reference context.",
                &[("current request", "current answer")],
            );
            let (sender, receiver) = std::sync::mpsc::channel();
            this.agent_chat.history_load = Some(PendingHistoryLoad {
                token: current.clone(),
                receiver,
            });
            sender.send(Ok(snapshot)).unwrap();
            this.poll_agent_history_load(cx);
            assert_eq!(
                this.agent_chat.entries,
                vec![
                    ChatEntry::RestoredContext("Earlier choices are reference context.".into()),
                    ChatEntry::You("current request".into()),
                    ChatEntry::Agent("current answer".into())
                ]
            );
            assert_eq!(this.agent_chat.history_project, current.project);
        });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn first_send_waits_for_the_saved_transcript_and_preserves_its_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let root = std::env::temp_dir().join(format!(
            "auris-agent-history-before-send-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        app.update(cx, |this, _| {
            let selected = this
                .session
                .add_default_instrument_track("Click-time selection")
                .unwrap();
            this.session
                .save_as(&root.join("BeforeSend.auris"))
                .unwrap();
            this.settings.agent.model = "offline-fixture".into();
            this.settings.agent.output_tokens = Some(2_048);
            this.settings.agent.policy.mode = auris_session::agent_policy::Mode::ReadOnly;
            this.selected_track = Some(selected);
            this.agent_chat.input = TextField::new("continue the saved conversation");
            this.agent_chat.focused = Some(AgentField::Chat);
            this.agent_chat.attachments = vec![PathBuf::from("reference.wav")];
            let (_sender, receiver) = std::sync::mpsc::channel();
            let token = this.agent_document_token();
            this.agent_chat.history_load = Some(PendingHistoryLoad { token, receiver });
            let click_preferences = this.settings.agent.clone();
            let click_context = this.agent_selection_context().to_string();

            this.agent_send();

            assert!(this.agent_chat.link.is_none());
            assert!(!this.agent_chat.busy);
            assert!(this.agent_operation_busy());
            assert!(this.agent_chat.field_mut().is_none());
            assert_eq!(
                this.agent_chat.input.content(),
                "continue the saved conversation"
            );
            assert_eq!(
                this.agent_chat
                    .pending_send
                    .as_ref()
                    .map(|message| (message.text.as_str(), message.attachments.as_slice())),
                Some((
                    "continue the saved conversation",
                    [PathBuf::from("reference.wav")].as_slice()
                ))
            );

            // Even a programmatic mutation cannot substitute a later composer value or a
            // second Send for the immutable submission already waiting on disk history. This
            // also models another Settings window and a canvas click changing live state.
            this.agent_chat.input = TextField::new("later mutation");
            this.agent_chat.attachments = vec![PathBuf::from("later.wav")];
            this.settings.agent.model = "changed-in-another-window".into();
            this.settings.agent.output_tokens = Some(8_192);
            this.settings.agent.policy.mode = auris_session::agent_policy::Mode::Bypass;
            this.selected_track = None;
            this.agent_send();
            let pending = this.agent_chat.pending_send.as_ref().unwrap();
            assert_eq!(pending.text, "continue the saved conversation");
            assert_eq!(pending.attachments, vec![PathBuf::from("reference.wav")]);
            assert_eq!(pending.preferences, click_preferences);
            assert_eq!(pending.selection_context, click_context);
            assert_ne!(pending.preferences, this.settings.agent);
            assert_ne!(
                pending.selection_context,
                this.agent_selection_context().to_string()
            );
        });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn send_retries_a_failed_history_read_instead_of_becoming_a_no_op(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let root =
            std::env::temp_dir().join(format!("auris-agent-history-retry-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        app.update(cx, |this, _| {
            this.session
                .save_as(&root.join("RetryHistory.auris"))
                .unwrap();
            this.settings.agent.model = "offline-fixture".into();
            this.agent_chat.input = TextField::new("retry this exact draft");
            this.agent_chat.history_error_project = this.session.path().map(Path::to_path_buf);

            this.agent_send();

            assert!(this.agent_chat.history_error_project.is_none());
            assert!(this.agent_chat.history_load.is_some());
            assert_eq!(
                this.agent_chat
                    .pending_send
                    .as_ref()
                    .map(|message| message.text.as_str()),
                Some("retry this exact draft")
            );
            assert!(this.agent_operation_busy());
        });
        let receiver = app.update(cx, |this, _| {
            this.agent_chat.pending_send = None;
            this.agent_chat
                .history_load
                .take()
                .expect("the retry still owns its history request")
                .receiver
        });
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the background history read finishes before cleanup")
            .expect("a project without saved conversation history loads as empty");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn save_as_discards_old_agent_channels_but_preserves_the_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let root =
            std::env::temp_dir().join(format!("auris-agent-save-as-rebind-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        app.update(cx, |this, _| {
            this.agent_chat.input = TextField::new("keep this draft");
            this.agent_chat.attachments = vec![PathBuf::from("keep-reference.wav")];
            this.agent_chat.entries = vec![ChatEntry::Agent("old path answer".into())];
            this.agent_chat.busy = true;
            let token = this.agent_document_token();
            let (sender, receiver) = std::sync::mpsc::channel();
            this.agent_chat.history_load = Some(PendingHistoryLoad { token, receiver });

            this.session.save_as(&root.join("Rebound.auris")).unwrap();
            this.agent_document_saved_from(None);

            assert!(!this.agent_chat.busy);
            assert!(this.agent_chat.link.is_none());
            assert!(this.agent_chat.pending_send.is_none());
            assert!(this.agent_chat.history_load.is_none());
            assert!(
                !this
                    .agent_chat
                    .entries
                    .contains(&ChatEntry::Agent("old path answer".into()))
            );
            assert_eq!(this.agent_chat.input.content(), "keep this draft");
            assert_eq!(
                this.agent_chat.attachments,
                vec![PathBuf::from("keep-reference.wav")]
            );
            assert!(sender.send(Err("stale load".into())).is_err());
        });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn stale_history_clear_failures_do_not_leak_into_another_project(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.panels.hide(crate::dock::Panel::Agent);
            let (sender, receiver) = std::sync::mpsc::channel();
            this.agent_chat.history_clear = Some(PendingHistoryClear {
                project: PathBuf::from("a-project-that-is-not-open.auris"),
                receiver,
            });
            sender.send(Err("old project failed".into())).unwrap();
            this.drain_agent(cx);
            assert!(this.agent_chat.entries.is_empty());
            assert!(!this.agent_operation_busy());
        });
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
                summary: String::new(),
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
    fn replacing_the_document_never_shows_the_previous_projects_transcript(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.agent_chat.entries = vec![
                ChatEntry::You("Project A secret".into()),
                ChatEntry::Agent("Project A answer".into()),
            ];
            this.agent_chat.input = TextField::new("draft for the next project");
            this.agent_chat.attachments = vec![PathBuf::from("Project-A-reference.wav")];

            this.new_project();

            assert_eq!(
                this.agent_chat.entries,
                vec![ChatEntry::Note(Key::AgentConversationReset)]
            );
            assert_eq!(
                this.agent_chat.input.content(),
                "draft for the next project",
                "the visible draft remains available for the next document"
            );
            assert!(
                this.agent_chat.attachments.is_empty(),
                "attachments never cross a project boundary"
            );
        });
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
                r#"{"event":"result","call_id":"call-7","tool":"analyze","ok":true,"text":"The mix — x\nmore"}"#
            ),
            Some(AgentEvent::Result {
                call_id: "call-7".to_string(),
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
    fn streamed_phases_reasoning_and_tool_arguments_cross_the_wire() {
        assert_eq!(
            parse_event(r#"{"event":"phase","phase":"prefill"}"#),
            Some(AgentEvent::Phase {
                phase: AgentPhase::Prefill
            })
        );
        assert_eq!(
            parse_event(r#"{"event":"reasoning_delta","text":"checking"}"#),
            Some(AgentEvent::ReasoningDelta {
                text: "checking".into()
            })
        );
        assert_eq!(
            parse_event(r#"{"event":"call","tool":"inspect","args":"{\"bar\":2}"}"#),
            Some(AgentEvent::Call {
                call_id: "legacy:inspect".into(),
                tool: "inspect".into(),
                args: "{\n  \"bar\": 2\n}".into()
            })
        );
    }

    #[test]
    fn streamed_logs_are_grouped_and_disclosures_start_closed() {
        let mut chat = AgentChat {
            busy: true,
            ..Default::default()
        };
        chat.absorb(
            AgentEvent::ReasoningDelta {
                text: "first ".into(),
            },
            None,
            false,
        );
        chat.absorb(
            AgentEvent::ReasoningDelta {
                text: "second".into(),
            },
            None,
            false,
        );
        chat.absorb(
            AgentEvent::TextDelta {
                text: "answer".into(),
            },
            None,
            false,
        );
        chat.absorb(
            AgentEvent::Answer {
                text: "answer".into(),
                input_tokens: 20,
                output_tokens: 4,
            },
            None,
            false,
        );

        assert_eq!(
            chat.entries,
            vec![
                ChatEntry::Reasoning("first second".into()),
                ChatEntry::Agent("answer".into())
            ]
        );
        assert!(chat.expanded.is_empty());
    }

    #[test]
    fn speed_uses_a_stable_recent_window() {
        let mut speed = SpeedMeter::default();
        let start = Instant::now();
        speed.record_at(start, 50);
        speed.record_at(start + Duration::from_secs(1), 10);
        speed.record_at(start + Duration::from_secs(2), 10);
        assert!((speed.rate().unwrap() - 10.0).abs() < 0.01);
        speed.reset();
        assert_eq!(speed.rate(), None);
    }

    #[test]
    fn a_call_row_is_filled_in_by_its_result() {
        let mut chat = AgentChat::default();
        chat.absorb(
            AgentEvent::Call {
                call_id: "call-1".to_string(),
                tool: "compose".to_string(),
                args: "{}".to_string(),
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
                call_id: "call-1".to_string(),
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
    fn parallel_calls_with_the_same_tool_name_finish_their_own_rows() {
        let mut chat = AgentChat::default();
        for call_id in ["first", "second"] {
            chat.absorb(
                AgentEvent::Call {
                    call_id: call_id.to_string(),
                    tool: "inspect_audio".to_string(),
                    args: String::new(),
                },
                None,
                false,
            );
        }
        chat.absorb(
            AgentEvent::Result {
                call_id: "second".to_string(),
                tool: "inspect_audio".to_string(),
                ok: true,
                line: "second result".to_string(),
                detail: "second result".to_string(),
            },
            None,
            false,
        );
        chat.absorb(
            AgentEvent::Result {
                call_id: "first".to_string(),
                tool: "inspect_audio".to_string(),
                ok: false,
                line: "first result".to_string(),
                detail: "first result".to_string(),
            },
            None,
            false,
        );

        assert!(matches!(
            &chat.entries[0],
            ChatEntry::Tool { ok: false, line, .. } if line == "first result"
        ));
        assert!(matches!(
            &chat.entries[1],
            ChatEntry::Tool { ok: true, line, .. } if line == "second result"
        ));
    }

    #[test]
    fn unmatched_empty_results_are_finished_rows() {
        let mut chat = AgentChat::default();
        chat.absorb(
            AgentEvent::Result {
                call_id: "missing".to_string(),
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
            Some(ChatEntry::Tool { ok: false, line, .. }) if line == "failed"
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
                call_id: "call-1".to_string(),
                tool: "compose".to_string(),
                args: "{}".to_string(),
            },
            None,
            false,
        );
        chat.absorb(
            AgentEvent::Result {
                call_id: "call-1".to_string(),
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
    fn nonfatal_notices_are_neutral_and_errors_finalize_running_tools() {
        let mut chat = AgentChat {
            busy: true,
            ..Default::default()
        };
        chat.absorb(
            AgentEvent::Notice {
                message: "Older turns were omitted".to_string(),
            },
            None,
            false,
        );
        assert!(matches!(chat.entries.last(), Some(ChatEntry::Status(_))));

        chat.absorb(
            AgentEvent::Call {
                call_id: "call-1".to_string(),
                tool: "compose".to_string(),
                args: String::new(),
            },
            None,
            false,
        );
        chat.absorb(
            AgentEvent::Error {
                message: "provider disconnected".to_string(),
            },
            None,
            false,
        );
        assert!(matches!(
            &chat.entries[1],
            ChatEntry::Tool { ok: false, line, .. } if line == "failed"
        ));
        assert!(chat.open_tools.is_empty());
    }

    #[test]
    fn an_empty_successful_model_listing_is_a_completed_fetch() {
        let mut chat = AgentChat::default();
        assert!(chat.needs_model_listing());
        chat.accept_model_listing(Ok(r#"{"models":[]}"#.to_string()));
        assert!(chat.models.is_empty());
        assert!(chat.models_error.is_none());
        assert!(chat.models_loaded);
        assert!(!chat.needs_model_listing());
    }

    #[test]
    fn a_stopped_tool_uses_a_neutral_terminal_mark() {
        assert_eq!(tool_mark(false, "stopped"), "■");
    }

    #[test]
    fn a_terminal_model_refresh_never_keeps_a_stale_context_ceiling() {
        let mut chat = AgentChat {
            chosen_model: "removed-model".to_string(),
            context_window: Some(65_536),
            ..Default::default()
        };

        chat.accept_model_listing(Ok(r#"{"models":[]}"#.to_string()));
        assert_eq!(chat.context_window, None);

        chat.context_window = Some(65_536);
        chat.accept_model_listing(Err("provider unavailable".to_string()));
        assert_eq!(chat.context_window, None);
    }

    #[test]
    fn loading_changed_preferences_discards_an_in_flight_model_listing() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let mut chat = AgentChat {
            fetching_models: true,
            models_rx: Some(receiver),
            models: vec![ModelOption {
                name: "old-model".to_string(),
                context_length: Some(8_192),
            }],
            models_error: Some("old error".to_string()),
            context_window: Some(8_192),
            ..Default::default()
        };

        chat.load_preferences(&AgentPreferences {
            model: "new-model".to_string(),
            ..Default::default()
        });

        assert!(!chat.fetching_models);
        assert!(chat.models_rx.is_none());
        assert!(chat.models.is_empty());
        assert!(chat.models_error.is_none());
        assert_eq!(chat.context_window, None);
        assert!(chat.needs_model_listing());
    }

    #[test]
    fn terminal_entries_drive_the_closed_panel_state() {
        let mut chat = AgentChat {
            busy: true,
            ..Default::default()
        };
        assert_eq!(chat.panel_status(), AgentPanelStatus::Running);
        chat.busy = false;
        chat.entries.push(ChatEntry::Agent("done".to_string()));
        assert_eq!(chat.panel_status(), AgentPanelStatus::Completed);
        chat.entries.push(ChatEntry::Error("failed".to_string()));
        assert_eq!(chat.panel_status(), AgentPanelStatus::Failed);
        assert_eq!(
            AgentPanelStatus::from_state(false, true, None),
            AgentPanelStatus::Pending
        );
    }

    #[gpui::test]
    fn model_selection_rolls_back_when_preferences_cannot_be_saved(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent = AgentPreferences {
                model: "first".to_string(),
                ..Default::default()
            };
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models = vec![
                ModelOption {
                    name: "first".to_string(),
                    context_length: Some(32_768),
                },
                ModelOption {
                    name: "second".to_string(),
                    context_length: Some(65_536),
                },
            ];
            this.agent_chat.context_window = Some(32_768);
            this.agent_chat.tokens_in = 23;
            this.agent_chat.tokens_out = 7;
            this.agent_chat.model_highlighted = 0;
            this.agent_chat.model_menu = true;

            let result = this.choose_agent_model_with(1, |_| Err("settings fixture failure"));

            assert_eq!(result, Err("settings fixture failure"));
            assert_eq!(
                (
                    this.settings.agent.model.as_str(),
                    this.agent_chat.chosen_model.as_str(),
                    this.agent_chat.context_window,
                    this.agent_chat.tokens_in,
                    this.agent_chat.tokens_out,
                    this.agent_chat.model_highlighted,
                    this.agent_chat.model_menu,
                ),
                ("first", "first", Some(32_768), 23, 7, 0, true)
            );
        });
    }

    #[gpui::test]
    fn stopping_finishes_the_running_tool_row(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.agent_chat.busy = true;
            this.agent_chat.absorb(
                AgentEvent::Call {
                    call_id: "call-1".to_string(),
                    tool: "inspect_audio".to_string(),
                    args: String::new(),
                },
                None,
                false,
            );
            this.agent_stop(cx);
            assert!(matches!(
                &this.agent_chat.entries[0],
                ChatEntry::Tool { ok: false, line, .. } if line == "stopped"
            ));
            assert!(this.agent_chat.open_tools.is_empty());
            assert_eq!(
                this.agent_chat.entries.last(),
                Some(&ChatEntry::Note(Key::AgentStopped))
            );
            assert_eq!(this.agent_chat.panel_status(), AgentPanelStatus::Idle);
            assert_eq!(
                note_colour(Key::AgentStopped, &this.theme),
                this.theme.text_muted
            );
        });
    }

    #[test]
    fn an_unexpected_worker_end_remains_a_failure() {
        let mut chat = AgentChat {
            busy: true,
            ..Default::default()
        };

        chat.absorb(AgentEvent::Ended, None, false);

        assert_eq!(chat.entries.last(), Some(&ChatEntry::Note(Key::AgentEnded)));
        assert_eq!(chat.panel_status(), AgentPanelStatus::Failed);
    }

    #[gpui::test]
    fn a_closed_agent_switch_shows_terminal_and_live_state(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.hide(crate::dock::Panel::Agent);
            this.agent_chat.busy = true;
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-panel-state-running").is_some());

        app.update(cx, |this, _| {
            this.agent_chat.busy = false;
            this.agent_chat.entries = vec![ChatEntry::Agent("done".to_string())];
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-panel-state-completed").is_some());

        app.update(cx, |this, _| {
            this.agent_chat.entries = vec![ChatEntry::Error("failed".to_string())];
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-panel-state-failed").is_some());
    }

    #[gpui::test]
    fn the_model_picker_supports_keys_dismissal_and_busy_disabling(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent = Default::default();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.configuring = true;
            this.agent_chat.models_loaded = true;
            this.agent_chat.models = vec![
                ModelOption {
                    name: "first".to_string(),
                    context_length: Some(32_768),
                },
                ModelOption {
                    name: "second".to_string(),
                    context_length: Some(65_536),
                },
            ];
            this.agent_chat.chosen_model = "first".to_string();
        });
        crate::harness::paint(&app, cx);

        crate::harness::click("agent-input", cx);
        crate::harness::paint(&app, cx);
        cx.simulate_input("draft");
        crate::harness::click("agent-model", cx);
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .model_focus
                        .as_ref()
                        .unwrap()
                        .is_focused(window),
                    "the app repaint stole keyboard focus back from the model selector"
                );
                assert_eq!(this.agent_chat.focused, None);
            });
        });
        cx.simulate_keystrokes("down enter");
        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.chosen_model, "second");
            assert!(!this.agent_chat.model_menu);
            assert_eq!(this.agent_chat.input.content(), "draft");
        });

        crate::harness::click("agent-model", cx);
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| assert!(!this.agent_chat.model_menu));
        crate::harness::click("agent-model", cx);
        crate::harness::click("agent-input", cx);
        app.read_with(cx, |this, _| assert!(!this.agent_chat.model_menu));

        app.update(cx, |this, _| this.agent_chat.busy = true);
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-model", cx);
        app.read_with(cx, |this, _| assert!(!this.agent_chat.model_menu));
    }

    #[gpui::test]
    fn empty_model_results_explain_recovery_and_fetching_hides_the_empty_state(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models.clear();
            this.agent_chat.models_error = None;
            this.agent_chat.fetching_models = false;
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-models-empty").is_some());

        app.update(cx, |this, _| this.agent_chat.fetching_models = true);
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-models-empty").is_none());
        // A disabled Refresh remains inert while the outstanding request owns the control.
        // The shared button regressions separately cover pointer and keyboard exclusion.
        crate::harness::click("agent-models-refresh", cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.fetching_models);
            assert!(this.agent_chat.models.is_empty());
        });

        app.update(cx, |this, _| this.agent_chat.fetching_models = false);
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-models-empty").is_some());
    }

    #[gpui::test]
    fn empty_or_failed_catalogue_disables_model_choice_but_keeps_refresh_available(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "configured-model-remains-visible".into();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models.clear();
            this.agent_chat.models_error = None;
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-model-current").is_some());
        crate::harness::click("agent-model", cx);
        app.read_with(cx, |this, _| assert!(!this.agent_chat.model_menu));
        cx.update(|window, cx| {
            app.update(cx, |this, _| this.focus_pane(Pane::Agent, window));
        });
        cx.simulate_keystrokes("tab");
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .input_focus()
                        .is_some_and(|focus| focus.is_focused(window)),
                    "Tab skips the disabled model selector"
                );
            });
        });

        app.update(cx, |this, _| {
            this.agent_chat.models = vec![ModelOption {
                name: "stale-model".into(),
                context_length: None,
            }];
            this.agent_chat.models_error = Some("provider unavailable".into());
            this.agent_chat.model_menu = true;
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-model-option-0").is_none());
        crate::harness::click("agent-model", cx);
        app.read_with(cx, |this, _| assert!(!this.agent_chat.model_menu));

        crate::harness::click("agent-models-refresh", cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.fetching_models);
            assert!(this.agent_chat.models.is_empty());
            assert!(this.agent_chat.models_error.is_none());
        });
    }

    #[gpui::test]
    fn queued_send_focuses_stop_locks_controls_and_cancels_without_losing_the_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let root =
            std::env::temp_dir().join(format!("auris-agent-queued-send-ui-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let (history_sender, history_receiver) = std::sync::mpsc::channel();
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.session
                .save_as(&root.join("QueuedSend.auris"))
                .unwrap();
            this.settings.agent.model = "first".into();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models = vec![ModelOption {
                name: "first".into(),
                context_length: Some(32_768),
            }];
            this.agent_chat.input = TextField::new("keep this queued draft");
            this.agent_chat.attachments = vec![PathBuf::from("keep-reference.wav")];
            let token = this.agent_document_token();
            this.agent_chat.history_load = Some(PendingHistoryLoad {
                token,
                receiver: history_receiver,
            });
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-input", cx);
        crate::harness::click("agent-send", cx);
        crate::harness::paint(&app, cx);

        assert!(cx.debug_bounds("agent-stop").is_some());
        assert!(cx.debug_bounds("agent-send-waiting-history").is_some());
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .stop_focus
                        .as_ref()
                        .is_some_and(|focus| focus.is_focused(window)),
                    "queued Send moves focus to its available cancellation"
                );
            });
        });

        crate::harness::click("agent-model", cx);
        crate::harness::click("agent-models-refresh", cx);
        crate::harness::click("agent-configure", cx);
        crate::harness::click("agent-mode-menu", cx);
        app.read_with(cx, |this, _| {
            assert!(!this.agent_chat.model_menu);
            assert!(!this.agent_chat.fetching_models);
            assert!(!this.agent_chat.controls.mode_menu);
            assert!(this.settings_window.is_none());
            assert_ne!(
                this.settings.agent.policy.mode,
                auris_session::agent_policy::Mode::Bypass
            );
        });

        crate::harness::click("agent-stop", cx);
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(this.agent_chat.pending_send.is_none());
                assert!(this.agent_chat.history_load.is_none());
                assert_eq!(this.agent_chat.input.content(), "keep this queued draft");
                assert_eq!(
                    this.agent_chat.attachments,
                    vec![PathBuf::from("keep-reference.wav")]
                );
                assert_eq!(
                    this.agent_chat.entries.last(),
                    Some(&ChatEntry::Note(Key::AgentSendCancelled))
                );
                assert!(
                    this.agent_chat
                        .input_focus()
                        .is_some_and(|focus| focus.is_focused(window))
                );
            });
        });
        assert!(history_sender.send(Err("stale".into())).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn japanese_tabs_reveal_the_model_control_in_a_minimum_width_panel(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            for panel in crate::dock::Panel::ALL {
                this.panels.hide(panel);
            }
            this.panels
                .set_size(crate::dock::Dock::Right, crate::dock::PanelLayout::MIN_SIDE);
            this.language = auris_i18n::Language::Japanese;
            this.settings.agent.model = "日本語環境で使う設定済みモデル".into();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models = vec![ModelOption {
                name: this.settings.agent.model.clone(),
                context_length: Some(32_768),
            }];
            // Exercise the tallest settled header as well as Japanese controls. The test text
            // system gives every glyph identical metrics, so ordinary translated copy alone can
            // still fit inside the 180px controls viewport.
            this.agent_chat.pending_reload = Some(PathBuf::from("Song.auris"));
            this.agent_chat.produced_project = Some(PathBuf::from("Created.auris"));
        });
        crate::harness::resize(&app, cx, gpui::size(px(720.0), px(720.0)));

        cx.simulate_keystrokes("secondary-alt-a");
        crate::harness::paint(&app, cx);
        let panel = cx
            .debug_bounds("agent-panel")
            .expect("the minimum-width agent panel is visible");
        for selector in ["agent-new-conversation", "agent-compact"] {
            let control = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} is visible"));
            assert!(
                control.left() >= panel.left() && control.right() <= panel.right(),
                "{selector} escapes the minimum-width panel: {control:?} outside {panel:?}"
            );
        }

        cx.simulate_keystrokes("tab tab");
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .model_focus
                        .as_ref()
                        .is_some_and(|focus| focus.is_focused(window)),
                    "real Tab reaches the enabled model selector"
                );
            });
        });
        let viewport = cx
            .debug_bounds("agent-panel-controls")
            .expect("the bounded controls viewport is visible");
        let model = cx
            .debug_bounds("agent-model")
            .expect("the focused model selector is visible");
        assert!(
            model.top() >= viewport.top() && model.bottom() <= viewport.bottom(),
            "focused model {model:?} is outside controls viewport {viewport:?}"
        );
        let (offset, max_offset) = app.read_with(cx, |this, _| {
            (
                this.agent_chat.controls_scroll.offset().y,
                this.agent_chat.controls_scroll.max_offset().height,
            )
        });
        if max_offset > px(0.0) {
            assert!(offset < px(0.0));
            assert!(
                cx.debug_bounds("agent-controls-scroll-cue").is_some(),
                "a clipped controls stack advertises its own scroll region"
            );
        }
    }

    #[gpui::test]
    fn long_attachment_names_keep_the_remove_affordance_inside_a_minimum_width_panel(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.panels
                .set_size(crate::dock::Dock::Right, crate::dock::PanelLayout::MIN_SIDE);
            this.agent_chat.models_loaded = true;
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.attachments = vec![PathBuf::from(
                "録音素材_ボーカルテイク_".repeat(12) + ".super-long-extension",
            )];
        });
        crate::harness::resize(&app, cx, gpui::size(px(720.0), px(720.0)));

        let panel = cx
            .debug_bounds("agent-panel")
            .expect("Agent panel is visible");
        let chip = cx
            .debug_bounds("agent-attachment-0")
            .expect("attachment chip is visible");
        let remove = cx
            .debug_bounds("agent-attachment-remove-0")
            .expect("remove affordance remains visible");
        assert!(chip.left() >= panel.left() && chip.right() <= panel.right());
        assert!(remove.left() >= chip.left() && remove.right() <= chip.right());
    }

    #[gpui::test]
    fn minimum_side_dock_keeps_model_gauge_and_composer_inside_the_agent_panel(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.panels
                .set_size(crate::dock::Dock::Right, crate::dock::PanelLayout::MIN_SIDE);
            this.settings.agent.model = "a-very-long-offline-model-name-".repeat(8);
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models = vec![ModelOption {
                name: this.settings.agent.model.clone(),
                context_length: Some(32_768),
            }];
            this.agent_chat.context_window = Some(32_768);
            this.agent_chat.tokens_in = 16_384;
            this.agent_chat.tokens_out = 128;
            this.agent_chat.input = TextField::new(
                "長い日本語の依頼文でも、入力欄と送信操作を狭いパネル内に保ちます。".repeat(4),
            );
        });
        crate::harness::resize(&app, cx, gpui::size(px(720.0), px(720.0)));

        let panel = cx
            .debug_bounds("agent-panel")
            .expect("Agent panel is visible");
        assert!(panel.size.width <= crate::dock::PanelLayout::MIN_SIDE);
        for selector in [
            "agent-new-conversation",
            "agent-compact",
            "agent-model",
            "agent-models-refresh",
            "agent-gauge-meter",
            "agent-input",
            "agent-send",
        ] {
            let control = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} is visible at the supported minimum width"));
            assert!(
                control.size.width > px(0.0)
                    && control.left() >= panel.left()
                    && control.right() <= panel.right(),
                "{selector} escapes the 180px Agent panel: {control:?} outside {panel:?}"
            );
        }
        for selector in ["agent-new-conversation", "agent-compact"] {
            let control = cx.debug_bounds(selector).unwrap();
            assert!(
                control.size.width >= panel.size.width * 0.5,
                "{selector} collapsed instead of yielding its label: {control:?} inside {panel:?}"
            );
        }
        // The minimum dock deliberately scrolls its three header sections. Reproduce a mouse
        // user following the visible scroll cue before opening the model menu.
        app.update(cx, |this, _| {
            this.agent_chat.controls_scroll.scroll_to_item(2);
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-model", cx);
        crate::harness::paint(&app, cx);
        let option = cx
            .debug_bounds("agent-model-option-0")
            .expect("the long model option is rendered");
        assert!(
            option.left() >= panel.left() && option.right() <= panel.right(),
            "the long model option remains horizontally bounded: {option:?} outside {panel:?}"
        );
    }

    #[gpui::test]
    fn shift_enter_inserts_a_visible_second_composer_line(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-input", cx);
        crate::harness::paint(&app, cx);
        cx.simulate_input("first line");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("second line");
        crate::harness::paint(&app, cx);

        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.input.content(), "first line\nsecond line");
            assert!(!this.agent_chat.busy);
        });
        assert!(
            cx.debug_bounds("agent-input").unwrap().size.height > Metrics::CONTROL_HEIGHT,
            "the second logical line must be visible"
        );
    }

    #[gpui::test]
    fn shortcut_and_tabs_reach_the_real_composer_focus_in_both_directions(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            for panel in crate::dock::Panel::ALL {
                this.panels.hide(panel);
            }
            this.settings.agent.model = "offline-fixture".to_string();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models = vec![ModelOption {
                name: this.settings.agent.model.clone(),
                context_length: Some(32_768),
            }];
        });
        crate::harness::paint(&app, cx);

        // Drive the same binding and focus traversal as the visible application. With only the
        // arrangement and Agent stops painted, the walk is Agent pane, model, then composer.
        cx.simulate_keystrokes("secondary-alt-a tab tab tab");
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(this.panels.is_open(crate::dock::Panel::Agent));
                assert!(
                    this.agent_chat
                        .input_focus()
                        .is_some_and(|focus| focus.is_focused(window)),
                    "Tab reaches the composer's own focus handle"
                );
                assert_eq!(this.agent_chat.focused, Some(AgentField::Chat));
            });
        });

        cx.simulate_input("一行目");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("二行目");
        let mode = app.read_with(cx, |this, _| this.settings.agent.policy.mode);
        cx.simulate_keystrokes("shift-tab");
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .model_focus
                        .as_ref()
                        .is_some_and(|focus| focus.is_focused(window)),
                    "Shift+Tab walks back to the preceding Agent control"
                );
                assert_eq!(this.settings.agent.policy.mode, mode);
                assert_eq!(this.agent_chat.input.content(), "一行目\n二行目");
            });
        });

        cx.simulate_keystrokes("tab");
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .input_focus()
                        .is_some_and(|focus| focus.is_focused(window))
                );
            });
        });
    }

    #[gpui::test]
    fn long_japanese_prose_soft_wraps_without_losing_the_caret_or_ime(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "offline-fixture".to_string();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models_error = Some("offline fixture".to_string());
        });
        crate::harness::resize(&app, cx, gpui::size(px(640.0), px(480.0)));
        crate::harness::click("agent-input", cx);
        crate::harness::paint(&app, cx);

        let prose =
            "長い日本語の依頼も単語間の空白を前提にせずパネルの幅で自然に折り返します。".repeat(24);
        cx.simulate_input(&prose);
        crate::harness::paint(&app, cx);
        let (scroll_y, visual_rows) = crate::ui::text_area::wrapped_area_state()
            .expect("the focused wrapped editor was painted");
        assert!(
            visual_rows > 6,
            "ordinary Japanese prose produced soft wraps"
        );
        assert!(
            scroll_y > px(0.0),
            "the capped editor viewport followed the caret instead of clipping it"
        );
        assert!(
            cx.debug_bounds("agent-input").unwrap().size.height
                <= crate::ui::text_area::area_height("a\na\na\na\na\na", 2, 6),
            "the composer leaves the transcript usable at narrow window sizes"
        );

        app.update(cx, |this, _| {
            let end = this.agent_chat.input.content().len();
            this.agent_chat
                .input
                .replace_and_mark(end..end, "かな", None);
        });
        crate::harness::paint(&app, cx);
        cx.simulate_keystrokes("tab");
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert_eq!(
                    this.agent_chat.input.marked(),
                    Some(prose.len()..prose.len() + 6)
                );
                assert!(
                    this.agent_chat
                        .input_focus()
                        .is_some_and(|focus| focus.is_focused(window)),
                    "Tab does not discard or move away from an active IME pre-edit"
                );
            });
        });
    }

    #[gpui::test]
    fn agent_settings_opens_on_the_agent_tab(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-configure", cx);
        cx.run_until_parked();
        let settings = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(settings.into(), cx);
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("agent-provider").is_some(),
            "Agent Settings must reveal the Agent tab, not General"
        );
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
