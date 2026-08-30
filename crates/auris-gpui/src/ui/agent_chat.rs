//! The agent panel: a conversation with a language model, beside the song it is about.
//!
//! The model itself lives in `auris-agent`, spawned here as a child process in its `--json`
//! mode — the window writes `{"say": …}` lines to its stdin and reads events back off its
//! stdout. Keeping it a process rather than a library is the frontend boundary doing its job:
//! this crate never learns what an LLM client is, the agent never learns what a window is, and
//! the pair that ships in the release archive is exactly the pair that talks here.
//!
//! The one genuinely new problem is that both ends hold the same file. The panel's answer:
//! the window **saves before every message**, so the agent always reads the document as it
//! stands — and when an event says the agent wrote the project back, the window reloads it,
//! automatically while it has nothing unsaved and by an offered button when it does. The
//! decisions behind that live in [`AgentChat::absorb`], which is plain data in and plain
//! instruction out, so the whole policy is tested without a window.

use std::io::{BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};

use auris_i18n::Key;
use auris_session::AgentPreferences;
use gpui::{AnyElement, IntoElement, MouseButton, MouseDownEvent, Window, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::text_field::TextField;
use crate::ui::widgets::{ButtonStyle, button};

/// One line of the conversation, as the panel shows it.
#[derive(Debug, PartialEq)]
pub(crate) enum ChatEntry {
    /// What the person said.
    You(String),
    /// What the model answered.
    Agent(String),
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
    /// Something went wrong — the process, the provider, the wire.
    Error(String),
    /// A note from the panel itself, translated when drawn.
    Note(Key),
}

/// One event off the agent's wire, already parsed.
#[derive(Debug, PartialEq)]
pub(crate) enum AgentEvent {
    /// The process is up, and named what answered the phone.
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
    /// The turn failed; the process is still alive.
    Error {
        /// What went wrong.
        message: String,
    },
    /// The process's stdout closed: it is gone.
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
    /// The base URL, in the settings section.
    Url,
    /// The API key's environment variable, in the settings section.
    KeyEnv,
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
/// provider having failed — and the thread that runs the subprocess should carry none.
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

/// The running child process and both ends of its wire.
struct AgentLink {
    child: Child,
    to_child: ChildStdin,
    from_child: Receiver<AgentEvent>,
}

impl Drop for AgentLink {
    fn drop(&mut self) {
        // Dropping the panel must not leave a model running unattended. Kill rather than wait:
        // the child may be minutes into a render, and the window is going away now.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Everything the agent panel is, apart from its pixels.
pub(crate) struct AgentChat {
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
    /// Whether the provider picker is dropped open.
    pub(crate) provider_menu: bool,
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
    /// The wire a model listing comes back on.
    models_rx: Option<Receiver<Result<Vec<ModelOption>, String>>>,
    /// Which field holds the keyboard, if any.
    pub(crate) focused: Option<AgentField>,
    /// Whether the settings section is showing.
    pub(crate) configuring: bool,
    /// Whether a message is in flight and unanswered.
    pub(crate) busy: bool,
    /// What the child said it resolved to, for the header.
    pub(crate) model_label: String,
    /// A project the agent rewrote while the window held unsaved edits, awaiting the button.
    pub(crate) pending_reload: Option<PathBuf>,
    /// Where the transcript is scrolled to.
    pub(crate) scroll: gpui::ScrollHandle,
    link: Option<AgentLink>,
}

impl Default for AgentChat {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            input: TextField::new(String::new()),
            chosen_model: String::new(),
            url_field: TextField::new(String::new()),
            key_env_field: TextField::new(String::new()),
            provider_openai: false,
            models: Vec::new(),
            fetching_models: false,
            models_error: None,
            provider_menu: false,
            model_menu: false,
            tokens_in: 0,
            tokens_out: 0,
            context_window: None,
            expanded: std::collections::BTreeSet::new(),
            models_rx: None,
            focused: None,
            configuring: false,
            busy: false,
            model_label: String::new(),
            pending_reload: None,
            scroll: gpui::ScrollHandle::new(),
            link: None,
        }
    }
}

impl AgentChat {
    /// Whether one of this panel's fields is being typed into.
    pub(crate) fn typing(&self) -> bool {
        self.focused.is_some()
    }

    /// The field the keyboard is in, mutably.
    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        Some(match self.focused? {
            AgentField::Chat => &mut self.input,
            AgentField::Url => &mut self.url_field,
            AgentField::KeyEnv => &mut self.key_env_field,
        })
    }

    /// The field the keyboard is in.
    pub(crate) fn field(&self) -> Option<&TextField> {
        Some(match self.focused? {
            AgentField::Chat => &self.input,
            AgentField::Url => &self.url_field,
            AgentField::KeyEnv => &self.key_env_field,
        })
    }

    /// Copies the saved preferences into the settings section's fields.
    pub(crate) fn load_preferences(&mut self, prefs: &AgentPreferences) {
        self.provider_openai = prefs.provider.trim() == "openai";
        self.chosen_model = prefs.model.trim().to_string();
        self.url_field = TextField::new(prefs.url.clone());
        self.key_env_field = TextField::new(prefs.api_key_env.clone());
    }

    /// The settings section's fields, read back out as preferences.
    pub(crate) fn preferences(&self) -> AgentPreferences {
        AgentPreferences {
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
            AgentEvent::Ready { model } => {
                self.model_label = model;
            }
            AgentEvent::Call { tool } => {
                self.entries.push(ChatEntry::Tool {
                    name: tool,
                    ok: true,
                    line: String::new(),
                    detail: String::new(),
                });
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
                let open_row = self.entries.iter_mut().rev().find(|entry| {
                    matches!(entry, ChatEntry::Tool { name, line, .. }
                        if *name == tool && line.is_empty())
                });
                match open_row {
                    Some(ChatEntry::Tool {
                        ok: row_ok,
                        line: row_line,
                        detail: row_detail,
                        ..
                    }) => {
                        *row_ok = ok;
                        *row_line = if line.is_empty() {
                            "done".to_string()
                        } else {
                            line
                        };
                        *row_detail = detail;
                    }
                    _ => self.entries.push(ChatEntry::Tool {
                        name: tool,
                        ok,
                        line,
                        detail,
                    }),
                }
            }
            AgentEvent::Changed { project } => {
                if let Some(open) = open
                    && same_file(&project, open)
                {
                    if dirty {
                        self.pending_reload = Some(project);
                        self.entries.push(ChatEntry::Note(Key::AgentReloadOffer));
                    } else {
                        self.entries.push(ChatEntry::Note(Key::AgentReloaded));
                        return Absorbed::Reload(project);
                    }
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
                self.entries.push(ChatEntry::Agent(text));
            }
            AgentEvent::Error { message } => {
                self.busy = false;
                self.entries.push(ChatEntry::Error(message));
            }
            AgentEvent::Ended => {
                self.busy = false;
                self.link = None;
                self.entries.push(ChatEntry::Note(Key::AgentEnded));
            }
        }
        Absorbed::Nothing
    }
}

/// Where the agent binary lives: beside this one.
///
/// The release archive ships them together, and a development build puts both in the same
/// target directory — the one layout rule the whole feature leans on.
fn agent_binary() -> PathBuf {
    let name = match cfg!(windows) {
        true => "auris-agent.exe",
        false => "auris-agent",
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Starts the agent in its JSON mode and wires both ends.
///
/// `folder` becomes the child's working directory, so a model told nothing else puts files
/// beside the song. The reader thread owns stdout for the child's whole life and speaks to the
/// window only through the channel; the window polls that channel on its repaint tick, the
/// same way it reads everything else another thread writes.
fn spawn_link(folder: Option<&Path>) -> Result<AgentLink, String> {
    let mut command = Command::new(agent_binary());
    command
        .arg("--json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(folder) = folder {
        command.current_dir(folder);
    }
    // Windows-only API, not a `cfg!` choice: without this flag a windowless application
    // spawning a console binary flashes a console window up over the music.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", agent_binary().display()))?;
    let to_child = child.stdin.take().ok_or("the child has no stdin")?;
    let stdout = child.stdout.take().ok_or("the child has no stdout")?;

    let (sender, from_child): (Sender<AgentEvent>, Receiver<AgentEvent>) =
        std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(event) = parse_event(&line)
                && sender.send(event).is_err()
            {
                return;
            }
        }
        let _ = sender.send(AgentEvent::Ended);
    });

    Ok(AgentLink {
        child,
        to_child,
        from_child,
    })
}

/// Asks `auris-agent models` what `prefs`' provider serves, off the window's thread.
///
/// One shot per question: the subprocess prints one JSON line and exits, the thread parses it
/// and puts the verdict on the channel, and the repaint tick picks it up — the same wire shape
/// as the conversation itself.
fn spawn_model_listing(prefs: &AgentPreferences) -> Receiver<Result<Vec<ModelOption>, String>> {
    let mut command = Command::new(agent_binary());
    command
        .arg("models")
        .arg("--provider")
        .arg(match prefs.provider.trim() {
            "openai" => "openai",
            _ => "ollama",
        })
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if !prefs.url.trim().is_empty() {
        command.arg("--url").arg(prefs.url.trim());
    }
    if !prefs.api_key_env.trim().is_empty() {
        command.arg("--api-key-env").arg(prefs.api_key_env.trim());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let answer = command
            .output()
            .map_err(|error| format!("could not run {}: {error}", agent_binary().display()))
            .and_then(|output| {
                let stdout = String::from_utf8_lossy(&output.stdout);
                parse_model_list(stdout.lines().next().unwrap_or_default())
            });
        let _ = sender.send(answer);
    });
    receiver
}

