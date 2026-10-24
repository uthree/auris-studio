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
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::icons::{Icon, icon};
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::text_field::TextField;
use crate::ui::widgets::{
    ButtonState, ButtonStyle, bounded_button_enabled, bounded_picker_label, button, button_enabled,
    disclosure, menu_picker,
};

#[cfg(test)]
mod articulation_eval;
#[cfg(test)]
mod composition_eval;
pub(crate) mod controls;
mod conversations;
#[cfg(test)]
mod performance_eval;
#[cfg(test)]
mod tool_surface_eval;
#[cfg(test)]
mod transcription_eval;

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
pub(crate) fn model_key_bindings() -> [gpui::KeyBinding; 11] {
    [
        gpui::KeyBinding::new(
            "secondary-r",
            NavigateAgentModel { key: "refresh" },
            Some("AurisAgentModel"),
        ),
        gpui::KeyBinding::new(
            "escape",
            NavigateAgentModel { key: "escape" },
            Some("AurisAgentDropdown"),
        ),
        gpui::KeyBinding::new(
            "enter",
            crate::ui::widgets::ClaimDisclosureActivation,
            Some("AurisAgentDropdown"),
        ),
        gpui::KeyBinding::new(
            "space",
            crate::ui::widgets::ClaimDisclosureActivation,
            Some("AurisAgentDropdown"),
        ),
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
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ChatEntry {
    /// What the person said.
    You {
        /// Message text.
        text: String,
        /// The files sent with this message, retained as display-only references.
        attachments: Vec<PathBuf>,
    },
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

impl ChatEntry {
    /// A text-only user entry, including older restored turns without file metadata.
    fn user(text: String) -> Self {
        Self::You {
            text,
            attachments: Vec::new(),
        }
    }
}

/// Preserve message boundaries while coalescing adjacent tool calls for presentation.
fn transcript_runs(entries: &[ChatEntry]) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start == entries.len() {
            return None;
        }
        let mut end = start + 1;
        if matches!(entries[start], ChatEntry::Tool { .. }) {
            while end < entries.len() && matches!(entries[end], ChatEntry::Tool { .. }) {
                end += 1;
            }
        }
        let run = start..end;
        start = end;
        Some(run)
    })
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
        /// The backend's unique id for this invocation, independent of the tool name.
        call_id: String,
        /// Its wire name.
        tool: String,
        /// Its provider-produced JSON arguments, formatted for the disclosure row.
        args: String,
    },
    /// A tool answered or refused.
    Result {
        /// The backend's unique id for the invocation this completes.
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

/// Why the composer cannot send right now.
///
/// Decided before the attempt, so the window can say it beside the composer and switch the
/// button off, instead of accepting a press and answering with a note at the far end of the
/// transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendBlocker {
    /// The agent rewrote the project while the window held unsaved edits.
    PendingChanges,
    /// No model has been chosen to talk to.
    NoModel,
}

/// The first thing standing between the composer and a send, in the order `agent_send` checks.
fn send_blocker(model_chosen: bool, pending_reload: bool) -> Option<SendBlocker> {
    if pending_reload {
        Some(SendBlocker::PendingChanges)
    } else if !model_chosen {
        Some(SendBlocker::NoModel)
    } else {
        None
    }
}

/// Whether a drag carries at least one file the composer would attach.
fn drop_offers_attachment(paths: &[PathBuf]) -> bool {
    paths
        .iter()
        .any(|path| auris_agent::attachment_kind(path).is_some())
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
    /// Title search in the conversation browser.
    ConversationSearch,
}

/// One model a provider reported serving.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ModelOption {
    /// The name the provider answers to.
    pub(crate) name: String,
    /// Its context window, when the provider says.
    pub(crate) context_length: Option<u64>,
    /// Human-readable model name supplied by the provider.
    pub(crate) display_name: Option<String>,
    /// Provider route when the option came from automatic discovery.
    pub(crate) connection: Option<auris_agent::ModelConnection>,
    /// Provider-reported reasoning levels, when present.
    pub(crate) efforts: Option<Vec<ReasoningEffort>>,
}

impl ModelOption {
    fn label(&self) -> String {
        let name = self.display_name.as_deref().unwrap_or(&self.name);
        let name = if let Some(suffix) = name.strip_prefix("gpt-") {
            let mut words = suffix.split('-');
            let version = words.next().unwrap_or_default();
            let rest = words
                .map(|word| {
                    let mut chars = word.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().chain(chars).collect::<String>()
                    })
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("GPT-{version} {rest}").trim().to_owned()
        } else {
            name.to_owned()
        };
        match &self.connection {
            Some(connection) => format!("{name} ({})", connection.label()),
            None => name,
        }
    }

    fn matches(&self, prefs: &AgentPreferences) -> bool {
        self.name == prefs.model
            && self
                .connection
                .as_ref()
                .is_none_or(|route| route.matches(prefs))
    }
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
                display_name: model
                    .get("display_name")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned),
                connection: model
                    .get("connection")
                    .and_then(|value| serde_json::from_value(value.clone()).ok()),
                efforts: model
                    .get("efforts")
                    .and_then(|value| value.as_array())
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_str().and_then(ReasoningEffort::named))
                            .collect()
                    }),
            })
        })
        .collect())
}

/// One cancellable worker owned by the panel. Dropping it stops the conversation.
struct AgentLink {
    worker: auris_agent::Worker,
    inspection: Option<PendingInspection>,
    listening: Option<PendingListening>,
    sound_search: Option<PendingSoundSearch>,
    job: Option<PendingAgentJob>,
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

struct PendingListening {
    revision: u64,
    operation: auris_session::agent_policy::Operation,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: Receiver<Result<auris_session::audio_audition::ListeningResult, String>>,
}

impl PendingListening {
    fn poll(
        &self,
        revision: u64,
        same_document: bool,
        policy: &auris_session::agent_policy::Policy,
    ) -> Option<Result<auris_session::audio_audition::ListeningResult, String>> {
        if self.revision != revision || !same_document {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return Some(Err(
                "The document changed during listening; request a fresh audition".into(),
            ));
        }
        if let auris_session::agent_policy::Decision::Deny(reason) = policy.decide(&self.operation)
        {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return Some(Err(reason));
        }
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Audio listening worker stopped".into())),
        }
    }
}

impl Drop for PendingListening {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

struct PendingSoundSearch {
    revision: u64,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: Receiver<Result<String, String>>,
}

struct PendingAgentJob {
    revision: u64,
    operation: auris_session::agent_policy::Operation,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: Receiver<Result<auris_session::live_agent::AgentResult, String>>,
}

impl PendingAgentJob {
    fn poll(
        &self,
        revision: u64,
        same_document: bool,
        policy: &auris_session::agent_policy::Policy,
    ) -> Option<Result<auris_session::live_agent::AgentResult, String>> {
        let error = if self.revision != revision || !same_document {
            Some("The document changed during the agent operation; retry it".to_string())
        } else if let auris_session::agent_policy::Decision::Deny(reason) =
            policy.decide(&self.operation)
        {
            Some(reason)
        } else {
            None
        };
        if let Some(error) = error {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return Some(Err(error));
        }
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Agent worker stopped unexpectedly".into())),
        }
    }
}

