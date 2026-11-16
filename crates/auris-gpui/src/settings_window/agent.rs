//! Connection and generation preferences in the application settings window.
use super::*;
use crate::ui::icons::{Icon, icon};
use crate::ui::provider_logo::provider_logo;
use crate::ui::widgets::{ButtonState, button_enabled, button_with_content};

pub(super) struct AgentSettings {
    prefs: AgentPreferences,
    saved: AgentDraft,
    selected_connection: auris_agent::ModelConnection,
    pub(super) fields: [TextField; 8],
    pub(super) focus: [FocusHandle; 8],
    /// The inputs the six one-line fields are typed into; the two prompts are prose areas.
    inputs: [crate::ui::text_input::MirroredInput; 6],
    pub(super) active: Option<usize>,
    expanded_providers: [bool; 4],
    provider_status: [Option<auris_agent::ProviderStatus>; 5],
    checked_fields: Option<[String; 6]>,
    checked_provider: String,
    checking_providers: bool,
    provider_check_focus: FocusHandle,
    provider_check: Option<gpui::Task<()>>,
    /// Updates run from the CLI rows, by row.
    updates: [ProviderUpdateRun; 4],
    /// A connection check to run once the one in flight has finished, because an update
    /// ended while it was running and its version may predate the update.
    recheck_providers: bool,
    pub(super) prompt_viewport:
        std::rc::Rc<std::cell::RefCell<crate::ui::text_area::ProseViewport>>,
}