impl AurisApp {
    /// Sends what is in the input field, starting the agent if it is not running.
    ///
    /// The window saves first, so the model reads the document as it stands — the other half
    /// of the bargain is in [`AgentChat::absorb`], where the model's writes come back.
    pub(crate) fn agent_send(&mut self) {
        let text = self.agent_chat.input.content().trim().to_string();
        if text.is_empty() || self.agent_chat.busy {
            return;
        }
        if !self.settings.agent.is_configured() {
            // Said out loud, not merely implied by the settings opening: the first live run
            // pressed Enter here, and a message that silently goes nowhere reads as a broken
            // send rather than as a missing model. The typed text stays put for after.
            self.agent_chat.configuring = true;
            self.agent_chat.load_preferences(&self.settings.agent);
            if !matches!(
                self.agent_chat.entries.last(),
                Some(ChatEntry::Note(Key::AgentNotConfigured))
            ) {
                self.agent_chat
                    .entries
                    .push(ChatEntry::Note(Key::AgentNotConfigured));
            }
            return;
        }
        if self.session.is_dirty()
            && self.session.path().is_some()
            && let Err(error) = self.session.save_in_place()
        {
            self.agent_chat
                .entries
                .push(ChatEntry::Error(error.to_string()));
            return;
        }

        if self.agent_chat.link.is_none() {
            let folder = self
                .session
                .path()
                .and_then(Path::parent)
                .map(Path::to_path_buf);
            match spawn_link(folder.as_deref()) {
                Ok(link) => self.agent_chat.link = Some(link),
                Err(error) => {
                    self.agent_chat.entries.push(ChatEntry::Error(error));
                    return;
                }
            }
        }

        let framed = framed_say(&text, self.session.path());
        let wire = serde_json::json!({ "say": framed }).to_string();
        if let Some(link) = self.agent_chat.link.as_mut()
            && let Err(error) = writeln!(link.to_child, "{wire}")
        {
            self.agent_chat
                .entries
                .push(ChatEntry::Error(error.to_string()));
            self.agent_chat.link = None;
            return;
        }
        self.agent_chat.entries.push(ChatEntry::You(text));
        self.agent_chat.busy = true;
        self.agent_chat.input = TextField::new(String::new());
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
            match answer {
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
        loop {
            let Some(link) = self.agent_chat.link.as_ref() else {
                return;
            };
            let Ok(event) = link.from_child.try_recv() else {
                return;
            };
            let open = self.session.path().map(Path::to_path_buf);
            let dirty = self.session.is_dirty();
            match self.agent_chat.absorb(event, open.as_deref(), dirty) {
                Absorbed::Nothing => cx.notify(),
                Absorbed::Reload(path) => {
                    self.open_project_at(path, cx);
                }
            }
        }
    }

    /// Throws the model list away and asks the provider again, with the form as it stands.
    pub(crate) fn agent_refresh_models(&mut self) {
        self.agent_chat.models.clear();
        self.agent_chat.models_error = None;
        self.agent_chat.fetching_models = true;
        self.agent_chat.model_menu = false;
        self.agent_chat.models_rx = Some(spawn_model_listing(&self.agent_chat.preferences()));
    }

    /// Reloads the project the agent rewrote, once the person says so.
    pub(crate) fn agent_reload(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(path) = self.agent_chat.pending_reload.take() {
            self.agent_chat
                .entries
                .push(ChatEntry::Note(Key::AgentReloaded));
            self.open_project_at(path, cx);
        }
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
        self.agent_chat.link = None;
        self.agent_chat.model_label = String::new();
        self.agent_chat.configuring = false;
        self.agent_chat.focused = None;
    }

    /// Answers for a key while one of the agent panel's fields holds the keyboard.
    ///
    /// The characters come through the platform's input handler like every other field's; this
    /// sees what that leaves out. Enter in the chat field sends; in a settings field it applies.
    pub(crate) fn agent_key(&mut self, event: &gpui::KeyDownEvent) -> bool {
        let Some(focused) = self.agent_chat.focused else {
            return false;
        };
        let key = event.keystroke.key.as_str();
        let composing = self
            .agent_chat
            .field()
            .is_some_and(|field| field.marked().is_some());
        if !composing {
            match (key, focused) {
                ("escape", _) => {
                    self.agent_chat.focused = None;
                    return true;
                }
                ("enter", AgentField::Chat) => {
                    self.agent_send();
                    return true;
                }
                // A finished URL or key name changes what the provider would answer, so the
                // model list is asked again rather than left describing the old endpoint.
                ("enter", _) => {
                    self.agent_chat.focused = None;
                    self.agent_refresh_models();
                    return true;
                }
                _ => {}
            }
        }
        let shift = event.keystroke.modifiers.shift;
        let secondary = event.keystroke.modifiers.secondary();
        self.agent_chat.field_mut().is_some_and(|field| {
            field.apply_key(key, shift, secondary) != crate::ui::text_field::KeyEffect::Ignored
        })
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
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let configured = self.settings.agent.is_configured();
        let configuring = self.agent_chat.configuring || !configured;
        if configuring && !configured && self.agent_chat.chosen_model.is_empty() {
            // First opening on an unconfigured machine: start the form from what is saved.
            self.agent_chat
                .load_preferences(&self.settings.agent.clone());
        }
        // The first time the settings section is on screen, ask the provider what it serves —
        // once, and only until an answer or a refusal lands; the refresh button asks again.
        if configuring
            && self.agent_chat.models.is_empty()
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
            .map(|(index, entry)| self.chat_row(index, entry, &theme, cx))
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
                    .h(Metrics::PANEL_HEADER_HEIGHT)
                    .px_2()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(div().child(self.t(Key::AgentPanel)))
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
                        configuring,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.agent_chat.configuring = !this.agent_chat.configuring;
                            if this.agent_chat.configuring {
                                let prefs = this.settings.agent.clone();
                                this.agent_chat.load_preferences(&prefs);
                            } else {
                                this.agent_chat.focused = None;
                            }
                            cx.notify();
                        }),
                    )),
            )
            .when(configuring, |this| this.child(self.agent_settings(cx)))
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
                        .children(rows)
                        .when(busy, |this| {
                            this.child(
                                div()
                                    .px_1p5()
                                    .text_xs()
                                    .text_color(theme.text_faint)
                                    .child(self.t(Key::AgentWorking)),
                            )
                        })
                        .when(
                            self.agent_chat.entries.is_empty() && !busy && !configuring,
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
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let (colour, text): (gpui::Hsla, String) = match entry {
            ChatEntry::You(text) => (theme.accent, text.clone()),
            ChatEntry::Agent(text) => (theme.text, text.clone()),
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
        div()
            .id(("agent-line", index))
            .px_1p5()
            .py_0p5()
            .text_xs()
            .text_color(colour)
            .when(bordered, |this| {
                this.border_l_2().border_color(theme.accent).ml_1()
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
            .child(text)
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
    fn agent_settings(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let openai = self.agent_chat.provider_openai;
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
                self.t(Key::AgentProviderLabel).to_string(),
                self.dropdown(
                    "agent-provider",
                    match openai {
                        true => "openai".to_string(),
                        false => "ollama".to_string(),
                    },
                    self.agent_chat.provider_menu,
                    &theme,
                    |this, _| {
                        this.agent_chat.provider_menu = !this.agent_chat.provider_menu;
                        this.agent_chat.model_menu = false;
                    },
                    cx,
                ),
                &theme,
            ))
            .when(self.agent_chat.provider_menu, |this| {
                this.child(self.option_rows(
                    "agent-provider-option",
                    &["ollama".to_string(), "openai".to_string()],
                    &theme,
                    |this, chosen, _| {
                        this.agent_chat.provider_openai = chosen == 1;
                        this.agent_chat.provider_menu = false;
                        // A different provider serves a different list, so it is asked afresh.
                        this.agent_refresh_models();
                    },
                    cx,
                ))
            })
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
                            this.agent_chat.provider_menu = false;
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
                        }
                        this.agent_chat.model_menu = false;
                    },
                    cx,
                ))
            })
            .child(labelled(
                self.t(Key::AgentUrlLabel).to_string(),
                self.agent_text_field("agent-url", AgentField::Url, cx),
                &theme,
            ))
            .child(labelled(
                self.t(Key::AgentKeyEnvLabel).to_string(),
                self.agent_text_field("agent-key-env", AgentField::KeyEnv, cx),
                &theme,
            ))
            .child(div().flex().justify_end().child(button(
                "agent-apply",
                self.t(Key::AgentApply),
                ButtonStyle::Normal,
                true,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.agent_apply_settings();
                    cx.notify();
                }),
            )))
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
        let focused = self.agent_chat.focused == Some(AgentField::Chat);
        let empty = self.agent_chat.input.content().is_empty();
        let placeholder = self.t(Key::AgentPlaceholder).to_string();
        div()
            .p_1()
            .border_t_1()
            .border_color(theme.border)
            .child(self.panel_field(
                "agent-input",
                AgentField::Chat,
                focused,
                empty,
                placeholder,
                &theme,
                cx,
            ))
            .into_any_element()
    }

    /// A settings-section text field.
    fn agent_text_field(
        &mut self,
        id: &'static str,
        field: AgentField,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let focused = self.agent_chat.focused == Some(field);
        self.panel_field(id, field, focused, false, String::new(), &theme, cx)
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
            AgentField::Url => &self.agent_chat.url_field,
            AgentField::KeyEnv => &self.agent_chat.key_env_field,
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