impl Drop for PendingAgentJob {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn spawn_agent_job(
    job: auris_session::live_agent::AgentJob,
    revision: u64,
    operation: auris_session::agent_policy::Operation,
) -> Result<PendingAgentJob, String> {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("auris-agent-job".into())
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run(&worker_cancel)))
                    .unwrap_or_else(|_| Err("Agent worker stopped unexpectedly".into()));
            let _ = sender.send(result);
        })
        .map_err(|e| e.to_string())?;
    Ok(PendingAgentJob {
        revision,
        operation,
        cancel,
        receiver,
    })
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
    conversations: conversations::Conversations,
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
    /// Conversation runtime, independent of the rig API provider.
    pub(crate) backend: String,
    /// Optional native Codex executable path.
    pub(crate) codex_path: String,
    pub(crate) claude_path: String,
    pub(crate) antigravity_path: String,
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
    /// Stable focus ancestor for the conversation header.
    header_section_focus: Option<gpui::FocusHandle>,
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
    expanded_tool_groups: std::collections::BTreeSet<usize>,
    /// Running tool rows by unique call id, so parallel calls sharing a name remain distinct.
    open_tools: std::collections::BTreeMap<String, usize>,
    /// The wire a model listing comes back on.
    pub(crate) models_rx: Option<Receiver<Result<String, String>>>,
    discovery_endpoint: Option<auris_session::AgentEndpoint>,
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
    /// Reference files selected by the user for the next message.
    attachments: Vec<PathBuf>,
    /// Every file the user attached in this window, whichever conversation it was for.
    ///
    /// A saved draft's attachments are restored only from this set. A transcript is a file in the
    /// project folder, which travels with a project, and a planted one could otherwise put any
    /// readable path on the next message for the user to send without noticing.
    attached_here: std::collections::HashSet<PathBuf>,
    attachment_error: Option<String>,
    /// Actual painted bounds used by the window's shared file-drop router.
    pub(crate) drop_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
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
            conversations: Default::default(),
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
            backend: String::new(),
            codex_path: String::new(),
            claude_path: String::new(),
            antigravity_path: String::new(),
            models: Vec::new(),
            fetching_models: false,
            models_error: None,
            models_loaded: false,
            model_menu: false,
            model_highlighted: 0,
            model_scroll: gpui::ScrollHandle::new(),
            header_section_focus: None,
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
            expanded_tool_groups: std::collections::BTreeSet::new(),
            open_tools: std::collections::BTreeMap::new(),
            models_rx: None,
            discovery_endpoint: None,
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
            attached_here: Default::default(),
            attachment_error: None,
            drop_bounds: Default::default(),
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
        self.expanded_tool_groups.clear();
        // A new document's transcript must not inherit the old document's scroll geometry.
        self.scroll = gpui::ScrollHandle::new();
        self.follow_tail = true;
        self.unread_entries = 0;
        if !summary.trim().is_empty() {
            self.push_entry(ChatEntry::RestoredContext(summary));
        }
        for (user, answer) in turns {
            self.push_entry(ChatEntry::user(user));
            self.push_entry(ChatEntry::Agent(answer));
        }
        self.jump_to_latest();
    }

    /// Appends one transcript row, keeping indexes coherent and following only a nearby tail.
    fn push_entry(&mut self, entry: ChatEntry) -> usize {
        let follow_tail = self.should_follow_tail();
        if !self.conversations.managed() && self.entries.len() >= CHAT_CAPACITY {
            self.entries.remove(0);
            self.expanded = self
                .expanded
                .iter()
                .filter_map(|index| index.checked_sub(1))
                .collect();
            self.expanded_tool_groups = self
                .expanded_tool_groups
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
        if self.focused == Some(AgentField::ConversationSearch) {
            self.conversations.focus.as_ref()
        } else {
            self.input_focus.as_ref()
        }
    }

    /// The field the keyboard is in, mutably.
    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        if (self.pending_send.is_some() || self.conversations.changing_conversation())
            && self.focused != Some(AgentField::ConversationSearch)
        {
            return None;
        }
        Some(match self.focused? {
            AgentField::Chat => &mut self.input,
            AgentField::ConversationSearch => &mut self.conversations.search,
        })
    }

    /// The field the keyboard is in.
    pub(crate) fn field(&self) -> Option<&TextField> {
        if self.pending_send.is_some() && self.focused != Some(AgentField::ConversationSearch) {
            return None;
        }
        Some(match self.focused? {
            AgentField::Chat => &self.input,
            AgentField::ConversationSearch => &self.conversations.search,
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
        self.discovery_endpoint = prefs.discovery_endpoint.clone();
        self.backend = prefs.backend.clone();
        self.codex_path = prefs.codex_path.clone();
        self.claude_path = prefs.claude_path.clone();
        self.antigravity_path = prefs.antigravity_path.clone();
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
            discovery_endpoint: self.discovery_endpoint.clone(),
            backend: self.backend.clone(),
            codex_path: self.codex_path.clone(),
            claude_path: self.claude_path.clone(),
            antigravity_path: self.antigravity_path.clone(),
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

    fn effort_choices(&self) -> Vec<ReasoningEffort> {
        let prefs = self.preferences();
        if let Some(efforts) = self
            .models
            .iter()
            .find(|option| option.matches(&prefs))
            .and_then(|option| option.efforts.as_ref())
        {
            return std::iter::once(ReasoningEffort::Default)
                .chain(efforts.iter().copied())
                .collect();
        }
        if self.backend == "antigravity" {
            return vec![
                ReasoningEffort::Default,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ];
        }
        controls::effort_choices(self.provider_openai || self.backend == "codex").to_vec()
    }

    fn selected_model_label(&self) -> String {
        let prefs = self.preferences();
        self.models
            .iter()
            .find(|option| option.matches(&prefs))
            .map_or_else(
                || {
                    ModelOption {
                        name: self.chosen_model.clone(),
                        connection: Some(auris_agent::ModelConnection::from_preferences(&prefs)),
                        ..Default::default()
                    }
                    .label()
                },
                ModelOption::label,
            )
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
                    .find(|option| option.matches(&self.preferences()))
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
            ChatEntry::You { .. }
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
                    Some(entry @ ChatEntry::You { .. }) => Some(entry.clone()),
                    _ => None,
                };
                self.replace_history(summary, turns);
                if let Some(current) = current {
                    self.push_entry(current);
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
    match history {
        Some(history) => auris_agent::Worker::spawn_conversation(prefs.clone(), history),
        None => auris_agent::Worker::spawn(
            prefs.clone(),
            folder.map(Path::to_path_buf),
            fresh_history,
            None,
        ),
    }
    .map(|worker| AgentLink {
        worker,
        inspection: None,
        listening: None,
        sound_search: None,
        job: None,
    })
}

/// Fetch provider models off the UI thread.
fn spawn_model_listing(prefs: &AgentPreferences) -> Receiver<Result<String, String>> {
    auris_agent::discover_models_background(prefs.clone())
}

impl AurisApp {
    /// Explicit agent vocal work owns the singer worker slot until its result is delivered.
    pub(crate) fn agent_vocal_busy(&self) -> bool {
        self.agent_chat
            .link
            .as_ref()
            .is_some_and(|link| link.job.is_some())
    }

    /// Attach dropped reference files without importing them into the song.
    pub(crate) fn agent_attach_files(&mut self, paths: &[PathBuf], cx: &mut gpui::Context<Self>) {
        if self.agent_chat.pending_send.is_some() {
            return;
        }
        self.agent_chat.attachment_error = None;
        for path in paths {
            if self.agent_chat.attachments.contains(path) {
                continue;
            }
            if auris_agent::attachment_kind(path).is_none() {
                self.agent_chat.attachment_error =
                    Some(self.t(Key::AgentAttachmentUnsupported).replace(
                        "{name}",
                        &path.file_name().unwrap_or_default().to_string_lossy(),
                    ));
                continue;
            }
            if self.agent_chat.attachments.len() >= auris_agent::ATTACHMENT_COUNT_LIMIT {
                self.agent_chat.attachment_error =
                    Some(self.t(Key::AgentAttachmentLimit).to_owned());
                break;
            }
            self.agent_chat.attached_here.insert(path.clone());
            self.agent_chat.attachments.push(path.clone());
        }
        cx.notify();
    }

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
        if self.agent_chat.conversations.managed() {
            if self.agent_chat.busy {
                self.agent_chat.finish_open_tools("stopped", "");
                self.agent_chat
                    .push_entry(ChatEntry::Note(Key::AgentStopped));
            }
            let navigation = std::mem::take(&mut self.agent_chat.conversations);
            let entries = std::mem::take(&mut self.agent_chat.entries);
            let attachments = std::mem::take(&mut self.agent_chat.attachments);
            self.agent_reset_conversation();
            self.agent_chat.conversations = navigation;
            self.agent_chat.entries = entries;
            self.agent_chat.attachments = attachments;
            if let Some(folder) = self.session.project_folder() {
                self.conversation_operation(conversations::Operation::Copy(
                    auris_session::conversations::ConversationStore::for_project(folder),
                ));
            }
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
        self.agent_chat.conversations.unavailable()
            || self.agent_chat.busy
            || self.agent_chat.pending_send.is_some()
            || self.agent_chat.controls.pending.is_some()
            || self.agent_history_clear_pending()
    }

    /// Whether Stop can abandon work without interrupting an irreversible history deletion.
    /// What would stop a send right now. A model picked in a settings form that is still open
    /// counts as chosen, because `agent_send` applies that form before it asks.
    fn agent_send_blocker(&self) -> Option<SendBlocker> {
        if controls::is_control_command(self.agent_chat.input.content().trim()) {
            return None;
        }
        let model_chosen = self.settings.agent.is_configured()
            || (self.agent_chat.configuring && self.agent_chat.preferences().is_configured());
        send_blocker(model_chosen, self.agent_chat.pending_reload.is_some())
    }

    fn agent_cancelable(&self) -> bool {
        self.agent_chat.busy
            || self.agent_chat.pending_send.is_some()
            || self.agent_chat.controls.pending.is_some()
    }

    /// Starts reading this saved project's transcript before any model provider is contacted.
    fn start_agent_history_load(&mut self) -> bool {
        let token = self.agent_document_token();
        if self.agent_chat.conversations.managed() {
            if self.agent_chat.conversations.unavailable() {
                return false;
            }
            if self.agent_chat.conversations.active_id().is_none() {
                return false;
            }
            if self.agent_chat.link.is_some() || self.agent_chat.loaded_history.is_some() {
                return true;
            }
            if self.agent_chat.history_load.is_some() {
                return false;
            }
            let Some(path) = self.agent_chat.conversations.memory_path() else {
                return false;
            };
            self.agent_chat.history_load = Some(PendingHistoryLoad {
                token,
                receiver: auris_agent::load_history_background(path, false),
            });
            return false;
        }
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
        let project = pending.token.project;
        match answer {
            Ok(snapshot) => {
                let summary = snapshot.summary().unwrap_or_default().to_string();
                let turns = snapshot.turns();
                if !self.agent_chat.conversations.managed() {
                    self.agent_chat.replace_history(summary, turns);
                }
                self.agent_chat.history_project = project;
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
                if self.agent_chat.conversations.managed() {
                    self.agent_chat.conversations.error = Some(error.clone());
                }
                self.agent_chat.history_error_project = project;
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
        let _ = self.flush_conversation_storage();
        self.agent_chat.conversations = Default::default();
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
        self.agent_chat.expanded_tool_groups.clear();
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

    /// Starts a new conversation while preserving existing transcripts and drafts.
    pub(crate) fn start_new_agent_conversation(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.agent_chat.conversations.managed() {
            self.agent_stop(cx);
            self.agent_chat.history_load = None;
            self.agent_chat.pending_send = None;
        }
        self.initialize_conversations();
        self.conversation_operation(conversations::Operation::Create);
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
        self.session.end_agent_request();
        self.agent_chat.controls.review = None;
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
        if let Err(error) =
            auris_agent::validate_attachments(&self.settings.agent, &self.agent_chat.attachments)
        {
            self.agent_chat.attachment_error = Some(
                if self.settings.agent.backend == "antigravity"
                    && self.agent_chat.attachments.iter().any(|path| {
                        auris_agent::attachment_kind(path)
                            == Some(auris_agent::AttachmentKind::Image)
                    })
                {
                    self.t(Key::AgentAttachmentImageUnsupported).to_owned()
                } else if self.agent_chat.attachments.iter().any(|path| {
                    auris_agent::attachment_kind(path) == Some(auris_agent::AttachmentKind::Audio)
                }) {
                    self.t(Key::AgentAttachmentAudioUnsupported).to_owned()
                } else {
                    error
                },
            );
            return;
        }
        self.agent_chat.attachment_error = None;
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
            if self.agent_chat.history_load.is_some() || self.agent_chat.conversations.managed() {
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
            let history = if message.token.project.is_some()
                || self.agent_chat.conversations.managed()
            {
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
            "attachments": &message.attachments,
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
        // Typed here, so it is something the user asked for. Restored turns are not, which is
        // why automatic review reads this list rather than the transcript.
        self.session.begin_agent_request();
        self.agent_chat
            .controls
            .typed_requests
            .push(message.text.clone());
        self.agent_chat.push_entry(ChatEntry::You {
            text: message.text,
            attachments: message.attachments,
        });
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

    fn start_agent_listening(&mut self, command: &serde_json::Value) -> Result<(), String> {
        if self.agent_chat.bound_project.as_deref() != self.session.path() {
            return Err("The open document changed; start a new conversation".into());
        }
        self.check_agent_edit(command)?;
        let auris_session::live_agent::Command::Listen {
            start_bar,
            bars,
            focus,
            compare_to,
        } = serde_json::from_value(command.clone()).map_err(|error| error.to_string())?
        else {
            return Err("Expected listen".into());
        };
        let operation = auris_session::agent_policy::Operation::parse(
            "edit_project",
            &serde_json::json!({"command":command}),
        )?;
        let job = self.session.begin_listening(
            start_bar,
            bars,
            focus.as_deref(),
            compare_to.as_deref(),
        )?;
        let link = self.agent_chat.link.as_mut().ok_or("The agent stopped")?;
        if link.listening.is_some() {
            return Err("A listening operation is already running".into());
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("auris-audio-listening".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    job.run(&worker_cancel)
                }))
                .unwrap_or_else(|_| Err("Audio listening worker stopped unexpectedly".into()));
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        link.listening = Some(PendingListening {
            revision: self.session.revision(),
            operation,
            cancel,
            receiver,
        });
        Ok(())
    }

    fn start_agent_job(&mut self, command: &serde_json::Value) -> Result<(), String> {
        if self.agent_chat.bound_project.as_deref() != self.session.path() {
            return Err("The open document changed; start a new conversation".into());
        }
        self.check_agent_edit(command)?;
        if self
            .agent_chat
            .link
            .as_ref()
            .ok_or("The agent stopped")?
            .job
            .is_some()
        {
            return Err("An agent operation is already running".into());
        }
        let operation = auris_session::agent_policy::Operation::parse(
            "edit_project",
            &serde_json::json!({"command":command}),
        )?;
        let command = serde_json::from_value(command.clone()).map_err(|e| e.to_string())?;
        if operation.mutating {
            self.cancel_auto_sing();
        }
        let job = self
            .session
            .begin_agent_job(command, self.settings.clone())?;
        let pending = spawn_agent_job(job, self.session.revision(), operation)?;
        self.agent_chat
            .link
            .as_mut()
            .ok_or("The agent stopped")?
            .job = Some(pending);
        Ok(())
    }

    fn poll_agent_job(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(link) = self.agent_chat.link.as_mut() else {
            return;
        };
        let Some(pending) = link.job.take() else {
            return;
        };
        let Some(result) = pending.poll(
            self.session.revision(),
            self.agent_chat.bound_project.as_deref() == self.session.path(),
            &self.settings.agent.policy,
        ) else {
            link.job = Some(pending);
            return;
        };
        let revision = self.session.revision();
        let result =
            result.and_then(|result| self.session.continue_agent_job(result, &pending.cancel));
        let response = match result {
            Ok(auris_session::live_agent::AgentStep::Work(job)) => {
                match spawn_agent_job(job, revision, pending.operation.clone()) {
                    Ok(next) => {
                        if let Some(link) = self.agent_chat.link.as_mut() {
                            link.job = Some(next);
                        }
                        return;
                    }
                    Err(error) => Err(error),
                }
            }
            Ok(auris_session::live_agent::AgentStep::Done(text)) => Ok(text),
            Err(error) => Err(error),
        };
        if let Some(link) = self.agent_chat.link.as_mut() {
            let _ = link.send(&serde_json::json!({"event":"edit_result","ok":response.is_ok(),"text":response.unwrap_or_else(|e|e)}).to_string());
        }
        if self.session.revision() != revision {
            self.resync_selection();
            self.cancel_auto_sing();
            self.invalidate_sung_previews();
            self.reset_drum_analysis();
        }
        cx.notify();
    }

    fn start_agent_sound_search(&mut self, command: &serde_json::Value) -> Result<(), String> {
        use auris_session::live_agent::Command;
        if self.agent_chat.bound_project.as_deref() != self.session.path() {
            return Err("The open document changed; start a new conversation".into());
        }
        self.check_agent_edit(command)?;
        let (request, refresh) =
            match serde_json::from_value(command.clone()).map_err(|e| e.to_string())? {
                Command::SearchInstruments { request, refresh } => (request, refresh),
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

    fn poll_agent_listening(&mut self) {
        let Some(link) = self.agent_chat.link.as_mut() else {
            return;
        };
        let Some(pending) = link.listening.as_ref() else {
            return;
        };
        let Some(result) = pending.poll(
            self.session.revision(),
            self.agent_chat.bound_project.as_deref() == self.session.path(),
            &self.settings.agent.policy,
        ) else {
            return;
        };
        link.listening = None;
        let result = result.and_then(|result| self.session.continue_listening(result));
        let wire = serde_json::json!({"event":"edit_result","ok":result.is_ok(),"text":result.unwrap_or_else(|error|error)});
        let _ = link.send(&wire.to_string());
    }

    /// Drains the agent's channel, obeying what each event asks for.
    ///
    /// Called from the repaint tick, beside `Session::poll` — the same shape as everything
    /// else another thread writes and this one reads.
    pub(crate) fn drain_agent(&mut self, cx: &mut gpui::Context<Self>) {
        self.poll_conversations(cx);
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
        self.poll_agent_listening();
        self.poll_agent_sound_search();
        self.poll_agent_job(cx);
        if self.poll_agent_review() {
            cx.notify();
        }
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
                if serde_json::from_value::<auris_session::live_agent::Command>(command.clone())
                    .is_ok_and(|command| command.needs_worker())
                {
                    if let Err(error) = self.start_agent_job(&command)
                        && let Some(link) = &self.agent_chat.link
                    {
                        let _ = link.send(
                            &serde_json::json!({"event":"edit_result","ok":false,"text":error})
                                .to_string(),
                        );
                    }
                    continue;
                }
                if matches!(command["action"].as_str(), Some("search_instruments")) {
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
                if command["action"] == "listen" {
                    if let Err(error) = self.start_agent_listening(&command)
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
            if !self.agent_chat.busy {
                self.session.end_agent_request();
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
        if self.conversation_navigation_key(event, cx) {
            return true;
        }
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
        if !composing && focused == AgentField::Chat && self.agent_chat.controls.pending.is_some() {
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
    ) -> AnyElement {
        self.initialize_conversations();
        if self.agent_chat.conversations.showing_list || self.agent_chat.conversations.previewing()
        {
            return self.render_conversation_browser(window, cx);
        }
        let theme = self.theme.clone();
        let composer_locked = self.agent_chat.pending_send.is_some()
            || self.agent_chat.conversations.changing_conversation();
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
        if self.agent_operation_busy() {
            self.agent_chat.model_menu = false;
        }

        let header_section_focus = self
            .agent_chat
            .header_section_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        if self.agent_chat.conversations.managed() && self.agent_chat.follow_tail {
            self.agent_chat.conversations.transcript_start =
                self.agent_chat.entries.len().saturating_sub(CHAT_CAPACITY);
        }
        let transcript_start = self
            .agent_chat
            .conversations
            .transcript_start
            .min(self.agent_chat.entries.len());
        let rows: Vec<AnyElement> = transcript_runs(&self.agent_chat.entries[transcript_start..])
            .map(|run| {
                let run = (run.start + transcript_start)..(run.end + transcript_start);
                if run.len() > 1 {
                    self.chat_tool_group(run, &theme, window, cx)
                } else {
                    self.chat_row(
                        run.start,
                        &self.agent_chat.entries[run.start],
                        &theme,
                        window,
                        cx,
                    )
                }
            })
            .collect();
        let operation_busy = self.agent_operation_busy();
        let drop_bounds = self.agent_chat.drop_bounds.clone();
        let accepts_drop = !composer_locked;
        let accent = theme.accent;
        let drop_background = theme.accent_soft;
        div()
            .id("agent-panel")
            .debug_selector(|| "agent-panel".to_string())
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.0))
            .min_w_0()
            .overflow_hidden()
            .relative()
            .bg(theme.surface_sunken)
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                this.agent_attach_files(paths.paths(), cx);
                cx.stop_propagation();
            }))
            .group("agent-panel-drop")
            // Changes nothing itself; registering it is what repaints the panel as a drag
            // enters and leaves, so the overlay at the end follows the pointer.
            .drag_over::<gpui::ExternalPaths>(|style, _, _, _| style)
            .child(
                gpui::canvas(
                    move |bounds, _, _| {
                        drop_bounds.set(Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            // Dropdown triggers and options stop propagation; other panel clicks dismiss
            // the open menu before carrying on to their own control.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    if this.agent_chat.model_menu
                        || this.agent_chat.controls.mode_menu
                        || this.agent_chat.controls.effort_menu
                    {
                        this.agent_chat.model_menu = false;
                        this.agent_chat.controls.mode_menu = false;
                        this.agent_chat.controls.effort_menu = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .id("agent-panel-header")
                    .debug_selector(|| "agent-panel-header".to_string())
                    .flex()
                    .flex_col()
                    .relative()
                    .flex_shrink()
                    .min_h_0()
                    .max_h(px(180.0))
                    .overflow_y_scroll()
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
                            .child(button(
                                "agent-conversations",
                                self.t(Key::AgentConversations),
                                ButtonStyle::Ghost,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.agent_chat.conversations.showing_list = true;
                                    this.focus_agent_field(AgentField::ConversationSearch);
                                    cx.notify();
                                }),
                            ))
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
                            .child(
                                bounded_button_enabled(
                                    "agent-new-conversation",
                                    self.t(Key::AgentNewConversation),
                                    ButtonStyle::Normal,
                                    ButtonState::available(false, true),
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.start_new_agent_conversation(cx);
                                        cx.notify();
                                    }),
                                )
                                .w(px(160.0)),
                            )
                            .child(div().flex_1())
                            .child(self.render_active_conversation_actions(cx))
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
                    ),
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
                        .when(transcript_start > 0, |this| {
                            this.child(button(
                                "agent-earlier-messages",
                                self.t(Key::AgentEarlierMessages),
                                ButtonStyle::Ghost,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.agent_chat.conversations.transcript_start = this
                                        .agent_chat
                                        .conversations
                                        .transcript_start
                                        .saturating_sub(CHAT_CAPACITY);
                                    this.agent_chat.follow_tail = false;
                                    cx.notify();
                                }),
                            ))
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
            .when(
                operation_busy || self.agent_chat.tokens_in > 0 || self.agent_chat.tokens_out > 0,
                |this| this.child(self.agent_status_row(&theme)),
            )
            .child(self.agent_slash_completions(cx))
            .child(self.agent_input_row(input_focus, stop_focus, cx))
            // Drawn over everything, hidden until a drag carrying something attachable is over
            // the panel. A recolour of the panel itself was covered by the header and composer.
            .child(
                div()
                    .id("agent-drop-overlay")
                    .debug_selector(|| "agent-drop-overlay".to_string())
                    .absolute()
                    .inset_0()
                    .invisible()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .rounded(Metrics::RADIUS_LG)
                    .border_2()
                    .border_color(accent)
                    .bg(drop_background)
                    .text_sm()
                    .text_color(theme.text)
                    .child(self.t(Key::AgentDropFiles))
                    .can_drop(move |value, _, _| {
                        accepts_drop
                            && value
                                .downcast_ref::<gpui::ExternalPaths>()
                                .is_some_and(|paths| drop_offers_attachment(paths.paths()))
                    })
                    .group_drag_over::<gpui::ExternalPaths>("agent-panel-drop", |style| {
                        style.visible()
                    }),
            )
            .into_any_element()
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
        } else if self.agent_chat.controls.review.is_some() {
            Key::AgentReviewing
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

    /// A compact, keyboard-accessible summary of consecutive tool calls.
    fn chat_tool_group(
        &self,
        run: std::ops::Range<usize>,
        theme: &Theme,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let start = run.start;
        let opened = self.agent_chat.expanded_tool_groups.contains(&start);
        let mut running = 0;
        let mut failed = 0;
        for entry in &self.agent_chat.entries[run.clone()] {
            if let ChatEntry::Tool { ok, line, .. } = entry {
                if line.is_empty() {
                    running += 1;
                } else if !ok {
                    failed += 1;
                }
            }
        }
        let mut label = self
            .t(Key::AgentToolGroup)
            .replace("{count}", &run.len().to_string());
        for (count, key) in [
            (running, Key::AgentToolGroupRunning),
            (failed, Key::AgentToolGroupFailed),
        ] {
            if count > 0 {
                label.push_str(" · ");
                label.push_str(&self.t(key).replace("{count}", &count.to_string()));
            }
        }
        div()
            .id(("agent-tool-group", start))
            .debug_selector(move || format!("agent-tool-group-{start}"))
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_1p5()
            .py_0p5()
            .text_xs()
            .text_color(if failed > 0 {
                theme.danger
            } else {
                theme.text_muted
            })
            .child(
                disclosure(
                    SharedString::from(format!("agent-tool-group-toggle-{start}")),
                    label,
                    opened,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        if !this.agent_chat.expanded_tool_groups.remove(&start) {
                            this.agent_chat.expanded_tool_groups.insert(start);
                        }
                        cx.notify();
                    }),
                )
                .when(failed > 0, |this| this.text_color(theme.danger)),
            )
            .when(opened, |this| {
                this.child(
                    div()
                        .id(("agent-tool-group-body", start))
                        .debug_selector(move || format!("agent-tool-group-body-{start}"))
                        .flex()
                        .flex_col()
                        .mt_1()
                        .border_l_1()
                        .border_color(theme.border_subtle)
                        .children(run.map(|index| {
                            self.chat_row(index, &self.agent_chat.entries[index], theme, window, cx)
                        })),
                )
            })
            .into_any_element()
    }

    /// Opens a historical reference only after an explicit user activation.
    fn open_agent_attachment(&mut self, path: PathBuf, cx: &mut gpui::Context<Self>) {
        cx.spawn(async move |this, cx| {
            let checked_path = path.clone();
            let url = cx
                .background_executor()
                .spawn(async move {
                    url::Url::from_file_path(&checked_path)
                        .ok()
                        .filter(|_| checked_path.is_file())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(url) = url {
                    cx.open_url(url.as_str());
                } else {
                    this.agent_chat.push_entry(ChatEntry::Error(
                        this.t(Key::AgentAttachmentUnavailable)
                            .replace("{path}", &path.to_string_lossy()),
                    ));
                    cx.notify();
                }
            });
        })
        .detach();
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
        let opened = if self.agent_chat.conversations.previewing() {
            self.agent_chat
                .conversations
                .preview_expanded
                .contains(&index)
        } else {
            self.agent_chat.expanded.contains(&index)
        };
        let openable = matches!(entry, ChatEntry::Reasoning(_))
            || matches!(entry, ChatEntry::Tool { args, detail, .. } if !args.is_empty() || !detail.is_empty())
            || matches!(entry, ChatEntry::RestoredContext(detail) if !detail.is_empty());
        let (colour, text): (gpui::Hsla, String) = match entry {
            ChatEntry::You { text, .. } => (theme.accent_text, text.clone()),
            ChatEntry::Agent(text) => (theme.text, text.clone()),
            ChatEntry::Reasoning(reasoning) => (
                theme.text_muted,
                format!(
                    "✳ {}{}",
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
                (theme.text_muted, format!("{mark} {name}  {line}"))
            }
            ChatEntry::Error(message) => (theme.danger, message.clone()),
            ChatEntry::Note(key) => (note_colour(*key, theme), self.t(*key).to_string()),
        };
        let bordered = matches!(entry, ChatEntry::You { .. });
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
                    this.agent_chat.toggle_conversation_entry(index);
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
                    this.agent_chat.toggle_conversation_entry(index);
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
                    this.agent_chat.toggle_conversation_entry(index);
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
                    ChatEntry::You { attachments, .. } if !attachments.is_empty() => {
                        Some(attachments)
                    }
                    _ => None,
                },
                |this, attachments| {
                    this.child(
                        div()
                            .id(("agent-message-attachments", index))
                            .debug_selector(move || format!("agent-message-attachments-{index}"))
                            .w_full()
                            .min_w_0()
                            .mt_1()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .text_color(theme.text_muted)
                            .child(
                                div()
                                    .text_color(theme.text_faint)
                                    .child(self.t(Key::AgentAttachedFiles)),
                            )
                            .children(attachments.iter().enumerate().map(
                                |(attachment_index, path)| {
                                    let full_path = path.to_string_lossy().into_owned();
                                    // A saved conversation may have originated on another platform.
                                    let name = full_path
                                        .rsplit(['/', '\\'])
                                        .next()
                                        .unwrap_or(&full_path)
                                        .replace(['\r', '\n'], " ");
                                    let path = path.clone();
                                    let tip = self
                                        .t(Key::AgentOpenAttachment)
                                        .replace("{path}", &full_path);
                                    button_enabled(
                                        SharedString::from(format!(
                                            "agent-message-attachment-{index}-{attachment_index}"
                                        )),
                                        "",
                                        ButtonStyle::Ghost,
                                        ButtonState::available(false, true),
                                        theme.accent,
                                        theme,
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_agent_attachment(path.clone(), cx);
                                        }),
                                    )
                                    .w_full()
                                    .min_w_0()
                                    .flex_shrink_0()
                                    .justify_start()
                                    .child(div().min_w_0().truncate().child(name))
                                    .tooltip(crate::ui::tooltip::keyed_tip(tip, "", theme))
                                },
                            )),
                    )
                },
            )
            .when_some(
                match entry {
                    ChatEntry::Agent(text) | ChatEntry::You { text, .. } => Some(text.clone()),
                    _ => None,
                },
                |this, text| {
                    this.on_mouse_down(
                        MouseButton::Right,
                        Self::opens_menu(cx, move |this, at| {
                            ContextMenu::new(at, this.t(Key::AgentPanel)).item(
                                this.t(Key::AgentCopyMarkdown),
                                MenuCommand::CopyAgentMarkdown(text.clone()),
                            )
                        }),
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
        if self.agent_operation_busy() {
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
            .position(|option| option.matches(&self.agent_chat.preferences()))
            .unwrap_or(0);
        self.agent_chat.model_menu = true;
        self.agent_chat.controls.mode_menu = false;
        self.agent_chat.controls.effort_menu = false;
        self.agent_chat
            .model_scroll
            .scroll_to_item(self.agent_chat.model_highlighted + 1);
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
        let previous_preferences = self.agent_chat.preferences();
        if let Some(connection) = &option.connection {
            let mut prefs = previous_preferences.clone();
            self.agent_chat.discovery_endpoint.get_or_insert_with(|| {
                auris_session::AgentEndpoint {
                    provider: prefs.provider.clone(),
                    url: prefs.url.clone(),
                    api_key_env: prefs.api_key_env.clone(),
                }
            });
            connection.apply_to(&mut prefs);
            self.agent_chat.backend = prefs.backend;
            self.agent_chat.provider_openai = prefs.provider == "openai";
            self.agent_chat.url_field = TextField::new(prefs.url);
            self.agent_chat.key_env_field = TextField::new(prefs.api_key_env);
        }
        if option
            .efforts
            .as_ref()
            .is_some_and(|efforts| !efforts.contains(&self.agent_chat.effort))
        {
            self.agent_chat.effort = ReasoningEffort::Default;
        }
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
            self.agent_chat.backend = previous_preferences.backend;
            self.agent_chat.provider_openai = previous_preferences.provider == "openai";
            self.agent_chat.url_field = TextField::new(previous_preferences.url);
            self.agent_chat.key_env_field = TextField::new(previous_preferences.api_key_env);
            self.agent_chat.effort = previous_preferences.effort;
            self.agent_chat.discovery_endpoint = previous_preferences.discovery_endpoint;
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
        if self.agent_operation_busy() {
            self.agent_chat.model_menu = false;
            return false;
        }
        if key == "refresh" {
            self.agent_refresh_models();
            return true;
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
                .scroll_to_item(self.agent_chat.model_highlighted + 1);
        }
        true
    }

    /// Two quiet selectors sharing the model/effort lane of the composer.
    fn agent_model_picker(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let enabled = !self.agent_operation_busy();
        let configured = !self.agent_chat.chosen_model.is_empty();
        let focus = self
            .agent_chat
            .model_focus
            .get_or_insert_with(|| {
                cx.focus_handle()
                    .tab_index(Pane::Agent.tab_index() + 1)
                    .tab_stop(true)
            })
            .clone()
            .tab_stop(enabled);
        let current = if configured {
            self.agent_chat.selected_model_label()
        } else if self.agent_chat.fetching_models {
            self.t(Key::AgentModelsFetching).to_owned()
        } else {
            self.t(Key::AgentChooseModel).to_owned()
        };
        let efforts = self.agent_chat.effort_choices();
        let effort_names = efforts
            .iter()
            .map(|effort| self.t(controls::effort_key(*effort)).to_owned())
            .collect::<Vec<_>>();
        let effort = self.agent_chat.effort;
        let effort_label = self
            .t(match effort {
                ReasoningEffort::Default => Key::AgentEffortDefaultShort,
                ReasoningEffort::Minimal => Key::AgentEffortMinimalShort,
                ReasoningEffort::Medium => Key::AgentEffortMediumShort,
                ReasoningEffort::Xhigh => Key::AgentEffortXhighShort,
                ReasoningEffort::Max => Key::AgentEffortMaxShort,
                _ => controls::effort_key(effort),
            })
            .to_owned();
        div()
            .flex()
            .items_center()
            .gap_0p5()
            .w_full()
            .min_w_0()
            .child(div().flex_1().min_w_0().child(self.model_dropdown(
                current,
                self.agent_chat.model_menu,
                enabled,
                configured,
                focus,
                cx,
            )))
            .child(
                div()
                    .w(px(56.0))
                    .min_w_0()
                    .flex_shrink_0()
                    .child(self.dropdown(
                        "agent-effort-menu",
                        effort_label,
                        self.agent_chat.controls.effort_menu,
                        &theme,
                        |this, _| {
                            if !this.agent_operation_busy() {
                                this.agent_chat.controls.effort_menu =
                                    !this.agent_chat.controls.effort_menu;
                                this.agent_chat.controls.mode_menu = false;
                                this.agent_chat.model_menu = false;
                            }
                        },
                        cx,
                    )),
            )
            .when(self.agent_chat.model_menu && enabled, |this| {
                let names = self
                    .agent_chat
                    .models
                    .iter()
                    .map(ModelOption::label)
                    .collect::<Vec<_>>();
                this.child(self.model_option_rows(&names, &theme, cx))
            })
            .when(self.agent_chat.controls.effort_menu, |this| {
                this.child(
                    self.option_rows(
                        "agent-effort-option",
                        &effort_names,
                        efforts
                            .iter()
                            .position(|choice| *choice == effort)
                            .unwrap_or(0),
                        &theme,
                        |this, index, _| {
                            if let Some(effort) =
                                this.agent_chat.effort_choices().get(index).copied()
                            {
                                this.agent_effort(effort);
                            }
                        },
                        cx,
                    ),
                )
            })
            .into_any_element()
    }
    /// A focusable, quiet model value beside the effort selector's disclosure indicator.
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
                theme.surface_raised
            })
            .border_1()
            .border_color(match open {
                true => theme.accent,
                false => gpui::transparent_black(),
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
            // Running a turn locks the route while keeping its selected name readable.
            .when(!enabled, |this| this.opacity(0.78))
            .child(
                div()
                    .id("agent-model-current")
                    .debug_selector(|| "agent-model-current".to_string())
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .text_color(if configured {
                        theme.text
                    } else {
                        theme.text_faint
                    })
                    .child(bounded_picker_label(current)),
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
            .debug_selector(|| "agent-model-options".into())
            .flex()
            .flex_col()
            .max_h(px(240.0))
            .p_1()
            .overflow_y_scroll()
            .track_scroll(&self.agent_chat.model_scroll)
            .w_full()
            .min_w_0()
            .rounded(Metrics::RADIUS_SM)
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_raised)
            .shadow_lg()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                |_: &MouseDownEvent, _, cx: &mut gpui::App| {
                    cx.stop_propagation();
                },
            )
            .child(
                button_enabled(
                    "agent-models-refresh",
                    self.t(Key::AgentModelsFetch),
                    ButtonStyle::Ghost,
                    ButtonState::available(false, !self.agent_chat.fetching_models),
                    theme.accent,
                    theme,
                    cx.listener(|this, _, window, cx| {
                        this.agent_refresh_models();
                        if let Some(focus) = &this.agent_chat.model_focus {
                            window.focus(focus);
                        }
                        cx.notify();
                    }),
                )
                .w_full()
                .flex_shrink_0()
                .tooltip(crate::ui::tooltip::keyed_tip(
                    self.t(Key::AgentModelsFetch),
                    &crate::actions::menu_keystroke("secondary-r"),
                    theme,
                )),
            )
            .when(self.agent_chat.fetching_models, |this| {
                this.child(
                    div()
                        .id("agent-models-fetching")
                        .debug_selector(|| "agent-models-fetching".to_owned())
                        .px_1p5()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::AgentModelsFetching)),
                )
            })
            .when_some(self.agent_chat.models_error.clone(), |this, error| {
                this.child(
                    div()
                        .px_1p5()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .when(
                self.agent_chat.models_loaded
                    && names.is_empty()
                    && self.agent_chat.models_error.is_none()
                    && !self.agent_chat.fetching_models,
                |this| {
                    this.child(
                        div()
                            .id("agent-models-empty")
                            .debug_selector(|| "agent-models-empty".to_owned())
                            .px_1p5()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(self.t(Key::AgentModelsEmpty)),
                    )
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
                    .flex_shrink_0()
                    .rounded(Metrics::RADIUS_SM)
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
                    .child(div().w(px(12.0)).flex_shrink_0().when(
                        self.agent_chat.models[index].matches(&self.agent_chat.preferences()),
                        |this| this.child(icon(Icon::Check, px(12.0), theme.accent)),
                    ))
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
        Self::agent_dropdown_popup(list)
    }

    /// Paints a dropdown above the conversation without changing the controls' layout.
    fn agent_dropdown_popup(list: impl IntoElement) -> AnyElement {
        gpui::deferred(
            div()
                .absolute()
                .bottom_full()
                .left_0()
                .w_full()
                .pb_1()
                .child(list),
        )
        .into_any_element()
    }

    /// A compact secondary control using the same field style as other pickers.
    fn dropdown(
        &mut self,
        id: &'static str,
        current: String,
        open: bool,
        theme: &Theme,
        toggle: impl Fn(&mut Self, &mut gpui::Context<Self>) + 'static,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let tooltip = format!(
            "{}: {current}",
            self.t(if id == "agent-mode-menu" {
                Key::AgentApprovalMode
            } else {
                Key::AgentEffort
            })
        );
        let focus = if id == "agent-mode-menu" {
            &mut self.agent_chat.controls.mode_focus
        } else {
            &mut self.agent_chat.controls.effort_focus
        }
        .get_or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
        .clone()
        .tab_stop(!self.agent_operation_busy());
        let warns =
            id == "agent-mode-menu" && controls::mode_warns(self.settings.agent.policy.mode);
        menu_picker(
            id,
            bounded_picker_label(current),
            theme,
            cx.listener(move |this, _, _, cx| {
                toggle(this, cx);
                cx.notify();
            }),
        )
        .w_full()
        .when(id == "agent-effort-menu", |this| this.px_1().gap_0p5())
        .bg(theme.surface_raised)
        .border_color(gpui::transparent_black())
        .track_focus(&focus)
        .key_context("AurisAgentDropdown")
        .on_action(|_: &crate::ui::widgets::ClaimDisclosureActivation, _, _| {})
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                if let Some(field) = this.agent_chat.field_mut() {
                    field.unmark();
                }
                this.agent_chat.focused = None;
                window.focus(&focus);
                cx.stop_propagation();
            }),
        )
        .when(warns, |this| this.text_color(theme.warning))
        .when(warns && !open, |this| this.border_color(theme.warning))
        .when(open, |this| this.border_color(theme.accent))
        .on_action(cx.listener(|this, _: &NavigateAgentModel, _, cx| {
            this.agent_chat.controls.mode_menu = false;
            this.agent_chat.controls.effort_menu = false;
            cx.stop_propagation();
            cx.notify();
        }))
        .tooltip(crate::ui::tooltip::keyed_tip(tooltip, "", theme))
        .into_any_element()
    }

    /// The keyboard-accessible choices in a secondary dropdown.
    fn option_rows(
        &self,
        id: &'static str,
        names: &[String],
        selected: usize,
        theme: &Theme,
        pick: impl Fn(&mut Self, usize, &mut gpui::Context<Self>) + Clone + 'static,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let mut list = div()
            .id((id, usize::MAX))
            .debug_selector(move || format!("{id}-popup"))
            .flex()
            .flex_col()
            .p_1()
            .max_h(px(160.0))
            .overflow_y_scroll()
            .rounded(Metrics::RADIUS_SM)
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_raised)
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
            });
        for (index, name) in names.iter().enumerate() {
            let pick = pick.clone();
            let permissions = id == "agent-mode-option" && index + 1 == names.len();
            let row_id: gpui::ElementId = if permissions {
                "agent-permissions".into()
            } else {
                (id, index).into()
            };
            list = list.child(
                button(
                    row_id,
                    name.clone(),
                    ButtonStyle::Ghost,
                    false,
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, window, cx| {
                        pick(this, index, cx);
                        if permissions && let Some(focus) = &this.agent_chat.controls.mode_focus {
                            window.focus(focus);
                        }
                        cx.notify();
                    }),
                )
                .w_full()
                .flex_shrink_0()
                .justify_start()
                .gap_1()
                .when(index == selected, |this| {
                    this.child(icon(Icon::Check, px(12.0), theme.accent))
                }),
            );
        }
        Self::agent_dropdown_popup(list)
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

    /// A single composer surface containing references, prose and a compact command row.
    fn agent_input_row(
        &mut self,
        input_focus: gpui::FocusHandle,
        stop_focus: gpui::FocusHandle,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let focused = self.agent_chat.focused == Some(AgentField::Chat);
        let locked = self.agent_chat.pending_send.is_some()
            || self.agent_chat.conversations.changing_conversation();
        let empty = self.agent_chat.input.content().is_empty();
        let placeholder = self.t(Key::AgentComposerPlaceholder).to_owned();
        let attachments = self.agent_chat.attachments.clone();
        let blocker = self.agent_send_blocker();
        let can_send = !self.agent_operation_busy()
            && blocker.is_none()
            && !self.agent_chat.input.content().trim().is_empty()
            && self.agent_chat.input.marked().is_none();
        let cancelable = self.agent_cancelable();
        let send_label = self.t(Key::AgentSend).to_owned();
        let bypass = controls::mode_warns(self.settings.agent.policy.mode);
        div()
            .id("agent-composer")
            .debug_selector(|| "agent-composer".to_owned())
            .flex()
            .flex_col()
            .relative()
            .flex_shrink_0()
            .min_w_0()
            .m_2()
            .p_1()
            .gap_1()
            .rounded(Metrics::RADIUS_LG)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(if focused && !locked {
                theme.accent
            } else if bypass {
                theme.warning
            } else {
                theme.border_subtle
            })
            .when(bypass, |this| {
                this.child(
                    div()
                        .id("agent-bypass-warning")
                        .debug_selector(|| "agent-bypass-warning".to_owned())
                        .px_1p5()
                        .text_xs()
                        .text_color(theme.warning)
                        .child(format!("⚠ {}", self.t(Key::AgentBypassWarning))),
                )
            })
            .when_some(blocker, |this, blocker| {
                let (message, action) = match blocker {
                    SendBlocker::PendingChanges => (Key::AgentReloadOffer, Key::AgentReload),
                    SendBlocker::NoModel => (Key::AgentNotConfigured, Key::AgentChooseModel),
                };
                this.child(
                    div()
                        .id("agent-send-blocked")
                        .debug_selector(|| "agent-send-blocked".to_owned())
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .text_xs()
                        .text_color(theme.warning)
                        .child(div().flex_1().min_w(px(80.0)).child(self.t(message)))
                        .child(button(
                            "agent-send-blocked-action",
                            self.t(action),
                            ButtonStyle::Normal,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, window, cx| {
                                match blocker {
                                    SendBlocker::PendingChanges => this.agent_reload(cx),
                                    SendBlocker::NoModel => {
                                        this.open_agent_model_menu();
                                        if let Some(focus) = &this.agent_chat.model_focus {
                                            window.focus(focus);
                                        }
                                    }
                                }
                                cx.notify();
                            }),
                        )),
                )
            })
            .when_some(self.agent_chat.attachment_error.clone(), |this, error| {
                this.child(
                    div()
                        .px_1p5()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .when(
                matches!(
                    self.settings.agent.backend.as_str(),
                    "codex" | "claude" | "antigravity"
                ) && attachments.iter().any(|path| {
                    auris_agent::attachment_kind(path) == Some(auris_agent::AttachmentKind::Audio)
                }),
                |this| {
                    this.child(
                        div()
                            .px_1p5()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(self.t(Key::AgentAttachmentAudioAnalysisHint)),
                    )
                },
            )
            .when(!attachments.is_empty(), |this| {
                this.child(
                    div()
                        .id("agent-attachments")
                        .flex()
                        .flex_col()
                        .max_h(px(96.0))
                        .overflow_y_scroll()
                        .gap_1()
                        .children(attachments.iter().enumerate().map(|(index, path)| {
                            let name = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            let remove_tip =
                                self.t(Key::AgentRemoveAttachment).replace("{name}", &name);
                            let open_tip = self
                                .t(Key::AgentOpenAttachment)
                                .replace("{path}", &path.to_string_lossy());
                            let path = path.clone();
                            // Only the cross removes. The whole chip used to, so a click meant to
                            // check which file this was threw the reference away.
                            div()
                                .id(("agent-attachment", index))
                                .debug_selector(move || format!("agent-attachment-{index}"))
                                .flex()
                                .items_center()
                                .w_full()
                                .min_w_0()
                                .flex_shrink_0()
                                .rounded(Metrics::RADIUS_SM)
                                .bg(theme.surface_sunken)
                                .child(
                                    button_enabled(
                                        ("agent-attachment-open", index),
                                        "",
                                        ButtonStyle::Ghost,
                                        ButtonState::available(false, true),
                                        theme.accent,
                                        &theme,
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_agent_attachment(path.clone(), cx);
                                        }),
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .justify_start()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .h_full()
                                            .child(bounded_picker_label(name)),
                                    )
                                    .tooltip(crate::ui::tooltip::keyed_tip(open_tip, "", &theme)),
                                )
                                .child(
                                    button_enabled(
                                        ("agent-attachment-remove", index),
                                        "",
                                        ButtonStyle::Ghost,
                                        ButtonState::available(false, !locked),
                                        theme.accent,
                                        &theme,
                                        cx.listener(move |this, _, _, cx| {
                                            if index < this.agent_chat.attachments.len() {
                                                this.agent_chat.attachments.remove(index);
                                                this.agent_chat.attachment_error = None;
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .w(Metrics::CONTROL_HEIGHT)
                                    .flex_shrink_0()
                                    .px_0()
                                    .gap_0()
                                    .justify_center()
                                    .child(icon(Icon::Cross, px(12.0), theme.text_muted))
                                    .tooltip(crate::ui::tooltip::keyed_tip(remove_tip, "", &theme)),
                                )
                        })),
                )
            })
            .when(locked, |this| {
                this.child(
                    div()
                        .id("agent-send-waiting-history")
                        .debug_selector(|| "agent-send-waiting-history".to_owned())
                        .px_1p5()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::AgentSendWaitingHistory)),
                )
            })
            .child(self.panel_field(
                "agent-input",
                AgentField::Chat,
                focused,
                empty,
                placeholder,
                input_focus,
                !locked,
                &theme,
                cx,
            ))
            .child(
                div()
                    .id("agent-panel-controls")
                    .debug_selector(|| "agent-panel-controls".to_owned())
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .w(relative(0.26))
                            .min_w_0()
                            .child(self.agent_controls(cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(80.0))
                            .child(self.agent_model_picker(cx)),
                    )
                    .when_else(
                        cancelable,
                        |this| {
                            this.child(
                                button(
                                    "agent-stop",
                                    "",
                                    ButtonStyle::Normal,
                                    false,
                                    theme.warning,
                                    &theme,
                                    cx.listener(|this, _, window, cx| {
                                        this.agent_cancel(window, cx)
                                    }),
                                )
                                .track_focus(&stop_focus)
                                .w(Metrics::CONTROL_HEIGHT)
                                .px_0()
                                .gap_0()
                                .justify_center()
                                .rounded_full()
                                .child(icon(Icon::Stop, px(12.0), theme.text))
                                .tooltip(
                                    crate::ui::tooltip::keyed_tip(
                                        self.t(Key::AgentStop),
                                        "",
                                        &theme,
                                    ),
                                ),
                            )
                        },
                        |this| {
                            this.child(
                                button_enabled(
                                    "agent-send",
                                    "",
                                    if can_send {
                                        ButtonStyle::Primary
                                    } else {
                                        ButtonStyle::Ghost
                                    },
                                    ButtonState::available(false, can_send),
                                    theme.accent,
                                    &theme,
                                    cx.listener(|this, _, window, cx| {
                                        this.agent_submit(window, cx);
                                        cx.notify();
                                    }),
                                )
                                .w(Metrics::CONTROL_HEIGHT)
                                .px_0()
                                .gap_0()
                                .justify_center()
                                .rounded_full()
                                .child(icon(
                                    Icon::ChevronUp,
                                    px(14.0),
                                    if can_send {
                                        theme.text_on_accent
                                    } else {
                                        theme.text_faint
                                    },
                                ))
                                .tooltip(crate::ui::tooltip::keyed_tip(send_label, "", &theme)),
                            )
                        },
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
            AgentField::ConversationSearch => &self.agent_chat.conversations.search,
        };
        let text = value.content().to_string();
        let selection = value.selection();
        let composing = value.marked().is_some();
        let marked = value.marked();
        let view = cx.entity();
        let pointer_focus = focus.clone();
        let (minimum, maximum) = if field == AgentField::ConversationSearch {
            let height = crate::ui::text_area::area_height("", 1, 1);
            (height, height)
        } else {
            (
                crate::ui::text_area::area_height("", 2, 6),
                crate::ui::text_area::area_height("a\na\na\na\na\na", 2, 6),
            )
        };

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

    #[test]
    fn pending_vocals_cancel_on_document_switch_policy_revocation_and_drop() {
        for (revision, same_document, mode) in [
            (2, true, Mode::Edit),
            (1, false, Mode::Edit),
            (1, true, Mode::ReadOnly),
        ] {
            let (_sender, receiver) = std::sync::mpsc::channel();
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let operation = auris_session::agent_policy::Operation::parse(
                "edit_project",
                &serde_json::json!({"command":{"action":"sing","track":1}}),
            )
            .unwrap();
            let pending = PendingAgentJob {
                revision: 1,
                operation,
                cancel: cancel.clone(),
                receiver,
            };
            assert!(
                pending
                    .poll(
                        revision,
                        same_document,
                        &auris_session::agent_policy::Policy {
                            mode,
                            ..Default::default()
                        }
                    )
                    .unwrap()
                    .is_err()
            );
            assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        }
        let (_sender, receiver) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let operation = auris_session::agent_policy::Operation::parse(
            "edit_project",
            &serde_json::json!({"command":{"action":"sing","track":1}}),
        )
        .unwrap();
        let pending = PendingAgentJob {
            revision: 1,
            operation,
            cancel: cancel.clone(),
            receiver,
        };
        assert!(pending.poll(1, true, &Default::default()).is_none());
        drop(pending);
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[gpui::test]
    fn live_harmony_and_lyrics_edit_the_open_window_and_undo_independently(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this,_| {
            this.settings.agent.policy.mode = Mode::Edit;
            let track = this.session.add_singer_track("Vocal");
            let clip = this.session.add_midi_clip(track,"Phrase",Ticks::ZERO,Ticks::QUARTER * 4).unwrap();
            this.session.add_note(clip,Note::new(62,Ticks::ZERO,Ticks::QUARTER)).unwrap();
            let before = this.project().clone();
            this.agent_edit(serde_json::json!({"action":"edit_harmony","start_bar":1,"bars":1,"key":"D major","chords":"| I |"})).unwrap();
            assert_eq!(this.project().harmony.chord_at(Ticks::ZERO).unwrap().to_string(),"D");
            let harmony = this.project().clone();
            this.agent_edit(serde_json::json!({"action":"write_lyrics","clip":clip.0,"text":"あ"})).unwrap();
            assert_eq!(this.project().midi_clip(clip).unwrap().1.notes[0].lyric,"あ");
            this.session.undo();
            assert_eq!(this.project(),&harmony);
            this.session.undo();
            assert_eq!(this.project(),&before);
            this.settings.agent.policy.mode = Mode::ReadOnly;
            assert!(this.agent_edit(serde_json::json!({"action":"write_lyrics","clip":clip.0,"text":"い"})).is_err());
            assert_eq!(this.project(),&before);
        });
    }

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
        assert!(
            cx.debug_bounds("agent-mode-menu-dropdown-indicator")
                .is_some()
        );
        assert!(cx.debug_bounds("agent-mode-option-1").is_none());
        let controls = cx.debug_bounds("agent-panel-controls").unwrap();
        crate::harness::click("agent-mode-menu", cx);
        assert!(cx.debug_bounds("agent-mode-option-1").is_some());
        assert_eq!(cx.debug_bounds("agent-panel-controls").unwrap(), controls);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .controls
                        .mode_focus
                        .as_ref()
                        .unwrap()
                        .is_focused(window),
                    "the mode trigger retains focus after opening"
                );
            });
        });
        cx.simulate_keystrokes("escape");
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(
                !this.agent_chat.controls.mode_menu,
                "Escape dismisses the mode menu state"
            )
        });
        cx.simulate_keystrokes("tab");
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .controls
                        .effort_focus
                        .as_ref()
                        .unwrap()
                        .is_focused(window)
                );
            });
        });
        cx.simulate_keystrokes("shift-tab");
        crate::harness::paint(&app, cx);
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.controls.mode_menu));
        crate::harness::click("agent-mode-option-1", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.policy.mode, Mode::Edit)
        });
    }

    #[gpui::test]
    fn manual_compaction_is_available_through_the_composer(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.model.clear();
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.agent_chat.input = TextField::new("/compact");
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-compact").is_none());
        crate::harness::click("agent-send", cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.input.content().is_empty());
            assert!(
                this.agent_chat
                    .entries
                    .contains(&ChatEntry::Note(Key::AgentCompactEmpty))
            );
        });
    }

    #[gpui::test]
    fn local_permissions_command_can_run_while_external_changes_wait(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let pending = PathBuf::from("Song/Song.auris");
        app.update(cx, |this, _| {
            this.settings.agent.model.clear();
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.agent_chat.pending_reload = Some(pending.clone());
            this.agent_chat.controls.rules_open = false;
            this.agent_chat.input = TextField::new("/permissions");
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-send-blocked").is_none());
        crate::harness::click("agent-send", cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.input.content().is_empty());
            assert!(this.agent_chat.controls.rules_open);
            assert_eq!(this.agent_chat.pending_reload, Some(pending));
            assert!(this.agent_chat.link.is_none());
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
            this.agent_chat
                .push_entry(ChatEntry::Agent("Other answer".into()));
            this.agent_chat.push_entry(ChatEntry::Agent(answer.into()));
        });
        crate::harness::resize(&app, cx, gpui::size(px(900.), px(600.)));
        assert!(cx.debug_bounds("agent-copy-1").is_none());
        let message = cx.debug_bounds("agent-line-1").unwrap();
        crate::harness::right_press(cx, message.center());
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            let menu = this.menu.as_ref().expect("message context menu is open");
            assert!(matches!(
                &menu.entries[..],
                [crate::ui::context_menu::MenuEntry::Item(item)]
                    if item.label == this.t(Key::AgentCopyMarkdown)
            ));
        });
        crate::harness::click("menu-item-0", cx);
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
    fn message_menu_copies_the_clicked_request_even_after_transcript_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let request = "Play **this** clip.\n\nKeep `120 BPM`.";
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "test-model".into();
            this.agent_chat.entries = vec![ChatEntry::user(request.into())];
        });
        crate::harness::paint(&app, cx);
        let message = cx.debug_bounds("agent-line-0").unwrap();
        crate::harness::right_press(cx, message.center());
        crate::harness::paint(&app, cx);
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| assert!(this.menu.is_none()));

        crate::harness::right_press(cx, message.center());
        app.update(cx, |this, _| {
            this.agent_chat.entries = vec![ChatEntry::Agent("Another message".into())];
        });
        crate::harness::paint(&app, cx);
        cx.simulate_keystrokes("down enter");
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some(request)
            );
        });
        app.read_with(cx, |this, _| assert!(this.menu.is_none()));
    }

    #[gpui::test]
    fn composer_actions_preserve_drafts_and_open_configuration(cx: &mut gpui::TestAppContext) {
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
        crate::harness::click("agent-stop", cx);
        app.read_with(cx, |this, _| {
            assert!(!this.agent_chat.busy);
            assert_eq!(this.agent_chat.input.content(), "再生してみて。");
            assert_eq!(
                this.agent_chat.entries,
                vec![ChatEntry::Note(Key::AgentStopped)]
            );
        });

        // Without a model the composer says so before the attempt: the button is off and the
        // reason sits beside it with the way out.
        app.update(cx, |this, _| this.agent_chat.entries.clear());
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-send-blocked").is_some());
        crate::harness::click("agent-send", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.input.content(), "再生してみて。");
            assert!(this.agent_chat.entries.is_empty());
        });
        crate::harness::click("agent-send-blocked-action", cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.model_menu));

        // Enter still goes through the send path and says why out loud.
        app.update(cx, |this, _| {
            this.agent_chat.model_menu = false;
            this.agent_send();
        });
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
                ChatEntry::user("再生してみて。".into()),
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
        let before = app.update(cx, |this, _| {
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
            before
        });
        crate::harness::paint(&app, cx);
        cx.simulate_keystrokes("secondary-z");
        app.read_with(cx, |this, _| assert_eq!(this.project(), &before));
    }

    #[gpui::test]
    fn live_transcription_import_respects_read_only_and_keyboard_undo(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let before = app.update(cx, |this,_| {
            let before = this.project().clone();
            let command = serde_json::json!({"action":"import_transcription","name":"Piano cover","at_tick":0,
                "draft":{"kind":"piano","transcription":{"source":{"method":"external"},"seconds":1.0,
                "notes":[{"pitch":60,"start":0.0,"end":1.0,"strength":0.7}]}}});
            this.settings.agent.policy = Default::default();
            this.settings.agent.policy.mode = Mode::ReadOnly;
            assert!(this.agent_edit(command.clone()).is_err());
            assert_eq!(this.project(),&before);
            this.settings.agent.policy.mode = Mode::Edit;
            this.agent_edit(command).unwrap();
            let piano = this.project().tracks.last().unwrap().kind.as_instrument().unwrap();
            assert_eq!(piano.instrument_id,"auris.physical.piano");
            assert_eq!(piano.clips[0].notes.len(),1);
            assert!(this.session.path().is_none());
            assert!(this.session.is_dirty());
            before
        });
        crate::harness::paint(&app, cx);
        cx.simulate_keystrokes("secondary-z");
        app.read_with(cx, |this, _| assert_eq!(this.project(), &before));
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
    fn listening_cancels_on_document_change_stop_or_denied_policy() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let operation = auris_session::agent_policy::Operation::parse(
            "edit_project",
            &serde_json::json!({"command":{
                "action":"listen","start_bar":1,"bars":1
            }}),
        )
        .unwrap();
        let pending = PendingListening {
            revision: 7,
            operation,
            cancel: cancel.clone(),
            receiver,
        };
        let mut policy = auris_session::agent_policy::Policy::default();
        assert!(pending.poll(7, true, &policy).is_none());
        policy.set_rule("edit_project.listen", Some(false)).unwrap();
        assert!(pending.poll(7, true, &policy).unwrap().is_err());
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        policy.set_rule("edit_project.listen", None).unwrap();
        assert!(pending.poll(8, true, &policy).unwrap().is_err());
        assert!(pending.poll(7, false, &policy).unwrap().is_err());
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        drop(pending);
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
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
                    ChatEntry::user("current request".into()),
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
            this.agent_chat.attachments = vec![PathBuf::from("reference.png")];
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
                    [PathBuf::from("reference.png")].as_slice()
                ))
            );

            // Even a programmatic mutation cannot substitute a later composer value or a
            // second Send for the immutable submission already waiting on disk history. This
            // also models another Settings window and a canvas click changing live state.
            this.agent_chat.input = TextField::new("later mutation");
            this.agent_chat.attachments = vec![PathBuf::from("later.png")];
            this.settings.agent.model = "changed-in-another-window".into();
            this.settings.agent.output_tokens = Some(8_192);
            this.settings.agent.policy.mode = auris_session::agent_policy::Mode::Bypass;
            this.selected_track = None;
            this.agent_send();
            let pending = this.agent_chat.pending_send.as_ref().unwrap();
            assert_eq!(pending.text, "continue the saved conversation");
            assert_eq!(pending.attachments, vec![PathBuf::from("reference.png")]);
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
        let current = ChatEntry::You {
            text: "continue".into(),
            attachments: vec![PathBuf::from("reference.wav")],
        };
        chat.push_entry(current.clone());
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
                ChatEntry::user("old request".into()),
                ChatEntry::Agent("old answer".into()),
                current
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
                ChatEntry::user("Project A secret".into()),
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
    fn consecutive_tools_group_without_crossing_message_boundaries() {
        let tool = || ChatEntry::Tool {
            name: "inspect".into(),
            args: "{}".into(),
            ok: true,
            line: "done".into(),
            detail: String::new(),
        };
        let entries = [
            tool(),
            tool(),
            ChatEntry::Agent("answer".into()),
            tool(),
            ChatEntry::user("next request".into()),
            tool(),
            tool(),
            tool(),
        ];
        assert_eq!(
            transcript_runs(&entries).collect::<Vec<_>>(),
            vec![0..2, 2..3, 3..4, 4..5, 5..8]
        );
        assert_eq!(transcript_runs(&[]).count(), 0);
    }

    #[gpui::test]
    fn consecutive_tool_logs_collapse_and_keep_individual_results_accessible(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.agent_chat.entries = (0..40)
                .map(|index| ChatEntry::Tool {
                    name: format!("inspect_{index}"),
                    args: "{\"bar\":2}".into(),
                    ok: index != 1,
                    line: if index == 39 {
                        String::new()
                    } else {
                        "finished".into()
                    },
                    detail: "Full tool result\nsecond line".into(),
                })
                .collect();
            this.agent_chat
                .entries
                .push(ChatEntry::Agent("Visible answer".into()));
        });
        crate::harness::paint(&app, cx);
        let group = cx.debug_bounds("agent-tool-group-0").unwrap();
        assert!(group.size.height < Metrics::CONTROL_HEIGHT * 2.0);
        assert!(cx.debug_bounds("agent-line-0").is_none());
        assert!(cx.debug_bounds("agent-line-40").is_some());

        crate::harness::click("agent-tool-group-toggle-0", cx);
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-tool-group-body-0").is_some());
        let collapsed = cx.debug_bounds("agent-line-0").unwrap().size.height;
        crate::harness::click("agent-tool-result-0", cx);
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-line-0").unwrap().size.height > collapsed);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.expanded_tool_groups.contains(&0));
            assert!(this.agent_chat.expanded.contains(&0));
        });

        crate::harness::click("agent-tool-group-toggle-0", cx);
        release_key("space", cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.expanded_tool_groups.contains(&0))
        });
        release_key("enter", cx);
        app.read_with(cx, |this, _| {
            assert!(!this.agent_chat.expanded_tool_groups.contains(&0))
        });
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
                ..Default::default()
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
                    ..Default::default()
                },
                ModelOption {
                    name: "second".to_string(),
                    context_length: Some(65_536),
                    ..Default::default()
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
    fn discovered_models_switch_the_route_even_when_names_are_identical(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent = AgentPreferences { model: "shared".into(), provider: "openai".into(), url: "https://example.invalid/v1".into(), ..Default::default() };
            this.agent_chat.load_preferences(&this.settings.agent.clone());
            this.agent_chat.accept_model_listing(Ok(r#"{"models":[{"name":"shared","connection":{"backend":"codex","provider":"openai","url":"","api_key_env":""}},{"name":"shared","connection":{"backend":"rig","provider":"ollama","url":"","api_key_env":""}}]}"#.into()));
            this.choose_agent_model_with(0, |_| Ok::<(), ()>(())).unwrap();
            assert_eq!(this.settings.agent.backend, "codex");
            assert_eq!(this.agent_chat.models[0].label(), "shared (Codex)");
            this.choose_agent_model_with(1, |_| Ok::<(), ()>(())).unwrap();
            assert_eq!(this.settings.agent.provider, "ollama");
            assert_eq!(this.settings.agent.discovery_endpoint.as_ref().unwrap().url, "https://example.invalid/v1");
            assert!(!this.agent_chat.models[0].matches(&this.settings.agent));
            assert!(this.agent_chat.models[1].matches(&this.settings.agent));
        });
    }

    #[gpui::test]
    fn claude_models_keep_their_route_and_executable_when_selected(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent = AgentPreferences {
                claude_path: "C:/Tools/claude.exe".into(),
                provider: "openai".into(),
                url: "https://example.invalid/v1".into(),
                ..Default::default()
            };
            this.agent_chat.load_preferences(&this.settings.agent.clone());
            this.agent_chat.accept_model_listing(Ok(r#"{"models":[{"name":"claude-fixture","connection":{"backend":"claude","provider":"openai","url":"","api_key_env":""}}]}"#.into()));
            this.choose_agent_model_with(0, |_| Ok::<(), ()>(())).unwrap();
            assert_eq!(this.settings.agent.backend, "claude");
            assert_eq!(this.settings.agent.model, "claude-fixture");
            assert_eq!(this.agent_chat.selected_model_label(), "claude-fixture (Claude Code)");
            assert!(this.agent_chat.models[0].matches(&this.settings.agent));
            assert_eq!(this.agent_chat.preferences().claude_path, "C:/Tools/claude.exe");
            assert_eq!(this.settings.agent.discovery_endpoint.as_ref().unwrap().url, "https://example.invalid/v1");
        });
    }

    #[gpui::test]
    fn antigravity_models_switch_the_runtime_and_keep_the_api_route(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent = AgentPreferences {
                antigravity_path: "C:/Tools/agy.exe".into(),
                provider: "openai".into(), url: "https://example.invalid/v1".into(),
                effort: ReasoningEffort::Xhigh,
                ..Default::default()
            };
            this.agent_chat.load_preferences(&this.settings.agent.clone());
            this.agent_chat.accept_model_listing(Ok(r#"{"models":[{"name":"agy-fixture","efforts":["low","medium","high"],"connection":{"backend":"antigravity","provider":"openai","url":"","api_key_env":""}}]}"#.into()));
            this.choose_agent_model_with(0, |_| Ok::<(), ()>(())).unwrap();
            assert_eq!(this.settings.agent.backend, "antigravity");
            assert_eq!(this.settings.agent.effort, ReasoningEffort::Default);
            assert_eq!(this.agent_chat.selected_model_label(), "agy-fixture (Antigravity CLI)");
            assert_eq!(this.agent_chat.preferences().antigravity_path, "C:/Tools/agy.exe");
            assert_eq!(this.settings.agent.url, "https://example.invalid/v1");
            assert_eq!(this.agent_chat.effort_choices(), vec![ReasoningEffort::Default, ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High]);
        });
    }

    #[gpui::test]
    fn agent_file_drops_attach_references_without_importing_audio(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
        });
        crate::harness::paint(&app, cx);
        let panel = cx.debug_bounds("agent-panel").unwrap();
        cx.simulate_mouse_move(panel.center(), None, gpui::Modifiers::default());
        cx.update(|window, cx| {
            app.update(cx, |this, cx| {
                let before = this.project().clone();
                let paths = [
                    "reference.wav",
                    "cover.png",
                    "lyrics.md",
                    "cover.png",
                    "unsupported.exe",
                ]
                .map(PathBuf::from);
                this.route_files_dropped(&paths, window, cx);
                assert_eq!(this.project(), &before);
                assert_eq!(this.agent_chat.attachments.len(), 3);
                assert!(this.agent_chat.attachment_error.is_some());
            })
        });
        crate::harness::paint(&app, cx);
        // Clicking a reference's name inspects it; only its cross takes it off the message.
        crate::harness::click("agent-attachment-open-1", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.attachments.len(), 3)
        });
        crate::harness::click("agent-attachment-remove-1", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.agent_chat.attachments,
                [PathBuf::from("reference.wav"), PathBuf::from("lyrics.md")]
            )
        });
    }

    #[gpui::test]
    fn model_and_effort_controls_stay_below_the_composer_and_menus_open_upward(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "gpt-6.1-sol".into();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models = vec![ModelOption {
                name: "gpt-6.1-sol".into(),
                ..Default::default()
            }];
        });
        crate::harness::paint(&app, cx);
        let composer = cx.debug_bounds("agent-composer").unwrap();
        let model = cx.debug_bounds("agent-model").unwrap();
        let effort = cx.debug_bounds("agent-effort-menu").unwrap();
        assert!(model.top() >= cx.debug_bounds("agent-input").unwrap().bottom());
        assert_eq!(model.top(), effort.top());
        assert!(model.bottom() <= composer.bottom());
        crate::harness::click("agent-model", cx);
        crate::harness::paint(&app, cx);
        let menu = cx.debug_bounds("agent-model-options").unwrap();
        assert!(menu.bottom() <= model.top());
        cx.simulate_keystrokes("escape");
        crate::harness::click("agent-effort-menu", cx);
        crate::harness::paint(&app, cx);
        let menu = cx.debug_bounds("agent-effort-option-popup").unwrap();
        assert!(menu.bottom() <= effort.top());
    }

    #[gpui::test]
    fn unsupported_audio_preserves_the_message_and_reference_for_another_model(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent = AgentPreferences {
                backend: "rig".into(),
                provider: "ollama".into(),
                model: "fixture".into(),
                ..Default::default()
            };
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.input = TextField::new("Inspect this mix");
            this.agent_chat.attachments = vec![PathBuf::from("mix.wav")];
            this.agent_send();
            assert!(this.agent_chat.attachment_error.is_some());
            assert_eq!(this.agent_chat.input.content(), "Inspect this mix");
            assert_eq!(this.agent_chat.attachments, [PathBuf::from("mix.wav")]);
            assert!(this.agent_chat.link.is_none());
        });
    }

    #[gpui::test]
    fn antigravity_image_rejection_keeps_the_draft_and_all_references(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent = AgentPreferences {
                backend: "antigravity".into(),
                model: "fixture".into(),
                ..Default::default()
            };
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.input = TextField::new("Use these references");
            this.agent_chat.attachments = vec!["image.png".into(), "reference.wav".into()];
            this.agent_send();
            assert_eq!(
                this.agent_chat.attachment_error.as_deref(),
                Some(this.t(Key::AgentAttachmentImageUnsupported))
            );
            assert_eq!(this.agent_chat.input.content(), "Use these references");
            assert_eq!(this.agent_chat.attachments.len(), 2);
            assert!(this.agent_chat.link.is_none());
        });
    }

    #[gpui::test]
    fn cli_audio_references_pass_send_validation_and_wait_for_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let folder = tempfile::tempdir().unwrap();
        app.update(cx, |this, _| {
            this.session
                .save_as(&folder.path().join("References.auris"))
                .unwrap();
            for backend in ["codex", "claude"] {
                this.agent_chat.pending_send = None;
                this.settings.agent = AgentPreferences {
                    backend: backend.into(),
                    provider: "ollama".into(),
                    model: "fixture".into(),
                    ..Default::default()
                };
                this.agent_chat
                    .load_preferences(&this.settings.agent.clone());
                this.agent_chat.input = TextField::new("Inspect this mix");
                this.agent_chat.attachments = vec![PathBuf::from("mix.wav")];
                let (_sender, receiver) = std::sync::mpsc::channel();
                this.agent_chat.history_load = Some(PendingHistoryLoad {
                    token: this.agent_document_token(),
                    receiver,
                });
                this.agent_send();
                assert!(this.agent_chat.attachment_error.is_none());
                let pending = this.agent_chat.pending_send.as_ref().unwrap();
                assert_eq!(pending.attachments, [PathBuf::from("mix.wav")]);
                assert_eq!(pending.preferences.backend, backend);
                assert_eq!(this.agent_chat.input.content(), "Inspect this mix");
                assert!(this.agent_chat.link.is_none());
            }
        });
    }

    #[gpui::test]
    fn selected_model_label_has_a_paintable_frame(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.load_preferences(&AgentPreferences {
                model: "gpt-6.1-sol".into(),
                ..Default::default()
            });
            this.agent_chat.models_loaded = true;
        });
        crate::harness::paint(&app, cx);
        let label = cx.debug_bounds("agent-model-current").unwrap();
        assert!(
            label.size.height > px(0.0),
            "the selected model must have space to paint: {label:?}"
        );
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
                    ..Default::default()
                },
                ModelOption {
                    name: "second".to_string(),
                    context_length: Some(65_536),
                    ..Default::default()
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
        crate::harness::click("agent-model", cx);
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-models-empty").is_some());

        app.update(cx, |this, _| this.agent_chat.fetching_models = true);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.fetching_models));
        assert!(cx.debug_bounds("agent-models-fetching").is_some());
        // GPUI retains debug bounds for removed deferred rows; check the newly painted state.
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
    fn empty_or_failed_catalogue_keeps_the_model_menu_available_for_recovery(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "configured-model-remains-visible".into();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.models_error = Some("provider unavailable".into());
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-model", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.model_menu));
        assert!(cx.debug_bounds("agent-models-refresh").is_some());
        cx.simulate_keystrokes("secondary-r");
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.fetching_models);
            assert!(!this.agent_chat.model_menu);
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
                ..Default::default()
            }];
            this.agent_chat.input = TextField::new("keep this queued draft");
            this.agent_chat.attachments = vec![PathBuf::from("keep-reference.png")];
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
                    vec![PathBuf::from("keep-reference.png")]
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
        if let Some(complete) = app.update(cx, |this, _| this.flush_conversation_storage()) {
            complete.recv_timeout(Duration::from_secs(10)).unwrap();
        }
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
                ..Default::default()
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
        let control = cx.debug_bounds("agent-new-conversation").unwrap();
        assert!(
            control.left() >= panel.left() && control.right() <= panel.right(),
            "new conversation escapes the minimum-width panel: {control:?} outside {panel:?}"
        );

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
    fn sent_attachments_remain_visible_inside_a_narrow_transcript_at_multiple_scales(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.panels
                .set_size(crate::dock::Dock::Right, crate::dock::PanelLayout::MIN_SIDE);
            this.agent_chat.models_loaded = true;
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.entries = vec![
                ChatEntry::You {
                    text: "Compare the references".into(),
                    attachments: vec![
                        PathBuf::from(format!(
                            "C:\\References\\{}.wav",
                            "長い録音ファイル名".repeat(20)
                        )),
                        PathBuf::from("/References/score.png"),
                    ],
                },
                ChatEntry::user("A message without attachments".into()),
            ];
        });
        for (scheme, language, font_size) in [
            ("midnight", auris_i18n::Language::English, 16.0),
            ("daylight", auris_i18n::Language::Japanese, 24.0),
        ] {
            app.update(cx, |this, _| {
                this.theme = Theme::named(scheme);
                this.language = language;
            });
            cx.update(|window, _| window.set_rem_size(px(font_size)));
            crate::harness::resize(&app, cx, gpui::size(px(720.0), px(720.0)));
            let panel = cx.debug_bounds("agent-panel").unwrap();
            let group = cx.debug_bounds("agent-message-attachments-0").unwrap();
            for selector in [
                "agent-message-attachment-0-0",
                "agent-message-attachment-0-1",
            ] {
                let row = cx.debug_bounds(selector).unwrap();
                assert_eq!(row.left(), group.left());
                assert_eq!(row.right(), group.right());
                assert!(row.left() >= panel.left() && row.right() <= panel.right());
            }
            assert!(cx.debug_bounds("agent-message-attachments-1").is_none());
            assert!(cx.debug_bounds("agent-attachment-remove-0").is_none());
        }
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.attachments.is_empty())
        });
    }

    #[gpui::test]
    fn transcript_attachments_open_the_selected_file_by_pointer_and_keyboard(
        cx: &mut gpui::TestAppContext,
    ) {
        let folder = tempfile::tempdir().unwrap();
        let paths = vec![
            folder.path().join("参考 #1 %.wav"),
            folder.path().join("score & notes.txt"),
        ];
        for path in &paths {
            std::fs::write(path, b"fixture").unwrap();
        }
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.entries = vec![ChatEntry::You {
                text: "References".into(),
                attachments: paths.clone(),
            }];
        });
        crate::harness::paint(&app, cx);
        assert_eq!(cx.opened_url(), None);
        crate::harness::click("agent-message-attachment-0-0", cx);
        cx.run_until_parked();
        assert_eq!(
            cx.opened_url(),
            Some(url::Url::from_file_path(&paths[0]).unwrap().into())
        );

        // Pointer focus stays on the first file; Tab reaches the next file's button.
        cx.simulate_keystrokes("tab");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        cx.run_until_parked();
        assert_eq!(
            cx.opened_url(),
            Some(url::Url::from_file_path(&paths[1]).unwrap().into())
        );
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.attachments.is_empty());
            assert!(this.agent_chat.attached_here.is_empty());
            assert_eq!(this.agent_chat.entries.len(), 1);
        });
    }

    #[gpui::test]
    fn missing_or_relative_transcript_attachments_report_an_error_without_opening(
        cx: &mut gpui::TestAppContext,
    ) {
        let folder = tempfile::tempdir().unwrap();
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.language = auris_i18n::Language::Japanese;
            this.agent_chat.models_loaded = true;
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.entries = vec![ChatEntry::You {
                text: "References".into(),
                attachments: vec![
                    folder.path().join("missing.wav"),
                    PathBuf::from("relative.txt"),
                ],
            }];
        });
        crate::harness::paint(&app, cx);
        for selector in [
            "agent-message-attachment-0-0",
            "agent-message-attachment-0-1",
        ] {
            crate::harness::click(selector, cx);
            cx.run_until_parked();
        }
        assert_eq!(cx.opened_url(), None);
        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.entries.len(), 3);
            assert!(this.agent_chat.entries[1..].iter().all(|entry| matches!(entry,
                ChatEntry::Error(message) if message.starts_with("添付ファイルを開けません：")
            )));
        });
    }

    #[gpui::test]
    fn minimum_side_dock_keeps_model_gauge_and_composer_inside_the_agent_panel(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.language = auris_i18n::Language::Japanese;
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
                ..Default::default()
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
            "agent-model",
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
        let control = cx.debug_bounds("agent-new-conversation").unwrap();
        assert!(
            control.size.width >= panel.size.width * 0.5,
            "new conversation collapsed instead of yielding its label: {control:?} inside {panel:?}"
        );
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
                ..Default::default()
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

    #[test]
    fn send_blockers_follow_the_order_send_checks_them() {
        assert_eq!(send_blocker(true, false), None);
        assert_eq!(send_blocker(false, false), Some(SendBlocker::NoModel));
        assert_eq!(send_blocker(false, true), Some(SendBlocker::PendingChanges));
        assert_eq!(send_blocker(true, true), Some(SendBlocker::PendingChanges));
    }

    #[test]
    fn only_attachable_files_raise_the_drop_overlay() {
        assert!(drop_offers_attachment(&[
            PathBuf::from("setup.exe"),
            PathBuf::from("take.wav"),
        ]));
        assert!(!drop_offers_attachment(&[PathBuf::from("setup.exe")]));
        assert!(!drop_offers_attachment(&[]));
    }

    #[gpui::test]
    fn pending_changes_switch_send_off_and_offer_the_accept_beside_the_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "gpt-6.1-sol".into();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_loaded = true;
            this.agent_chat.input = TextField::new("もう少し明るく");
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-send-blocked").is_none());

        app.update(cx, |this, _| {
            this.agent_chat.pending_reload = Some(PathBuf::from("Song/Song.auris"));
        });
        crate::harness::paint(&app, cx);
        let banner = cx
            .debug_bounds("agent-send-blocked")
            .expect("the reason is shown before sending");
        let composer = cx.debug_bounds("agent-composer").unwrap();
        assert!(banner.top() >= composer.top() && banner.bottom() <= composer.bottom());
        assert!(cx.debug_bounds("agent-send-blocked-action").is_some());
        crate::harness::click("agent-send", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.input.content(), "もう少し明るく");
            assert!(this.agent_chat.entries.is_empty());
        });
    }
}