/// Where an update started from a CLI row has got to.
#[derive(Default)]
enum ProviderUpdateRun {
    /// No update has been started since the last check.
    #[default]
    Idle,
    /// The update is running; dropping the task stops watching it, not the update.
    Running(#[allow(dead_code)] gpui::Task<()>),
    /// The update finished; a fresh check is reading the new version.
    Done,
    /// The update failed, with what it printed.
    Failed(String),
}

/// A problem in a provider's details: what went wrong, in the warning colour, over what the
/// provider or the update printed.
fn problem(title: &str, detail: &str, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded(Metrics::RADIUS_MD)
        .bg(theme.atelier.colors.warning_wash)
        .text_size(theme.atelier.typography.size_sm)
        .child(div().text_color(theme.warning).child(title.to_owned()))
        .child(div().text_color(theme.text).child(detail.to_owned()))
        .into_any_element()
}

/// The version an installed CLI printed, as a version number: `2.1.322 (Claude Code)` and
/// `codex-cli 0.9.0` both read as `v…`. Anything else is shown as printed.
fn short_version(printed: &str) -> String {
    printed
        .split_whitespace()
        .find_map(|word| {
            let number = word.strip_prefix('v').unwrap_or(word);
            semver::Version::parse(number)
                .ok()
                .map(|_| format!("v{number}"))
        })
        .unwrap_or_else(|| printed.to_owned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AgentDraft {
    provider: String,
    url: String,
    api_key_env: String,
    codex_path: String,
    claude_path: String,
    opencode_path: String,
    antigravity_path: String,
    context_tokens: Option<u32>,
    output_tokens: Option<u32>,
    max_tool_failures: Option<u8>,
    max_tool_calls: Option<u32>,
    system_prompt: String,
}

impl AgentSettings {
    fn draft(&self) -> AgentDraft {
        let prompt = self.fields[Self::PROMPT_FIELD].content();
        AgentDraft {
            provider: self.prefs.provider.clone(),
            url: self.fields[0].content().trim().to_owned(),
            api_key_env: self.fields[1].content().trim().to_owned(),
            codex_path: self.fields[2].content().trim().to_owned(),
            claude_path: self.fields[3].content().trim().to_owned(),
            opencode_path: self.fields[4].content().trim().to_owned(),
            antigravity_path: self.fields[5].content().trim().to_owned(),
            context_tokens: self.prefs.context_tokens,
            output_tokens: self.prefs.output_tokens,
            max_tool_failures: self.prefs.max_tool_failures,
            max_tool_calls: self.prefs.max_tool_calls,
            system_prompt: if prompt.trim().is_empty()
                || prompt == auris_agent::default_system_prompt()
            {
                String::new()
            } else {
                prompt.to_owned()
            },
        }
    }

    fn restore(&mut self, draft: AgentDraft) {
        self.prefs.provider = draft.provider;
        self.prefs.url = draft.url.clone();
        self.prefs.api_key_env = draft.api_key_env.clone();
        self.prefs.codex_path = draft.codex_path.clone();
        self.prefs.claude_path = draft.claude_path.clone();
        self.prefs.opencode_path = draft.opencode_path.clone();
        self.prefs.antigravity_path = draft.antigravity_path.clone();
        self.prefs.context_tokens = draft.context_tokens;
        self.prefs.output_tokens = draft.output_tokens;
        self.prefs.max_tool_failures = draft.max_tool_failures;
        self.prefs.max_tool_calls = draft.max_tool_calls;
        self.prefs.system_prompt = draft.system_prompt.clone();
        for (field, value) in self.fields[..6].iter_mut().zip([
            draft.url,
            draft.api_key_env,
            draft.codex_path,
            draft.claude_path,
            draft.opencode_path,
            draft.antigravity_path,
        ]) {
            *field = TextField::new(value);
        }
        self.fields[Self::PROMPT_FIELD] = TextField::new(if draft.system_prompt.is_empty() {
            auris_agent::default_system_prompt().to_owned()
        } else {
            draft.system_prompt
        });
        self.fields[Self::PROMPT_FIELD].place_caret(0, false);
    }
}

impl AgentSettings {
    /// The inputs of the one-line fields.
    #[cfg(test)]
    pub(super) fn inputs(&self) -> impl Iterator<Item = &crate::ui::text_input::MirroredInput> {
        self.inputs.iter()
    }

    pub(super) const PROMPT_FIELD: usize = 6;
    pub(super) const PREFERENCE_FIELD: usize = 7;

    pub(super) fn new(prefs: AgentPreferences, cx: &mut Context<SettingsWindow>) -> Self {
        let selected_connection = auris_agent::ModelConnection::from_preferences(&prefs);
        let mut prefs = prefs;
        let mut prompt = TextField::new(auris_agent::system_prompt(&prefs));
        prompt.place_caret(0, false);
        if let Some(endpoint) = &prefs.discovery_endpoint {
            prefs.provider.clone_from(&endpoint.provider);
            prefs.url.clone_from(&endpoint.url);
            prefs.api_key_env.clone_from(&endpoint.api_key_env);
        }
        let saved = AgentDraft {
            provider: prefs.provider.clone(),
            url: prefs.url.trim().to_owned(),
            api_key_env: prefs.api_key_env.trim().to_owned(),
            codex_path: prefs.codex_path.trim().to_owned(),
            claude_path: prefs.claude_path.trim().to_owned(),
            opencode_path: prefs.opencode_path.trim().to_owned(),
            antigravity_path: prefs.antigravity_path.trim().to_owned(),
            context_tokens: prefs.context_tokens,
            output_tokens: prefs.output_tokens,
            max_tool_failures: prefs.max_tool_failures,
            max_tool_calls: prefs.max_tool_calls,
            system_prompt: if prompt.content() == auris_agent::default_system_prompt() {
                String::new()
            } else {
                prefs.system_prompt.clone()
            },
        };
        let inputs: [crate::ui::text_input::MirroredInput; 6] = std::array::from_fn(|index| {
            crate::ui::text_input::MirroredInput::new(
                cx,
                move |this: &mut SettingsWindow, event, text, cx| {
                    match event {
                        atelier::TextInputEvent::Changed => {
                            let field = &mut this.agent.fields[index];
                            let all = 0..field.content().len();
                            field.replace(all, &text);
                        }
                        atelier::TextInputEvent::Submit => this.apply_agent(cx),
                        atelier::TextInputEvent::Cancel => {}
                        // Completion keys reach an input only while `set_completing` says candidates are
                        // open, which none of these does.
                        _ => {}
                    }
                    cx.notify();
                },
            )
        });
        Self {
            selected_connection,
            saved,
            fields: [
                TextField::new(prefs.url.clone()),
                TextField::new(prefs.api_key_env.clone()),
                TextField::new(prefs.codex_path.clone()),
                TextField::new(prefs.claude_path.clone()),
                TextField::new(prefs.opencode_path.clone()),
                TextField::new(prefs.antigravity_path.clone()),
                prompt,
                TextField::new(String::new()),
            ],
            focus: [
                inputs[0].focus_handle().clone(),
                inputs[1].focus_handle().clone(),
                inputs[2].focus_handle().clone(),
                inputs[3].focus_handle().clone(),
                inputs[4].focus_handle().clone(),
                inputs[5].focus_handle().clone(),
                cx.focus_handle().tab_stop(true),
                cx.focus_handle().tab_stop(true),
            ],
            inputs,
            active: None,
            expanded_providers: [false; 4],
            provider_status: Default::default(),
            checked_fields: None,
            checked_provider: String::new(),
            checking_providers: false,
            provider_check_focus: cx.focus_handle(),
            provider_check: None,
            updates: Default::default(),
            recheck_providers: false,
            prompt_viewport: Default::default(),
            prefs,
        }
    }
}

impl SettingsWindow {
    fn check_agent_providers(&mut self, cx: &mut Context<Self>) {
        if self.agent.checking_providers {
            return;
        }
        let fields: [String; 6] =
            std::array::from_fn(|i| self.agent.fields[i].content().trim().to_owned());
        let mut prefs = self.agent.prefs.clone();
        prefs.url.clone_from(&fields[0]);
        prefs.api_key_env.clone_from(&fields[1]);
        prefs.codex_path.clone_from(&fields[2]);
        prefs.claude_path.clone_from(&fields[3]);
        prefs.opencode_path.clone_from(&fields[4]);
        prefs.antigravity_path.clone_from(&fields[5]);
        prefs.discovery_endpoint = None;
        // Harness windows never contact installed CLIs or the developer's API accounts.
        let receiver = if cfg!(test) {
            std::sync::mpsc::channel().1
        } else {
            auris_agent::check_providers_background(prefs)
        };
        self.agent.checked_fields = Some(fields);
        self.agent
            .checked_provider
            .clone_from(&self.agent.prefs.provider);
        self.agent.provider_status = Default::default();
        self.agent.checking_providers = true;
        self.watch_agent_providers(receiver, cx);
        cx.notify();
    }

    fn watch_agent_providers(
        &mut self,
        receiver: std::sync::mpsc::Receiver<(usize, auris_agent::ProviderStatus)>,
        cx: &mut Context<Self>,
    ) {
        self.agent.provider_check = Some(cx.spawn(async move |this, cx| {
            loop {
                let mut results = Vec::new();
                let disconnected = loop {
                    match receiver.try_recv() {
                        Ok(result) => results.push(result),
                        Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
                    }
                };
                if (!results.is_empty() || disconnected)
                    && this
                        .update(cx, |this, cx| {
                            for (index, status) in results {
                                this.agent.provider_status[index] = Some(status);
                            }
                            if disconnected {
                                this.agent.checking_providers = false;
                                if std::mem::take(&mut this.agent.recheck_providers) {
                                    this.check_agent_providers(cx);
                                }
                            }
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
                if disconnected {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
        }));
    }

    fn agent_provider_status_is_current(&self, row: usize) -> bool {
        let Some(fields) = &self.agent.checked_fields else {
            return false;
        };
        if row < 4 {
            fields[row + 2] == self.agent.fields[row + 2].content().trim()
        } else {
            self.agent.checked_provider == self.agent.prefs.provider
                && fields[0] == self.agent.fields[0].content().trim()
                && fields[1] == self.agent.fields[1].content().trim()
        }
    }

    /// Run the update the last check offered for CLI row `row`.
    fn update_agent_provider(&mut self, row: usize, cx: &mut Context<Self>) {
        if matches!(self.agent.updates[row], ProviderUpdateRun::Running(_)) {
            return;
        }
        let Some(command) = self.agent.provider_status[row]
            .as_ref()
            .and_then(|status| status.update_command.clone())
        else {
            return;
        };
        // Harness windows never run a package manager.
        let receiver = if cfg!(test) {
            drop(command);
            // An update that never reports, so a test can hand in the result it wants.
            let (sender, receiver) = std::sync::mpsc::channel();
            std::mem::forget(sender);
            receiver
        } else {
            auris_agent::update_provider_background(command)
        };
        self.watch_agent_update(row, receiver, cx);
        cx.notify();
    }

    fn watch_agent_update(
        &mut self,
        row: usize,
        receiver: std::sync::mpsc::Receiver<Result<(), String>>,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx| {
            let result = loop {
                match receiver.try_recv() {
                    Ok(result) => break result,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        break Err("The update stopped without reporting a result".into());
                    }
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.agent.updates[row] = ProviderUpdateRun::Done;
                        if this.agent.checking_providers {
                            this.agent.recheck_providers = true;
                        } else {
                            this.check_agent_providers(cx);
                        }
                    }
                    Err(error) => {
                        this.agent.updates[row] = ProviderUpdateRun::Failed(error);
                        // The output is in the details, so they open on it.
                        this.agent.expanded_providers[row] = true;
                    }
                }
                cx.notify();
            });
        });
        self.agent.updates[row] = ProviderUpdateRun::Running(task);
    }

    /// What a provider row says about its connection: a toned badge, then the version and the
    /// number of models, all quiet. Release news is the row's action, not part of this line.
    fn agent_provider_status(&self, row: usize) -> AnyElement {
        let status = self.agent.provider_status[row].as_ref();
        let current = self.agent_provider_status_is_current(row);
        let checking = current && status.is_none() && self.agent.checking_providers;
        let (label, tone) = if !current {
            (Key::AgentProviderCheckNeeded, atelier::Tone::Neutral)
        } else if let Some(status) = status {
            match (status.error.is_some(), status.models) {
                (false, Some(0)) => (Key::AgentProviderNoModels, atelier::Tone::Warning),
                (false, Some(_)) => (Key::AgentProviderConnected, atelier::Tone::Success),
                // A CLI that was not found is a choice not taken yet, not a fault.
                _ if row < 4 && status.executable.is_none() => {
                    (Key::AgentProviderNotInstalled, atelier::Tone::Neutral)
                }
                _ => (Key::AgentProviderUnavailable, atelier::Tone::Warning),
            }
        } else if self.agent.checking_providers {
            (Key::AgentProviderChecking, atelier::Tone::Neutral)
        } else {
            (Key::AgentProviderUnavailable, atelier::Tone::Warning)
        };
        let mut line = div()
            .id(("agent-provider-status", row))
            .debug_selector(move || format!("agent-provider-status-{row}"))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .text_size(self.theme.atelier.typography.size_xs)
            .text_color(self.theme.text_muted)
            .when(checking, |line| {
                line.child(crate::ui::widgets::activity_indicator(
                    gpui::ElementId::Name(format!("agent-provider-checking-{row}").into()),
                    self.theme.text_muted,
                ))
            })
            .child(if checking {
                div().child(self.t(label)).into_any_element()
            } else {
                atelier::Badge::new(self.t(label))
                    .tone(tone)
                    .dot()
                    .into_any_element()
            });
        if current && let Some(status) = status {
            let mut facts = Vec::new();
            if let Some(version) = &status.version {
                facts.push(short_version(version));
            }
            if let Some(count) = status.models.filter(|_| status.error.is_none()) {
                facts.push(
                    self.t(Key::AgentProviderModelCount)
                        .replace("{count}", &count.to_string()),
                );
            }
            if !facts.is_empty() {
                line = line.child(div().child(facts.join(" · ")));
            }
        }
        line.into_any_element()
    }

    /// The action at the end of CLI row `row`: Update when a newer release can be installed from
    /// here, How to update when it cannot, and what an update did once it has run.
    fn agent_provider_action(&self, row: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = &self.theme;
        let status = self.agent.provider_status[row]
            .as_ref()
            .filter(|_| self.agent_provider_status_is_current(row));
        let available = match status.and_then(|status| status.update.as_ref()) {
            Some(auris_agent::ProviderUpdate::Available(version)) => Some(version.clone()),
            _ => None,
        };
        let badge = |key: Key, tone: atelier::Tone| {
            div()
                .id(gpui::SharedString::from(format!(
                    "agent-provider-update-state-{row}"
                )))
                .debug_selector(move || format!("agent-provider-update-state-{row}"))
                .child(atelier::Badge::new(self.t(key)).tone(tone))
                .into_any_element()
        };
        let action = match &self.agent.updates[row] {
            ProviderUpdateRun::Running(_) => div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(theme.atelier.typography.size_sm)
                .text_color(theme.text_muted)
                .child(crate::ui::widgets::activity_indicator(
                    gpui::ElementId::Name(format!("agent-provider-updating-{row}").into()),
                    theme.accent,
                ))
                .child(self.t(Key::AgentUpdateRunning))
                .into_any_element(),
            ProviderUpdateRun::Done if available.is_none() => {
                badge(Key::AgentUpdateDone, atelier::Tone::Success)
            }
            ProviderUpdateRun::Failed(_) if available.is_none() => {
                badge(Key::AgentUpdateFailed, atelier::Tone::Danger)
            }
            _ => {
                let version = available?;
                let command = status.and_then(|status| status.update_command.as_ref());
                let button = if let Some(command) = command {
                    atelier::Button::new(
                        gpui::SharedString::from(format!("agent-provider-update-{row}")),
                        self.t(Key::AgentUpdate),
                    )
                    .primary()
                    .icon(IconName::ArrowUp)
                    .tooltip(
                        self.t(Key::AgentUpdateRuns)
                            .replace("{command}", &command.display()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_agent_provider(row, cx);
                    }))
                } else {
                    atelier::Button::new(
                        gpui::SharedString::from(format!("agent-provider-update-{row}")),
                        self.t(Key::AgentUpdateManual),
                    )
                    .tooltip(self.t(Key::AgentUpdateManualHelp))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.agent.expanded_providers[row] = true;
                        cx.notify();
                    }))
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        atelier::Badge::new(
                            self.t(Key::AgentUpdateAvailable)
                                .replace("{version}", &version),
                        )
                        .tone(atelier::Tone::Accent),
                    )
                    .child(button.size(ControlSize::Small))
                    .into_any_element()
            }
        };
        Some(action)
    }

    fn apply_agent(&mut self, cx: &mut Context<Self>) {
        let Ok(save) = self.app.update(cx, |app, _| app.settings_saver()) else {
            return;
        };
        self.apply_agent_with(cx, save);
    }

    fn apply_agent_with<E>(
        &mut self,
        cx: &mut Context<Self>,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) where
        E: std::fmt::Display,
    {
        self.agent.prefs.url = self.agent.fields[0].content().trim().to_owned();
        self.agent.prefs.api_key_env = self.agent.fields[1].content().trim().to_owned();
        self.agent.prefs.codex_path = self.agent.fields[2].content().trim().to_owned();
        self.agent.prefs.claude_path = self.agent.fields[3].content().trim().to_owned();
        self.agent.prefs.opencode_path = self.agent.fields[4].content().trim().to_owned();
        self.agent.prefs.antigravity_path = self.agent.fields[5].content().trim().to_owned();
        let prompt = self.agent.fields[AgentSettings::PROMPT_FIELD].content();
        self.agent.prefs.system_prompt =
            if prompt.trim().is_empty() || prompt == auris_agent::default_system_prompt() {
                String::new()
            } else {
                prompt.to_owned()
            };
        let draft = self.agent.prefs.clone();
        let selected_connection = self.agent.selected_connection.clone();
        let result = self.app.update(cx, move |app, cx| {
            // Model and live permission controls belong to the panel and may have changed
            // since this settings window opened. Never overwrite them with this draft.
            let mut prefs = app.settings.agent.clone();
            let owns_connection = selected_connection.matches(&prefs);
            prefs.discovery_endpoint = Some(auris_session::AgentEndpoint {
                provider: draft.provider.clone(),
                url: draft.url.clone(),
                api_key_env: draft.api_key_env.clone(),
            });
            prefs.codex_path = draft.codex_path;
            prefs.claude_path = draft.claude_path;
            prefs.opencode_path = draft.opencode_path;
            prefs.antigravity_path = draft.antigravity_path;
            if owns_connection
                && !matches!(
                    prefs.backend.as_str(),
                    "codex" | "claude" | "opencode" | "antigravity"
                )
            {
                prefs.provider = draft.provider;
                prefs.url = draft.url;
                prefs.api_key_env = draft.api_key_env;
            }
            prefs.context_tokens = draft.context_tokens;
            prefs.output_tokens = draft.output_tokens;
            prefs.max_tool_failures = draft.max_tool_failures;
            prefs.max_tool_calls = draft.max_tool_calls;
            prefs.system_prompt = draft.system_prompt;
            app.agent_apply_preferences_with(prefs, save)?;
            cx.notify();
            Ok::<_, E>(
                owns_connection
                    .then(|| auris_agent::ModelConnection::from_preferences(&app.settings.agent)),
            )
        });
        self.status = match result {
            Ok(Ok(connection)) => {
                if let Some(connection) = connection {
                    self.agent.selected_connection = connection;
                }
                self.agent.saved = self.agent.draft();
                self.t(Key::AgentSettingsApplied).to_owned()
            }
            Ok(Err(error)) => error.to_string(),
            Err(error) => error.to_string(),
        };
        cx.notify();
    }

    fn revert_agent(&mut self, cx: &mut Context<Self>) {
        let saved = self.agent.saved.clone();
        self.agent.restore(saved);
        self.agent.active = None;
        self.agent.prompt_viewport.borrow_mut().follow_caret();
        self.agent.checked_fields = None;
        self.agent.provider_status = Default::default();
        self.status.clear();
        cx.notify();
    }

    pub(super) fn agent_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.agent.active else {
            return false;
        };
        if index == AgentSettings::PREFERENCE_FIELD && self.personalization.busy {
            return event.keystroke.key != "tab";
        }
        if index >= AgentSettings::PROMPT_FIELD {
            if index == AgentSettings::PREFERENCE_FIELD {
                self.personalization.viewport.borrow_mut().follow_caret();
            } else {
                self.agent.prompt_viewport.borrow_mut().follow_caret();
            }
            let key = event.keystroke.key.as_str();
            let shift = event.keystroke.modifiers.shift;
            let secondary = event.keystroke.modifiers.secondary();
            if self.agent.fields[index].marked().is_some() {
                return false;
            }
            match key {
                "enter" if secondary => {
                    if index == AgentSettings::PREFERENCE_FIELD {
                        self.save_preference(cx);
                    } else {
                        self.apply_agent(cx);
                    }
                }
                "enter" => self.agent.fields[index].insert("\n"),
                "up" => self.agent.fields[index].move_up(shift),
                "down" => self.agent.fields[index].move_down(shift),
                "home" if secondary => self.agent.fields[index].place_caret(0, shift),
                "end" if secondary => {
                    let end = self.agent.fields[index].content().len();
                    self.agent.fields[index].place_caret(end, shift);
                }
                _ => {
                    let handled = self.agent.fields[index]
                        .apply_key_with_clipboard(key, shift, secondary, true, cx)
                        != KeyEffect::Ignored;
                    if handled {
                        cx.notify();
                    }
                    return handled;
                }
            }
            cx.notify();
            return true;
        }
        // A one-line field is Atelier's input, which takes its own keys; Return applies.
        false
    }

    fn agent_field(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let id = [
            "agent-url",
            "agent-key-env",
            "agent-codex-path",
            "agent-claude-path",
            "agent-opencode-path",
            "agent-antigravity-path",
        ][index];
        let input = self.agent.inputs[index].sync(self.agent.fields[index].content(), cx);
        div()
            .w_full()
            .min_w_0()
            // On the way down, before the input under the pointer takes the press.
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                this.agent.active = Some(index);
                this.editing_search = false;
                cx.notify();
            }))
            .child(
                atelier::TextField::new()
                    .id(id)
                    .debug_selector(move || id.to_owned())
                    .track_focus(&self.agent.inputs[index].tab_stop())
                    .tab_index(0)
                    .child(input),
            )
            .into_any_element()
    }

    pub(super) fn agent_prompt(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let viewport = if index == AgentSettings::PREFERENCE_FIELD {
            self.personalization.viewport.clone()
        } else {
            self.agent.prompt_viewport.clone()
        };
        let click_viewport = viewport.clone();
        let field = &self.agent.fields[index];
        let focus = self.agent.focus[index].clone();
        let pointer_focus = focus.clone();
        div()
            .id(if index == AgentSettings::PREFERENCE_FIELD {
                "preference-context"
            } else {
                "agent-system-prompt"
            })
            .debug_selector(move || {
                if index == AgentSettings::PREFERENCE_FIELD {
                    "preference-context"
                } else {
                    "agent-system-prompt"
                }
                .to_owned()
            })
            .track_focus(&focus)
            .tab_index(0)
            .w_full()
            .min_w_0()
            .h_64()
            .flex_shrink_0()
            .bg(self.theme.surface_sunken)
            .border_1()
            .rounded(Metrics::RADIUS_SM)
            .border_color(self.theme.border)
            .focus(|el| el.border_color(self.theme.accent))
            .cursor_text()
            .child(crate::ui::text_area::editable_scrolling_area(
                field.content().to_owned().into(),
                field.selection(),
                field.marked(),
                self.agent.active == Some(index)
                    && !(index == AgentSettings::PREFERENCE_FIELD && self.personalization.busy),
                focus,
                cx.entity(),
                self.theme.clone(),
                viewport,
            ))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.agent.fields[index].marked().is_none()
                        && let Some(offset) = click_viewport.borrow().offset_at(
                            window,
                            this.agent.fields[index].content(),
                            event.position,
                        )
                    {
                        this.agent.fields[index].place_caret(offset, event.modifiers.shift);
                    }
                    this.agent.active = Some(index);
                    this.editing_search = false;
                    window.focus(&pointer_focus);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let viewport = if index == AgentSettings::PREFERENCE_FIELD {
                        &this.personalization.viewport
                    } else {
                        &this.agent.prompt_viewport
                    };
                    viewport.borrow_mut().scroll_by(
                        event
                            .delta
                            .pixel_delta(crate::ui::text_area::AREA_LINE_HEIGHT)
                            .y,
                    );
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn agent_provider_row(
        &self,
        index: usize,
        name: &'static str,
        logo: &'static str,
        label: Key,
        help: Key,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let row = index - 2;
        let expanded = self.agent.expanded_providers[row];
        // The disclosure mark leads, as in a Finder list, so that the row's own action can sit
        // at its end without the two reading as one control.
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .min_w_0()
            .child(icon(
                if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                Metrics::DISCLOSURE_INDICATOR_SIZE,
                theme.text_muted,
            ))
            .child(
                div()
                    .size_9()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(Metrics::RADIUS_MD)
                    .bg(theme.surface_sunken)
                    .child(provider_logo(logo, theme.text)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().text_color(theme.text).child(name))
                    .child(self.agent_provider_status(row)),
            );
        let action = self.agent_provider_action(row, cx);
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button_with_content(
                            ("agent-provider-details", index),
                            header,
                            ButtonStyle::Ghost,
                            crate::ui::widgets::ButtonState::Enabled(
                                crate::ui::widgets::Latch::Off,
                            ),
                            theme.accent,
                            theme,
                            cx.listener(move |this, _, _, cx| {
                                this.agent.expanded_providers[row] =
                                    !this.agent.expanded_providers[row];
                                cx.notify();
                            }),
                        )
                        .h_auto()
                        .flex_1()
                        .min_w_0()
                        .p_3(),
                    )
                    .children(action.map(|action| div().flex_none().pr_3().child(action))),
            )
            .when(expanded, |view| {
                let status = self.agent.provider_status[row]
                    .as_ref()
                    .filter(|_| self.agent_provider_status_is_current(row));
                let mut details = div()
                    .w_full()
                    .min_w_0()
                    .pl(gpui::px(52.))
                    .pr_3()
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap_3();
                if let ProviderUpdateRun::Failed(error) = &self.agent.updates[row] {
                    details = details.child(problem(self.t(Key::AgentUpdateFailed), error, theme));
                }
                if let Some(error) = status.and_then(|status| status.error.as_ref()) {
                    details =
                        details.child(problem(self.t(Key::AgentProviderUnavailable), error, theme));
                }
                if let Some(path) = status.and_then(|status| status.executable.as_ref()) {
                    details = details.child(
                        div()
                            .flex()
                            .gap_2()
                            .text_size(theme.atelier.typography.size_sm)
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(theme.text_muted)
                                    .child(self.t(Key::AgentProviderLocation)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_color(theme.text)
                                    .child(path.display().to_string()),
                            ),
                    );
                }
                if matches!(
                    status.and_then(|status| status.update.as_ref()),
                    Some(auris_agent::ProviderUpdate::Unavailable)
                ) {
                    details = details.child(note(self.t(Key::AgentUpdateUnknown), theme));
                }
                details = details.child(
                    FormField::new(self.t(label))
                        .description(self.t(help))
                        .child(self.agent_field(index, cx)),
                );
                let (_, _, _, url) = crate::setup_guide::AGENT_SETUP[row];
                const GUIDE: [&str; 4] = [
                    "agent-provider-guide-0",
                    "agent-provider-guide-1",
                    "agent-provider-guide-2",
                    "agent-provider-guide-3",
                ];
                details = details.child(crate::setup_guide::setup_link(
                    GUIDE[row],
                    self.t(Key::AgentProviderSetupGuide),
                    url,
                    theme,
                ));
                view.child(details)
            })
            .into_any_element()
    }

    pub(super) fn render_agent(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.agent.checked_fields.is_none() {
            self.check_agent_providers(cx);
        }
        let theme = self.theme.clone();
        let provider = self.agent.prefs.provider.clone();
        let provider = if provider.is_empty() {
            "ollama".to_owned()
        } else {
            provider
        };
        let provider_control = self.dropdown(
            "agent-provider",
            vec![
                ("ollama".to_owned(), "Ollama".to_owned(), String::new()),
                (
                    "openai".to_owned(),
                    "OpenAI compatible".to_owned(),
                    String::new(),
                ),
            ],
            &provider,
            |this, value, cx| {
                this.agent.prefs.provider = value;
                cx.notify();
            },
            cx,
        );
        let providers = div().flex().flex_col().w_full().min_w_0().children([
            self.agent_provider_row(
                2,
                "Codex",
                "openai",
                Key::AgentCodexPathLabel,
                Key::AgentCodexHelp,
                cx,
            ),
            self.agent_provider_row(
                3,
                "Claude Code",
                "claude",
                Key::AgentClaudePathLabel,
                Key::AgentClaudeHelp,
                cx,
            ),
            self.agent_provider_row(
                4,
                "OpenCode",
                "opencode",
                Key::AgentOpenCodePathLabel,
                Key::AgentOpenCodeHelp,
                cx,
            ),
            self.agent_provider_row(
                5,
                "Antigravity CLI",
                "antigravity",
                Key::AgentAntigravityPathLabel,
                Key::AgentAntigravityHelp,
                cx,
            ),
        ]);
        let mut rows = vec![
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(div().text_lg().child(self.t(Key::AgentProviders)))
                .child(
                    atelier::Button::new("agent-check-providers", self.t(Key::AgentProviderCheck))
                        .icon(IconName::Refresh)
                        .size(ControlSize::Small)
                        .tooltip(self.t(Key::AgentProviderCheckHelp))
                        .track_focus(&self.agent.provider_check_focus)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.agent.active = None;
                            this.editing_search = false;
                            window.focus(&this.agent.provider_check_focus);
                            this.agent.updates = Default::default();
                            this.check_agent_providers(cx);
                        })),
                )
                .into_any_element(),
            note(self.t(Key::AgentProvidersIntro), &theme),
            providers.into_any_element(),
            div()
                .pt_4()
                .flex()
                .items_center()
                .gap_3()
                .child(provider_logo(
                    if provider == "ollama" {
                        "ollama"
                    } else {
                        "api"
                    },
                    theme.text,
                ))
                .child(div().text_base().child(self.t(Key::AgentApiConnection)))
                .child(self.agent_provider_status(4))
                .into_any_element(),
            FormField::new(self.t(Key::AgentProviderLabel))
                .child(provider_control)
                .into_any_element(),
            FormField::new(self.t(Key::AgentUrlLabel))
                .child(self.agent_field(0, cx))
                .into_any_element(),
            FormField::new(self.t(Key::AgentKeyEnvLabel))
                .child(self.agent_field(1, cx))
                .into_any_element(),
        ];
        if self.agent_provider_status_is_current(4)
            && let Some(error) = self.agent.provider_status[4]
                .as_ref()
                .and_then(|status| status.error.as_ref())
        {
            rows.push(problem(
                self.t(Key::AgentProviderUnavailable),
                error,
                &theme,
            ));
        }
        if provider == "ollama" {
            let context = self.agent.prefs.context_tokens.unwrap_or(32768);
            let output = self.agent.prefs.output_tokens.unwrap_or(4096);
            rows.push(
                FormField::new(self.t(Key::AgentContextTokens))
                    .child(
                        self.dropdown(
                            "agent-context-tokens",
                            [32768, 65536, 131072, 262144]
                                .into_iter()
                                .map(|n| (n, format!("{}K", n / 1024), String::new()))
                                .collect(),
                            &context,
                            |this, value, cx| {
                                this.agent.prefs.context_tokens = Some(value);
                                this.agent.prefs.output_tokens = Some(
                                    this.agent
                                        .prefs
                                        .output_tokens
                                        .unwrap_or(4096)
                                        .min(value / 4),
                                );
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                    .into_any_element(),
            );
            rows.push(
                FormField::new(self.t(Key::AgentOutputTokens))
                    .child(
                        self.dropdown(
                            "agent-output-tokens",
                            [4096, 8192, 16384, 32768, 65536]
                                .into_iter()
                                .map(|n| (n, format!("{}K", n / 1024), String::new()))
                                .collect(),
                            &output,
                            |this, value, cx| {
                                this.agent.prefs.output_tokens = Some(value);
                                if value >= this.agent.prefs.context_tokens.unwrap_or(32768) / 2 {
                                    this.agent.prefs.context_tokens = Some(value * 4);
                                }
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                    .into_any_element(),
            );
            rows.push(note(self.t(Key::AgentOutputTokensHelp), &theme));
        }
        let limit = self.agent.prefs.tool_call_limit();
        let mut limits = vec![100, 250, 500, 1000, 2000, 5000, 10000];
        limits.push(limit);
        limits.sort_unstable();
        limits.dedup();
        rows.push(
            FormField::new(self.t(Key::AgentToolCallLimit))
                .description(self.t(Key::AgentToolCallLimitHelp))
                .child(
                    self.dropdown(
                        "agent-tool-call-limit",
                        limits
                            .into_iter()
                            .map(|n| (n, n.to_string(), String::new()))
                            .collect(),
                        &limit,
                        |this, value, cx| {
                            this.agent.prefs.max_tool_calls = Some(value as u32);
                            cx.notify();
                        },
                        cx,
                    ),
                )
                .into_any_element(),
        );
        rows.push(
            FormField::new(self.t(Key::AgentToolFailureLimit))
                .child(
                    self.dropdown(
                        "agent-tool-failure-limit",
                        (1..=20usize)
                            .map(|n| (n, n.to_string(), String::new()))
                            .collect(),
                        &self.agent.prefs.tool_failure_limit(),
                        |this, value, cx| {
                            this.agent.prefs.max_tool_failures = Some(value as u8);
                            cx.notify();
                        },
                        cx,
                    ),
                )
                .into_any_element(),
        );
        rows.push(note(self.t(Key::AgentToolFailureLimitHelp), &theme));
        rows.push(
            FormField::new(self.t(Key::AgentSystemPrompt))
                .description(self.t(Key::AgentSystemPromptHelp))
                .child(self.agent_prompt(AgentSettings::PROMPT_FIELD, cx))
                .into_any_element(),
        );
        rows.push(
            button(
                "agent-prompt-reset",
                self.t(Key::AgentPromptReset),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.agent.fields[AgentSettings::PROMPT_FIELD] =
                        TextField::new(auris_agent::default_system_prompt());
                    this.agent.fields[AgentSettings::PROMPT_FIELD].place_caret(0, false);
                    this.agent.prompt_viewport.borrow_mut().follow_caret();
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        // The workshop's learned preferences are the other text the model is given about this
        // person, so the page that holds the system prompt points at where they are edited.
        rows.push(
            FormField::new(self.t(Key::PreferenceHeading))
                .description(self.t(Key::AgentPreferencesLinkHelp))
                .child(
                    atelier::Button::new(
                        "agent-preferences-link",
                        self.t(Key::AgentPreferencesLink),
                    )
                    .ghost()
                    .icon(IconName::Music)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_tab(SettingsTab::Personalization, window, cx)
                    })),
                )
                .into_any_element(),
        );
        rows.push(
            button_enabled(
                "agent-apply",
                self.t(Key::AgentApply),
                ButtonStyle::Primary,
                ButtonState::available(false, self.agent.draft() != self.agent.saved),
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| this.apply_agent(cx)),
            )
            .into_any_element(),
        );
        rows.push(
            button_enabled(
                "agent-revert",
                self.t(Key::AgentRevert),
                ButtonStyle::Normal,
                ButtonState::available(false, self.agent.draft() != self.agent.saved),
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| this.revert_agent(cx)),
            )
            .into_any_element(),
        );
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap_3()
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn agent_draft_revert_restores_only_the_saved_settings(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        handle
            .update(cx, |this, _, cx| {
                assert_eq!(this.agent.draft(), this.agent.saved);
                this.agent.fields[2] = TextField::new("draft-codex.exe");
                this.agent.prefs.max_tool_failures = Some(17);
                assert_ne!(this.agent.draft(), this.agent.saved);
                this.revert_agent(cx);
                assert_eq!(this.agent.draft(), this.agent.saved);
                assert_eq!(this.agent.fields[2].content(), this.agent.saved.codex_path);
            })
            .unwrap();
    }

    #[gpui::test]
    fn provider_results_arrive_independently_and_edits_invalidate_only_their_route(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        cx.run_until_parked();
        let (sender, receiver) = std::sync::mpsc::channel();
        handle
            .update(cx, |this, _, cx| {
                this.agent.checking_providers = true;
                this.watch_agent_providers(receiver, cx);
            })
            .unwrap();
        sender
            .send((
                0,
                auris_agent::ProviderStatus {
                    version: Some("codex-cli fixture".into()),
                    models: Some(3),
                    ..Default::default()
                },
            ))
            .unwrap();
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        handle
            .update(cx, |this, _, _| {
                assert_eq!(
                    this.agent.provider_status[0].as_ref().unwrap().models,
                    Some(3)
                );
                assert!(this.agent.provider_status[1].is_none());
                assert!(this.agent.checking_providers);
                this.agent.fields[2] = TextField::new("different-codex.exe");
                assert!(!this.agent_provider_status_is_current(0));
                assert!(this.agent_provider_status_is_current(1));
                assert!(this.agent_provider_status_is_current(4));
                this.agent.fields[0] = TextField::new("http://different.invalid");
                assert!(!this.agent_provider_status_is_current(4));
            })
            .unwrap();
        drop(sender);
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        handle
            .read_with(cx, |this, _| assert!(!this.agent.checking_providers))
            .unwrap();
    }

    #[test]
    fn versions_read_as_numbers_whatever_the_cli_prints_around_them() {
        assert_eq!(short_version("2.1.322 (Claude Code)"), "v2.1.322");
        assert_eq!(short_version("codex-cli 0.9.0"), "v0.9.0");
        assert_eq!(short_version("v1.19.1"), "v1.19.1");
        assert_eq!(short_version("nightly build"), "nightly build");
    }

    #[gpui::test]
    fn a_newer_release_is_installed_from_its_row_and_checked_again(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        cx.run_until_parked();
        let available = |command: Option<auris_agent::UpdateCommand>| auris_agent::ProviderStatus {
            executable: Some("/fixture/claude".into()),
            version: Some("2.1.0 (Claude Code)".into()),
            models: Some(5),
            update: Some(auris_agent::ProviderUpdate::Available("2.2.0".into())),
            update_command: command,
            ..Default::default()
        };
        handle
            .update(cx, |this, _, cx| {
                this.agent.checking_providers = false;
                this.agent.provider_status[1] = Some(available(Some(auris_agent::UpdateCommand {
                    program: "/fixture/claude".into(),
                    args: vec!["update".into()],
                })));
                // Codex is out of date too, but installed some way Auris cannot update.
                this.agent.provider_status[0] = Some(available(None));
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        // A release that cannot be installed from here opens the instructions instead.
        crate::harness::click("agent-provider-update-0", cx);
        cx.run_until_parked();
        handle
            .read_with(cx, |this, _| assert!(this.agent.expanded_providers[0]))
            .unwrap();

        crate::harness::click("agent-provider-update-1", cx);
        cx.run_until_parked();
        handle
            .read_with(cx, |this, _| {
                assert!(matches!(
                    this.agent.updates[1],
                    ProviderUpdateRun::Running(_)
                ));
            })
            .unwrap();
        assert!(cx.debug_bounds("agent-provider-updating-1").is_some());

        // A failure keeps the button for another try and opens the details on what it printed.
        let (sender, receiver) = std::sync::mpsc::channel();
        handle
            .update(cx, |this, _, cx| this.watch_agent_update(1, receiver, cx))
            .unwrap();
        sender
            .send(Err("claude update failed: offline".into()))
            .unwrap();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        handle
            .read_with(cx, |this, _| {
                assert!(matches!(&this.agent.updates[1], ProviderUpdateRun::Failed(error) if error.contains("offline")));
                assert!(this.agent.expanded_providers[1]);
            })
            .unwrap();
        assert!(cx.debug_bounds("agent-provider-update-1").is_some());

        // Success checks again, so that the row reads the version just installed.
        let (sender, receiver) = std::sync::mpsc::channel();
        handle
            .update(cx, |this, _, cx| this.watch_agent_update(1, receiver, cx))
            .unwrap();
        sender.send(Ok(())).unwrap();
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        handle
            .read_with(cx, |this, _| {
                assert!(matches!(this.agent.updates[1], ProviderUpdateRun::Done));
                // The harness's check reports nothing, so the old status is simply gone.
                assert!(this.agent.provider_status[1].is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn connection_check_does_not_apply_unsaved_paths_and_is_keyboard_accessible(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        cx.run_until_parked();
        handle
            .update(cx, |this, _, _| {
                this.agent.fields[2] = TextField::new("unsaved-codex.exe");
            })
            .unwrap();
        crate::harness::click("agent-check-providers", cx);
        cx.run_until_parked();
        handle
            .update(cx, |this, window, _| {
                assert_eq!(
                    this.agent.checked_fields.as_ref().unwrap()[2],
                    "unsaved-codex.exe"
                );
                assert_eq!(this.agent.fields[2].content(), "unsaved-codex.exe");
                assert!(this.agent.provider_check_focus.is_focused(window));
                this.agent.checked_fields.as_mut().unwrap()[2] = "previous.exe".into();
            })
            .unwrap();
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        cx.run_until_parked();
        handle
            .read_with(cx, |this, _| {
                assert_eq!(
                    this.agent.checked_fields.as_ref().unwrap()[2],
                    "unsaved-codex.exe"
                );
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_ne!(this.settings.agent.codex_path, "unsaved-codex.exe")
        });
        for selector in [
            "agent-provider-status-0",
            "agent-provider-status-1",
            "agent-provider-status-2",
            "agent-provider-status-3",
        ] {
            assert!(cx.debug_bounds(selector).is_some());
        }
    }

    #[gpui::test]
    fn failure_limit_draft_reaches_saved_preferences_and_worker_configuration(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        handle
            .update(cx, |this, _, cx| {
                this.agent.prefs.max_tool_failures = Some(8);
                this.apply_agent_with(cx, |settings| {
                    assert_eq!(settings.agent.tool_failure_limit(), 8);
                    Ok::<(), String>(())
                });
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.max_tool_failures, Some(8));
            assert_eq!(this.agent_chat.preferences().tool_failure_limit(), 8);
        });
    }

    #[gpui::test]
    fn tool_call_limit_dropdown_saves_and_reaches_the_panel(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(760.0), px(1200.0)));
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        click_visible_control("agent-tool-call-limit", cx);
        cx.simulate_keystrokes("down enter");
        handle
            .update(cx, |this, _, cx| {
                assert_ne!(
                    this.agent.draft(),
                    this.agent.saved,
                    "Changing only the limit must enable Apply"
                );
                this.apply_agent_with(cx, |settings| {
                    assert_eq!(settings.agent.tool_call_limit(), 2000);
                    Ok::<(), String>(())
                });
                assert_eq!(this.agent.draft(), this.agent.saved);
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.agent_chat.preferences().tool_call_limit(), 2000);
        });
    }

    #[gpui::test]
    fn tool_call_limit_draft_can_revert_without_changing_saved_preferences(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        handle
            .update(cx, |this, _, cx| {
                this.agent.prefs.max_tool_calls = Some(5000);
                assert_ne!(this.agent.draft(), this.agent.saved);
                this.revert_agent(cx);
                assert_eq!(this.agent.prefs.max_tool_calls, None);
                assert_eq!(this.agent.draft(), this.agent.saved);
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.max_tool_calls, None);
        });
    }

    #[gpui::test]
    fn provider_disclosure_keeps_unsaved_paths_when_closed(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_settings(cx));
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        assert!(cx.debug_bounds("agent-codex-path").is_none());
        crate::harness::click("agent-provider-details-2", cx);
        crate::harness::click("agent-codex-path", cx);
        cx.simulate_input("draft-codex.exe");
        crate::harness::click("agent-provider-details-2", cx);
        handle
            .read_with(cx, |this, _| assert!(!this.agent.expanded_providers[0]))
            .unwrap();
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        cx.run_until_parked();
        handle
            .read_with(cx, |this, _| assert!(this.agent.expanded_providers[0]))
            .unwrap();
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.agent.fields[2].content(), "draft-codex.exe");
            })
            .unwrap();
    }

    fn click_visible_control(selector: &'static str, cx: &mut VisualTestContext) {
        let body = cx.debug_bounds("settings-body").unwrap();
        let field = cx.debug_bounds(selector).unwrap();
        // Additional settings can move a control below the initial viewport.
        // Scroll as a user would before testing their pointer and caret behavior.
        if field.top() < body.top() || field.bottom() > body.bottom() {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: body.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(
                    px(0.0),
                    body.center().y - field.center().y,
                )),
                ..Default::default()
            });
            cx.run_until_parked();
        }
        let field = cx.debug_bounds(selector).unwrap();
        assert!(
            field.top() >= body.top() && field.bottom() <= body.bottom(),
            "{selector} must be inside the scrolling settings body before clicking: {field:?} outside {body:?}"
        );
        crate::harness::click(selector, cx);
    }

    #[gpui::test]
    fn antigravity_path_settings_keep_the_selected_model_and_api_connection(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent = AgentPreferences {
                backend: "antigravity".into(),
                model: "agy-fixture".into(),
                opencode_path: "C:/Tools/opencode.exe".into(),
                provider: "openai".into(),
                url: "https://example.invalid/v1".into(),
                ..Default::default()
            };
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.open_settings(cx);
        });
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(760.0), px(1200.0)));
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        crate::harness::click("agent-provider-details-5", cx);
        click_visible_control("agent-antigravity-path", cx);
        cx.simulate_input("C:/Tools/agy.exe");
        handle
            .update(cx, |this, _, cx| {
                this.apply_agent_with(cx, |_| Ok::<(), String>(()))
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.backend, "antigravity");
            assert_eq!(this.settings.agent.model, "agy-fixture");
            assert_eq!(this.settings.agent.opencode_path, "C:/Tools/opencode.exe");
            assert_eq!(
                this.agent_chat.preferences().antigravity_path,
                "C:/Tools/agy.exe"
            );
            assert_eq!(this.settings.agent.url, "https://example.invalid/v1");
        });
    }

    #[gpui::test]
    fn long_prompt_scrolls_without_moving_selection_and_clicks_the_scrolled_text(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent.system_prompt = "Keep this musical instruction.\n".repeat(80);
            this.open_settings(cx);
        });
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(1020.0), px(1600.0)));
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        click_visible_control("agent-system-prompt", cx);
        let bounds = cx.debug_bounds("agent-system-prompt").unwrap();
        let position = bounds.origin + gpui::point(px(12.0), px(12.0));
        cx.simulate_click(position, gpui::Modifiers::none());
        let before = handle
            .update(cx, |this, _, _| {
                this.agent.fields[AgentSettings::PROMPT_FIELD].selection()
            })
            .unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-300.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        handle
            .update(cx, |this, _, _| {
                assert_eq!(
                    this.agent.fields[AgentSettings::PROMPT_FIELD].selection(),
                    before
                )
            })
            .unwrap();
        cx.simulate_click(position, gpui::Modifiers::none());
        handle
            .update(cx, |this, _, _| {
                assert!(
                    this.agent.fields[AgentSettings::PROMPT_FIELD]
                        .selection()
                        .start
                        > before.start + 100
                )
            })
            .unwrap();
    }

    #[gpui::test]
    fn prompt_editing_applies_multiline_text_and_reset_only_after_apply(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent.system_prompt = "Original instructions".into();
            this.settings.agent.opencode_path = "C:/Tools/opencode.exe".into();
            this.settings.agent.antigravity_path = "C:/Tools/agy.exe".into();
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.open_settings(cx);
        });
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(1020.0), px(1600.0)));
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        click_visible_control("agent-system-prompt", cx);
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("二番も作る。");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("Keep the melody.");
        let expected = "二番も作る。\nKeep the melody.";
        handle
            .update(cx, |this, _, cx| {
                assert_eq!(
                    this.agent.fields[AgentSettings::PROMPT_FIELD].content(),
                    expected
                );
                this.apply_agent_with(cx, |_| Err("cannot save"));
                assert_eq!(this.status, "cannot save");
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.system_prompt, "Original instructions")
        });
        handle
            .update(cx, |this, _, cx| {
                assert_eq!(
                    this.agent.fields[AgentSettings::PROMPT_FIELD].content(),
                    expected
                );
                this.apply_agent_with(cx, |_| Ok::<(), String>(()));
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.system_prompt, expected);
            assert_eq!(this.agent_chat.preferences().system_prompt, expected);
            assert_eq!(this.settings.agent.opencode_path, "C:/Tools/opencode.exe");
            assert_eq!(this.settings.agent.antigravity_path, "C:/Tools/agy.exe");
        });
        click_visible_control("agent-prompt-reset", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.system_prompt, expected)
        });
        handle
            .update(cx, |this, _, cx| {
                assert_eq!(
                    this.agent.fields[AgentSettings::PROMPT_FIELD].content(),
                    auris_agent::default_system_prompt()
                );
                this.apply_agent_with(cx, |_| Ok::<(), String>(()));
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert!(this.settings.agent.system_prompt.is_empty())
        });
    }

    #[gpui::test]
    fn connection_settings_preserve_the_selected_runtime(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent = AgentPreferences {
                backend: "codex".into(),
                model: "codex-model".into(),
                provider: "openai".into(),
                url: "https://example.invalid/v1".into(),
                api_key_env: "RIG_KEY".into(),
                ..Default::default()
            };
            this.open_settings(cx);
        });
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(760.0), px(1200.0)));
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        assert!(cx.debug_bounds("agent-backend").is_none());
        assert!(cx.debug_bounds("agent-provider").is_some());
        crate::harness::click("agent-provider-details-2", cx);
        crate::harness::click("agent-codex-path", cx);
        cx.simulate_input("C:\\Tools\\codex.exe");
        handle
            .update(cx, |this, _, cx| {
                this.apply_agent_with(cx, |_| Ok::<(), String>(()))
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.backend, "codex");
            assert_eq!(this.settings.agent.codex_path, "C:\\Tools\\codex.exe");
            assert_eq!(this.settings.agent.provider, "openai");
            assert_eq!(this.settings.agent.url, "https://example.invalid/v1");
            assert_eq!(this.settings.agent.api_key_env, "RIG_KEY");
            assert_eq!(this.settings.agent.model, "codex-model");
            assert_eq!(this.agent_chat.backend, "codex");
            assert_eq!(
                this.agent_chat.preferences().codex_path,
                "C:\\Tools\\codex.exe"
            );
        });
    }

    #[gpui::test]
    fn claude_connection_settings_preserve_the_model_and_api_route(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent = AgentPreferences {
                backend: "claude".into(),
                model: "claude-model".into(),
                codex_path: "C:/Tools/codex.exe".into(),
                provider: "openai".into(),
                url: "https://example.invalid/v1".into(),
                api_key_env: "RIG_KEY".into(),
                ..Default::default()
            };
            this.open_settings(cx);
        });
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(760.0), px(1200.0)));
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        assert!(cx.debug_bounds("agent-backend").is_none());
        crate::harness::click("agent-provider-details-3", cx);
        crate::harness::click("agent-claude-path", cx);
        cx.simulate_input("C:/Tools/claude.exe");
        click_visible_control("agent-url", cx);
        cx.simulate_keystrokes("secondary-a");
        cx.simulate_input("https://next.invalid/v1");
        handle
            .update(cx, |this, _, cx| {
                this.apply_agent_with(cx, |_| Ok::<(), String>(()))
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.backend, "claude");
            assert_eq!(this.settings.agent.model, "claude-model");
            assert_eq!(
                this.agent_chat.preferences().claude_path,
                "C:/Tools/claude.exe"
            );
            assert_eq!(this.settings.agent.codex_path, "C:/Tools/codex.exe");
            assert_eq!(this.settings.agent.url, "https://example.invalid/v1");
            assert_eq!(
                this.settings.agent.discovery_endpoint.as_ref().unwrap().url,
                "https://next.invalid/v1"
            );
        });
    }

    #[gpui::test]
    fn opencode_connection_settings_preserve_the_model_and_api_route(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent = AgentPreferences {
                backend: "opencode".into(),
                model: "provider/model".into(),
                url: "https://example.invalid/v1".into(),
                ..Default::default()
            };
            this.open_settings(cx);
        });
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        crate::harness::click("agent-provider-details-4", cx);
        click_visible_control("agent-opencode-path", cx);
        cx.simulate_input("C:/Tools/opencode.exe");
        handle
            .update(cx, |this, _, cx| {
                this.apply_agent_with(cx, |_| Ok::<(), String>(()))
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.backend, "opencode");
            assert_eq!(this.settings.agent.model, "provider/model");
            assert_eq!(this.settings.agent.url, "https://example.invalid/v1");
            assert_eq!(
                this.agent_chat.preferences().opencode_path,
                "C:/Tools/opencode.exe"
            );
        });
    }

    #[gpui::test]
    fn connection_settings_move_out_of_the_panel_and_preserve_live_model_changes(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent = AgentPreferences {
                model: "first".into(),
                url: "http://127.0.0.1:1".into(),
                ..Default::default()
            };
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.agent_chat.models_error = Some("offline fixture".into());
            this.panels.show(crate::dock::Panel::Agent);
            this.open_settings(cx);
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-model").is_some());
        assert!(cx.debug_bounds("agent-url").is_none());
        assert!(cx.debug_bounds("agent-provider").is_none());
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(760.0), px(1200.0)));
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        let body = cx
            .debug_bounds("settings-body")
            .expect("the settings body is visible");
        for selector in ["agent-url", "agent-key-env"] {
            let field = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} is visible"));
            assert!(
                field.left() >= body.left() && field.right() <= body.right(),
                "{selector} escapes the settings body: {field:?} outside {body:?}"
            );
            assert!(
                field.size.width >= body.size.width * 0.5,
                "{selector} collapsed despite a wide settings body: {field:?} inside {body:?}"
            );
        }
        click_visible_control("agent-url", cx);
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("http://127.0.0.1:2".into()))
        });
        cx.simulate_keystrokes("secondary-a secondary-v");
        click_visible_control("agent-key-env", cx);
        cx.simulate_input("OLLAMA_KEY");
        click_visible_control("agent-output-tokens", cx);
        cx.simulate_keystrokes("down enter");
        app.update(cx, |this, _| {
            this.settings.agent.model = "second".into();
            this.settings.agent.policy.mode = auris_session::agent_policy::Mode::Plan;
            this.agent_chat.busy = true;
        });
        click_visible_control("agent-apply", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.model, "second");
            assert_eq!(this.settings.agent.url, "http://127.0.0.1:2");
            assert_eq!(this.settings.agent.api_key_env, "OLLAMA_KEY");
            assert_eq!(this.settings.agent.output_tokens, Some(8192));
            assert_eq!(
                this.settings.agent.policy.mode,
                auris_session::agent_policy::Mode::Plan
            );
            assert!(
                this.agent_chat.busy,
                "applying settings keeps the current turn running"
            );
            assert_eq!(this.agent_chat.chosen_model, "second");
        });
        handle
            .update(cx, |this, _, cx| {
                this.agent.fields[0] = TextField::new("http://127.0.0.1:3");
                this.apply_agent_with(cx, |_| Ok::<(), String>(()));
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.url, "http://127.0.0.1:3");
            assert_eq!(this.settings.agent.model, "second");
        });
        crate::harness::click("settings-search", cx);
        cx.simulate_input("出力トークン");
        cx.run_until_parked();
        assert!(cx.debug_bounds("agent-output-tokens").is_some());
        assert!(cx.debug_bounds("agent-model").is_none());
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.search.as_str(), "出力トークン")
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_click_is_measured_against_its_own_field_after_another_window_paints(
        cx: &mut TestAppContext,
    ) {
        // Where a field was painted used to live in one cell, overwritten by whichever field
        // painted last. With the URL being edited here and the library search painted in the
        // main window afterwards, a click near the start of the URL was measured against the
        // library box and the caret went to the end.
        let (app, main) = crate::harness::open(cx);
        app.update(main, |this, cx| {
            this.settings.agent = AgentPreferences {
                url: "http://127.0.0.1:11434/v1".into(),
                ..Default::default()
            };
            this.open_settings(cx);
        });
        let handle = app.read_with(main, |this, _| this.settings_window.unwrap());
        let settings = &mut VisualTestContext::from_window(handle.into(), main);
        settings.run_until_parked();
        crate::harness::click("tab-agent", settings);
        click_visible_control("agent-url", settings);
        settings.run_until_parked();

        crate::harness::click("library-search", main);
        crate::harness::paint(&app, main);

        let url = settings
            .debug_bounds("agent-url")
            .expect("the URL field is drawn");
        settings.simulate_click(
            gpui::point(url.left() + px(14.0), url.center().y),
            gpui::Modifiers::none(),
        );
        handle
            .update(settings, |this, _, cx| {
                let field = this.agent.inputs[0].input().read(cx).buffer();
                assert!(field.selection().is_empty(), "{:?}", field.selection());
                assert!(
                    field.selection().start < field.text().len() / 2,
                    "a click near the start put the caret at {} of {}",
                    field.selection().start,
                    field.text().len()
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn clicking_a_connection_field_leaves_a_caret_rather_than_a_selection(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent = AgentPreferences {
                url: "http://127.0.0.1:1".into(),
                ..Default::default()
            };
            this.open_settings(cx);
        });
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();
        crate::harness::click("tab-agent", cx);
        click_visible_control("agent-url", cx);
        handle
            .update(cx, |this, _, cx| {
                let field = this.agent.inputs[0].input().read(cx).buffer();
                assert!(
                    field.selection().is_empty(),
                    "a click left {:?} selected, so no caret was drawn",
                    field.selection()
                );
            })
            .unwrap();
        // Typing extends the address instead of replacing it, which is what a visible caret at
        // the end promised.
        cx.simulate_input("0");
        handle
            .update(cx, |this, _, _| {
                assert_eq!(this.agent.fields[0].content(), "http://127.0.0.1:10");
                assert_eq!(this.agent.fields[1].selection(), 0..0);
            })
            .unwrap();
    }

    #[gpui::test]
    fn failed_apply_preserves_the_draft_and_never_reports_success(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent = AgentPreferences {
                model: "saved-model".to_string(),
                provider: "ollama".to_string(),
                url: "http://127.0.0.1:11434".to_string(),
                ..Default::default()
            };
            this.agent_chat
                .load_preferences(&this.settings.agent.clone());
            this.open_settings(cx);
        });
        let handle = app.read_with(cx, |this, _| this.settings_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        cx.run_until_parked();

        handle
            .update(cx, |this, _, cx| {
                this.agent.prefs.provider = "openai".to_string();
                this.agent.fields[0] = TextField::new("https://example.invalid/v1");
                this.agent.fields[1] = TextField::new("TEST_AGENT_KEY");

                this.apply_agent_with(cx, |_| Err("settings fixture failure"));

                assert_eq!(
                    (
                        this.agent.prefs.provider.as_str(),
                        this.agent.fields[0].content(),
                        this.agent.fields[1].content(),
                        this.status.as_str(),
                    ),
                    (
                        "openai",
                        "https://example.invalid/v1",
                        "TEST_AGENT_KEY",
                        "settings fixture failure",
                    )
                );
            })
            .unwrap();
        app.read_with(cx, |this, _| {
            assert_eq!(
                (
                    this.settings.agent.provider.as_str(),
                    this.settings.agent.url.as_str(),
                    this.agent_chat.provider_openai,
                    this.agent_chat.url_field.content(),
                ),
                (
                    "ollama",
                    "http://127.0.0.1:11434",
                    false,
                    "http://127.0.0.1:11434",
                )
            );
        });
    }
}
