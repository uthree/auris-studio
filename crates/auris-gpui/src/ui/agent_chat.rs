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
        "result" => AgentEvent::Result {
            tool: text("tool"),
            ok: parsed
                .get("ok")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            line: text("text").lines().next().unwrap_or_default().to_string(),
        },
        "changed" => AgentEvent::Changed {
            project: PathBuf::from(text("project")),
        },
        "answer" => AgentEvent::Answer { text: text("text") },
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
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentField {
    /// The message being written.
    Chat,
    /// The model name, in the settings section.
    Model,
    /// The base URL, in the settings section.
    Url,
    /// The API key's environment variable, in the settings section.
    KeyEnv,
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
    /// The model name being edited, while the settings section is open.
    pub(crate) model_field: TextField,
    /// The base URL being edited.
    pub(crate) url_field: TextField,
    /// The API key variable being edited.
    pub(crate) key_env_field: TextField,
    /// Whether the provider under edit is the OpenAI-compatible one.
    pub(crate) provider_openai: bool,
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
            model_field: TextField::new(String::new()),
            url_field: TextField::new(String::new()),
            key_env_field: TextField::new(String::new()),
            provider_openai: false,
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
            AgentField::Model => &mut self.model_field,
            AgentField::Url => &mut self.url_field,
            AgentField::KeyEnv => &mut self.key_env_field,
        })
    }

    /// The field the keyboard is in.
    pub(crate) fn field(&self) -> Option<&TextField> {
        Some(match self.focused? {
            AgentField::Chat => &self.input,
            AgentField::Model => &self.model_field,
            AgentField::Url => &self.url_field,
            AgentField::KeyEnv => &self.key_env_field,
        })
    }

    /// Copies the saved preferences into the settings section's fields.
    pub(crate) fn load_preferences(&mut self, prefs: &AgentPreferences) {
        self.provider_openai = prefs.provider.trim() == "openai";
        self.model_field = TextField::new(prefs.model.clone());
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
            model: self.model_field.content().trim().to_string(),
            url: self.url_field.content().trim().to_string(),
            api_key_env: self.key_env_field.content().trim().to_string(),
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
            AgentEvent::Ready { model } => {
                self.model_label = model;
            }
            AgentEvent::Call { tool } => {
                self.entries.push(ChatEntry::Tool {
                    name: tool,
                    ok: true,
                    line: String::new(),
                });
            }
            AgentEvent::Result { tool, ok, line } => {
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
                        ..
                    }) => {
                        *row_ok = ok;
                        *row_line = if line.is_empty() {
                            "done".to_string()
                        } else {
                            line
                        };
                    }
                    _ => self.entries.push(ChatEntry::Tool {
                        name: tool,
                        ok,
                        line,
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
            AgentEvent::Answer { text } => {
                self.busy = false;
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
            self.agent_chat.configuring = true;
            self.agent_chat.load_preferences(&self.settings.agent);
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
                ("enter", _) => {
                    self.agent_apply_settings();
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
        if configuring && self.agent_chat.model_field.content().is_empty() && !configured {
            // First opening on an unconfigured machine: start the form from what is saved.
            self.agent_chat
                .load_preferences(&self.settings.agent.clone());
        }

        let rows: Vec<AnyElement> = self
            .agent_chat
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| self.chat_row(index, entry, &theme))
            .collect();
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
            .child(self.agent_input_row(cx))
    }

    /// One transcript row.
    fn chat_row(&self, index: usize, entry: &ChatEntry, theme: &Theme) -> AnyElement {
        let (colour, text): (gpui::Hsla, String) = match entry {
            ChatEntry::You(text) => (theme.accent, text.clone()),
            ChatEntry::Agent(text) => (theme.text, text.clone()),
            ChatEntry::Tool { name, ok, line } => {
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
        div()
            .id(("agent-line", index))
            .px_1p5()
            .py_0p5()
            .text_xs()
            .text_color(colour)
            .when(bordered, |this| {
                this.border_l_2().border_color(theme.accent).ml_1()
            })
            .child(text)
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
                div()
                    .flex()
                    .gap_1()
                    .child(button(
                        "agent-provider-ollama",
                        "ollama".to_string(),
                        ButtonStyle::Normal,
                        !openai,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.agent_chat.provider_openai = false;
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "agent-provider-openai",
                        "openai".to_string(),
                        ButtonStyle::Normal,
                        openai,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.agent_chat.provider_openai = true;
                            cx.notify();
                        }),
                    ))
                    .into_any_element(),
                &theme,
            ))
            .child(labelled(
                self.t(Key::AgentModelLabel).to_string(),
                self.agent_text_field("agent-model", AgentField::Model, cx),
                &theme,
            ))
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
            AgentField::Model => &self.agent_chat.model_field,
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
                line: "The mix — x".to_string()
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
            },
            None,
            false,
        );
        assert_eq!(chat.entries.len(), 1, "the result fills the call's row");
        assert!(matches!(
            chat.entries.last(),
            Some(ChatEntry::Tool { ok: true, line, .. }) if line == "Wrote X"
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
            },
            None,
            false,
        );
        assert!(!chat.busy);
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
